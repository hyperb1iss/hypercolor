//! Layout palette — available devices + zone group management.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api::{self, ZoneTopologySummary};
use crate::app::DevicesContext;
use crate::components::layout_canvas::device_accent_colors;
use crate::icons::*;
use crate::layout_geometry;
use crate::toasts;
use hypercolor_types::spatial::{DeviceZone, NormalizedPosition, SpatialLayout, ZoneGroup};

/// Group color presets — works in both dark and light themes.
const GROUP_COLORS: &[&str] = &[
    "#e135ff", "#80ffea", "#ff6ac1", "#f1fa8c", "#50fa7b", "#82AAFF", "#ff9e64", "#c792ea",
];

/// Device palette for adding zones to the layout, with group management.
#[component]
pub fn LayoutPalette(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] selected_zone_id: Signal<Option<String>>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    set_is_dirty: WriteSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    // Stabilize the device list — only re-render when actual data changes,
    // not on every 5-second refetch poll.
    let stable_devices = Memo::new(move |_| {
        ctx.devices_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    });

    // Track which devices are collapsed
    let (collapsed_devices, set_collapsed_devices) =
        signal(std::collections::HashSet::<String>::new());

    // Cached attachment bindings per device — fetched lazily on expand.
    let (attachment_cache, set_attachment_cache) =
        signal(std::collections::HashMap::<String, Vec<api::AttachmentBindingSummary>>::new());
    let (import_in_flight, set_import_in_flight) = signal(false);

    // Fetch attachments for a device (if not already cached).
    let fetch_attachments = move |device_id: String| {
        if attachment_cache.get_untracked().contains_key(&device_id) {
            return;
        }
        let did = device_id.clone();
        leptos::task::spawn_local(async move {
            if let Ok(profile) = api::fetch_device_attachments(&did).await {
                set_attachment_cache.update(|cache| {
                    cache.insert(did, profile.bindings);
                });
            }
        });
    };

    // Derive group list from layout
    let groups = Signal::derive(move || {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| l.groups.clone())
                .unwrap_or_default()
        })
    });

    // Count zones per group
    let group_zone_counts = Signal::derive(move || {
        layout.with(|current| {
            let Some(l) = current.as_ref() else {
                return std::collections::HashMap::new();
            };
            let mut counts = std::collections::HashMap::new();
            for zone in &l.zones {
                if let Some(gid) = &zone.group_id {
                    *counts.entry(gid.clone()).or_insert(0usize) += 1;
                }
            }
            counts
        })
    });

    // Create a new group
    let create_group = move || {
        let current_groups = groups.get();
        let color_idx = current_groups.len() % GROUP_COLORS.len();
        let new_group = ZoneGroup {
            id: format!("group_{}", uuid_v4_hex()),
            name: format!("Group {}", current_groups.len() + 1),
            color: Some(GROUP_COLORS[color_idx].to_string()),
        };
        set_layout.update(|l| {
            if let Some(layout) = l {
                layout.groups.push(new_group);
            }
        });
        set_is_dirty.set(true);
    };

    // Delete a group (ungroups all zones)
    let delete_group = move |group_id: String| {
        set_layout.update(|l| {
            if let Some(layout) = l {
                layout.groups.retain(|g| g.id != group_id);
                for zone in &mut layout.zones {
                    if zone.group_id.as_deref() == Some(&group_id) {
                        zone.group_id = None;
                    }
                }
            }
        });
        set_is_dirty.set(true);
    };

    view! {
        <div class="p-3 space-y-4">
            // Groups section
            <div class="space-y-2">
                <div class="flex items-center justify-between">
                    <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                        <Icon icon=LuGroup width="12px" height="12px" />
                        "Groups"
                    </h3>
                    <button
                        class="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium border whitespace-nowrap transition-all btn-press"
                        style="background: rgba(225, 53, 255, 0.06); border-color: rgba(225, 53, 255, 0.15); color: rgb(225, 53, 255)"
                        on:click=move |_| create_group()
                    >
                        <Icon icon=LuPlus width="10px" height="10px" />
                        "New"
                    </button>
                </div>

                {move || {
                    let current_groups = groups.get();
                    let counts = group_zone_counts.get();
                    if current_groups.is_empty() {
                        view! {
                            <div class="text-[10px] text-fg-tertiary/50 italic">"No groups yet"</div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex flex-wrap gap-1.5">
                                {current_groups.into_iter().map(|group| {
                                    let gid_delete = group.id.clone();
                                    let color = group.color.clone().unwrap_or_else(|| "#e135ff".to_string());
                                    let rgb = hex_to_rgb(&color);
                                    let count = counts.get(&group.id).copied().unwrap_or(0);
                                    view! {
                                        <div
                                            class="flex items-center gap-1 px-2 py-1 rounded-full text-[13px] font-medium border
                                                   chip-interactive cursor-pointer group/chip"
                                            style=format!(
                                                "color: rgb({rgb}); border-color: rgba({rgb}, 0.25); background: rgba({rgb}, 0.08); \
                                                 --glow-rgb: {rgb}"
                                            )
                                        >
                                            <div
                                                class="w-2 h-2 rounded-full shrink-0"
                                                style=format!("background: rgb({rgb})")
                                            />
                                            {group.name}
                                            <span class="text-[9px] opacity-60">{count}</span>
                                            <button
                                                class="ml-0.5 opacity-0 group-hover/chip:opacity-60 hover:opacity-100 transition-opacity"
                                                on:click=move |ev| {
                                                    ev.stop_propagation();
                                                    delete_group(gid_delete.clone());
                                                }
                                            >
                                                <Icon icon=LuX width="10px" height="10px" />
                                            </button>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            // Separator
            <div class="h-px bg-edge-subtle" />

            // Devices section
            <div class="space-y-2">
                <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                    <Icon icon=LuCpu width="12px" height="12px" />
                    "Devices"
                </h3>

                {move || {
                    let devices = stable_devices.get();
                    if devices.is_empty() {
                        return view! {
                            <div class="flex flex-col items-center py-6 space-y-2">
                                <Icon icon=LuCpu width="24px" height="24px" style="color: rgba(139, 133, 160, 0.2)" />
                                <div class="text-[10px] text-fg-tertiary">"No devices connected"</div>
                            </div>
                        }.into_any();
                    }

                    view! {
                        <div class="space-y-2">
                            {devices.into_iter().enumerate().map(|(idx, dev)| {
                                        let device_id = dev.layout_device_id.clone();
                                        let device_name = dev.name.clone();
                                        let connection_label = dev.connection_label.clone();
                                        let backend = dev.backend.clone();
                                        let (primary_rgb, secondary_rgb) = device_accent_colors(&device_id);
                                        let fallback_leds = dev.total_leds;
                                        let has_multi_zones = dev.zones.len() > 1;
                                        let zone_count = dev.zones.len();

                                        // --- Single-zone handling ---
                                        let single_zone_summary = (!has_multi_zones)
                                            .then(|| dev.zones.first().cloned())
                                            .flatten();
                                        let single_topo = (!has_multi_zones)
                                            .then(|| topology_icon(dev.zones.first()));

                                        // Layout membership for single-zone device
                                        let single_zone_in_layout = {
                                            let did = device_id.clone();
                                            let zone_name_key = single_zone_summary
                                                .as_ref()
                                                .map(|z| z.name.clone());
                                            Signal::derive(move || {
                                                if has_multi_zones {
                                                    return false;
                                                }
                                                layout.with(|current| {
                                                    current
                                                        .as_ref()
                                                        .map(|l| {
                                                            l.zones.iter().any(|z| {
                                                                if z.device_id != did {
                                                                    return false;
                                                                }
                                                                match zone_name_key.as_deref() {
                                                                    Some(name) => {
                                                                        z.zone_name.as_deref()
                                                                            == Some(name)
                                                                    }
                                                                    None => z.zone_name.is_none(),
                                                                }
                                                            })
                                                        })
                                                        .unwrap_or(false)
                                                })
                                            })
                                        };

                                        // Is any zone for this device currently selected on canvas?
                                        let device_is_active = {
                                            let did = device_id.clone();
                                            Signal::derive(move || {
                                                let Some(sel_zone) = selected_zone_id.get() else {
                                                    return false;
                                                };
                                                layout.with(|current| {
                                                    current
                                                        .as_ref()
                                                        .and_then(|l| l.zones.iter().find(|z| z.id == sel_zone))
                                                        .is_some_and(|z| z.device_id == did)
                                                })
                                            })
                                        };

                                        // Find first zone_id for this device in the layout
                                        let first_zone_id_in_layout = {
                                            let did = device_id.clone();
                                            Signal::derive(move || {
                                                layout.with(|current| {
                                                    current
                                                        .as_ref()
                                                        .and_then(|l| {
                                                            l.zones.iter().find(|z| z.device_id == did).map(|z| z.id.clone())
                                                        })
                                                })
                                            })
                                        };

                                        // --- Whole-device layout membership ---
                                        let any_zone_in_layout = {
                                            let did = device_id.clone();
                                            Signal::derive(move || {
                                                layout.with(|current| {
                                                    current.as_ref().is_some_and(|l| {
                                                        l.zones.iter().any(|z| z.device_id == did)
                                                    })
                                                })
                                            })
                                        };

                                        // --- Multi-zone handling ---
                                        let collapse_key = dev.layout_device_id.clone();
                                        let collapse_key2 = dev.layout_device_id.clone();
                                        let is_collapsed = {
                                            let key = collapse_key.clone();
                                            Signal::derive(move || {
                                                collapsed_devices.get().contains(&key)
                                            })
                                        };

                                        let rgb_for_indicator = primary_rgb.clone();
                                        let rgb_for_zones = primary_rgb.clone();
                                        let rgb = primary_rgb.clone();
                                        let rgb2 = secondary_rgb.clone();
                                        let mut entries: Vec<(
                                            Option<api::ZoneSummary>,
                                            String,
                                            usize,
                                        )> = if has_multi_zones {
                                            dev.zones
                                                .iter()
                                                .cloned()
                                                .map(|zone| {
                                                    let leds = zone.led_count;
                                                    (Some(zone.clone()), zone.name, leds)
                                                })
                                                .collect()
                                        } else {
                                            vec![]
                                        };
                                        entries.sort_by(|left, right| left.1.cmp(&right.1));

                                        let stagger =
                                            format!("animation-delay: {}ms", idx * 40);

                                        view! {
                                            <div
                                                class="rounded-lg overflow-hidden transition-all animate-fade-in-up"
                                                style=move || {
                                                    let active = device_is_active.get();
                                                    let border_opacity = if active { 0.5 } else { 0.15 };
                                                    let bg_opacity = if active { 0.08 } else { 0.03 };
                                                    let bg_opacity2 = if active { 0.04 } else { 0.01 };
                                                    let shadow = if active {
                                                        format!(
                                                            "box-shadow: 0 0 16px rgba({rgb}, 0.15), 0 2px 8px rgba(0,0,0,0.2); "
                                                        )
                                                    } else {
                                                        "box-shadow: 0 1px 4px rgba(0,0,0,0.15); ".to_string()
                                                    };
                                                    format!(
                                                        "--glow-rgb: {rgb}; {stagger}; \
                                                         border: 1px solid rgba({rgb}, {border_opacity}); \
                                                         background: linear-gradient(135deg, rgba({rgb}, {bg_opacity}), rgba({rgb2}, {bg_opacity2})); \
                                                         {shadow}"
                                                    )
                                                }
                                            >
                                                // Device header
                                                <button
                                                    class="w-full flex items-center gap-2 px-2.5 py-2 text-left transition-colors
                                                           hover:bg-white/[0.03]"
                                                    on:click=move |_| {
                                                        if has_multi_zones {
                                                            // Select first zone in layout (if any)
                                                            if let Some(zid) = first_zone_id_in_layout.get_untracked() {
                                                                set_selected_zone_id.set(Some(zid));
                                                            }
                                                            // Toggle collapse + fetch attachments on expand
                                                            let was_collapsed = collapsed_devices.get_untracked().contains(&collapse_key2);
                                                            set_collapsed_devices.update(
                                                                |set| {
                                                                    if set.contains(
                                                                        &collapse_key2,
                                                                    ) {
                                                                        set.remove(
                                                                            &collapse_key2,
                                                                        );
                                                                    } else {
                                                                        set.insert(
                                                                            collapse_key2
                                                                                .clone(),
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                            if was_collapsed {
                                                                fetch_attachments(collapse_key.clone());
                                                            }
                                                        } else {
                                                            // Select zone on canvas if it's in layout
                                                            if let Some(zid) = first_zone_id_in_layout.get_untracked() {
                                                                set_selected_zone_id.set(Some(zid));
                                                            }
                                                        }
                                                    }
                                                >
                                                    // Device accent gradient strip
                                                    <div
                                                        class="w-1 self-stretch rounded-full shrink-0"
                                                        style=format!(
                                                            "background: linear-gradient(180deg, rgb({primary_rgb}), rgb({secondary_rgb}))"
                                                        )
                                                    />
                                                    <div class="flex-1 min-w-0">
                                                        <div class="flex items-center gap-1.5">
                                                            <span class="text-[11px] font-medium text-fg-primary truncate">
                                                                {device_name}
                                                            </span>
                                                            <span
                                                                class="text-[8px] font-mono uppercase tracking-wider px-1 py-0.5 rounded border shrink-0"
                                                                style=format!(
                                                                    "color: rgba({primary_rgb}, 0.8); border-color: rgba({primary_rgb}, 0.2); background: rgba({primary_rgb}, 0.06)"
                                                                )
                                                            >
                                                                {backend}
                                                            </span>
                                                        </div>
                                                        {connection_label.as_ref().map(|label| {
                                                            view! {
                                                                <div class="text-[9px] font-mono text-fg-tertiary/80 truncate mt-0.5">
                                                                    {label.clone()}
                                                                </div>
                                                            }
                                                        })}
                                                        <div class="text-[10px] text-fg-tertiary font-mono flex items-center gap-1.5 mt-0.5">
                                                            <span>
                                                                {fallback_leds} " LEDs"
                                                            </span>
                                                            {has_multi_zones
                                                                .then(|| {
                                                                    view! {
                                                                        <>
                                                                            <span class="opacity-40">
                                                                                "·"
                                                                            </span>
                                                                            <span>
                                                                                {zone_count}
                                                                                " zones"
                                                                            </span>
                                                                        </>
                                                                    }
                                                                })}
                                                        </div>
                                                    </div>

                                                    // Right side: chevron + add/remove-all (multi) or toggle (single)
                                                    {if has_multi_zones {
                                                        let toggle_all_rgb = rgb_for_indicator.clone();
                                                        let toggle_all_did = device_id.clone();
                                                        let toggle_all_dname = dev.name.clone();
                                                        let toggle_all_zones = dev.zones.clone();
                                                        view! {
                                                            <div class="shrink-0 flex items-center gap-1">
                                                                // Add-all / remove-all toggle
                                                                {move || {
                                                                    let did = toggle_all_did.clone();
                                                                    let dname = toggle_all_dname.clone();
                                                                    let zones = toggle_all_zones.clone();
                                                                    if any_zone_in_layout.get() {
                                                                        view! {
                                                                            <button
                                                                                class="w-5 h-5 flex items-center justify-center rounded
                                                                                       transition-all shrink-0 opacity-30 hover:opacity-80 btn-press"
                                                                                style="color: var(--color-text-tertiary)"
                                                                                title="Remove all zones"
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    remove_all_device_zones(
                                                                                        &did,
                                                                                        &set_layout,
                                                                                        &set_selected_zone_id,
                                                                                        &set_is_dirty,
                                                                                    );
                                                                                }
                                                                            >
                                                                                <Icon icon=LuX width="10px" height="10px" />
                                                                            </button>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <button
                                                                                class="w-6 h-6 flex items-center justify-center rounded-md
                                                                                       border transition-all shrink-0 btn-press"
                                                                                style=format!(
                                                                                    "background: rgba({toggle_all_rgb}, 0.08); border-color: rgba({toggle_all_rgb}, 0.2); color: rgb({toggle_all_rgb})"
                                                                                )
                                                                                title="Add all zones"
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    add_all_device_zones(
                                                                                        &did,
                                                                                        &dname,
                                                                                        &zones,
                                                                                        fallback_leds,
                                                                                        &layout,
                                                                                        &set_layout,
                                                                                        &set_selected_zone_id,
                                                                                        &set_is_dirty,
                                                                                    );
                                                                                }
                                                                            >
                                                                                <Icon icon=LuPlus width="12px" height="12px" />
                                                                            </button>
                                                                        }.into_any()
                                                                    }
                                                                }}
                                                                // Expand/collapse chevron
                                                                <div
                                                                    class="text-fg-tertiary"
                                                                    style=move || {
                                                                        if is_collapsed.get() {
                                                                            "transform: rotate(-90deg); transition: transform 0.2s ease"
                                                                        } else {
                                                                            "transition: transform 0.2s ease"
                                                                        }
                                                                    }
                                                                >
                                                                    <Icon
                                                                        icon=LuChevronDown
                                                                        width="14px"
                                                                        height="14px"
                                                                    />
                                                                </div>
                                                            </div>
                                                        }
                                                            .into_any()
                                                    } else {
                                                        let toggle_rgb = rgb_for_indicator.clone();
                                                        let toggle_did = device_id.clone();
                                                        let toggle_dname = dev.name.clone();
                                                        let toggle_zone = single_zone_summary.clone();
                                                        view! {
                                                            <div class="shrink-0 flex items-center gap-1.5">
                                                                {single_topo}
                                                                {move || {
                                                                    let did = toggle_did.clone();
                                                                    let zone = toggle_zone.clone();
                                                                    let dname = toggle_dname.clone();
                                                                    if single_zone_in_layout.get() {
                                                                        view! {
                                                                            <button
                                                                                class="w-5 h-5 flex items-center justify-center rounded
                                                                                       transition-all shrink-0 opacity-30 hover:opacity-80 btn-press"
                                                                                style="color: var(--color-text-tertiary)"
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    remove_device_zone(
                                                                                        &did,
                                                                                        zone.as_ref().map(|z| z.name.as_str()),
                                                                                        &set_layout,
                                                                                        &set_selected_zone_id,
                                                                                        &set_is_dirty,
                                                                                    );
                                                                                }
                                                                            >
                                                                                <Icon icon=LuX width="10px" height="10px" />
                                                                            </button>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <button
                                                                                class="w-6 h-6 flex items-center justify-center rounded-md
                                                                                       border transition-all shrink-0 btn-press"
                                                                                style=format!(
                                                                                    "background: rgba({toggle_rgb}, 0.08); border-color: rgba({toggle_rgb}, 0.2); color: rgb({toggle_rgb})"
                                                                                )
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    let (canvas_width, canvas_height) =
                                                                                        current_canvas_dimensions(&layout);
                                                                                    let new_zone = create_default_zone(
                                                                                        &did,
                                                                                        &dname,
                                                                                        zone.as_ref(),
                                                                                        fallback_leds,
                                                                                        canvas_width,
                                                                                        canvas_height,
                                                                                    );
                                                                                    let zone_id = new_zone.id.clone();
                                                                                    set_layout.update(|l| {
                                                                                        if let Some(layout) = l {
                                                                                            layout.zones.push(new_zone);
                                                                                        }
                                                                                    });
                                                                                    set_selected_zone_id.set(Some(zone_id));
                                                                                    set_is_dirty.set(true);
                                                                                }
                                                                            >
                                                                                <Icon icon=LuPlus width="12px" height="12px" />
                                                                            </button>
                                                                        }.into_any()
                                                                    }
                                                                }}
                                                            </div>
                                                        }
                                                            .into_any()
                                                    }}
                                                </button>

                                                // Zone rows (multi-zone only)
                                                {has_multi_zones.then(|| {
                                                    let device_id = device_id.clone();
                                                    view! {
                                                        <div
                                                            class="border-t px-1.5 py-1 space-y-0.5 overflow-hidden transition-all duration-200"
                                                            style=move || {
                                                                if is_collapsed.get() {
                                                                    "max-height: 0; opacity: 0; padding: 0; border: none".to_string()
                                                                } else {
                                                                    format!("max-height: 500px; opacity: 1; border-color: rgba({primary_rgb}, 0.1)")
                                                                }
                                                            }
                                                        >
                                                            {entries
                                                                .into_iter()
                                                                .map(
                                                                    |(
                                                                        zone_summary,
                                                                        display_name,
                                                                        led_count,
                                                                    )| {
                                                                        let zone_name_key =
                                                                            zone_summary
                                                                                .as_ref()
                                                                                .map(|z| {
                                                                                    z.name.clone()
                                                                                });
                                                                        let in_layout = {
                                                                            let did =
                                                                                device_id.clone();
                                                                            let zone_name =
                                                                                zone_name_key
                                                                                    .clone();
                                                                            Signal::derive(
                                                                                move || {
                                                                                    layout.with(
                                                                                    |current| {
                                                                                        current
                                                                                            .as_ref()
                                                                                            .map(
                                                                                                |l| {
                                                                                                    l.zones.iter().any(|z| {
                                                                                                        if z.device_id != did {
                                                                                                            return false;
                                                                                                        }
                                                                                                        match zone_name.as_deref() {
                                                                                                            Some(name) => z.zone_name.as_deref() == Some(name),
                                                                                                            None => z.zone_name.is_none(),
                                                                                                        }
                                                                                                    })
                                                                                                },
                                                                                            )
                                                                                            .unwrap_or(
                                                                                                false,
                                                                                            )
                                                                                    },
                                                                                )
                                                                                },
                                                                            )
                                                                        };

                                                                        // Is this specific zone selected?
                                                                        let zone_is_selected = {
                                                                            let did = device_id.clone();
                                                                            let zn = zone_name_key.clone();
                                                                            Signal::derive(move || {
                                                                                let Some(sel) = selected_zone_id.get() else {
                                                                                    return false;
                                                                                };
                                                                                layout.with(|current| {
                                                                                    current.as_ref()
                                                                                        .and_then(|l| l.zones.iter().find(|z| z.id == sel))
                                                                                        .is_some_and(|z| {
                                                                                            z.device_id == did && z.zone_name.as_deref() == zn.as_deref()
                                                                                        })
                                                                                })
                                                                            })
                                                                        };

                                                                        // Find zone_id to select on click
                                                                        let zone_id_for_select = {
                                                                            let did = device_id.clone();
                                                                            let zn = zone_name_key.clone();
                                                                            Signal::derive(move || {
                                                                                layout.with(|current| {
                                                                                    current.as_ref().and_then(|l| {
                                                                                        l.zones.iter().find(|z| {
                                                                                            z.device_id == did && z.zone_name.as_deref() == zn.as_deref()
                                                                                        }).map(|z| z.id.clone())
                                                                                    })
                                                                                })
                                                                            })
                                                                        };

                                                                        let topo_icon =
                                                                            topology_icon(
                                                                                zone_summary
                                                                                    .as_ref(),
                                                                            );
                                                                        let zone_for_toggle =
                                                                            zone_summary.clone();
                                                                        let did_for_toggle =
                                                                            device_id.clone();
                                                                        let dname_for_toggle =
                                                                            dev.name.clone();
                                                                        let zone_rgb2 = rgb_for_zones.clone();
                                                                        let zone_rgb3 = rgb_for_zones.clone();
                                                                        let toggle_zone_name = zone_name_key.clone();

                                                                        // Attachment binding for this zone/slot
                                                                        let binding_zone_name = display_name.clone();
                                                                        let binding_device_id = device_id.clone();
                                                                        let zone_binding = Signal::derive(move || {
                                                                            let cache = attachment_cache.get();
                                                                            cache.get(&binding_device_id).and_then(|bindings| {
                                                                                bindings.iter().find(|b| {
                                                                                    b.slot_id.eq_ignore_ascii_case(&binding_zone_name)
                                                                                        || b.slot_id == binding_zone_name
                                                                                }).cloned()
                                                                            })
                                                                        });

                                                                        view! {
                                                                            <div
                                                                                class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg
                                                                                        cursor-pointer hover:bg-white/[0.04] transition-all group/zone"
                                                                                style=move || {
                                                                                    if zone_is_selected.get() {
                                                                                        format!(
                                                                                            "background: rgba({zone_rgb2}, 0.08); \
                                                                                             box-shadow: inset 2px 0 0 rgb({zone_rgb2})"
                                                                                        )
                                                                                    } else {
                                                                                        String::new()
                                                                                    }
                                                                                }
                                                                                on:click=move |_| {
                                                                                    if let Some(zid) = zone_id_for_select.get_untracked() {
                                                                                        set_selected_zone_id.set(Some(zid));
                                                                                    }
                                                                                }
                                                                            >
                                                                                // Topology icon
                                                                                <div class="text-fg-tertiary/50 shrink-0">
                                                                                    {topo_icon}
                                                                                </div>
                                                                                <div class="flex-1 min-w-0">
                                                                                    <div class="text-[11px] text-fg-primary truncate">
                                                                                        {display_name}
                                                                                    </div>
                                                                                    <div class="text-[8px] text-fg-tertiary/60 font-mono tabular-nums">
                                                                                        {led_count}
                                                                                        " LEDs"
                                                                                    </div>
                                                                                </div>
                                                                                // Toggle button — add or remove zone
                                                                                {move || {
                                                                                    let did = did_for_toggle.clone();
                                                                                    let zn = toggle_zone_name.clone();
                                                                                    let zone_entry = zone_for_toggle.clone();
                                                                                    let dname = dname_for_toggle.clone();
                                                                                    if in_layout.get() {
                                                                                        view! {
                                                                                            <button
                                                                                                class="w-5 h-5 flex items-center justify-center rounded
                                                                                                       transition-all shrink-0 opacity-0 group-hover/zone:opacity-30
                                                                                                       hover:!opacity-80 btn-press"
                                                                                                style="color: var(--color-text-tertiary)"
                                                                                                on:click=move |ev| {
                                                                                                    ev.stop_propagation();
                                                                                                    remove_device_zone(
                                                                                                        &did,
                                                                                                        zn.as_deref(),
                                                                                                        &set_layout,
                                                                                                        &set_selected_zone_id,
                                                                                                        &set_is_dirty,
                                                                                                    );
                                                                                                }
                                                                                            >
                                                                                                <Icon icon=LuX width="10px" height="10px" />
                                                                                            </button>
                                                                                        }.into_any()
                                                                                    } else {
                                                                                        view! {
                                                                                            <button
                                                                                                class="w-6 h-6 flex items-center justify-center rounded-md
                                                                                                       border transition-all shrink-0 btn-press"
                                                                                                style=format!(
                                                                                                    "background: rgba({zone_rgb3}, 0.08); border-color: rgba({zone_rgb3}, 0.2); color: rgb({zone_rgb3})"
                                                                                                )
                                                                                                on:click=move |ev| {
                                                                                                    ev.stop_propagation();
                                                                                                    let (canvas_width, canvas_height) =
                                                                                                        current_canvas_dimensions(&layout);
                                                                                                    let new_zone = create_default_zone(
                                                                                                        &did,
                                                                                                        &dname,
                                                                                                        zone_entry.as_ref(),
                                                                                                        fallback_leds,
                                                                                                        canvas_width,
                                                                                                        canvas_height,
                                                                                                    );
                                                                                                    let zone_id = new_zone.id.clone();
                                                                                                    set_layout.update(|l| {
                                                                                                        if let Some(layout) = l {
                                                                                                            layout.zones.push(new_zone);
                                                                                                        }
                                                                                                    });
                                                                                                    set_selected_zone_id.set(Some(zone_id));
                                                                                                    set_is_dirty.set(true);
                                                                                                }
                                                                                            >
                                                                                                <Icon icon=LuPlus width="12px" height="12px" />
                                                                                            </button>
                                                                                        }.into_any()
                                                                                    }
                                                                                }}
                                                                            </div>
                                                                        }
                                                                    },
                                                                )
                                                                .collect_view()}
                                                        </div>
                                                    }
                                                })}
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                                </div>
                    }
                        .into_any()
                }}
            </div>

            // Separator
            <div class="h-px bg-edge-subtle" />

            // Attachments section — configure device attachments from the layout page
            <div class="space-y-2">
                <div class="flex items-center justify-between">
                    <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                        <Icon icon=LuCable width="12px" height="12px" />
                        "Attachments"
                    </h3>
                </div>

                // Device picker for attachments
                <select
                    class="w-full bg-surface-sunken border border-edge-subtle rounded-lg px-2.5 py-1.5 text-[11px] text-fg-primary
                           focus:outline-none focus:border-accent-muted glow-ring transition-all"
                    on:change=move |ev| {
                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                        if let Some(el) = target {
                            let val = el.value();
                            if val.is_empty() {
                                set_attachment_device_id.set(None);
                            } else {
                                set_attachment_device_id.set(Some(val));
                            }
                        }
                    }
                >
                    <option value="" selected=move || attachment_device_id.get().is_none()>"Select device..."</option>
                    {move || {
                        stable_devices.get().into_iter().map(|dev| {
                            let did = dev.id.clone();
                            let did2 = dev.id.clone();
                            let label = format!("{} ({} LEDs)", dev.name, dev.total_leds);
                            view! {
                                <option
                                    value=did
                                    selected=move || attachment_device_id.get().as_deref() == Some(&did2)
                                >
                                    {label}
                                </option>
                            }
                        }).collect_view()
                    }}
                </select>

                // Inline attachment panel for selected device
                {move || {
                    let selected_id = attachment_device_id.get()?;
                    let devices = stable_devices.get();
                    let device = devices.into_iter().find(|d| d.id == selected_id)?;

                    let device_id_signal = Signal::derive({
                        let id = selected_id.clone();
                        move || id.clone()
                    });
                    let device_signal = Signal::derive({
                        let dev = device.clone();
                        move || Some(dev.clone())
                    });

                    Some(view! {
                        <div class="animate-fade-in">
                            <AttachmentPanel device_id=device_id_signal device=device_signal />
                        </div>
                    })
                }}
            </div>
        </div>
    }
}

/// Return an appropriate icon view based on zone topology.
fn topology_icon(zone: Option<&api::ZoneSummary>) -> leptos::prelude::AnyView {
    match zone.and_then(|z| z.topology_hint.as_ref()) {
        Some(ZoneTopologySummary::Strip) => {
            view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Matrix { .. }) => {
            view! { <Icon icon=LuGrid2x2 width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Ring { .. }) => {
            view! { <Icon icon=LuCircle width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Point) => {
            view! { <Icon icon=LuCircleDot width="12px" height="12px" /> }.into_any()
        }
        _ => view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any(),
    }
}

/// Create a default `DeviceZone` placed at canvas center.
fn create_default_zone(
    device_id: &str,
    device_name: &str,
    zone: Option<&api::ZoneSummary>,
    total_leds: usize,
    canvas_width: u32,
    canvas_height: u32,
) -> DeviceZone {
    let defaults = layout_geometry::default_zone_visuals(
        device_name,
        zone,
        total_leds,
        canvas_width,
        canvas_height,
    );
    let zone_name = zone.map(|z| z.name.clone());
    let display_name = zone.map_or_else(
        || device_name.to_owned(),
        |z| {
            if z.name.eq_ignore_ascii_case(device_name) {
                device_name.to_owned()
            } else {
                format!("{device_name} · {}", z.name)
            }
        },
    );

    DeviceZone {
        id: format!("zone_{}", uuid_v4_hex()),
        name: display_name,
        device_id: device_id.to_string(),
        zone_name,
        position: NormalizedPosition::new(0.5, 0.5),
        size: layout_geometry::normalize_zone_size_for_editor(
            NormalizedPosition::new(0.5, 0.5),
            defaults.size,
            &defaults.topology,
        ),
        rotation: 0.0,
        scale: 1.0,
        orientation: defaults.orientation,
        topology: defaults.topology,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: defaults.shape,
        shape_preset: defaults.shape_preset,
        group_id: None,
        attachment: None,
    }
}

fn current_canvas_dimensions(layout: &Signal<Option<SpatialLayout>>) -> (u32, u32) {
    layout.with_untracked(|current| {
        current
            .as_ref()
            .map(|layout| (layout.canvas_width.max(1), layout.canvas_height.max(1)))
            .unwrap_or((320, 200))
    })
}

/// Remove a device zone from the layout by device_id + zone_name.
fn remove_device_zone(
    device_id: &str,
    zone_name: Option<&str>,
    set_layout: &WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_is_dirty: &WriteSignal<bool>,
) {
    set_layout.update(|l| {
        if let Some(layout) = l {
            layout
                .zones
                .retain(|z| !(z.device_id == device_id && z.zone_name.as_deref() == zone_name));
        }
    });
    set_selected_zone_id.set(None);
    set_is_dirty.set(true);
}

/// Remove ALL zones for a device from the layout in one action.
fn remove_all_device_zones(
    device_id: &str,
    set_layout: &WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_is_dirty: &WriteSignal<bool>,
) {
    set_layout.update(|l| {
        if let Some(layout) = l {
            layout.zones.retain(|z| z.device_id != device_id);
        }
    });
    set_selected_zone_id.set(None);
    set_is_dirty.set(true);
}

/// Add ALL zones for a device to the layout in one action, skipping any already present.
#[allow(clippy::too_many_arguments)]
fn add_all_device_zones(
    device_id: &str,
    device_name: &str,
    zones: &[api::ZoneSummary],
    total_leds: usize,
    layout: &Signal<Option<SpatialLayout>>,
    set_layout: &WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_is_dirty: &WriteSignal<bool>,
) {
    let (canvas_width, canvas_height) = current_canvas_dimensions(layout);
    let existing_zone_names: std::collections::HashSet<Option<String>> =
        layout.with_untracked(|current| {
            current
                .as_ref()
                .map(|l| {
                    l.zones
                        .iter()
                        .filter(|z| z.device_id == device_id)
                        .map(|z| z.zone_name.clone())
                        .collect()
                })
                .unwrap_or_default()
        });

    let mut first_new_id = None;
    set_layout.update(|l| {
        if let Some(current_layout) = l {
            for zone in zones {
                let zn = Some(zone.name.clone());
                if existing_zone_names.contains(&zn) {
                    continue;
                }
                let new_zone = create_default_zone(
                    device_id,
                    device_name,
                    Some(zone),
                    total_leds,
                    canvas_width,
                    canvas_height,
                );
                if first_new_id.is_none() {
                    first_new_id = Some(new_zone.id.clone());
                }
                current_layout.zones.push(new_zone);
            }
        }
    });

    if let Some(id) = first_new_id {
        set_selected_zone_id.set(Some(id));
    }
    set_is_dirty.set(true);
}

/// Generate a short pseudo-random hex ID.
fn uuid_v4_hex() -> String {
    let r = js_sys::Math::random();
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let n = (r * 4_294_967_295.0) as u32;
    format!("{n:08x}")
}

/// Convert a hex color like "#e135ff" to "225, 53, 255" RGB string.
fn hex_to_rgb(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return "225, 53, 255".to_string();
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(225);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(53);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);
    format!("{r}, {g}, {b}")
}
