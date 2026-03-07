//! Layout canvas — live effect preview with draggable zone overlays and group containers.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::device_card::backend_accent_rgb;
use hypercolor_types::spatial::{NormalizedPosition, SpatialLayout};

/// Canvas viewport with zone overlay divs and group containers.
#[component]
pub fn LayoutCanvas(
    #[prop(into)] layout: Signal<Option<SpatialLayout>>,
    #[prop(into)] selected_zone_id: Signal<Option<String>>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    set_is_dirty: WriteSignal<bool>,
) -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let ws_fps = Signal::derive(move || ws.fps.get());

    let viewport_ref = NodeRef::<leptos::html::Div>::new();

    // Drag state
    let (interaction, set_interaction) = signal(None::<InteractionState>);

    // Derive just the zone IDs — only re-renders the zone list when zones are added/removed,
    // NOT when positions change during drag.
    let zone_ids = Memo::new(move |_| {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| l.zones.iter().map(|z| z.id.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });

    // Derive group data for rendering group containers
    let group_bounds = Memo::new(move |_| {
        layout.with(|current| {
            let Some(l) = current.as_ref() else {
                return Vec::new();
            };
            l.groups
                .iter()
                .filter_map(|group| {
                    let member_zones: Vec<_> = l
                        .zones
                        .iter()
                        .filter(|z| z.group_id.as_deref() == Some(&group.id))
                        .collect();
                    if member_zones.is_empty() {
                        return None;
                    }
                    // Compute bounding box of member zones
                    let mut min_x = f32::MAX;
                    let mut min_y = f32::MAX;
                    let mut max_x = f32::MIN;
                    let mut max_y = f32::MIN;
                    for z in &member_zones {
                        let left = z.position.x - z.size.x * 0.5;
                        let right = z.position.x + z.size.x * 0.5;
                        let top = z.position.y - z.size.y * 0.5;
                        let bottom = z.position.y + z.size.y * 0.5;
                        min_x = min_x.min(left);
                        min_y = min_y.min(top);
                        max_x = max_x.max(right);
                        max_y = max_y.max(bottom);
                    }
                    // Add padding
                    let pad = 0.02;
                    min_x = (min_x - pad).max(0.0);
                    min_y = (min_y - pad).max(0.0);
                    max_x = (max_x + pad).min(1.0);
                    max_y = (max_y + pad).min(1.0);
                    Some(GroupBounds {
                        id: group.id.clone(),
                        name: group.name.clone(),
                        color: group
                            .color
                            .clone()
                            .unwrap_or_else(|| "#e135ff".to_string()),
                        left: min_x,
                        top: min_y,
                        width: max_x - min_x,
                        height: max_y - min_y,
                        zone_count: member_zones.len(),
                    })
                })
                .collect()
        })
    });

    let viewport_style = Signal::derive(move || {
        layout
            .with(|current| {
                current.as_ref().map(|layout| {
                    format!(
                        "aspect-ratio: {} / {};",
                        layout.canvas_width.max(1),
                        layout.canvas_height.max(1)
                    )
                })
            })
            .unwrap_or_else(|| "aspect-ratio: 320 / 200;".to_string())
    });
    let preview_aspect_ratio = Signal::derive(move || {
        layout
            .with(|current| {
                current.as_ref().map(|layout| {
                    format!(
                        "{} / {}",
                        layout.canvas_width.max(1),
                        layout.canvas_height.max(1)
                    )
                })
            })
            .unwrap_or_else(|| "320 / 200".to_string())
    });

    let has_zones = Signal::derive(move || !zone_ids.get().is_empty());

    view! {
        <div
            class="relative w-full h-full overflow-hidden"
            style="background: var(--color-surface-base)"
            on:mouseup=move |_| {
                set_interaction.set(None);
            }
            on:mouseleave=move |_| {
                set_interaction.set(None);
            }
            on:mousemove=move |ev| {
                let Some(interaction_state) = interaction.get() else { return };
                let Some(viewport) = viewport_ref.get() else { return };
                let rect = viewport.get_bounding_client_rect();
                let cw = rect.width();
                let ch = rect.height();
                if cw <= 0.0 || ch <= 0.0 { return; }

                let mouse_x = f64::from(ev.client_x()) - rect.left();
                let mouse_y = f64::from(ev.client_y()) - rect.top();

                #[allow(clippy::cast_possible_truncation)]
                let mouse_norm = NormalizedPosition::new((mouse_x / cw) as f32, (mouse_y / ch) as f32);

                match interaction_state {
                    InteractionState::Drag(drag) => {
                        let norm_x = (mouse_norm.x - drag.offset_x).clamp(0.0, 1.0);
                        let norm_y = (mouse_norm.y - drag.offset_y).clamp(0.0, 1.0);
                        let zone_id = drag.zone_id.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                                    zone.position.x = norm_x;
                                    zone.position.y = norm_y;
                                }
                            }
                        });
                    }
                    InteractionState::Resize(resize) => {
                        let zone_id = resize.zone_id.clone();
                        set_layout.update(|l| {
                            if let Some(layout) = l {
                                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                                    resize_zone(zone, &resize, mouse_norm);
                                }
                            }
                        });
                    }
                }
                set_is_dirty.set(true);
            }
        >
            // Dot grid background pattern
            <div
                class="absolute inset-0 pointer-events-none opacity-[0.06]"
                style="background-image: radial-gradient(circle, var(--color-text-tertiary) 1px, transparent 1px); background-size: 20px 20px;"
            />

            <div class="absolute inset-0 flex items-center justify-center p-4">
                <div
                    class="relative h-full max-w-full shrink-0"
                    node_ref=viewport_ref
                    style=move || viewport_style.get()
                    on:click=move |_| {
                        set_selected_zone_id.set(None);
                    }
                >
                    // Live effect canvas background
                    <div class="absolute inset-0 pointer-events-none rounded-lg overflow-hidden">
                        <CanvasPreview
                            frame=canvas_frame
                            fps=ws_fps
                            show_fps=false
                            aspect_ratio=preview_aspect_ratio.get()
                        />
                    </div>

                    // Subtle border around the viewport
                    <div class="absolute inset-0 rounded-lg border border-edge-subtle/50 pointer-events-none" />

                    // Group containers — rendered behind zones
                    {move || {
                        group_bounds.get().into_iter().map(|group| {
                            let left_pct = group.left * 100.0;
                            let top_pct = group.top * 100.0;
                            let w_pct = group.width * 100.0;
                            let h_pct = group.height * 100.0;
                            let color = group.color.clone();
                            let rgb = hex_to_rgb(&color);
                            view! {
                                <div
                                    class="absolute rounded-lg pointer-events-none"
                                    style=format!(
                                        "left: {left_pct:.2}%; top: {top_pct:.2}%; width: {w_pct:.2}%; height: {h_pct:.2}%; \
                                         border: 1.5px dashed rgba({rgb}, 0.4); \
                                         background: rgba({rgb}, 0.04);"
                                    )
                                >
                                    // Group name label
                                    <div
                                        class="absolute -top-5 left-1 text-[8px] font-mono px-1.5 py-0.5 rounded glass-subtle whitespace-nowrap"
                                        style=format!("color: rgba({rgb}, 0.8)")
                                    >
                                        {group.name} " (" {group.zone_count} ")"
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}

                    // Zone overlays — keyed on zone IDs, only re-renders when zones are added/removed
                    {move || {
                        zone_ids.get().into_iter().map(|zone_id| {
                            let zid = zone_id.clone();
                            let zid_select = zone_id.clone();
                            let zid_drag = zone_id.clone();
                            let zid_drag2 = zone_id.clone();
                            let zid_resize_nw = zone_id.clone();
                            let zid_resize_ne = zone_id.clone();
                            let zid_resize_sw = zone_id.clone();
                            let zid_resize_se = zone_id.clone();

                            // Derive per-zone position/style reactively from the layout signal
                            let zone_style = Signal::derive({
                                let zid = zid.clone();
                                move || {
                                    layout.with(|current| {
                                        let layout = current.as_ref()?;
                                        let zone = layout.zones.iter().find(|z| z.id == zid)?;
                                        let x_pct = zone.position.x * 100.0;
                                        let y_pct = zone.position.y * 100.0;
                                        let w_pct = zone.size.x * 100.0;
                                        let h_pct = zone.size.y * 100.0;
                                        let rotation = zone.rotation.to_degrees();

                                        // Use group color if available, else backend accent
                                        let rgb = zone
                                            .group_id
                                            .as_deref()
                                            .and_then(|gid| {
                                                layout.groups.iter().find(|g| g.id == gid)
                                            })
                                            .and_then(|g| g.color.as_deref())
                                            .map(hex_to_rgb)
                                            .unwrap_or_else(|| {
                                                let backend =
                                                    zone.device_id.split(':').next().unwrap_or("");
                                                backend_accent_rgb(backend).to_string()
                                            });

                                        Some((
                                            format!(
                                                "left: {x_pct:.2}%; top: {y_pct:.2}%; width: {w_pct:.2}%; height: {h_pct:.2}%; \
                                                 transform: translate(-50%, -50%) rotate({rotation:.1}deg)"
                                            ),
                                            rgb,
                                            zone.name.clone(),
                                            zone.topology.led_count(),
                                        ))
                                    })
                                }
                            });

                            let is_selected = {
                                let zid = zid.clone();
                                Signal::derive(move || selected_zone_id.get().as_deref() == Some(&zid))
                            };

                            view! {
                                <div
                                    class="absolute border-2 rounded-md cursor-move group transition-[border-color,box-shadow] duration-200"
                                    style=move || {
                                        let Some((base, rgb, _, _)) = zone_style.get() else {
                                            return "display: none".to_string();
                                        };
                                        let selected = is_selected.get();
                                        let border_opacity = if selected { "0.9" } else { "0.5" };
                                        let bg = if selected {
                                            format!("background: rgba({rgb}, 0.06)")
                                        } else {
                                            format!("background: rgba({rgb}, 0.02)")
                                        };
                                        let shadow = if selected {
                                            format!("box-shadow: 0 0 20px rgba({rgb}, 0.35), 0 0 4px rgba({rgb}, 0.5)")
                                        } else {
                                            String::new()
                                        };
                                        format!("{base}; border-color: rgba({rgb}, {border_opacity}); {bg}; {shadow}")
                                    }
                                    on:mousedown=move |ev| {
                                        ev.stop_propagation();
                                        ev.prevent_default();
                                        set_selected_zone_id.set(Some(zid_select.clone()));

                                        let Some(viewport) = viewport_ref.get() else { return };
                                        let rect = viewport.get_bounding_client_rect();
                                        let cw = rect.width();
                                        let ch = rect.height();
                                        if cw <= 0.0 || ch <= 0.0 { return; }

                                        #[allow(clippy::cast_possible_truncation)]
                                        let mouse_norm_x = ((f64::from(ev.client_x()) - rect.left()) / cw) as f32;
                                        #[allow(clippy::cast_possible_truncation)]
                                        let mouse_norm_y = ((f64::from(ev.client_y()) - rect.top()) / ch) as f32;

                                        // Read zone position without tracking
                                        let zone_pos = layout.get_untracked()
                                            .and_then(|l| l.zones.iter().find(|z| z.id == zid_drag).map(|z| (z.position.x, z.position.y)));

                                        if let Some((zx, zy)) = zone_pos {
                                            set_interaction.set(Some(InteractionState::Drag(DragState {
                                                zone_id: zid_drag2.clone(),
                                                offset_x: mouse_norm_x - zx,
                                                offset_y: mouse_norm_y - zy,
                                            })));
                                        }
                                    }
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                    }
                                >
                                    // Zone label — glass micro-panel
                                    <div
                                        class="absolute -top-6 left-0 text-[9px] font-mono whitespace-nowrap
                                                opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none
                                                px-2 py-0.5 rounded glass-subtle"
                                        style=move || {
                                            zone_style.get()
                                                .map(|(_, rgb, _, _)| format!("color: rgba({rgb}, 0.9)"))
                                                .unwrap_or_default()
                                        }
                                    >
                                        {move || zone_style.get().map(|(_, _, name, count)| format!("{name} · {count} LEDs")).unwrap_or_default()}
                                    </div>

                                    // Resize handles (selected only) — small circles with accent glow
                                    {move || is_selected.get().then(|| {
                                        let zid_resize_nw = zid_resize_nw.clone();
                                        let zid_resize_ne = zid_resize_ne.clone();
                                        let zid_resize_sw = zid_resize_sw.clone();
                                        let zid_resize_se = zid_resize_se.clone();

                                        let handle_class = "absolute w-3 h-3 rounded-full border-2 transition-[box-shadow,transform] duration-150 \
                                                           hover:scale-125";

                                        let handle_style = move || {
                                            zone_style.get()
                                                .map(|(_, rgb, _, _)| format!(
                                                    "background: rgba({rgb}, 0.9); border-color: rgba(255,255,255,0.6); \
                                                     box-shadow: 0 0 8px rgba({rgb}, 0.4)"
                                                ))
                                                .unwrap_or_default()
                                        };

                                        view! {
                                            <div
                                                class=format!("{handle_class} -top-1.5 -left-1.5 cursor-nw-resize")
                                                style=handle_style
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    let zone_id = zid_resize_nw.clone();
                                                    begin_resize(
                                                        &viewport_ref, &layout, &set_selected_zone_id, &set_interaction,
                                                        &zone_id, ResizeHandle::NorthWest, ev.client_x(), ev.client_y(),
                                                    );
                                                }
                                            />
                                            <div
                                                class=format!("{handle_class} -top-1.5 -right-1.5 cursor-ne-resize")
                                                style=handle_style
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    let zone_id = zid_resize_ne.clone();
                                                    begin_resize(
                                                        &viewport_ref, &layout, &set_selected_zone_id, &set_interaction,
                                                        &zone_id, ResizeHandle::NorthEast, ev.client_x(), ev.client_y(),
                                                    );
                                                }
                                            />
                                            <div
                                                class=format!("{handle_class} -bottom-1.5 -left-1.5 cursor-sw-resize")
                                                style=handle_style
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    let zone_id = zid_resize_sw.clone();
                                                    begin_resize(
                                                        &viewport_ref, &layout, &set_selected_zone_id, &set_interaction,
                                                        &zone_id, ResizeHandle::SouthWest, ev.client_x(), ev.client_y(),
                                                    );
                                                }
                                            />
                                            <div
                                                class=format!("{handle_class} -bottom-1.5 -right-1.5 cursor-se-resize")
                                                style=handle_style
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    let zone_id = zid_resize_se.clone();
                                                    begin_resize(
                                                        &viewport_ref, &layout, &set_selected_zone_id, &set_interaction,
                                                        &zone_id, ResizeHandle::SouthEast, ev.client_x(), ev.client_y(),
                                                    );
                                                }
                                            />
                                        }
                                    })}

                                    // LED count indicator
                                    <div class="absolute inset-0 flex items-center justify-center pointer-events-none">
                                        <div class="text-[8px] font-mono text-white/25 select-none tabular-nums">
                                            {move || zone_style.get().map(|(_, _, _, count)| count).unwrap_or(0)}
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}

                    // Empty canvas state
                    <Show when=move || !has_zones.get()>
                        <div class="absolute inset-0 flex items-center justify-center pointer-events-none">
                            <div class="text-center space-y-2 animate-fade-in">
                                <div class="text-fg-tertiary/30 text-sm font-medium">"Add devices from the palette"</div>
                                <div class="text-fg-tertiary/20 text-xs">"Drag zones to position them on the canvas"</div>
                            </div>
                        </div>
                    </Show>
                </div>
            </div>
        </div>
    }
}

#[derive(Clone, Debug, PartialEq)]
struct GroupBounds {
    id: String,
    name: String,
    color: String,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    zone_count: usize,
}

#[derive(Clone, Debug)]
struct DragState {
    zone_id: String,
    offset_x: f32,
    offset_y: f32,
}

#[derive(Clone, Debug)]
struct ResizeState {
    zone_id: String,
    handle: ResizeHandle,
    start_mouse: NormalizedPosition,
    start_center: NormalizedPosition,
    start_size: NormalizedPosition,
}

#[derive(Clone, Debug)]
enum InteractionState {
    Drag(DragState),
    Resize(ResizeState),
}

#[derive(Clone, Copy, Debug)]
enum ResizeHandle {
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

fn begin_resize(
    viewport_ref: &NodeRef<leptos::html::Div>,
    layout: &Signal<Option<SpatialLayout>>,
    set_selected_zone_id: &WriteSignal<Option<String>>,
    set_interaction: &WriteSignal<Option<InteractionState>>,
    zone_id: &str,
    handle: ResizeHandle,
    client_x: i32,
    client_y: i32,
) {
    let Some(viewport) = viewport_ref.get() else {
        return;
    };
    let rect = viewport.get_bounding_client_rect();
    let cw = rect.width();
    let ch = rect.height();
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }

    #[allow(clippy::cast_possible_truncation)]
    let mouse = NormalizedPosition::new(
        ((f64::from(client_x) - rect.left()) / cw) as f32,
        ((f64::from(client_y) - rect.top()) / ch) as f32,
    );

    let zone_snapshot = layout.get_untracked().and_then(|current| {
        current
            .zones
            .iter()
            .find(|z| z.id == zone_id)
            .map(|zone| (zone.position, zone.size))
    });

    let Some((start_center, start_size)) = zone_snapshot else {
        return;
    };

    set_selected_zone_id.set(Some(zone_id.to_owned()));
    set_interaction.set(Some(InteractionState::Resize(ResizeState {
        zone_id: zone_id.to_owned(),
        handle,
        start_mouse: mouse,
        start_center,
        start_size,
    })));
}

fn resize_zone(
    zone: &mut hypercolor_types::spatial::DeviceZone,
    resize: &ResizeState,
    current_mouse: NormalizedPosition,
) {
    const MIN_SIZE: f32 = 0.04;

    let start_left = resize.start_center.x - resize.start_size.x * 0.5;
    let start_right = resize.start_center.x + resize.start_size.x * 0.5;
    let start_top = resize.start_center.y - resize.start_size.y * 0.5;
    let start_bottom = resize.start_center.y + resize.start_size.y * 0.5;

    let dx = current_mouse.x - resize.start_mouse.x;
    let dy = current_mouse.y - resize.start_mouse.y;

    let (mut left, mut right, mut top, mut bottom) =
        (start_left, start_right, start_top, start_bottom);

    match resize.handle {
        ResizeHandle::NorthWest => {
            left = (start_left + dx).clamp(0.0, start_right - MIN_SIZE);
            top = (start_top + dy).clamp(0.0, start_bottom - MIN_SIZE);
        }
        ResizeHandle::NorthEast => {
            right = (start_right + dx).clamp(start_left + MIN_SIZE, 1.0);
            top = (start_top + dy).clamp(0.0, start_bottom - MIN_SIZE);
        }
        ResizeHandle::SouthWest => {
            left = (start_left + dx).clamp(0.0, start_right - MIN_SIZE);
            bottom = (start_bottom + dy).clamp(start_top + MIN_SIZE, 1.0);
        }
        ResizeHandle::SouthEast => {
            right = (start_right + dx).clamp(start_left + MIN_SIZE, 1.0);
            bottom = (start_bottom + dy).clamp(start_top + MIN_SIZE, 1.0);
        }
    }

    zone.position.x = ((left + right) * 0.5).clamp(0.0, 1.0);
    zone.position.y = ((top + bottom) * 0.5).clamp(0.0, 1.0);
    zone.size.x = (right - left).max(MIN_SIZE);
    zone.size.y = (bottom - top).max(MIN_SIZE);
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
