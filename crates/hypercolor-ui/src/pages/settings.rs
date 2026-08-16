//! Settings page — config management with horizontal tab nav and live editing.

use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_query_map;
use leptos_use::{UseIntersectionObserverOptions, use_intersection_observer_with_options};

use crate::api;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::settings_controls::SectionHeader;
use crate::components::settings_sections::*;
use crate::config_state::{ConfigApplyTracker, ConfigContext, apply_config_key, config_key_value};
use crate::extensions::SettingsExtensionSections;
use crate::icons::*;
use crate::settings_audio_devices::{
    AudioDeviceChoice, AudioDeviceLoadState, resolve_audio_device_dropdown,
};
use hypercolor_leptos_ext::events::{document as browser_document, scroll_into_view_start};

/// Section IDs for nav and scroll spy, in visual order. Sections cluster
/// into the four tab groups (General, Behavior, Connectivity, System).
const SECTION_IDS: &[&str] = &[
    "audio",
    "capture",
    "input",
    "session",
    "network",
    "discovery",
    "rendering",
    "developer",
    "about",
];

fn settings_section_targets(extension_ids: &[&'static str]) -> Vec<web_sys::Element> {
    let Some(doc) = browser_document() else {
        return Vec::new();
    };

    SECTION_IDS
        .iter()
        .chain(extension_ids)
        .filter_map(|id| doc.get_element_by_id(&format!("section-{id}")))
        .collect()
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let config_ctx = expect_context::<ConfigContext>();
    let devices_resource = api::daemon_resource(api::fetch_audio_devices);
    let drivers_resource = api::daemon_resource(api::fetch_drivers);
    let config = config_ctx.config;
    let set_config = config_ctx.set_config;
    let (active_section, set_active_section) = signal("audio".to_string());
    // Extension-contributed sections (empty in the standalone OSS app).
    let extension_sections = use_context::<SettingsExtensionSections>().unwrap_or_default();
    let extension_ids: Vec<&'static str> = extension_sections
        .0
        .iter()
        .map(|section| section.id)
        .collect();

    // Only transitions once: false -> true. Memo deduplicates, so downstream
    // closures reading this won't re-run on every config update.
    let config_loaded = Memo::new(move |_| config.get().is_some());

    let section_targets = {
        let extension_ids = extension_ids.clone();
        Signal::derive(move || {
            if config_loaded.get() {
                settings_section_targets(&extension_ids)
            } else {
                Vec::new()
            }
        })
    };
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
    let audio_device_dropdown = Memo::new(move |_| {
        let configured_device = config
            .get()
            .map(|current| current.audio.device)
            .filter(|device| !device.trim().is_empty());

        match devices_resource.get() {
            None => resolve_audio_device_dropdown(
                configured_device.as_deref(),
                AudioDeviceLoadState::Loading,
            ),
            Some(Err(_)) => resolve_audio_device_dropdown(
                configured_device.as_deref(),
                AudioDeviceLoadState::Error,
            ),
            Some(Ok(data)) => {
                let devices = data
                    .devices
                    .iter()
                    .map(|device| AudioDeviceChoice {
                        id: device.id.clone(),
                        name: device.name.clone(),
                        description: device.description.clone(),
                    })
                    .collect::<Vec<_>>();
                resolve_audio_device_dropdown(
                    configured_device.as_deref(),
                    AudioDeviceLoadState::Ready(&devices),
                )
            }
        }
    });
    let driver_modules = Signal::derive(move || {
        drivers_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
    });

    let config_applies = Arc::new(Mutex::new(ConfigApplyTracker::default()));
    let on_change = Callback::new(move |(key, value): (String, serde_json::Value)| {
        let previous = config
            .get_untracked()
            .as_ref()
            .and_then(|current| config_key_value(current, &key));
        let generation = config_applies
            .lock()
            .expect("config apply tracker lock poisoned")
            .begin(&key);
        set_config.update(|cfg| {
            if let Some(cfg) = cfg {
                apply_config_key(cfg, &key, &value);
            }
        });

        let config_applies = Arc::clone(&config_applies);
        leptos::task::spawn_local(async move {
            if let Err(e) = api::set_config_value(&key, &value).await {
                leptos::logging::warn!("Config set failed: {e}");
                let is_current = config_applies
                    .lock()
                    .expect("config apply tracker lock poisoned")
                    .finish_if_current(&key, generation);
                if is_current {
                    if let Some(previous) = previous {
                        set_config.update(|cfg| {
                            if let Some(cfg) = cfg {
                                apply_config_key(cfg, &key, &previous);
                            }
                        });
                    }
                }
            } else {
                config_applies
                    .lock()
                    .expect("config apply tracker lock poisoned")
                    .finish_if_current(&key, generation);
                // Driver entries are masked on the generic config read,
                // so the inventory is what tells this page whether a
                // driver is enabled — re-read it after writing one.
                if key.starts_with("drivers.") {
                    drivers_resource.refetch();
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
            config_ctx.refresh.run(());
        });
    });

    // Scroll to section on tab click
    let scroll_to = move |id: &str| {
        set_active_section.set(id.to_string());
        if let Some(doc) = browser_document()
            && let Some(el) = doc.get_element_by_id(&format!("section-{id}"))
        {
            scroll_into_view_start(&el);
        }
    };

    // Deep-link target: callers (e.g. the first-run welcome overlay's
    // "Set up RGB hardware support" CTA) pass `?focus=<section-id>` so
    // the page scrolls past the default Audio section onto the section
    // they actually wanted. spawn_local yields to the event loop so the
    // section DOM is mounted by the time we look it up.
    let query = use_query_map();
    let focus_ids = extension_ids.clone();
    Effect::new(move |_| {
        let Some(focus) = query.with(|map| map.get("focus")) else {
            return;
        };
        if !SECTION_IDS.contains(&focus.as_str()) && !focus_ids.contains(&focus.as_str()) {
            return;
        }
        set_active_section.set(focus.clone());
        let focus_id = focus.clone();
        leptos::task::spawn_local(async move {
            if let Some(doc) = browser_document()
                && let Some(el) = doc.get_element_by_id(&format!("section-{focus_id}"))
            {
                scroll_into_view_start(&el);
            }
        });
    });

    // Tab data
    struct TabEntry {
        /// Scroll target — the first section of the group.
        id: &'static str,
        label: &'static str,
        icon: icondata_core::Icon,
        separator_before: bool,
        /// Section IDs this tab lights up for as the scroll spy passes them.
        members: Vec<&'static str>,
    }

    let mut tabs = vec![
        TabEntry {
            id: "audio",
            label: "General",
            icon: LuSlidersHorizontal,
            separator_before: false,
            members: vec!["audio", "capture", "input"],
        },
        TabEntry {
            id: "session",
            label: "Behavior",
            icon: LuPower,
            separator_before: false,
            members: vec!["session"],
        },
        TabEntry {
            id: "network",
            label: "Connectivity",
            icon: LuGlobe,
            separator_before: false,
            members: vec!["network", "discovery"],
        },
        TabEntry {
            id: "rendering",
            label: "System",
            icon: LuGauge,
            separator_before: false,
            members: vec!["rendering", "developer", "about"],
        },
    ];
    tabs.extend(
        extension_sections
            .0
            .iter()
            .enumerate()
            .map(|(index, section)| TabEntry {
                id: section.id,
                label: section.label,
                icon: section.icon,
                members: vec![section.id],
                separator_before: index == 0,
            }),
    );

    view! {
        <div class="flex flex-col h-full">
            <PageHeader
                icon=LuSettings2
                title="Settings"
                accent=PageAccent::Yellow
            >
                <HeaderTrailing slot>
                    <div
                        class="flex shrink-0 items-center gap-1.5 text-[11px]"
                        style="color: rgba(128, 255, 234, 0.5)"
                    >
                        <Icon icon=LuInfo width="12px" height="12px" />
                        "Auto-saved"
                    </div>
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <div class="flex items-center gap-0.5 w-full h-full overflow-x-auto scrollbar-none">
                        {tabs.into_iter().map(|tab| {
                            let id = tab.id;
                            let members = tab.members.clone();
                            let is_active = Memo::new(move |_| {
                                let current = active_section.get();
                                members.iter().any(|member| *member == current)
                            });

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
                                // `accent-yellow` feeds the page accent into
                                // `--glow-rgb` for the active icon/underline,
                                // and the active text rides `text-fg-primary`
                                // so the light theme stays legible.
                                <button
                                    class="accent-yellow flex items-center gap-1.5 px-3 h-full text-[13px] shrink-0 relative transition-colors duration-200 cursor-pointer"
                                    class=("text-fg-primary", is_active)
                                    class=("text-fg-tertiary/60", move || !is_active.get())
                                    on:click=move |_| scroll_to(id)
                                >
                                    <span
                                        class="w-4 h-4 flex items-center justify-center shrink-0"
                                        class=("text-electric-yellow", is_active)
                                    >
                                        <Icon icon=tab.icon width="14px" height="14px" />
                                    </span>
                                    <span class="whitespace-nowrap">{tab.label}</span>
                                    // Active underline — page-accent glow
                                    <div
                                        class="absolute bottom-0 left-2 right-2 h-[2px] rounded-full transition-all duration-300"
                                        style=move || if is_active.get() {
                                            "background: rgb(var(--glow-rgb)); box-shadow: 0 0 10px rgba(var(--glow-rgb), 0.4); opacity: 1"
                                        } else {
                                            "opacity: 0"
                                        }
                                    />
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </HeaderToolbar>
            </PageHeader>

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
                                style="animation: enter-fade 0.4s ease-out 0.05s both"
                            >
                                <AudioSection
                                    config=config
                                    on_change=on_change
                                    on_reset=on_reset
                                    audio_devices=Signal::derive(move || {
                                        audio_device_dropdown.with(|state| state.options.clone())
                                    })
                                    audio_device_placeholder=Signal::derive(move || {
                                        audio_device_dropdown
                                            .with(|state| state.placeholder.clone())
                                    })
                                    audio_device_disabled=Signal::derive(move || {
                                        audio_device_dropdown.with(|state| state.disabled)
                                    })
                                />
                            </div>
                            // Card order mirrors the four tab groups:
                            // General (audio, capture, input), Behavior
                            // (session), Connectivity (network, discovery),
                            // System (rendering, developer, about).
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.1s both"
                            >
                                <CaptureSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.125s both"
                            >
                                <InputSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.15s both"
                            >
                                <SessionSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.2s both"
                            >
                                <NetworkSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.25s both"
                            >
                                <DiscoverySection
                                    config=config
                                    driver_modules=driver_modules
                                    on_change=on_change
                                    on_reset=on_reset
                                />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.3s both"
                            >
                                <RenderingSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.325s both"
                            >
                                <DeveloperSection config=config on_change=on_change on_reset=on_reset />
                            </div>
                            <div
                                class="settings-card"
                                style="animation: enter-fade 0.4s ease-out 0.35s both"
                            >
                                <AboutSection />
                            </div>
                            // Extension sections — the page supplies the card,
                            // anchor, and header so they read exactly like core
                            // sections; the extension supplies only the rows.
                            {extension_sections.0.iter().enumerate().map(|(index, section)| {
                                let delay = 0.4 + 0.05 * (index as f64);
                                view! {
                                    <div
                                        class="settings-card"
                                        style=format!("animation: enter-fade 0.4s ease-out {delay}s both")
                                    >
                                        <section
                                            id=format!("section-{}", section.id)
                                            class="pt-5 pb-3 space-y-0"
                                        >
                                            <SectionHeader title=section.label icon=section.icon />
                                            {(section.view)()}
                                        </section>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    })
                }}
            </div>
        </div>
    }
}
