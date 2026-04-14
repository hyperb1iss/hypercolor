//! Settings page — config management with horizontal tab nav and live editing.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::{UseIntersectionObserverOptions, use_intersection_observer_with_options};
use wasm_bindgen::JsCast;

use hypercolor_types::config::HypercolorConfig;

use crate::api;
use crate::components::page_header::PageHeader;
use crate::components::settings_sections::*;
use crate::icons::*;

/// Section IDs for nav and scroll spy.
const SECTION_IDS: &[&str] = &[
    "audio",
    "capture",
    "rendering",
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

fn settings_section_targets() -> Vec<web_sys::Element> {
    let Some(doc) = web_sys::window().and_then(|window| window.document()) else {
        return Vec::new();
    };

    SECTION_IDS
        .iter()
        .filter_map(|id| doc.get_element_by_id(&format!("section-{id}")))
        .collect()
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

    let section_targets = Signal::derive(move || {
        if config_loaded.get() {
            settings_section_targets()
        } else {
            Vec::new()
        }
    });
    let _scroll_spy = use_intersection_observer_with_options(
        section_targets,
        move |entries, _| {
            for entry in entries {
                if entry.is_intersecting() {
                    let id = entry.target().id();
                    if let Some(section) = id.strip_prefix("section-") {
                        set_active_section.set(section.to_string());
                    }
                }
            }
        },
        UseIntersectionObserverOptions::default()
            .root_margin("-100px 0px -60% 0px")
            .thresholds(vec![0.2]),
    );

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
            id: "rendering",
            label: "Rendering",
            icon: LuGauge,
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
            <div class="sticky top-0 z-10 shrink-0 glass-dense border-b border-edge-default">
                <div class="flex items-end justify-between gap-4 px-6 pt-5 pb-4">
                    <PageHeader
                        icon=LuSettings2
                        title="Settings"
                        subtitle="Engine, capture sources, and diagnostics."
                        accent_rgb="241, 250, 140"
                        gradient="linear-gradient(105deg,#80ffea 0%,#e8f0ff 50%,#f1fa8c 100%)"
                    />
                    <div
                        class="flex shrink-0 items-center gap-1.5 text-xs"
                        style="color: rgba(128, 255, 234, 0.4)"
                    >
                        <Icon icon=LuInfo width="12px" height="12px" />
                        "Auto-saved"
                    </div>
                </div>

                // Tab bar
                <div class="flex items-center gap-0.5 px-6 overflow-x-auto scrollbar-none border-t border-edge-subtle/10">
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
                                style="animation: fade-in 0.4s ease-out 0.125s both"
                            >
                                <RenderingSection config=config on_change=on_change on_reset=on_reset />
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
