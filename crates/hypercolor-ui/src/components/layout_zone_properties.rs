//! Layout zone properties panel — horizontal editor below canvas viewport.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::app::DevicesContext;
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
        <div class="h-full px-5 py-3 overflow-y-auto">
            {move || {
                let Some(zone) = zone_snapshot.get() else {
                    return view! {
                        <div class="flex items-center justify-center h-full gap-2.5">
                            <Icon icon=LuMousePointerClick width="18px" height="18px" style="color: rgba(139, 133, 160, 0.15)" />
                            <div class="text-xs text-fg-tertiary">"Click a zone on the canvas to edit its properties"</div>
                        </div>
                    }.into_any();
                };

                let ctx = expect_context::<DevicesContext>();

                let zone_id = zone.id.clone();
                let zone_name = zone.name.clone();
                let device_id_display = zone.device_id.clone();
                let device_id_title = zone.device_id.clone();
                let channel_name = zone.zone_name.clone();
                let pos_x = zone.position.x;
                let pos_y = zone.position.y;
                let size_w = zone.size.x;
                let size_h = zone.size.y;
                let rotation_deg = zone.rotation.to_degrees();
                let scale = zone.scale;
                let led_count = zone.topology.led_count();
                let topology_label = topology_name(&zone.topology);
                let current_group_id = zone.group_id.clone();
                let attachment = zone.attachment.clone();

                // Compute the default display name for reset.
                let default_name = {
                    let device_name = ctx
                        .devices_resource
                        .get_untracked()
                        .and_then(|r| r.ok())
                        .and_then(|devices| {
                            devices
                                .iter()
                                .find(|d| d.layout_device_id == zone.device_id)
                                .map(|d| d.name.clone())
                        })
                        .unwrap_or_else(|| zone.device_id.clone());
                    match &zone.zone_name {
                        Some(zn) if !zn.eq_ignore_ascii_case(&device_name) => {
                            format!("{device_name} · {zn}")
                        }
                        _ => device_name,
                    }
                };
                let name_is_default = zone_name == default_name;

                let display_order = zone.display_order;

                let zid_name = zone_id.clone();
                let zid_name_reset = zone_id.clone();
                let zid_channel = zone_id.clone();
                let zid_pos_x = zone_id.clone();
                let zid_pos_y = zone_id.clone();
                let zid_center_h = zone_id.clone();
                let zid_center_v = zone_id.clone();
                let zid_size_w = zone_id.clone();
                let zid_size_h = zone_id.clone();
                let zid_rotation = zone_id.clone();
                let zid_rotation_input = zone_id.clone();
                let zid_scale = zone_id.clone();
                let zid_scale_input = zone_id.clone();
                let zid_group = zone_id.clone();
                let zid_front = zone_id.clone();
                let zid_up = zone_id.clone();
                let zid_down = zone_id.clone();
                let zid_back = zone_id.clone();
                let zid_remove = zone_id;

                view! {
                    <div class="space-y-3">
                        // Row 1: Zone identity — name, channel, metadata, group, layer, actions
                        <div class="flex items-center gap-2 min-w-0">
                            // Name input
                            <input
                                type="text"
                                class="min-w-0 flex-1 max-w-80 bg-surface-sunken border border-edge-subtle rounded-md px-2.5 py-1.5
                                       text-sm text-fg-primary placeholder-fg-tertiary
                                       focus:outline-none focus:border-accent-muted glow-ring transition-all"
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

                            // Name reset (only if custom)
                            {(!name_is_default).then(|| {
                                let default = default_name.clone();
                                view! {
                                    <button
                                        class="shrink-0 text-fg-tertiary/40 hover:text-accent transition-colors btn-press"
                                        title="Reset to default name"
                                        on:click=move |_| {
                                            let val = default.clone();
                                            let zid = zid_name_reset.clone();
                                            update_zone(zid, Box::new(move |z| z.name = val));
                                        }
                                    >
                                        <Icon icon=LuRotateCcw width="12px" height="12px" />
                                    </button>
                                }
                            })}

                            <div class="w-px h-4 bg-edge-subtle/30 shrink-0" />

                            // Channel input
                            <div class="flex items-center gap-1.5 shrink-0">
                                <span class="text-[9px] text-fg-tertiary/60 font-mono uppercase">"Ch"</span>
                                <input
                                    type="text"
                                    placeholder="\u{2014}"
                                    class="w-24 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1.5
                                           text-xs text-fg-primary font-mono placeholder-fg-tertiary/30
                                           focus:outline-none focus:border-accent-muted glow-ring transition-all"
                                    prop:value=channel_name.clone().unwrap_or_default()
                                    on:change=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target {
                                            let val = el.value();
                                            let zid = zid_channel.clone();
                                            let zone_name = if val.trim().is_empty() { None } else { Some(val) };
                                            update_zone(zid, Box::new(move |z| z.zone_name = zone_name));
                                        }
                                    }
                                />
                            </div>

                            <div class="w-px h-4 bg-edge-subtle/30 shrink-0" />

                            // Metadata badges
                            <div
                                class="shrink-0 max-w-40 text-xs text-fg-tertiary/70 font-mono bg-surface-overlay/40
                                       rounded-md px-2 py-1 border border-edge-subtle/60 truncate cursor-default"
                                title=device_id_title
                            >
                                {device_id_display}
                            </div>
                            <span class="shrink-0 text-xs text-fg-tertiary/70 font-mono bg-surface-overlay/40
                                         rounded-md px-2 py-1 border border-edge-subtle/60 whitespace-nowrap">
                                {topology_label} " \u{00b7} " {led_count}
                            </span>

                            // Attachment badge
                            {attachment.map(|att| {
                                let label = att.template_id.clone();
                                let detail = match att.led_count {
                                    Some(count) => format!("{label} ({count} LEDs)"),
                                    None => label.clone(),
                                };
                                view! {
                                    <span class="shrink-0 text-xs font-mono px-2 py-1 rounded-md truncate max-w-[160px]"
                                        style="color: rgb(128, 255, 234); background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.15)"
                                        title=detail
                                    >
                                        <Icon icon=LuCable width="10px" height="10px" style="display: inline; vertical-align: -1px; margin-right: 3px" />
                                        {label}
                                    </span>
                                }
                            })}

                            // Group dropdown
                            <select
                                class="shrink-0 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1.5
                                       text-xs text-fg-primary focus:outline-none focus:border-accent-muted glow-ring transition-all"
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
                                <option value="" selected=current_group_id.is_none()>"No group"</option>
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

                            <div class="flex-1" />

                            // Layer controls
                            <div class="flex items-center gap-1 shrink-0">
                                {layer_icon_button(LuSkipForward, "Bring to front", {
                                    let zid = zid_front;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let max = layout.zones.iter().map(|z| z.display_order).max().unwrap_or(0);
                                                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                    zone.display_order = max + 1;
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                {layer_icon_button(LuChevronUp, "Move up one layer", {
                                    let zid = zid_up;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let current_order = layout.zones.iter()
                                                    .find(|z| z.id == zid)
                                                    .map(|z| z.display_order);
                                                if let Some(order) = current_order {
                                                    let next_up = layout.zones.iter()
                                                        .filter(|z| z.display_order > order)
                                                        .map(|z| z.display_order)
                                                        .min();
                                                    if let Some(swap_order) = next_up {
                                                        for z in &mut layout.zones {
                                                            if z.id == zid {
                                                                z.display_order = swap_order;
                                                            } else if z.display_order == swap_order {
                                                                z.display_order = order;
                                                            }
                                                        }
                                                    } else if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                        zone.display_order += 1;
                                                    }
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                {layer_icon_button(LuChevronDown, "Move down one layer", {
                                    let zid = zid_down;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let current_order = layout.zones.iter()
                                                    .find(|z| z.id == zid)
                                                    .map(|z| z.display_order);
                                                if let Some(order) = current_order {
                                                    let next_down = layout.zones.iter()
                                                        .filter(|z| z.display_order < order)
                                                        .map(|z| z.display_order)
                                                        .max();
                                                    if let Some(swap_order) = next_down {
                                                        for z in &mut layout.zones {
                                                            if z.id == zid {
                                                                z.display_order = swap_order;
                                                            } else if z.display_order == swap_order {
                                                                z.display_order = order;
                                                            }
                                                        }
                                                    } else if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                        zone.display_order -= 1;
                                                    }
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                {layer_icon_button(LuSkipBack, "Send to back", {
                                    let zid = zid_back;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let min = layout.zones.iter().map(|z| z.display_order).min().unwrap_or(0);
                                                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                    zone.display_order = min - 1;
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                <span class="text-[10px] font-mono text-fg-tertiary/40 tabular-nums ml-1">
                                    {display_order}
                                </span>
                            </div>

                            <div class="w-px h-4 bg-edge-subtle/30 shrink-0" />

                            // Remove button
                            <button
                                class="shrink-0 p-1.5 rounded-md text-status-error/40 hover:text-status-error hover:bg-status-error/10
                                       transition-all btn-press"
                                title="Remove zone from layout"
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
                                <Icon icon=LuTrash2 width="13px" height="13px" />
                            </button>
                        </div>

                        // Row 2: Transforms — fixed grid layout so controls don't float
                        <div class="flex items-center gap-5">
                            // Position
                            <div class="flex items-center gap-2">
                                <span class="text-[10px] text-fg-tertiary/70 font-mono uppercase tracking-wide shrink-0 w-6">"Pos"</span>
                                {zone_number_input("X", pos_x, "0.01", 2, "0", "1", {
                                    let zid = zid_pos_x;
                                    move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.position.x = val))
                                })}
                                {zone_number_input("Y", pos_y, "0.01", 2, "0", "1", {
                                    let zid = zid_pos_y;
                                    move |val: f32| update_zone(zid.clone(), Box::new(move |z| z.position.y = val))
                                })}
                                // Center buttons
                                <button
                                    class="shrink-0 p-1 rounded-md border transition-all btn-press
                                           text-fg-tertiary/50 hover:text-accent border-edge-subtle/40 hover:border-accent-muted/40"
                                    title="Center horizontally"
                                    on:click=move |_| {
                                        let zid = zid_center_h.clone();
                                        update_zone(zid, Box::new(|z| z.position.x = 0.5));
                                    }
                                >
                                    <Icon icon=LuAlignCenterHorizontal width="12px" height="12px" />
                                </button>
                                <button
                                    class="shrink-0 p-1 rounded-md border transition-all btn-press
                                           text-fg-tertiary/50 hover:text-accent border-edge-subtle/40 hover:border-accent-muted/40"
                                    title="Center vertically"
                                    on:click=move |_| {
                                        let zid = zid_center_v.clone();
                                        update_zone(zid, Box::new(|z| z.position.y = 0.5));
                                    }
                                >
                                    <Icon icon=LuAlignCenterVertical width="12px" height="12px" />
                                </button>
                            </div>

                            <div class="w-px h-5 bg-edge-subtle/20 shrink-0" />

                            // Size
                            <div class="flex items-center gap-2">
                                <span class="text-[10px] text-fg-tertiary/70 font-mono uppercase tracking-wide shrink-0 w-6">"Size"</span>
                                {zone_number_input("W", size_w, "0.001", 3, "0", "1", {
                                    let zid = zid_size_w;
                                    move |val: f32| {
                                        let locked = keep_aspect_ratio.get_untracked();
                                        update_zone(zid.clone(), Box::new(move |z| {
                                            z.size = layout_geometry::update_zone_size(
                                                z.size, SizeAxis::Width, val, locked,
                                            );
                                        }))
                                    }
                                })}
                                {zone_number_input("H", size_h, "0.001", 3, "0", "1", {
                                    let zid = zid_size_h;
                                    move |val: f32| {
                                        let locked = keep_aspect_ratio.get_untracked();
                                        update_zone(zid.clone(), Box::new(move |z| {
                                            z.size = layout_geometry::update_zone_size(
                                                z.size, SizeAxis::Height, val, locked,
                                            );
                                        }))
                                    }
                                })}
                                <button
                                    class="shrink-0 p-1 rounded-md border transition-all btn-press"
                                    title=move || if keep_aspect_ratio.get() { "Aspect ratio linked" } else { "Aspect ratio free" }
                                    style=move || {
                                        if keep_aspect_ratio.get() {
                                            "background: rgba(128, 255, 234, 0.08); border-color: rgba(128, 255, 234, 0.25); color: rgb(128, 255, 234)".to_string()
                                        } else {
                                            "background: rgba(139, 133, 160, 0.04); border-color: rgba(139, 133, 160, 0.14); color: rgba(139, 133, 160, 0.6)".to_string()
                                        }
                                    }
                                    on:click=move |_| {
                                        set_keep_aspect_ratio.update(|locked| *locked = !*locked);
                                    }
                                >
                                    {move || if keep_aspect_ratio.get() {
                                        view! { <Icon icon=LuLink width="12px" height="12px" /> }.into_any()
                                    } else {
                                        view! { <Icon icon=LuUnlink width="12px" height="12px" /> }.into_any()
                                    }}
                                </button>
                            </div>

                            <div class="w-px h-5 bg-edge-subtle/20 shrink-0" />

                            // Rotation — slider + number input
                            <div class="flex items-center gap-2 flex-1 min-w-40">
                                <span class="text-[10px] text-fg-tertiary/70 font-mono uppercase tracking-wide shrink-0 w-6">"Rot"</span>
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
                                <div class="flex items-center gap-0.5 shrink-0">
                                    <input
                                        type="number"
                                        min="0" max="360" step="1"
                                        class="w-14 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1.5
                                               text-xs text-fg-primary font-mono tabular-nums text-right
                                               focus:outline-none focus:border-accent-muted glow-ring transition-all"
                                        prop:value=format!("{rotation_deg:.0}")
                                        on:change=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target {
                                                if let Ok(deg) = el.value().parse::<f32>() {
                                                    let rad = deg.to_radians();
                                                    let zid = zid_rotation_input.clone();
                                                    update_zone(zid, Box::new(move |z| z.rotation = rad));
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-xs font-mono text-fg-tertiary/50">"\u{00b0}"</span>
                                </div>
                            </div>

                            <div class="w-px h-5 bg-edge-subtle/20 shrink-0" />

                            // Scale — slider + number input
                            <div class="flex items-center gap-2 flex-1 min-w-40">
                                <span class="text-[10px] text-fg-tertiary/70 font-mono uppercase tracking-wide shrink-0">"Scale"</span>
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
                                <div class="flex items-center gap-0.5 shrink-0">
                                    <input
                                        type="number"
                                        min="0.5" max="3.0" step="0.1"
                                        class="w-14 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1.5
                                               text-xs text-fg-primary font-mono tabular-nums text-right
                                               focus:outline-none focus:border-accent-muted glow-ring transition-all"
                                        prop:value=format!("{scale:.1}")
                                        on:change=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target {
                                                if let Ok(s) = el.value().parse::<f32>() {
                                                    let zid = zid_scale_input.clone();
                                                    update_zone(zid, Box::new(move |z| z.scale = s));
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-xs font-mono text-fg-tertiary/50">"x"</span>
                                </div>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// Icon-only button for layer ordering controls.
fn layer_icon_button(
    icon: icondata_core::Icon,
    title: &'static str,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            class="p-1 rounded-md text-fg-tertiary/50 hover:text-accent transition-all btn-press"
            title=title
            on:click=on_click
        >
            <Icon icon=icon width="13px" height="13px" />
        </button>
    }
}

/// Inline labeled number input for zone properties.
fn zone_number_input(
    label: &'static str,
    value: f32,
    step: &'static str,
    precision: usize,
    min: &'static str,
    max: &'static str,
    on_change: impl Fn(f32) + Clone + 'static,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-1">
            <span class="text-[10px] text-fg-tertiary/60 font-mono w-3">{label}</span>
            <input
                type="number"
                step=step
                min=min
                max=max
                class="w-[4.5rem] bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1.5
                       text-xs text-fg-primary font-mono tabular-nums
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
