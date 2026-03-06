//! Layout palette — available devices to add to the layout.

use std::f32::consts::FRAC_PI_2;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, ZoneTopologySummary};
use crate::app::DevicesContext;
use crate::components::device_card::backend_accent_rgb;
use crate::icons::*;
use hypercolor_types::spatial::{
    Corner, DeviceZone, LedTopology, NormalizedPosition, Orientation, SpatialLayout,
    StripDirection, Winding, ZoneShape,
};

/// Device palette for adding zones to the layout.
#[component]
pub fn LayoutPalette(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    set_is_dirty: WriteSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    view! {
        <div class="p-3 space-y-3">
            <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-dim">"Devices"</h3>

            <Suspense fallback=|| view! {
                <div class="text-xs text-fg-dim animate-pulse">"Loading..."</div>
            }>
                {move || {
                    ctx.devices_resource.get().map(|result| {
                        let devices = result.unwrap_or_default();
                        if devices.is_empty() {
                            return view! {
                                <div class="text-xs text-fg-dim">"No devices connected"</div>
                            }.into_any();
                        }

                        // Group by backend
                        let mut backends: std::collections::BTreeMap<String, Vec<_>> = std::collections::BTreeMap::new();
                        for dev in &devices {
                            backends.entry(dev.backend.clone()).or_default().push(dev.clone());
                        }

                        view! {
                            <div class="space-y-3">
                                {backends.into_iter().map(|(backend, devs)| {
                                    let rgb = backend_accent_rgb(&backend).to_string();
                                    let badge_style = format!(
                                        "color: rgb({rgb}); border-color: rgba({rgb}, 0.2); background: rgba({rgb}, 0.06)"
                                    );
                                    view! {
                                        <div>
                                            <div
                                                class="text-[9px] font-mono uppercase tracking-wider px-1.5 py-0.5 rounded border mb-1.5 inline-block"
                                                style=badge_style
                                            >
                                                {backend}
                                            </div>
                                            <div class="space-y-1">
                                                {devs.into_iter().map(|dev| {
                                                    let device_id = dev.layout_device_id.clone();
                                                    let device_name = dev.name.clone();
                                                    let fallback_leds = dev.total_leds;
                                                    let mut entries: Vec<(Option<api::ZoneSummary>, String, usize)> = if dev.zones.is_empty() {
                                                        vec![(None, dev.name.clone(), dev.total_leds)]
                                                    } else {
                                                        dev.zones
                                                            .iter()
                                                            .cloned()
                                                            .map(|zone| {
                                                                let display_name = if dev.zones.len() > 1 {
                                                                    format!("{} · {}", dev.name, zone.name)
                                                                } else {
                                                                    dev.name.clone()
                                                                };
                                                                let leds = zone.led_count;
                                                                (Some(zone), display_name, leds)
                                                            })
                                                            .collect()
                                                    };
                                                    entries.sort_by(|left, right| left.1.cmp(&right.1));

                                                    entries
                                                        .into_iter()
                                                        .map(|(zone_summary, display_name, led_count)| {
                                                            let zone_name_key = zone_summary.as_ref().map(|z| z.name.clone());
                                                                let in_layout = {
                                                                    let did = device_id.clone();
                                                                    let zone_name = zone_name_key.clone();
                                                                    Signal::derive(move || {
                                                                        layout.with(|current| {
                                                                            current
                                                                                .as_ref()
                                                                                .map(|l| {
                                                                                    l.zones.iter().any(|z| {
                                                                                        if z.device_id != did {
                                                                                            return false;
                                                                                        }
                                                                                        match zone_name.as_deref() {
                                                                                            Some(name) => z.zone_name.as_deref() == Some(name),
                                                                                            None => z.zone_name.is_none(),
                                                                                        }
                                                                                    })
                                                                                })
                                                                                .unwrap_or(false)
                                                                        })
                                                                    })
                                                                };

                                                            let topology_chip = zone_summary
                                                                .as_ref()
                                                                .map(topology_label)
                                                                .unwrap_or_else(|| "strip".to_owned());
                                                            let zone_for_add = zone_summary.clone();
                                                            let did_for_add = device_id.clone();
                                                            let dname_for_add = device_name.clone();
                                                            let display_led_count = led_count;

                                                            view! {
                                                                <div class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg bg-layer-2/40 border border-white/[0.03]
                                                                            hover:bg-layer-2/60 hover:border-white/[0.06] transition-all group">
                                                                    <div class="flex-1 min-w-0">
                                                                        <div class="text-[11px] text-fg truncate">{display_name}</div>
                                                                        <div class="text-[9px] text-fg-dim font-mono flex items-center gap-1.5">
                                                                            <span>{display_led_count} " LEDs"</span>
                                                                            <span class="opacity-60">"·"</span>
                                                                            <span class="uppercase tracking-wide">{topology_chip}</span>
                                                                        </div>
                                                                    </div>
                                                                    {move || {
                                                                        if in_layout.get() {
                                                                            view! {
                                                                                <Icon icon=LuCheck width="14px" height="14px" style="color: rgba(80, 250, 123, 0.6); flex-shrink: 0" />
                                                                            }.into_any()
                                                                        } else {
                                                                            let zone_entry = zone_for_add.clone();
                                                                            let did = did_for_add.clone();
                                                                            let dname = dname_for_add.clone();
                                                                            view! {
                                                                                <button
                                                                                    class="px-1.5 py-0.5 rounded text-[9px] font-medium text-electric-purple
                                                                                           bg-electric-purple/[0.08] border border-electric-purple/20
                                                                                           hover:bg-electric-purple/[0.15] transition-all opacity-0
                                                                                           group-hover:opacity-100 shrink-0"
                                                                                    on:click=move |_| {
                                                                                        let zone = create_default_zone(
                                                                                            &did,
                                                                                            &dname,
                                                                                            zone_entry.as_ref(),
                                                                                            fallback_leds,
                                                                                        );
                                                                                        let zone_id = zone.id.clone();
                                                                                        set_layout.update(|l| {
                                                                                            if let Some(layout) = l {
                                                                                                layout.zones.push(zone);
                                                                                            }
                                                                                        });
                                                                                        set_selected_zone_id.set(Some(zone_id));
                                                                                        set_is_dirty.set(true);
                                                                                    }
                                                                                >"Add"</button>
                                                                            }.into_any()
                                                                        }
                                                                    }}
                                                                </div>
                                                            }
                                                        })
                                                        .collect_view()
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    })
                }}
            </Suspense>
        </div>
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
        sampling_mode: None,
        edge_behavior: None,
        shape: defaults.shape,
        shape_preset: defaults.shape_preset,
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
    // by SDKs: strimer cables, fan rings, and AIO pump/radiator zones.
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

fn topology_label(zone: &api::ZoneSummary) -> String {
    match zone.topology_hint.as_ref() {
        Some(ZoneTopologySummary::Strip) => "strip".to_owned(),
        Some(ZoneTopologySummary::Matrix { rows, cols }) => format!("matrix {rows}x{cols}"),
        Some(ZoneTopologySummary::Ring { count }) => format!("ring {count}"),
        Some(ZoneTopologySummary::Point) => "point".to_owned(),
        Some(ZoneTopologySummary::Custom) => "custom".to_owned(),
        None => zone.topology.clone(),
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
