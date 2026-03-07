//! Layout palette — available devices + zone group management.

use std::f32::consts::FRAC_PI_2;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, ZoneTopologySummary};
use crate::app::DevicesContext;
use crate::components::device_card::backend_accent_rgb;
use crate::icons::*;
use hypercolor_types::spatial::{
    Corner, DeviceZone, LedTopology, NormalizedPosition, Orientation, SpatialLayout,
    StripDirection, Winding, ZoneGroup, ZoneShape,
};

/// Group color presets — works in both dark and light themes.
const GROUP_COLORS: &[&str] = &[
    "#e135ff", "#80ffea", "#ff6ac1", "#f1fa8c", "#50fa7b", "#82AAFF", "#ff9e64", "#c792ea",
];

/// Device palette for adding zones to the layout, with group management.
#[component]
pub fn LayoutPalette(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
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
                        class="px-1.5 py-0.5 rounded text-[10px] font-medium border transition-all btn-press"
                        style="background: rgba(225, 53, 255, 0.06); border-color: rgba(225, 53, 255, 0.15); color: rgb(225, 53, 255)"
                        on:click=move |_| create_group()
                    >
                        <Icon icon=LuPlus width="10px" height="10px" />
                        " New"
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
                                        let backend = dev.backend.clone();
                                        let rgb = backend_accent_rgb(&backend).to_string();
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

                                        // Clones for single-zone add handler
                                        let header_zone = single_zone_summary.clone();
                                        let header_did = device_id.clone();
                                        let header_dname = dev.name.clone();

                                        // --- Multi-zone handling ---
                                        let collapse_key = dev.layout_device_id.clone();
                                        let collapse_key2 = dev.layout_device_id.clone();
                                        let is_collapsed = {
                                            let key = collapse_key.clone();
                                            Signal::derive(move || {
                                                collapsed_devices.get().contains(&key)
                                            })
                                        };

                                        let rgb_for_indicator = rgb.clone();
                                        let rgb_for_zones = rgb.clone();
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
                                                class="rounded-xl border border-edge-subtle bg-surface-overlay/30 overflow-hidden
                                                       transition-all card-hover animate-fade-in-up"
                                                style=format!(
                                                    "--glow-rgb: {rgb}; {stagger}"
                                                )
                                            >
                                                // Device header
                                                <button
                                                    class="w-full flex items-center gap-2 px-2.5 py-2 text-left transition-colors
                                                           hover:bg-surface-hover/30"
                                                    on:click=move |_| {
                                                        if has_multi_zones {
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
                                                        } else if !single_zone_in_layout
                                                            .get_untracked()
                                                        {
                                                            let zone = create_default_zone(
                                                                &header_did,
                                                                &header_dname,
                                                                header_zone.as_ref(),
                                                                fallback_leds,
                                                            );
                                                            let zone_id = zone.id.clone();
                                                            set_layout.update(|l| {
                                                                if let Some(layout) = l {
                                                                    layout.zones.push(zone);
                                                                }
                                                            });
                                                            set_selected_zone_id
                                                                .set(Some(zone_id));
                                                            set_is_dirty.set(true);
                                                        }
                                                    }
                                                >
                                                    // Backend accent strip
                                                    <div
                                                        class="w-1 self-stretch rounded-full shrink-0"
                                                        style=format!(
                                                            "background: rgb({rgb})"
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
                                                                    "color: rgba({rgb}, 0.8); border-color: rgba({rgb}, 0.2); background: rgba({rgb}, 0.06)"
                                                                )
                                                            >
                                                                {backend}
                                                            </span>
                                                        </div>
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

                                                    // Right side: chevron (multi) or add/check (single)
                                                    {if has_multi_zones {
                                                        view! {
                                                            <div
                                                                class="text-fg-tertiary shrink-0"
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
                                                        }
                                                            .into_any()
                                                    } else {
                                                        view! {
                                                            <div class="shrink-0 flex items-center gap-1.5">
                                                                {single_topo}
                                                                {move || {
                                                                    if single_zone_in_layout.get() {
                                                                        view! {
                                                                            <div style="color: rgba(80, 250, 123, 0.6)">
                                                                                <Icon
                                                                                    icon=LuCheck
                                                                                    width="14px"
                                                                                    height="14px"
                                                                                />
                                                                            </div>
                                                                        }
                                                                            .into_any()
                                                                    } else {
                                                                        view! {
                                                                            <div style=format!(
                                                                                "color: rgb({rgb_for_indicator})"
                                                                            )>
                                                                                <Icon
                                                                                    icon=LuPlus
                                                                                    width="14px"
                                                                                    height="14px"
                                                                                />
                                                                            </div>
                                                                        }
                                                                            .into_any()
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
                                                            class="border-t border-edge-subtle/50 px-1.5 py-1 space-y-0.5 overflow-hidden transition-all duration-200"
                                                            style=move || {
                                                                if is_collapsed.get() {
                                                                    "max-height: 0; opacity: 0; padding: 0; border: none"
                                                                } else {
                                                                    "max-height: 500px; opacity: 1"
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

                                                                        let topo_icon =
                                                                            topology_icon(
                                                                                zone_summary
                                                                                    .as_ref(),
                                                                            );
                                                                        let zone_for_add =
                                                                            zone_summary.clone();
                                                                        let did_for_add =
                                                                            device_id.clone();
                                                                        let dname_for_add =
                                                                            dev.name.clone();
                                                                        let zone_rgb =
                                                                            rgb_for_zones.clone();

                                                                        view! {
                                                                            <div class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg
                                                                                        hover:bg-surface-hover/30 transition-all group/zone">
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
                                                                                {move || {
                                                                                    if in_layout.get() {
                                                                                        view! {
                                                                                            <div class="shrink-0" style="color: rgba(80, 250, 123, 0.5)">
                                                                                                <Icon
                                                                                                    icon=LuCheck
                                                                                                    width="12px"
                                                                                                    height="12px"
                                                                                                />
                                                                                            </div>
                                                                                        }
                                                                                            .into_any()
                                                                                    } else {
                                                                                        let zone_entry =
                                                                                            zone_for_add.clone();
                                                                                        let did =
                                                                                            did_for_add.clone();
                                                                                        let dname =
                                                                                            dname_for_add
                                                                                                .clone();
                                                                                        view! {
                                                                                            <button
                                                                                                class="w-6 h-6 flex items-center justify-center rounded-md
                                                                                                       border transition-all shrink-0 btn-press"
                                                                                                style=format!(
                                                                                                    "background: rgba({zone_rgb}, 0.08); border-color: rgba({zone_rgb}, 0.2); color: rgb({zone_rgb})"
                                                                                                )
                                                                                                on:click=move |_| {
                                                                                                    let zone = create_default_zone(
                                                                                                        &did,
                                                                                                        &dname,
                                                                                                        zone_entry.as_ref(),
                                                                                                        fallback_leds,
                                                                                                    );
                                                                                                    let zone_id = zone.id.clone();
                                                                                                    set_layout
                                                                                                        .update(|l| {
                                                                                                            if let Some(layout) = l {
                                                                                                                layout.zones.push(zone);
                                                                                                            }
                                                                                                        });
                                                                                                    set_selected_zone_id.set(Some(zone_id));
                                                                                                    set_is_dirty.set(true);
                                                                                                }
                                                                                            >
                                                                                                <Icon
                                                                                                    icon=LuPlus
                                                                                                    width="12px"
                                                                                                    height="12px"
                                                                                                />
                                                                                            </button>
                                                                                        }
                                                                                            .into_any()
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
) -> DeviceZone {
    #[allow(clippy::cast_possible_truncation)]
    let fallback_led_count = total_leds as u32;
    let defaults = defaults_for_zone(zone, fallback_led_count);
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
        size: defaults.size,
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

#[derive(Debug)]
struct ZoneDefaults {
    topology: LedTopology,
    size: NormalizedPosition,
    orientation: Option<Orientation>,
    shape: Option<ZoneShape>,
    shape_preset: Option<String>,
}

fn defaults_for_zone(zone: Option<&api::ZoneSummary>, fallback_led_count: u32) -> ZoneDefaults {
    #[allow(clippy::cast_possible_truncation)]
    let led_count = zone
        .map(|z| z.led_count)
        .map(|count| count as u32)
        .unwrap_or(fallback_led_count)
        .max(1);
    let zone_name = zone
        .map(|z| z.name.to_ascii_lowercase())
        .unwrap_or_default();
    let topology_hint = zone.and_then(|z| z.topology_hint.clone());

    // Keyword-first overrides for hardware families commonly exposed as "custom"
    if zone_name.contains("strimer") || zone_name.contains("cable") {
        let rows = if led_count >= 48 { 4 } else { 2 };
        let cols = (led_count / rows).max(8);
        return matrix_defaults(rows, cols, Some("strimer-generic"));
    }
    if zone_name.contains("fan") {
        return ring_defaults(led_count.max(12), Some("fan-ring"));
    }
    if zone_name.contains("aio") || zone_name.contains("pump") {
        return ring_defaults(led_count.max(12), Some("aio-pump-ring"));
    }
    if zone_name.contains("radiator") || zone_name.contains("rad") {
        return ZoneDefaults {
            topology: LedTopology::Strip {
                count: led_count,
                direction: StripDirection::LeftToRight,
            },
            size: NormalizedPosition::new(0.35, 0.08),
            orientation: Some(Orientation::Horizontal),
            shape: Some(ZoneShape::Rectangle),
            shape_preset: Some("aio-radiator-strip".to_owned()),
        };
    }

    match topology_hint {
        Some(ZoneTopologySummary::Strip) => ZoneDefaults {
            topology: LedTopology::Strip {
                count: led_count,
                direction: StripDirection::LeftToRight,
            },
            size: NormalizedPosition::new(if led_count > 80 { 0.4 } else { 0.26 }, 0.05),
            orientation: Some(Orientation::Horizontal),
            shape: Some(ZoneShape::Rectangle),
            shape_preset: None,
        },
        Some(ZoneTopologySummary::Matrix { rows, cols }) => matrix_defaults(rows, cols, None),
        Some(ZoneTopologySummary::Ring { count }) => ring_defaults(count, None),
        Some(ZoneTopologySummary::Point) => ZoneDefaults {
            topology: LedTopology::Point,
            size: NormalizedPosition::new(0.08, 0.08),
            orientation: None,
            shape: Some(ZoneShape::Ring),
            shape_preset: None,
        },
        Some(ZoneTopologySummary::Custom) | None => {
            if led_count <= 1 {
                ZoneDefaults {
                    topology: LedTopology::Point,
                    size: NormalizedPosition::new(0.08, 0.08),
                    orientation: None,
                    shape: Some(ZoneShape::Ring),
                    shape_preset: None,
                }
            } else {
                ZoneDefaults {
                    topology: LedTopology::Strip {
                        count: led_count,
                        direction: StripDirection::LeftToRight,
                    },
                    size: NormalizedPosition::new(0.24, 0.05),
                    orientation: Some(Orientation::Horizontal),
                    shape: Some(ZoneShape::Rectangle),
                    shape_preset: Some("generic-strip".to_owned()),
                }
            }
        }
    }
}

fn matrix_defaults(rows: u32, cols: u32, shape_preset: Option<&str>) -> ZoneDefaults {
    let clamped_rows = rows.max(1);
    let clamped_cols = cols.max(1);
    let aspect = clamped_cols as f32 / clamped_rows as f32;
    let width = (0.16 * aspect).clamp(0.12, 0.45);
    let height = (width / aspect).clamp(0.06, 0.25);

    ZoneDefaults {
        topology: LedTopology::Matrix {
            width: clamped_cols,
            height: clamped_rows,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
        size: NormalizedPosition::new(width, height),
        orientation: Some(if aspect >= 1.0 {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }),
        shape: Some(ZoneShape::Rectangle),
        shape_preset: shape_preset.map(str::to_owned),
    }
}

fn ring_defaults(count: u32, shape_preset: Option<&str>) -> ZoneDefaults {
    ZoneDefaults {
        topology: LedTopology::Ring {
            count: count.max(1),
            start_angle: -FRAC_PI_2,
            direction: Winding::Clockwise,
        },
        size: NormalizedPosition::new(0.16, 0.16),
        orientation: Some(Orientation::Radial),
        shape: Some(ZoneShape::Ring),
        shape_preset: shape_preset.map(str::to_owned),
    }
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
