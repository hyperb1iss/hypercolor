//! Layout canvas — live effect preview with draggable zone overlays and group containers.

use leptos::ev;
use leptos::prelude::*;

use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::layout_geometry::{self, ResizeHandle};
use crate::style_utils::{device_accent_colors, hex_to_rgb};
use hypercolor_types::spatial::{NormalizedPosition, SpatialLayout, ZoneShape};

/// Canvas viewport with zone overlay divs and group containers.
#[component]
pub fn LayoutCanvas() -> impl IntoView {
    let editor = expect_context::<crate::components::layout_builder::LayoutEditorContext>();
    let layout = editor.layout;
    let selected_zone_id = editor.selected_zone_id;
    let keep_aspect_ratio = editor.keep_aspect_ratio;
    let hidden_zones = editor.hidden_zones;
    let set_selected_zone_id = editor.set_selected_zone_id;
    let set_layout = editor.set_layout;
    let set_is_dirty = editor.set_is_dirty;

    let ws = expect_context::<WsContext>();
    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let preview_fps = Signal::derive(move || ws.preview_fps.get());
    let preview_target_fps = Signal::derive(move || ws.preview_target_fps.get());

    let canvas_slot_ref = NodeRef::<leptos::html::Div>::new();
    let viewport_ref = NodeRef::<leptos::html::Div>::new();
    let (canvas_slot_size, set_canvas_slot_size) = signal((0.0_f64, 0.0_f64));

    // Drag state
    let (interaction, set_interaction) = signal(None::<InteractionState>);

    // Track which zone is actively being dragged/resized so we can disable
    // CSS transitions on it (prevents visual lag during interaction).
    let interacting_zone_id = Signal::derive(move || {
        interaction.get().map(|state| match &state {
            InteractionState::Drag(d) => d.zone_id.clone(),
            InteractionState::Resize(r) => r.zone_id.clone(),
        })
    });

    // Derive zone IDs sorted by display_order — only re-renders when zones are added/removed
    // or their stacking order changes, NOT when positions change during drag.
    let zone_ids = Memo::new(move |_| {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| {
                    let mut sorted: Vec<_> = l
                        .zones
                        .iter()
                        .enumerate()
                        .map(|(i, z)| (z.id.clone(), z.display_order, i))
                        .collect();
                    // Stable sort: by display_order, then by original vector position
                    sorted.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
                    sorted.into_iter().map(|(id, _, _)| id).collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    // Derive group data for rendering group containers (excludes fully hidden groups)
    let group_bounds = Memo::new(move |_| {
        let hidden = hidden_zones.get();
        layout.with(|current| {
            let Some(l) = current.as_ref() else {
                return Vec::new();
            };
            l.groups
                .iter()
                .filter_map(|group| {
                    // Only include visible member zones in the bounding box
                    let member_zones: Vec<_> = l
                        .zones
                        .iter()
                        .filter(|z| {
                            z.group_id.as_deref() == Some(&group.id) && !hidden.contains(&z.id)
                        })
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
                        color: group.color.clone().unwrap_or_else(|| "#e135ff".to_string()),
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

    let layout_ratio = Signal::derive(move || {
        layout
            .with(|current| {
                current.as_ref().map(|layout| {
                    f64::from(layout.canvas_width.max(1)) / f64::from(layout.canvas_height.max(1))
                })
            })
            .unwrap_or(320.0 / 200.0)
    });
    let viewport_style = Signal::derive(move || {
        let ratio = layout_ratio.get();
        let (slot_width, slot_height) = canvas_slot_size.get();

        if slot_width > 0.0 && slot_height > 0.0 && slot_width / slot_height > ratio {
            format!(
                "height: 100%; width: auto; max-width: 100%; max-height: 100%; aspect-ratio: {ratio};"
            )
        } else {
            format!(
                "width: 100%; height: auto; max-width: 100%; max-height: 100%; aspect-ratio: {ratio};"
            )
        }
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

    canvas_slot_ref.on_load({
        move |_| {
            update_canvas_slot_size(canvas_slot_ref, set_canvas_slot_size);
        }
    });

    let _resize_listener = window_event_listener(ev::resize, move |_| {
        update_canvas_slot_size(canvas_slot_ref, set_canvas_slot_size);
    });

    view! {
        <div
            node_ref=canvas_slot_ref
            class="relative w-full h-full overflow-hidden"
            style="background: var(--color-surface-base)"
            on:mouseup=move |_| {
                // Normalize zone size on interaction end (deferred from mousemove
                // to prevent strip aspect enforcement from fighting the user mid-drag).
                if let Some(state) = interaction.try_get_untracked().flatten() {
                    let zone_id = match &state {
                        InteractionState::Drag(d) => d.zone_id.clone(),
                        InteractionState::Resize(r) => r.zone_id.clone(),
                    };
                    set_layout.update(|l| {
                        if let Some(layout) = l
                            && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                                zone.size = layout_geometry::normalize_zone_size_for_editor(
                                    zone.position, zone.size, &zone.topology,
                                );
                            }
                    });
                }
                set_interaction.set(None);
            }
            on:mouseleave=move |_| {
                if let Some(state) = interaction.try_get_untracked().flatten() {
                    let zone_id = match &state {
                        InteractionState::Drag(d) => d.zone_id.clone(),
                        InteractionState::Resize(r) => r.zone_id.clone(),
                    };
                    set_layout.update(|l| {
                        if let Some(layout) = l
                            && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                                zone.size = layout_geometry::normalize_zone_size_for_editor(
                                    zone.position, zone.size, &zone.topology,
                                );
                            }
                    });
                }
                set_interaction.set(None);
            }
            on:mousemove=move |ev| {
                let Some(interaction_state) = interaction.get_untracked() else {
                    return;
                };
                let Some(viewport) = viewport_ref.try_get_untracked().flatten() else {
                    return;
                };
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
                                let _ = layout_geometry::drag_zone_to_position(
                                    layout,
                                    &zone_id,
                                    NormalizedPosition::new(norm_x, norm_y),
                                );
                            }
                        });
                    }
                    InteractionState::Resize(resize) => {
                        let zone_id = resize.zone_id.clone();
                        let keep_ratio = keep_aspect_ratio.get_untracked();
                        set_layout.update(|l| {
                            if let Some(layout) = l
                                && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                                    let (position, size) = layout_geometry::resize_zone_from_handle(
                                        resize.start_center,
                                        resize.start_size,
                                        resize.start_mouse,
                                        resize.handle,
                                        mouse_norm,
                                        keep_ratio,
                                        resize.rotation,
                                    );
                                    zone.position = position;
                                    // Raw size — normalization deferred to mouseup to prevent
                                    // strip aspect enforcement from fighting the user mid-drag.
                                    zone.size = size;
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

            <div class="absolute inset-0 flex items-start justify-center p-2 overflow-hidden">
                <div
                    class="relative rounded-lg overflow-hidden bg-black"
                    node_ref=viewport_ref
                    style=move || viewport_style.get()
                    on:click=move |_| {
                        set_selected_zone_id.set(None);
                    }
                >
                    // Live effect canvas background
                    <div class="absolute inset-0 pointer-events-none">
                        <CanvasPreview
                            frame=canvas_frame
                            fps=preview_fps
                            fps_target=preview_target_fps
                            show_fps=false
                            aspect_ratio=preview_aspect_ratio.get_untracked()
                        />
                    </div>

                    // Subtle border around the viewport
                    <div class="absolute inset-0 rounded-lg border border-edge-subtle/30 pointer-events-none" />

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
                                         border: 2px solid rgba({rgb}, 0.6); \
                                         background: linear-gradient(180deg, rgba({rgb}, 0.12) 0%, rgba({rgb}, 0.04) 100%); \
                                         box-shadow: 0 0 16px rgba({rgb}, 0.15), inset 0 0 12px rgba({rgb}, 0.05);"
                                    )
                                >
                                    // Top accent bar
                                    <div
                                        class="absolute top-0 left-2 right-2 h-px"
                                        style=format!("background: linear-gradient(90deg, transparent, rgba({rgb}, 0.6), transparent)")
                                    />
                                    // Group name label
                                    <div
                                        class="absolute -top-3.5 left-2 text-[10px] font-semibold font-mono px-2 py-0.5 rounded-md whitespace-nowrap"
                                        style=format!(
                                            "color: rgb({rgb}); background: rgba(0, 0, 0, 0.7); \
                                             border: 1px solid rgba({rgb}, 0.4); \
                                             text-shadow: 0 0 8px rgba({rgb}, 0.5); backdrop-filter: blur(4px)"
                                        )
                                    >
                                        {group.name} " (" {group.zone_count} ")"
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}

                    // Zone overlays — keyed on zone IDs sorted by display_order
                    {move || {
                        let ids = zone_ids.get();
                        let zone_count = ids.len();
                        ids.into_iter().enumerate().map(|(render_index, zone_id)| {
                        let base_z_index = render_index + 10; // 10+ to stay above groups
                        let elevated_z_index = zone_count + 100; // selected zone always on top
                        let _ = base_z_index; // used below in style closure
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
                                        let scale = zone.scale;

                                        // Use group color if available, else unique per-device color
                                        let (primary, secondary) = zone
                                            .group_id
                                            .as_deref()
                                            .and_then(|gid| {
                                                layout.groups.iter().find(|g| g.id == gid)
                                            })
                                            .and_then(|g| g.color.as_deref())
                                            .map(|hex| {
                                                let rgb = hex_to_rgb(hex);
                                                (rgb.clone(), rgb)
                                            })
                                            .unwrap_or_else(|| {
                                                device_accent_colors(&zone.device_id)
                                            });

                                        Some(ZoneRenderData {
                                            position_style: format!(
                                                "left: {x_pct:.2}%; top: {y_pct:.2}%; width: {w_pct:.2}%; height: {h_pct:.2}%; \
                                                 transform: translate(-50%, -50%) rotate({rotation:.1}deg) scale({scale:.3})"
                                            ),
                                            primary_rgb: primary,
                                            secondary_rgb: secondary,
                                            name: zone.name.clone(),
                                            led_count: zone.topology.led_count(),
                                            shape: zone.shape.clone(),
                                        })
                                    })
                                }
                            });

                            let is_selected = {
                                let zid = zid.clone();
                                Signal::derive(move || selected_zone_id.get().as_deref() == Some(&zid))
                            };

                            let is_hidden = {
                                let zid = zid.clone();
                                Signal::derive(move || hidden_zones.get().contains(&zid))
                            };

                            let is_interacting = {
                                let zid = zid.clone();
                                Signal::derive(move || interacting_zone_id.get().as_deref() == Some(&zid))
                            };

                            view! {
                                <div
                                    class=move || if is_interacting.get() {
                                        "absolute rounded-md cursor-move group"
                                    } else {
                                        "absolute rounded-md cursor-move group transition-[border-color,box-shadow,background,opacity] duration-300"
                                    }
                                    style=move || {
                                        let Some(zd) = zone_style.get() else {
                                            return "display: none".to_string();
                                        };
                                        let selected = is_selected.get();
                                        let hidden = is_hidden.get();
                                        let border = if selected {
                                            format!("border: 2px solid rgba({}, 0.85)", zd.primary_rgb)
                                        } else {
                                            format!("border: 1.5px solid rgba({}, 0.35)", zd.primary_rgb)
                                        };
                                        let bg = if selected {
                                            format!(
                                                "background: linear-gradient(135deg, rgba({}, 0.14), rgba({}, 0.08))",
                                                zd.primary_rgb, zd.secondary_rgb
                                            )
                                        } else {
                                            format!(
                                                "background: linear-gradient(135deg, rgba({}, 0.08), rgba({}, 0.03))",
                                                zd.primary_rgb, zd.secondary_rgb
                                            )
                                        };
                                        let shadow = if selected {
                                            format!(
                                                "box-shadow: 0 0 28px rgba({}, 0.4), 0 0 8px rgba({}, 0.6), inset 0 1px 0 rgba(255,255,255,0.05)",
                                                zd.primary_rgb, zd.secondary_rgb
                                            )
                                        } else {
                                            "box-shadow: 0 2px 8px rgba(0,0,0,0.3), inset 0 1px 0 rgba(255,255,255,0.03)"
                                                .to_string()
                                        };
                                        let shape = zone_shape_style(&zd.shape);
                                        let z = if selected { elevated_z_index } else { base_z_index };
                                        let visibility = if hidden {
                                            "opacity: 0.08; pointer-events: none; filter: grayscale(1)"
                                        } else {
                                            "opacity: 1"
                                        };
                                        format!(
                                            "{}; {}; {}; {}; {}; z-index: {z}; backdrop-filter: blur(4px) saturate(120%); {}",
                                            zd.position_style, border, bg, shadow, shape, visibility
                                        )
                                    }
                                    on:mousedown=move |ev| {
                                        ev.stop_propagation();
                                        ev.prevent_default();
                                        set_selected_zone_id.set(Some(zid_select.clone()));

                                        let Some(viewport) = viewport_ref.try_get_untracked().flatten() else {
                                            return;
                                        };
                                        let rect = viewport.get_bounding_client_rect();
                                        let cw = rect.width();
                                        let ch = rect.height();
                                        if cw <= 0.0 || ch <= 0.0 { return; }

                                        #[allow(clippy::cast_possible_truncation)]
                                        let mouse_norm_x = ((f64::from(ev.client_x()) - rect.left()) / cw) as f32;
                                        #[allow(clippy::cast_possible_truncation)]
                                        let mouse_norm_y = ((f64::from(ev.client_y()) - rect.top()) / ch) as f32;

                                        // Read zone position without tracking
                                        let zone_pos = layout.try_get_untracked()
                                            .flatten()
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
                                    {move || {
                                        zone_style.get().and_then(|zd| {
                                            ring_inner_style(
                                                &zd.shape,
                                                &zd.primary_rgb,
                                                &zd.secondary_rgb,
                                            )
                                            .map(|style| {
                                                view! {
                                                    <div
                                                        class="absolute inset-[20%] rounded-full pointer-events-none"
                                                        style=style
                                                    />
                                                }
                                                    .into_any()
                                            })
                                        })
                                    }}

                                    // Zone label — glass micro-panel (hover)
                                    <div
                                        class="absolute -top-6 left-0 text-[9px] font-mono whitespace-nowrap
                                                opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none
                                                px-2 py-0.5 rounded glass-subtle"
                                        style=move || {
                                            zone_style.get()
                                                .map(|zd| format!("color: rgba({}, 0.9)", zd.primary_rgb))
                                                .unwrap_or_default()
                                        }
                                    >
                                        {move || zone_style.get().map(|zd| format!("{} · {} LEDs", zd.name, zd.led_count)).unwrap_or_default()}
                                    </div>

                                    // Resize handles (selected only) — small circles with accent glow.
                                    // Counter-rotated so they stay axis-aligned, with dynamic cursors.
                                    {move || is_selected.get().then(|| {
                                        let zid_resize_nw = zid_resize_nw.clone();
                                        let zid_resize_ne = zid_resize_ne.clone();
                                        let zid_resize_sw = zid_resize_sw.clone();
                                        let zid_resize_se = zid_resize_se.clone();

                                        let handle_class = "absolute w-3 h-3 rounded-full border-2 transition-[box-shadow,transform] duration-150 \
                                                           hover:scale-125";

                                        // Derive rotation for counter-rotate + cursor
                                        let zone_rotation_deg = {
                                            let zid = zid.clone();
                                            Signal::derive(move || {
                                                layout.with(|current| {
                                                    current.as_ref()
                                                        .and_then(|l| l.zones.iter().find(|z| z.id == zid))
                                                        .map(|z| z.rotation.to_degrees())
                                                        .unwrap_or(0.0)
                                                })
                                            })
                                        };

                                        let handle_style_nw = move || {
                                            let rot = zone_rotation_deg.get();
                                            let cursor = rotated_cursor(ResizeHandle::NorthWest, rot);
                                            zone_style.get()
                                                .map(|zd| format!(
                                                    "background: rgba({}, 0.9); border-color: rgba(255,255,255,0.6); \
                                                     box-shadow: 0 0 8px rgba({}, 0.4); cursor: {cursor}",
                                                    zd.primary_rgb, zd.primary_rgb
                                                ))
                                                .unwrap_or_default()
                                        };
                                        let handle_style_ne = move || {
                                            let rot = zone_rotation_deg.get();
                                            let cursor = rotated_cursor(ResizeHandle::NorthEast, rot);
                                            zone_style.get()
                                                .map(|zd| format!(
                                                    "background: rgba({}, 0.9); border-color: rgba(255,255,255,0.6); \
                                                     box-shadow: 0 0 8px rgba({}, 0.4); cursor: {cursor}",
                                                    zd.primary_rgb, zd.primary_rgb
                                                ))
                                                .unwrap_or_default()
                                        };
                                        let handle_style_sw = move || {
                                            let rot = zone_rotation_deg.get();
                                            let cursor = rotated_cursor(ResizeHandle::SouthWest, rot);
                                            zone_style.get()
                                                .map(|zd| format!(
                                                    "background: rgba({}, 0.9); border-color: rgba(255,255,255,0.6); \
                                                     box-shadow: 0 0 8px rgba({}, 0.4); cursor: {cursor}",
                                                    zd.primary_rgb, zd.primary_rgb
                                                ))
                                                .unwrap_or_default()
                                        };
                                        let handle_style_se = move || {
                                            let rot = zone_rotation_deg.get();
                                            let cursor = rotated_cursor(ResizeHandle::SouthEast, rot);
                                            zone_style.get()
                                                .map(|zd| format!(
                                                    "background: rgba({}, 0.9); border-color: rgba(255,255,255,0.6); \
                                                     box-shadow: 0 0 8px rgba({}, 0.4); cursor: {cursor}",
                                                    zd.primary_rgb, zd.primary_rgb
                                                ))
                                                .unwrap_or_default()
                                        };

                                        view! {
                                            <div
                                                class=format!("{handle_class} -top-1.5 -left-1.5")
                                                style=handle_style_nw
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
                                                class=format!("{handle_class} -top-1.5 -right-1.5")
                                                style=handle_style_ne
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
                                                class=format!("{handle_class} -bottom-1.5 -left-1.5")
                                                style=handle_style_sw
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
                                                class=format!("{handle_class} -bottom-1.5 -right-1.5")
                                                style=handle_style_se
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

                                    // Zone identity — full-bleed radial vignette for contrast
                                    <div
                                        class="absolute inset-0 flex flex-col items-center justify-center pointer-events-none overflow-hidden p-1"
                                        style="background: radial-gradient(ellipse at center, rgba(0,0,0,0.55) 0%, rgba(0,0,0,0.2) 60%, transparent 100%)"
                                    >
                                        <div
                                            class="text-[10px] font-semibold leading-snug text-center max-w-full select-none break-words line-clamp-2 shrink-0"
                                            style=move || {
                                                zone_style.get()
                                                    .map(|zd| format!(
                                                        "color: rgba({}, 0.95); text-shadow: 0 1px 2px rgba(0,0,0,0.8), 0 0 8px rgba({}, 0.35)",
                                                        zd.primary_rgb, zd.primary_rgb
                                                    ))
                                                    .unwrap_or_default()
                                            }
                                        >
                                            {move || zone_style.get().map(|zd| zd.name.clone()).unwrap_or_default()}
                                        </div>
                                        <div
                                            class="text-[8px] font-mono select-none tabular-nums mt-0.5 shrink min-h-0 overflow-hidden"
                                            style="color: rgba(255, 255, 255, 0.55); text-shadow: 0 1px 2px rgba(0,0,0,0.6)"
                                        >
                                            {move || zone_style.get().map(|zd| format!("{} LEDs", zd.led_count)).unwrap_or_default()}
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}

                    // Empty canvas hint — shown over the live effect when no zones are placed
                    <Show when=move || !has_zones.get()>
                        <div class="absolute inset-0 flex items-center justify-center pointer-events-none">
                            <div class="text-center space-y-1.5 px-4 py-3 rounded-xl bg-black/50 backdrop-blur-sm">
                                <div class="text-white/40 text-sm font-medium">"Add devices from the palette"</div>
                                <div class="text-white/25 text-xs">"Drag zones to position them on the canvas"</div>
                            </div>
                        </div>
                    </Show>
                </div>
            </div>
        </div>
    }
}

/// Per-zone render data extracted from layout signal.
#[derive(Clone, Debug, PartialEq)]
struct ZoneRenderData {
    position_style: String,
    primary_rgb: String,
    secondary_rgb: String,
    name: String,
    led_count: u32,
    shape: Option<ZoneShape>,
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
    rotation: f32,
}

#[derive(Clone, Debug)]
enum InteractionState {
    Drag(DragState),
    Resize(ResizeState),
}

#[allow(clippy::too_many_arguments)]
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
    let Some(viewport) = viewport_ref.try_get_untracked().flatten() else {
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

    let zone_snapshot = layout.try_get_untracked().flatten().and_then(|current| {
        current
            .zones
            .iter()
            .find(|z| z.id == zone_id)
            .map(|zone| (zone.position, zone.size, zone.rotation))
    });

    let Some((start_center, start_size, rotation)) = zone_snapshot else {
        return;
    };

    set_selected_zone_id.set(Some(zone_id.to_owned()));
    set_interaction.set(Some(InteractionState::Resize(ResizeState {
        zone_id: zone_id.to_owned(),
        handle,
        start_mouse: mouse,
        start_center,
        start_size,
        rotation,
    })));
}

fn update_canvas_slot_size(
    canvas_slot_ref: NodeRef<leptos::html::Div>,
    set_canvas_slot_size: WriteSignal<(f64, f64)>,
) {
    if let Some(slot) = canvas_slot_ref.try_get_untracked().flatten() {
        let rect = slot.get_bounding_client_rect();
        set_canvas_slot_size.set((rect.width(), rect.height()));
    }
}

fn zone_shape_style(shape: &Option<ZoneShape>) -> String {
    match shape {
        Some(ZoneShape::Ring) | Some(ZoneShape::Arc { .. }) => "border-radius: 999px".to_owned(),
        _ => String::new(),
    }
}

fn ring_inner_style(
    shape: &Option<ZoneShape>,
    primary_rgb: &str,
    secondary_rgb: &str,
) -> Option<String> {
    match shape {
        Some(ZoneShape::Ring) => Some(format!(
            "border: 1px solid rgba({primary_rgb}, 0.16); \
             background: radial-gradient(circle, rgba(0, 0, 0, 0.5), rgba({secondary_rgb}, 0.04)); \
             box-shadow: inset 0 0 18px rgba(0, 0, 0, 0.45)"
        )),
        _ => None,
    }
}

/// Compute the CSS cursor for a resize handle, accounting for zone rotation.
///
/// Each handle has a base angle (NW=315°, NE=45°, SE=135°, SW=225°). We add
/// the zone rotation, then snap to the nearest 45° cursor direction.
fn rotated_cursor(handle: ResizeHandle, rotation_deg: f32) -> &'static str {
    let base = match handle {
        ResizeHandle::NorthWest => 315.0,
        ResizeHandle::NorthEast => 45.0,
        ResizeHandle::SouthEast => 135.0,
        ResizeHandle::SouthWest => 225.0,
    };
    let effective = (base + rotation_deg).rem_euclid(360.0);
    // Snap to nearest 45° sector
    let sector = ((effective + 22.5) / 45.0) as u32 % 8;
    match sector {
        0 => "n-resize",
        1 => "ne-resize",
        2 => "e-resize",
        3 => "se-resize",
        4 => "s-resize",
        5 => "sw-resize",
        6 => "w-resize",
        7 => "nw-resize",
        _ => "nw-resize",
    }
}
