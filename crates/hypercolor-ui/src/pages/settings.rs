//! Settings page — config management with horizontal tab nav and live editing.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use hypercolor_types::config::HypercolorConfig;

use crate::api;
use crate::components::settings_sections::*;
use crate::icons::*;

/// Section IDs for nav and scroll spy.
const SECTION_IDS: &[&str] = &[
    "audio",
    "capture",
    "network",
    "session",
    "discovery",
    "developer",
    "about",
];

/// Apply a dotted-key config change to a `HypercolorConfig` via serde JSON round-trip.
fn apply_config_key(config: &mut HypercolorConfig, key: &str, value: &serde_json::Value) {
    let Ok(mut root) = serde_json::to_value(&*config) else {
        return;
    };

    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut cursor = &mut root;
    for &part in parents {
        let Some(obj) = cursor.as_object_mut() else {
            return;
        };
        cursor = obj
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    if let Some(obj) = cursor.as_object_mut() {
        obj.insert(leaf[0].to_owned(), value.clone());
    }

    if let Ok(updated) = serde_json::from_value(root) {
        *config = updated;
    }
}

/// Set up an IntersectionObserver via JS interop to track which section is visible.
/// Uses a negative top margin to account for the sticky tab header (~100px).
fn setup_scroll_spy(set_active: WriteSignal<String>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(doc) = window.document() else { return };

    let callback = Closure::wrap(Box::new(move |entries: wasm_bindgen::JsValue| {
        let entries = js_sys::Array::from(&entries);
        for entry in entries.iter() {
            let is_intersecting = js_sys::Reflect::get(&entry, &"isIntersecting".into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if is_intersecting && let Ok(target) = js_sys::Reflect::get(&entry, &"target".into()) {
                let id = js_sys::Reflect::get(&target, &"id".into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                if let Some(section) = id.strip_prefix("section-") {
                    set_active.set(section.to_string());
                }
            }
        }
    }) as Box<dyn FnMut(wasm_bindgen::JsValue)>);

    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"threshold".into(), &0.2.into());
    // Offset top for sticky header height
    let _ = js_sys::Reflect::set(&opts, &"rootMargin".into(), &"-100px 0px -60% 0px".into());

    let ctor = js_sys::Reflect::get(
        &wasm_bindgen::JsValue::from(&window),
        &"IntersectionObserver".into(),
    )
    .ok()
    .and_then(|v| v.dyn_into::<js_sys::Function>().ok());

    if let Some(ctor) = ctor
        && let Ok(observer) =
            js_sys::Reflect::construct(&ctor, &js_sys::Array::of2(callback.as_ref(), &opts))
    {
        let observe_fn = js_sys::Reflect::get(&observer, &"observe".into())
            .ok()
            .and_then(|v| v.dyn_into::<js_sys::Function>().ok());

        if let Some(observe) = observe_fn {
            for id in SECTION_IDS {
                if let Some(el) = doc.get_element_by_id(&format!("section-{id}")) {
                    let _ = observe.call1(&observer, &el);
                }
            }
        }
    }

    callback.forget();
}

/// Smooth-scroll an element into view via JS interop.
fn scroll_element_into_view(el: &web_sys::Element) {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"behavior".into(), &"smooth".into());
    let _ = js_sys::Reflect::set(&opts, &"block".into(), &"start".into());
    if let Ok(func) = js_sys::Reflect::get(el, &"scrollIntoView".into())
        && let Ok(func) = func.dyn_into::<js_sys::Function>()
    {
        let _ = func.call1(el, &opts);
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let config_resource = LocalResource::new(api::fetch_config);
    let devices_resource = LocalResource::new(api::fetch_audio_devices);
    let (config, set_config) = signal(None::<HypercolorConfig>);
    let (active_section, set_active_section) = signal("audio".to_string());

    // Only transitions once: false -> true. Memo deduplicates, so downstream
    // closures reading this won't re-run on every config update.
    let config_loaded = Memo::new(move |_| config.get().is_some());

    // Seed config signal from resource
    Effect::new(move |_| {
        if let Some(Ok(cfg)) = config_resource.get() {
            set_config.set(Some(cfg));
        }
    });

    // Install scroll spy once content is in the DOM
    let spy_installed = StoredValue::new(false);
    Effect::new(move |_| {
        if config_loaded.get() && !spy_installed.get_value() {
            spy_installed.set_value(true);
            let set_section = set_active_section;
            if let Some(w) = web_sys::window() {
                let cb = Closure::once(move || setup_scroll_spy(set_section));
                let _ = w.set_timeout_with_callback(cb.as_ref().unchecked_ref());
                cb.forget();
            }
        }
    });

    // Audio device options for dropdown
    let audio_devices = Memo::new(move |_| {
        devices_resource
            .get()
            .and_then(|r| r.ok())
            .map(|data| {
                data.devices
                    .into_iter()
                    .map(|d| {
                        let label = if d.description.is_empty() || d.description == d.name {
                            d.name.clone()
                        } else if d.description.to_ascii_lowercase().contains("unavailable") {
                            format!("{} (Unavailable)", d.name)
                        } else {
                            d.name.clone()
                        };
                        (d.id, label)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![("default".to_string(), "Default".to_string())])
    });

    // Change handler — optimistic update + persist
    let on_change = Callback::new(move |(key, value): (String, serde_json::Value)| {
        set_config.update(|cfg| {
            if let Some(cfg) = cfg {
                apply_config_key(cfg, &key, &value);
            }
        });

        leptos::task::spawn_local(async move {
            if let Err(e) = api::set_config_value(&key, &value).await {
                leptos::logging::warn!("Config set failed: {e}");
                if let Ok(fresh) = api::fetch_config().await {
                    set_config.set(Some(fresh));
                }
            }
        });
    });

    // Section reset handler
    let on_reset = Callback::new(move |key: String| {
        leptos::task::spawn_local(async move {
            if let Err(e) = api::reset_config_key(&key).await {
                leptos::logging::warn!("Config reset failed: {e}");
            }
            if let Ok(fresh) = api::fetch_config().await {
                set_config.set(Some(fresh));
            }
        });
    });

    // Scroll to section on tab click
    let scroll_to = move |id: &str| {
        set_active_section.set(id.to_string());
        if let Some(window) = web_sys::window()
            && let Some(doc) = window.document()
            && let Some(el) = doc.get_element_by_id(&format!("section-{id}"))
        {
            scroll_element_into_view(&el);
        }
    };

    // Tab data
    struct TabEntry {
        id: &'static str,
        label: &'static str,
        icon: icondata_core::Icon,
        separator_before: bool,
    }

    let tabs = vec![
        TabEntry {
            id: "audio",
            label: "Audio",
            icon: LuAudioLines,
            separator_before: false,
        },
        TabEntry {
            id: "capture",
            label: "Capture",
            icon: LuMonitor,
            separator_before: false,
        },
        TabEntry {
            id: "network",
            label: "Network",
            icon: LuGlobe,
            separator_before: false,
        },
        TabEntry {
            id: "session",
            label: "Session",
            icon: LuPower,
            separator_before: false,
        },
        TabEntry {
            id: "discovery",
            label: "Discovery",
            icon: LuRadar,
            separator_before: false,
        },
        TabEntry {
            id: "developer",
            label: "Developer",
            icon: LuCode,
            separator_before: true,
        },
        TabEntry {
            id: "about",
            label: "About",
            icon: LuInfo,
            separator_before: false,
        },
    ];

    view! {
        <div class="flex flex-col h-full animate-fade-in">
            // Sticky header with title + tab bar
            <div class="sticky top-0 z-10 shrink-0 glass-dense">
                <div class="flex items-center justify-between px-6 pt-5 pb-3">
                    <div class="flex items-center gap-2">
                        <span style="color: #f1fa8c; filter: drop-shadow(0 0 8px rgba(241, 250, 140, 0.65))">
                            <Icon icon=LuSettings2 width="20px" height="20px" />
                        </span>
                        <h1
                            class="leading-none logo-gradient-text"
                            style="font-family:'Orbitron',sans-serif; font-weight:900; font-size:22px; \
                                   letter-spacing:-0.01em; \
                                   background-image:linear-gradient(105deg,#80ffea 0%,#e8f0ff 50%,#f1fa8c 100%)"
                        >
                            "Settings"
                        </h1>
                    </div>
                    <div
                        class="flex items-center gap-1.5 text-xs"
                        style="color: rgba(128, 255, 234, 0.4)"
                    >
                        <Icon icon=LuInfo width="12px" height="12px" />
                        "Auto-saved"
                    </div>
                </div>

                // Tab bar
                <div class="flex items-center gap-0.5 px-5 overflow-x-auto scrollbar-none border-b border-edge-subtle/15">
                    {tabs.into_iter().map(|tab| {
                        let id = tab.id;
                        let is_active = Memo::new(move |_| active_section.get() == id);

                        let separator = if tab.separator_before {
                            Some(view! {
                                <div
                                    class="w-px h-4 mx-1.5 shrink-0"
                                    style="background: rgba(139, 133, 160, 0.15)"
                                />
                            })
                        } else {
                            None
                        };

                        view! {
                            {separator}
                            <button
                                class="flex items-center gap-1.5 px-3 py-2.5 text-sm shrink-0 relative transition-colors duration-200 cursor-pointer"
                                style=move || if is_active.get() {
                                    "color: rgb(230, 237, 243)"
                                } else {
                                    "color: rgba(139, 133, 160, 0.6)"
                                }
                                on:click=move |_| scroll_to(id)
                            >
                                <span
                                    class="w-4 h-4 flex items-center justify-center shrink-0"
                                    style=move || if is_active.get() {
                                        "color: rgb(128, 255, 234)"
                                    } else {
                                        ""
                                    }
                                >
                                    <Icon icon=tab.icon width="14px" height="14px" />
                                </span>
                                <span class="whitespace-nowrap">{tab.label}</span>
                                // Active underline — cyan glow
                                <div
                                    class="absolute bottom-0 left-2 right-2 h-[2px] rounded-full transition-all duration-300"
                                    style=move || if is_active.get() {
                                        "background: rgb(128, 255, 234); box-shadow: 0 0 10px rgba(128, 255, 234, 0.4); opacity: 1"
                                    } else {
                                        "opacity: 0"
                                    }
                                />
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto scroll-smooth">
                // Loading skeleton
                {move || {
                    if !config_loaded.get() {
                        Some(view! {
                            <div class="px-6 pb-6 max-w-3xl mx-auto space-y-4">
                                {(0..5).map(|i| view! {
                                    <div
                                        class="rounded-lg border border-edge-subtle/20 bg-surface-overlay/5 h-36 animate-pulse"
                                        style=format!("animation-delay: {}ms", i * 80)
                                    />
                                }).collect_view()}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                // Settings content — rendered once when config loads, never destroyed.
                // Fine-grained Signal::derive inside each section handles reactive updates
                // without causing DOM rebuild (no flicker on control changes).
                {move || {
                    config_loaded.get().then(|| view! {
                        <div class="px-6 pb-6 pt-4 max-w-4xl mx-auto space-y-3">
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.05s both"
                            >
                                <AudioSection
                                    config=config
                                    on_change=on_change
                                    on_reset=on_reset
                                    audio_devices=Signal::derive(move || audio_devices.get())
                                />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.1s both"
                            >
                                <CaptureSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.15s both"
                            >
                                <NetworkSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.2s both"
                            >
                                <SessionSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.25s both"
                            >
                                <DiscoverySection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.3s both"
                            >
                                <DeveloperSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: fade-in 0.4s ease-out 0.35s both"
                            >
                                <AboutSection />
                            </div>
                        </div>
                    })
                }}
            </div>
        </div>
    }
}
