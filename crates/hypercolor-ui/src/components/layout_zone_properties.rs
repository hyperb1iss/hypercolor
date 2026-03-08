//! Layout zone properties panel — horizontal editor below canvas viewport.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::icons::*;
use crate::layout_geometry::{self, SizeAxis};
use hypercolor_types::spatial::SpatialLayout;

/// Zone properties editor (bottom panel of layout builder).
#[component]
pub fn LayoutZoneProperties(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] selected_zone_id: Signal<Option<String>>,
    #[prop(into)] keep_aspect_ratio: Signal<bool>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    set_keep_aspect_ratio: WriteSignal<bool>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    set_is_dirty: WriteSignal<bool>,
) -> impl IntoView {
    // Derive selected zone snapshot for display
    let zone_snapshot = Signal::derive(move || {
        let id = selected_zone_id.get()?;
        layout.with(|current| {
            current
                .as_ref()
                .and_then(|l| l.zones.iter().find(|z| z.id == id).cloned())
        })
    });

    // Derive available groups
    let available_groups = Signal::derive(move || {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| l.groups.clone())
                .unwrap_or_default()
        })
    });

    // Helper to update a zone field
    let update_zone =
        move |zone_id: String,
              updater: Box<dyn FnOnce(&mut hypercolor_types::spatial::DeviceZone)>| {
            set_layout.update(|l| {
                if let Some(layout) = l {
                    if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                        updater(zone);
                        zone.size = layout_geometry::normalize_zone_size_for_editor(
                            zone.position,
                            zone.size,
                            &zone.topology,
                        );
                    }
                }
            });
            set_is_dirty.set(true);
        };

    view! {
        <div class="h-full px-4 py-2.5">
            {move || {
                let Some(zone) = zone_snapshot.get() else {
                    return view! {
                        <div class="flex items-center justify-center py-3 gap-2">
                            <Icon icon=LuMousePointerClick width="16px" height="16px" style="color: rgba(139, 133, 160, 0.15)" />
                            <div class="text-[10px] text-fg-tertiary">"Click a zone on the canvas to edit its properties"</div>
                        </div>
                    }.into_any();
                };

                let zone_id = zone.id.clone();
                let zone_name = zone.name.clone();
                let device_id_display = zone.device_id.clone();
                let pos_x = zone.position.x;
                let pos_y = zone.position.y;
                let size_w = zone.size.x;
                let size_h = zone.size.y;
                let rotation_deg = zone.rotation.to_degrees();
                let scale = zone.scale;
                let led_count = zone.topology.led_count();
                let topology_label = topology_name(&zone.topology);
                let current_group_id = zone.group_id.clone();

                let zid_name = zone_id.clone();
                let zid_pos_x = zone_id.clone();
                let zid_pos_y = zone_id.clone();
                let zid_size_w = zone_id.clone();
                let zid_size_h = zone_id.clone();
                let zid_rotation = zone_id.clone();
                let zid_scale = zone_id.clone();
                let zid_group = zone_id.clone();
                let zid_remove = zone_id;

                view! {
                    <div class="space-y-3">
                        // Header row: title + remove button
                        <div class="flex items-center justify-between">
                            <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                                <Icon icon=LuSettings2 width="12px" height="12px" />
                                "Zone Properties"
                            </h3>
                            <button
                                class="flex items-center gap-1 px-2 py-0.5 rounded-md text-[9px] font-medium
                                       border transition-all btn-press text-status-error/50 hover:text-status-error"
                                style="background: rgba(255, 99, 99, 0.04); border-color: rgba(255, 99, 99, 0.12)"
                                on:click=move |_| {
                                    let zid = zid_remove.clone();
                                    set_layout.update(|l| {
                                        if let Some(layout) = l {
                                            layout.zones.retain(|z| z.id != zid);
                                        }
                                    });
                                    set_selected_zone_id.set(None);
                                    set_is_dirty.set(true);
                                }
                            >
                                <Icon icon=LuTrash2 width="10px" height="10px" />
                                "Remove"
                            </button>
                        </div>

                        // Row 1: Identity — name, device, topology, group
                        <div class="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-4">
                            // Name
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Name"</label>
                                <input
                                    type="text"
                                    class="w-full bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[11px] text-fg-primary
                                           placeholder-fg-tertiary focus:outline-none focus:border-accent-muted glow-ring transition-all"
                                    prop:value=zone_name
                                    on:change=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target {
                                            let val = el.value();
                                            let zid = zid_name.clone();
                                            update_zone(zid, Box::new(move |z| z.name = val));
                                        }
                                    }
                                />
                            </div>

                            // Device (read-only)
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Device"</label>
                                <div class="text-[11px] text-fg-primary font-mono bg-surface-overlay/60 rounded-md px-2 py-1 border border-edge-subtle truncate">
                                    {device_id_display}
                                </div>
                            </div>

                            // Topology (read-only)
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Topology"</label>
                                <div class="text-[11px] text-fg-primary bg-surface-overlay/60 rounded-md px-2 py-1 border border-edge-subtle">
                                    {topology_label} " · " {led_count} " LEDs"
                                </div>
                            </div>

                            // Group
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Group"</label>
                                <select
                                    class="w-full bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[11px] text-fg-primary
                                           focus:outline-none focus:border-accent-muted glow-ring transition-all"
                                    on:change=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                                        if let Some(el) = target {
                                            let val = el.value();
                                            let group_id = if val.is_empty() { None } else { Some(val) };
                                            let zid = zid_group.clone();
                                            update_zone(zid, Box::new(move |z| z.group_id = group_id));
                                        }
                                    }
                                >
                                    <option value="" selected=current_group_id.is_none()>"None"</option>
                                    {available_groups.get().into_iter().map(|group| {
                                        let gid = group.id.clone();
                                        let is_current = current_group_id.as_deref() == Some(&gid);
                                        let color = group.color.clone().unwrap_or_else(|| "#e135ff".to_string());
                                        let rgb = hex_to_rgb(&color);
                                        view! {
                                            <option
                                                value=gid
                                                selected=is_current
                                                style=format!("color: rgb({rgb})")
                                            >
                                                {group.name}
                                            </option>
                                        }
                                    }).collect_view()}
                                </select>
                            </div>
                        </div>

                        // Row 2: Transform — position, size, rotation, scale
                        <div class="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-[auto_auto_minmax(0,1fr)_minmax(0,1fr)] items-end">
                            // Position
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Position"</label>
                                <div class="flex items-center gap-1.5">
                                    {zone_number_input("X", pos_x, "0.01", 3, {
                                        let zid = zid_pos_x;
                                        move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.position.x = val))
                                    })}
                                    {zone_number_input("Y", pos_y, "0.01", 3, {
                                        let zid = zid_pos_y;
                                        move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.position.y = val))
                                    })}
                                </div>
                            </div>

                            // Size
                            <div class="space-y-0.5">
                                <div class="flex items-center justify-between gap-2">
                                    <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Size"</label>
                                    <button
                                        class="flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[8px] font-mono uppercase tracking-[0.08em] transition-all btn-press"
                                        style=move || {
                                            if keep_aspect_ratio.get() {
                                                "background: rgba(128, 255, 234, 0.08); border-color: rgba(128, 255, 234, 0.2); color: rgb(128, 255, 234)".to_string()
                                            } else {
                                                "background: rgba(139, 133, 160, 0.06); border-color: rgba(139, 133, 160, 0.16); color: rgba(139, 133, 160, 0.9)".to_string()
                                            }
                                        }
                                        on:click=move |_| {
                                            set_keep_aspect_ratio.update(|locked| *locked = !*locked);
                                        }
                                    >
                                        {move || if keep_aspect_ratio.get() { "Linked" } else { "Free" }}
                                        {move || if keep_aspect_ratio.get() {
                                            view! {
                                                <Icon icon=LuCheck width="10px" height="10px" />
                                            }.into_any()
                                        } else {
                                            view! {
                                                <Icon icon=LuX width="10px" height="10px" />
                                            }.into_any()
                                        }}
                                    </button>
                                </div>
                                <div class="flex items-center gap-1.5">
                                    {zone_number_input("W", size_w, "0.0001", 4, {
                                        let zid = zid_size_w;
                                        move |val: f32| {
                                            let locked = keep_aspect_ratio.get_untracked();
                                            update_zone(zid.clone(), Box::new(move |z| {
                                                z.size = layout_geometry::update_zone_size(
                                                    z.size,
                                                    SizeAxis::Width,
                                                    val,
                                                    locked,
                                                );
                                            }))
                                        }
                                    })}
                                    {zone_number_input("H", size_h, "0.0001", 4, {
                                        let zid = zid_size_h;
                                        move |val: f32| {
                                            let locked = keep_aspect_ratio.get_untracked();
                                            update_zone(zid.clone(), Box::new(move |z| {
                                                z.size = layout_geometry::update_zone_size(
                                                    z.size,
                                                    SizeAxis::Height,
                                                    val,
                                                    locked,
                                                );
                                            }))
                                        }
                                    })}
                                </div>
                            </div>

                            // Rotation
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Rotation"</label>
                                <div class="flex items-center gap-2">
                                    <input
                                        type="range"
                                        min="0" max="360" step="1"
                                        class="min-w-0 flex-1"
                                        prop:value=format!("{rotation_deg:.0}")
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target {
                                                if let Ok(deg) = el.value().parse::<f32>() {
                                                    let rad = deg.to_radians();
                                                    let zid = zid_rotation.clone();
                                                    update_zone(zid, Box::new(move |z| z.rotation = rad));
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-[10px] font-mono text-fg-tertiary tabular-nums w-7 text-right shrink-0">
                                        {format!("{rotation_deg:.0}")} "\u{00b0}"
                                    </span>
                                </div>
                            </div>

                            // Scale
                            <div class="space-y-0.5">
                                <label class="text-[8px] text-fg-tertiary font-mono uppercase tracking-wider">"Scale"</label>
                                <div class="flex items-center gap-2">
                                    <input
                                        type="range"
                                        min="0.5" max="3.0" step="0.1"
                                        class="min-w-0 flex-1"
                                        prop:value=format!("{scale:.1}")
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target {
                                                if let Ok(s) = el.value().parse::<f32>() {
                                                    let zid = zid_scale.clone();
                                                    update_zone(zid, Box::new(move |z| z.scale = s));
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-[10px] font-mono text-fg-tertiary tabular-nums w-7 text-right shrink-0">
                                        {format!("{scale:.1}")} "x"
                                    </span>
                                </div>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// Inline labeled number input for zone properties.
fn zone_number_input(
    label: &'static str,
    value: f32,
    step: &'static str,
    precision: usize,
    on_change: impl Fn(f32) + Clone + 'static,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-1">
            <span class="text-[8px] text-fg-tertiary font-mono w-3">{label}</span>
            <input
                type="number"
                step=step
                min="0"
                max="1"
                class="w-16 bg-surface-sunken border border-edge-subtle rounded-md px-1.5 py-1 text-[10px] text-fg-primary font-mono tabular-nums
                       focus:outline-none focus:border-accent-muted glow-ring transition-all"
                prop:value=format!("{value:.precision$}")
                on:change=move |ev| {
                    let on_change = on_change.clone();
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target {
                        if let Ok(v) = el.value().parse::<f32>() {
                            on_change(v);
                        }
                    }
                }
            />
        </div>
    }
}

/// Human-readable topology name.
fn topology_name(topology: &hypercolor_types::spatial::LedTopology) -> &'static str {
    match topology {
        hypercolor_types::spatial::LedTopology::Strip { .. } => "Strip",
        hypercolor_types::spatial::LedTopology::Matrix { .. } => "Matrix",
        hypercolor_types::spatial::LedTopology::Ring { .. } => "Ring",
        hypercolor_types::spatial::LedTopology::ConcentricRings { .. } => "Concentric Rings",
        hypercolor_types::spatial::LedTopology::PerimeterLoop { .. } => "Perimeter Loop",
        hypercolor_types::spatial::LedTopology::Point => "Point",
        hypercolor_types::spatial::LedTopology::Custom { .. } => "Custom",
    }
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
