//! Settings page — config management with sectioned nav and live editing.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use hypercolor_types::config::HypercolorConfig;

use crate::api;
use crate::components::settings_sections::*;
use crate::icons::*;

/// Apply a dotted-key config change to a `HypercolorConfig` via serde JSON round-trip.
fn apply_config_key(config: &mut HypercolorConfig, key: &str, value: &serde_json::Value) {
    let Ok(mut root) = serde_json::to_value(&*config) else {
        return;
    };

    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return;
    }

    // Navigate to the parent object
    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut cursor = &mut root;
    for &part in parents {
        cursor = cursor
            .as_object_mut()
            .expect("config path should be an object")
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    // Set the leaf value
    if let Some(obj) = cursor.as_object_mut() {
        obj.insert(leaf[0].to_owned(), value.clone());
    }

    if let Ok(updated) = serde_json::from_value(root) {
        *config = updated;
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let config_resource = LocalResource::new(api::fetch_config);
    let devices_resource = LocalResource::new(api::fetch_audio_devices);
    let (config, set_config) = signal(None::<HypercolorConfig>);
    let (active_section, set_active_section) = signal("audio".to_string());

    // Seed config signal from resource
    Effect::new(move |_| {
        if let Some(Ok(cfg)) = config_resource.get() {
            set_config.set(Some(cfg));
        }
    });

    // Derive config path for the About section
    let config_path = Memo::new(move |_| {
        // We don't have the config path from the config itself — it comes from status
        String::new()
    });

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
        // Optimistic: update local config
        set_config.update(|cfg| {
            if let Some(cfg) = cfg {
                apply_config_key(cfg, &key, &value);
            }
        });

        // Persist to daemon
        leptos::task::spawn_local(async move {
            if let Err(e) = api::set_config_value(&key, &value).await {
                leptos::logging::warn!("Config set failed: {e}");
                // On error, re-fetch to revert
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
            // Re-fetch full config after reset
            if let Ok(fresh) = api::fetch_config().await {
                set_config.set(Some(fresh));
            }
        });
    });

    // Scroll to section (using JS interop to avoid extra web-sys feature flags)
    let scroll_to = move |id: &str| {
        let section_id = format!("section-{id}");
        set_active_section.set(id.to_string());
        if let Some(window) = web_sys::window() {
            if let Some(doc) = window.document() {
                if let Some(el) = doc.get_element_by_id(&section_id) {
                    let opts = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(&opts, &"behavior".into(), &"smooth".into());
                    let _ = js_sys::Reflect::set(&opts, &"block".into(), &"start".into());
                    if let Ok(func) = js_sys::Reflect::get(&el, &"scrollIntoView".into()) {
                        if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                            let _ = func.call1(&el, &opts);
                        }
                    }
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
            // Section nav rail
            <nav class="w-44 shrink-0 border-r border-edge-subtle bg-surface-base py-6 px-2.5 space-y-0.5">
                <div class="px-2 pb-3">
                    <h1 class="text-lg font-medium text-fg-primary">"Settings"</h1>
                </div>
                {sections.into_iter().map(|section| {
                    let id = section.id;
                    let is_active = Memo::new(move |_| active_section.get() == id);

                    let item = view! {
                        <button
                            class="flex items-center gap-2.5 w-full px-2.5 py-2 rounded-lg text-sm text-left transition-all duration-150"
                            class:bg-accent-muted=move || is_active.get()
                            class:text-fg-primary=move || is_active.get()
                            class:text-fg-tertiary=move || !is_active.get()
                            on:click=move |_| scroll_to(id)
                        >
                            <span
                                class="w-4 h-4 flex items-center justify-center shrink-0"
                                class:text-accent=move || is_active.get()
                            >
                                <Icon icon=section.icon width="15px" height="15px" />
                            </span>
                            <span class="truncate">{section.label}</span>
                        </button>
                    };

                    if section.divider_before {
                        view! {
                            <div class="h-px bg-border-subtle/30 my-2 mx-2" />
                            {item}
                        }.into_any()
                    } else {
                        item.into_any()
                    }
                }).collect_view()}
            </nav>

            // Scrollable content
            <div class="flex-1 overflow-y-auto">
                <Suspense fallback=move || view! {
                    <div class="p-6 space-y-4">
                        {(0..4).map(|_| view! {
                            <div class="rounded-xl border border-edge-subtle bg-surface-overlay/20 h-48 animate-pulse" />
                        }).collect_view()}
                    </div>
                }>
                    {move || {
                        config.get().map(|_cfg| {
                            let config_signal = Signal::derive(move || config.get().unwrap_or_default());

                            view! {
                                <div class="p-6 space-y-4 max-w-3xl">
                                    // Restart notice
                                    <div class="flex items-center gap-2 px-4 py-2.5 rounded-lg text-xs"
                                         style="background: rgba(241, 250, 140, 0.04); border: 1px solid rgba(241, 250, 140, 0.08); color: rgba(241, 250, 140, 0.6)">
                                        <Icon icon=LuInfo width="13px" height="13px" />
                                        "Changes save automatically. Settings marked "
                                        <span class="font-mono px-1 py-0.5 rounded text-[9px]"
                                              style="background: rgba(241, 250, 140, 0.08); color: rgba(241, 250, 140, 0.7)">
                                            "restart"
                                        </span>
                                        " take effect after daemon restart."
                                    </div>

                                    <AudioSection
                                        config=config_signal
                                        on_change=on_change
                                        on_reset=on_reset
                                        audio_devices=Signal::derive(move || audio_devices.get())
                                    />
                                    <CaptureSection config=config_signal on_change=on_change on_reset=on_reset />
                                    <EngineSection config=config_signal on_change=on_change on_reset=on_reset />
                                    <NetworkSection config=config_signal on_change=on_change on_reset=on_reset />
                                    <SessionSection config=config_signal on_change=on_change on_reset=on_reset />
                                    <DiscoverySection config=config_signal on_change=on_change on_reset=on_reset />
                                    <DeveloperSection config=config_signal on_change=on_change on_reset=on_reset />
                                    <AboutSection config_path=Signal::derive(move || config_path.get()) />
                                </div>
                            }
                        })
                    }}
                </Suspense>
            </div>
        </div>
    }
}
