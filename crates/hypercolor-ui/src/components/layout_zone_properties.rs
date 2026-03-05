//! Layout zone properties panel — edit selected zone placement and topology.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::icons::*;
use hypercolor_types::spatial::SpatialLayout;

/// Zone properties editor (right sidebar of layout builder).
#[component]
pub fn LayoutZoneProperties(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] selected_zone_id: Signal<Option<String>>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    set_is_dirty: WriteSignal<bool>,
) -> impl IntoView {
    // Derive selected zone snapshot for display
    let zone_snapshot = Signal::derive(move || {
        let id = selected_zone_id.get()?;
        let l = layout.get()?;
        l.zones.into_iter().find(|z| z.id == id)
    });

    // Helper to update a zone field
    let update_zone =
        move |zone_id: String,
              updater: Box<dyn FnOnce(&mut hypercolor_types::spatial::DeviceZone)>| {
            set_layout.update(|l| {
                if let Some(layout) = l {
                    if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                        updater(zone);
                    }
                }
            });
            set_is_dirty.set(true);
        };

    view! {
        <div class="p-3 space-y-3">
            {move || {
                let Some(zone) = zone_snapshot.get() else {
                    return view! {
                        <div class="text-center py-8">
                            <div class="text-xs text-fg-dim">"Select a zone to edit"</div>
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

                // Clone zone_id for each closure that needs it
                let zid_name = zone_id.clone();
                let zid_pos_x = zone_id.clone();
                let zid_pos_y = zone_id.clone();
                let zid_size_w = zone_id.clone();
                let zid_size_h = zone_id.clone();
                let zid_rotation = zone_id.clone();
                let zid_scale = zone_id.clone();
                let zid_remove = zone_id;

                view! {
                    <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-dim">"Zone Properties"</h3>

                    // Zone name
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Name"</label>
                        <input
                            type="text"
                            class="w-full bg-layer-2 border border-white/[0.06] rounded px-2.5 py-1.5 text-xs text-fg
                                   focus:outline-none focus:border-electric-purple/20"
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

                    // Device ID (read-only)
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Device"</label>
                        <div class="text-xs text-fg font-mono bg-layer-2/60 rounded px-2.5 py-1.5 border border-white/[0.03] truncate">
                            {device_id_display}
                        </div>
                    </div>

                    // Topology info
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Topology"</label>
                        <div class="text-xs text-fg bg-layer-2/60 rounded px-2.5 py-1.5 border border-white/[0.03]">
                            {topology_label} " · " {led_count} " LEDs"
                        </div>
                    </div>

                    // Position
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Position"</label>
                        <div class="grid grid-cols-2 gap-2">
                            {zone_number_input("X", pos_x, {
                                let zid = zid_pos_x;
                                move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.position.x = val))
                            })}
                            {zone_number_input("Y", pos_y, {
                                let zid = zid_pos_y;
                                move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.position.y = val))
                            })}
                        </div>
                    </div>

                    // Size
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Size"</label>
                        <div class="grid grid-cols-2 gap-2">
                            {zone_number_input("W", size_w, {
                                let zid = zid_size_w;
                                move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.size.x = val))
                            })}
                            {zone_number_input("H", size_h, {
                                let zid = zid_size_h;
                                move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.size.y = val))
                            })}
                        </div>
                    </div>

                    // Rotation
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Rotation"</label>
                        <div class="flex items-center gap-2">
                            <input
                                type="range"
                                min="0" max="360" step="1"
                                class="flex-1 accent-electric-purple"
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
                            <span class="text-[10px] font-mono text-fg-dim tabular-nums w-8 text-right">
                                {format!("{rotation_deg:.0}")} "°"
                            </span>
                        </div>
                    </div>

                    // Scale
                    <div class="space-y-1">
                        <label class="text-[10px] text-fg-dim">"Scale"</label>
                        <div class="flex items-center gap-2">
                            <input
                                type="range"
                                min="0.5" max="3.0" step="0.1"
                                class="flex-1 accent-electric-purple"
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
                            <span class="text-[10px] font-mono text-fg-dim tabular-nums w-8 text-right">
                                {format!("{scale:.1}")} "×"
                            </span>
                        </div>
                    </div>

                    // Remove button
                    <div class="pt-2 border-t border-white/[0.04]">
                        <button
                            class="w-full px-3 py-1.5 rounded-lg text-xs font-medium bg-error-red/[0.08] border border-error-red/20
                                   text-error-red hover:bg-error-red/[0.15] transition-all btn-press"
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
                            <Icon icon=LuTrash2 width="14px" height="14px" />
                            " Remove from Layout"
                        </button>
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
    on_change: impl Fn(f32) + Clone + 'static,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-1.5">
            <span class="text-[9px] text-fg-dim font-mono w-3">{label}</span>
            <input
                type="number"
                step="0.01"
                min="0"
                max="1"
                class="flex-1 bg-layer-2 border border-white/[0.06] rounded px-2 py-1 text-[11px] text-fg font-mono
                       focus:outline-none focus:border-electric-purple/20 w-full"
                prop:value=format!("{value:.3}")
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
