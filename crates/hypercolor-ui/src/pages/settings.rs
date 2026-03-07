//! Settings page — config management with sectioned nav and live editing.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use hypercolor_types::config::HypercolorConfig;

use crate::api;
use crate::components::settings_sections::*;
use crate::icons::*;

/// Section IDs for nav and scroll spy.
const SECTION_IDS: &[&str] = &[
    "audio", "capture", "engine", "network", "session", "discovery", "developer", "about",
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
        cursor = cursor
            .as_object_mut()
            .expect("config path should be an object")
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
fn setup_scroll_spy(set_active: WriteSignal<String>) {
    let Some(window) = web_sys::window() else { return };
    let Some(doc) = window.document() else { return };

    let callback = Closure::wrap(Box::new(move |entries: wasm_bindgen::JsValue| {
        let entries = js_sys::Array::from(&entries);
        for entry in entries.iter() {
            let is_intersecting = js_sys::Reflect::get(&entry, &"isIntersecting".into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if is_intersecting {
                if let Some(target) = js_sys::Reflect::get(&entry, &"target".into()).ok() {
                    let id = js_sys::Reflect::get(&target, &"id".into())
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    if let Some(section) = id.strip_prefix("section-") {
                        set_active.set(section.to_string());
                    }
                }
            }
        }
    }) as Box<dyn FnMut(wasm_bindgen::JsValue)>);

    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"threshold".into(), &0.2.into());
    let _ = js_sys::Reflect::set(
        &opts,
        &"rootMargin".into(),
        &"-5% 0px -65% 0px".into(),
    );

    let ctor = js_sys::Reflect::get(
        &wasm_bindgen::JsValue::from(&window),
        &"IntersectionObserver".into(),
    )
    .ok()
    .and_then(|v| v.dyn_into::<js_sys::Function>().ok());

    if let Some(ctor) = ctor {
        if let Ok(observer) =
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
    }

    callback.forget();
}

/// Smooth-scroll an element into view via JS interop.
fn scroll_element_into_view(el: &web_sys::Element) {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"behavior".into(), &"smooth".into());
    let _ = js_sys::Reflect::set(&opts, &"block".into(), &"start".into());
    if let Ok(func) = js_sys::Reflect::get(el, &"scrollIntoView".into()) {
        if let Ok(func) = func.dyn_into::<js_sys::Function>() {
            let _ = func.call1(el, &opts);
        }
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

    // Stable config signal — created once, fine-grained updates flow through.
    let config_signal = Signal::derive(move || config.get().unwrap_or_default());

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

    // Derive config path for About section (comes from status, not config)
    let config_path = Memo::new(move |_| String::new());

    // Audio device options for dropdown
    let audio_devices = Memo::new(move |_| {
        devices_resource
            .get()
            .and_then(|r| r.ok())
            .map(|data| {
                data.devices
                    .into_iter()
                    .map(|d| (d.id, d.name))
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

    // Scroll to section on nav click
    let scroll_to = move |id: &str| {
        set_active_section.set(id.to_string());
        if let Some(window) = web_sys::window() {
            if let Some(doc) = window.document() {
                if let Some(el) = doc.get_element_by_id(&format!("section-{id}")) {
                    scroll_element_into_view(&el);
                }
            }
        }
    };

    // Section nav data
    struct NavEntry {
        id: &'static str,
        label: &'static str,
        icon: icondata_core::Icon,
        divider_before: bool,
    }

    let sections = vec![
        NavEntry { id: "audio", label: "Audio", icon: LuAudioLines, divider_before: false },
        NavEntry { id: "capture", label: "Capture", icon: LuMonitor, divider_before: false },
        NavEntry { id: "engine", label: "Engine", icon: LuZap, divider_before: false },
        NavEntry { id: "network", label: "Network", icon: LuGlobe, divider_before: false },
        NavEntry { id: "session", label: "Session", icon: LuPower, divider_before: false },
        NavEntry { id: "discovery", label: "Discovery", icon: LuRadar, divider_before: false },
        NavEntry { id: "developer", label: "Developer", icon: LuCode, divider_before: true },
        NavEntry { id: "about", label: "About", icon: LuInfo, divider_before: false },
    ];

    view! {
        <div class="flex h-full -m-6 animate-fade-in">
            // Section nav rail — cyan-tinted to differentiate from main sidebar
            <nav
                class="w-48 shrink-0 border-r border-edge-subtle py-6 px-3 space-y-0.5"
                style="background: linear-gradient(180deg, rgba(128, 255, 234, 0.015) 0%, rgba(20, 18, 28, 0.98) 100%)"
            >
                <div class="px-2 pb-4">
                    <h1 class="text-base font-medium text-fg-primary tracking-wide">"Settings"</h1>
                    <div
                        class="h-px mt-3"
                        style="background: linear-gradient(90deg, rgba(128, 255, 234, 0.2), transparent)"
                    />
                </div>
                {sections.into_iter().map(|section| {
                    let id = section.id;
                    let is_active = Memo::new(move |_| active_section.get() == id);

                    let item = view! {
                        <button
                            class="flex items-center gap-2.5 w-full px-2.5 py-2 rounded-lg text-sm text-left transition-all duration-200 relative"
                            style=move || if is_active.get() {
                                "background: rgba(128, 255, 234, 0.06); color: rgb(230, 237, 243)"
                            } else {
                                "color: rgba(139, 133, 160, 0.7)"
                            }
                            on:click=move |_| scroll_to(id)
                        >
                            // Active indicator — cyan glow bar
                            <div
                                class="absolute left-0 top-1/2 -translate-y-1/2 w-[2px] h-4 rounded-r-full transition-all duration-300"
                                style=move || if is_active.get() {
                                    "background: rgb(128, 255, 234); box-shadow: 0 0 8px rgba(128, 255, 234, 0.5); opacity: 1"
                                } else {
                                    "opacity: 0"
                                }
                            />
                            <span
                                class="w-4 h-4 flex items-center justify-center shrink-0 transition-colors duration-200"
                                style=move || if is_active.get() {
                                    "color: rgb(128, 255, 234)"
                                } else {
                                    ""
                                }
                            >
                                <Icon icon=section.icon width="15px" height="15px" />
                            </span>
                            <span class="truncate">{section.label}</span>
                        </button>
                    };

                    if section.divider_before {
                        view! {
                            <div
                                class="h-px my-2.5 mx-2"
                                style="background: linear-gradient(90deg, rgba(128, 255, 234, 0.1), transparent)"
                            />
                            {item}
                        }.into_any()
                    } else {
                        item.into_any()
                    }
                }).collect_view()}
            </nav>

            // Scrollable content
            <div class="flex-1 overflow-y-auto scroll-smooth">
                // Loading skeleton
                {move || {
                    if !config_loaded.get() {
                        Some(view! {
                            <div class="p-6 space-y-4 max-w-3xl">
                                {(0..5).map(|i| view! {
                                    <div
                                        class="rounded-xl border border-edge-subtle bg-surface-overlay/10 h-44 animate-pulse"
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
                        <div class="p-6 space-y-4 max-w-3xl">
                            // Deferred notice — all settings need daemon restart in v1
                            <div
                                class="flex items-center gap-2 px-4 py-2.5 rounded-lg text-xs animate-fade-in"
                                style="background: rgba(128, 255, 234, 0.02); border: 1px solid rgba(128, 255, 234, 0.06); color: rgba(128, 255, 234, 0.45)"
                            >
                                <Icon icon=LuInfo width="13px" height="13px" />
                                "Changes are saved to disk automatically. All settings take effect after daemon restart."
                            </div>

                            <div style="animation: fade-in 0.4s ease-out 0.05s both">
                                <AudioSection
                                    config=config_signal
                                    on_change=on_change
                                    on_reset=on_reset
                                    audio_devices=Signal::derive(move || audio_devices.get())
                                />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.1s both">
                                <CaptureSection config=config_signal on_change=on_change on_reset=on_reset />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.15s both">
                                <EngineSection config=config_signal on_change=on_change on_reset=on_reset />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.2s both">
                                <NetworkSection config=config_signal on_change=on_change on_reset=on_reset />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.25s both">
                                <SessionSection config=config_signal on_change=on_change on_reset=on_reset />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.3s both">
                                <DiscoverySection config=config_signal on_change=on_change on_reset=on_reset />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.35s both">
                                <DeveloperSection config=config_signal on_change=on_change on_reset=on_reset />
                            </div>
                            <div style="animation: fade-in 0.4s ease-out 0.4s both">
                                <AboutSection config_path=Signal::derive(move || config_path.get()) />
                            </div>
                        </div>
                    })
                }}
            </div>
        </div>
    }
}
