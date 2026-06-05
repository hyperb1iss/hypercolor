//! Layout canvas — live effect preview with draggable zone overlays.
//!
//! The hot path during drag/resize is intentionally non-reactive: a single
//! `requestAnimationFrame` scheduler reads the latest pointer position from a
//! `Cell`, computes the new zone geometry against an immutable base snapshot,
//! and writes the result *directly* to the cached zone DOM elements. The
//! layout signal is only updated once on `mouseup`. This bypasses the
//! reactive flush that would otherwise trigger O(N²) zone-style recomputes
//! on every mousemove and lets the `CanvasPreview` RAF loop keep painting.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use leptos::ev;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;

use hypercolor_leptos_ext::raf::Scheduler;

use crate::app::{DevicesContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::device_card::{DeviceClass, classify_device};
use crate::compound_selection::{self, CompoundDepth};
use crate::layout_geometry::{self, ResizeHandle};
use crate::layout_utils;
use crate::style_utils::device_accent_colors;
use hypercolor_types::spatial::{NormalizedPosition, Output, ZoneShape};

mod interaction;
mod overlays;
mod render;

use interaction::{
    DragRuntime, InteractionKind, collect_zone_elements, pointer_to_normalized,
    update_canvas_slot_size,
};
use overlays::{CanvasDepthBreadcrumb, CompoundBoundingBoxOutline};
use render::{ZoneRenderData, ring_inner_style, rotated_cursor, zone_shape_style};

/// Throttle the in-drag preview push to the daemon. Matches the existing
/// debounce we use outside drags so the spatial engine isn't recomputed at
/// 60 Hz when a single coalesced 75 ms cadence is enough for a smooth feel.
const PREVIEW_PUSH_INTERVAL_MS: f64 = 75.0;

/// Canvas viewport with zone overlay divs.
#[component]
pub fn LayoutCanvas() -> impl IntoView {
    let editor = expect_context::<crate::components::layout_builder::LayoutEditorContext>();
    let layout = editor.layout;
    let selected_zone_ids = editor.selected_zone_ids;
    let compound_depth = editor.compound_depth;
    let keep_aspect_ratio = editor.keep_aspect_ratio;
    let hidden_zones = editor.hidden_zones;
    let hovered_zone_ids = editor.hovered_zone_ids;
    // When anything is focused (selected or hovered), the rest of the canvas
    // recedes so a dense layout reads clearly around the focus.
    let has_focus = Signal::derive(move || {
        !selected_zone_ids.get().is_empty() || !hovered_zone_ids.get().is_empty()
    });
    let set_selected_zone_ids = editor.set_selected_zone_ids;
    let set_compound_depth = editor.set_compound_depth;
    let set_layout = editor.set_layout;
    let set_is_dirty = editor.set_is_dirty;
    let push_preview = editor.push_preview;
    let devices_ctx = expect_context::<DevicesContext>();
    let zone_display_ctx =
        expect_context::<crate::components::layout_builder::LayoutZoneDisplayContext>();

    let ws = expect_context::<WsContext>();
    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let preview_fps = Signal::derive(move || ws.preview_fps.get());
    let preview_target_fps = Signal::derive(move || ws.preview_target_fps.get());

    let canvas_slot_ref = NodeRef::<leptos::html::Div>::new();
    let viewport_ref = NodeRef::<leptos::html::Div>::new();
    let (canvas_slot_size, set_canvas_slot_size) = signal((0.0_f64, 0.0_f64));

    // Active drag/resize runtime — non-reactive, accessed by mouse handlers
    // and the RAF scheduler. Cleared on mouseup. `LocalStorage` lets us
    // store the (non-Send) `web_sys::HtmlElement` cache while the
    // `StoredValue` handle itself stays `Copy + Send + Sync` so it can ride
    // along inside reactive `move ||` closures.
    let drag_runtime: StoredValue<Option<DragRuntime>, LocalStorage> = StoredValue::new_local(None);

    // Reactive flag exposing which zone (if any) is being interacted with.
    // Used purely to toggle a CSS class that disables zone transitions.
    let interacting_zone_id = RwSignal::new(None::<String>);

    // RAF-driven step. Reads `pending_mouse` from the runtime, runs the
    // geometry math on the running zone copy, paints affected elements
    // directly, and throttles preview pushes to the daemon.
    let scheduler: Rc<RefCell<Option<Scheduler>>> = Rc::new(RefCell::new(None));
    {
        let layout_signal = layout;
        let scheduler_inst = Scheduler::new(move |frame_info| {
            let painted_change = drag_runtime
                .try_update_value(|opt| opt.as_mut().is_some_and(DragRuntime::step))
                .unwrap_or(false);
            if !painted_change {
                return;
            }

            // Throttle daemon preview pushes — 75 ms matches the existing
            // debounce we use outside of drags.
            let now_ms = frame_info.monotonic_ms;
            let should_push = drag_runtime
                .try_update_value(|opt| {
                    let runtime = opt.as_mut()?;
                    if now_ms - runtime.last_preview_push_ms.get() < PREVIEW_PUSH_INTERVAL_MS {
                        return None;
                    }
                    runtime.last_preview_push_ms.set(now_ms);
                    Some(runtime.current_zones.clone())
                })
                .flatten();
            // Build a layout snapshot with the in-flight zones overlaid on
            // the rest of the saved layout (canvas dims, sampling, etc.)
            // and push it to the daemon. This keeps the LED preview live
            // without touching the reactive layout signal.
            if let Some(zones) = should_push
                && let Some(mut snapshot) = layout_signal.with_untracked(Clone::clone)
            {
                snapshot.zones = zones;
                push_preview.run(snapshot);
            }
        });
        *scheduler.borrow_mut() = Some(scheduler_inst);
    }

    // Derive zone IDs sorted by display_order — only re-renders when zones are added/removed
    // or their stacking order changes, NOT when positions change during drag.
    let suppressed_zone_ids = Memo::new(move |_| {
        layout.with(|current| {
            current
                .as_ref()
                .map(layout_utils::suppressed_attachment_source_zone_ids)
                .unwrap_or_default()
        })
    });

    let zone_ids = Memo::new(move |_| {
        let suppressed = suppressed_zone_ids.get();
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| {
                    let mut sorted: Vec<_> = l
                        .zones
                        .iter()
                        .filter(|zone| !suppressed.contains(&zone.id))
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

    // Per-id zone lookup memo — lets every per-zone style closure resolve
    // its zone in O(1). Replaces the O(N) `zones.iter().find(|z| z.id == zid)`
    // scan that ran inside each `zone_style` derive.
    let zones_by_id: Memo<HashMap<String, Output>> = Memo::new(move |_| {
        layout
            .with(|current| {
                current.as_ref().map(|l| {
                    l.zones
                        .iter()
                        .map(|z| (z.id.clone(), z.clone()))
                        .collect::<HashMap<_, _>>()
                })
            })
            .unwrap_or_default()
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

    // Commit the in-flight runtime to the layout signal and clear it. Run
    // when the pointer is released or leaves the canvas slot. Idempotent:
    // safe to call when no runtime is active.
    let finish_interaction = {
        let layout_signal = layout;
        move || {
            let Some(mut runtime) = drag_runtime.try_update_value(Option::take).flatten() else {
                return;
            };
            interacting_zone_id.set(None);

            if !runtime.moved.get() {
                // No actual movement happened — discard without touching
                // the signal or history stack.
                set_layout.finish_interaction();
                return;
            }

            // Apply size normalization (strip aspect / ring squaring) once
            // at release. Doing this mid-drag would fight the pointer.
            for zone in &mut runtime.current_zones {
                zone.size = layout_geometry::normalize_zone_size_for_editor(
                    zone.position,
                    zone.size,
                    &zone.topology,
                );
            }
            let final_zones = std::mem::take(&mut runtime.current_zones);
            let committed = set_layout.commit_zones(final_zones);
            set_layout.finish_interaction();
            if committed {
                set_is_dirty.set(true);
                if let Some(snapshot) = layout_signal.get_untracked() {
                    push_preview.run(snapshot);
                }
            }
        }
    };
    let finish_for_mouseup = finish_interaction;
    let finish_for_leave = finish_interaction;

    let scheduler_for_move = Rc::clone(&scheduler);

    view! {
        <div
            node_ref=canvas_slot_ref
            class="relative w-full h-full overflow-hidden"
            style="background: var(--color-surface-base)"
            on:mouseup=move |_| finish_for_mouseup()
            on:mouseleave=move |_| finish_for_leave()
            on:mousemove=move |ev| {
                // Lightweight hot path: stash the latest pointer position
                // and ask the RAF scheduler for a frame. All the real work
                // happens in the scheduler callback at most once per frame,
                // so 120-Hz mousemove storms collapse to ~60-Hz updates.
                let active = drag_runtime.with_value(Option::is_some);
                if !active {
                    return;
                }
                let Some(viewport) = viewport_ref.try_get_untracked().flatten() else {
                    return;
                };
                let viewport_el: web_sys::HtmlElement = (*viewport).clone();
                let Some(mouse_norm) = pointer_to_normalized(
                    &viewport_el,
                    ev.client_x(),
                    ev.client_y(),
                ) else {
                    return;
                };
                drag_runtime.with_value(|opt| {
                    if let Some(runtime) = opt.as_ref() {
                        runtime.pending_mouse.set(Some(mouse_norm));
                    }
                });
                if let Some(scheduler) = scheduler_for_move.borrow().as_ref() {
                    scheduler.schedule();
                }
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
                        set_selected_zone_ids.set(std::collections::HashSet::new());
                        set_compound_depth.set(CompoundDepth::Root);
                    }
                    on:keydown=move |ev| {
                        if ev.key() == "Escape" {
                            let depth = compound_depth.get_untracked();
                            match depth {
                                CompoundDepth::Slot { device_id, .. } => {
                                    // Exit slot → back to device level, select device compound
                                    set_compound_depth.set(CompoundDepth::Device { device_id: device_id.clone() });
                                    layout.with_untracked(|l| {
                                        if let Some(l) = l.as_ref() {
                                            set_selected_zone_ids.set(compound_selection::device_compound_ids(l, &device_id));
                                        }
                                    });
                                }
                                CompoundDepth::Device { .. } => {
                                    set_compound_depth.set(CompoundDepth::Root);
                                    set_selected_zone_ids.set(std::collections::HashSet::new());
                                }
                                CompoundDepth::Root => {
                                    set_selected_zone_ids.set(std::collections::HashSet::new());
                                }
                            }
                        }
                    }
                    tabindex="0"
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

                    // Zone overlays — keyed on zone IDs sorted by display_order
                    {move || {
                        let ids = zone_ids.get();
                        let zone_count = ids.len();
                        ids.into_iter().enumerate().map(|(render_index, zone_id)| {
                        let base_z_index = render_index + 10;
                        let elevated_z_index = zone_count + 100; // selected zone always on top
                        let _ = base_z_index; // used below in style closure
                            let zid = zone_id.clone();
                            let zid_select = zone_id.clone();
                            let zid_dblclick = zone_id.clone();
                            let zid_drag = zone_id.clone();
                            let zid_drag2 = zone_id.clone();
                            let zid_resize_nw = zone_id.clone();
                            let zid_resize_ne = zone_id.clone();
                            let zid_resize_sw = zone_id.clone();
                            let zid_resize_se = zone_id.clone();

                            // Derive per-zone position/style reactively from the layout signal.
                            // Uses the indexed `zones_by_id` memo for O(1) lookup so this
                            // closure no longer scans the full zone vec on every layout update.
                            let zone_style = Signal::derive({
                                let zid = zid.clone();
                                move || {
                                    let devices = devices_ctx
                                        .devices_resource
                                        .get()
                                        .and_then(Result::ok)
                                        .unwrap_or_default();
                                    let attachment_profiles =
                                        zone_display_ctx.attachment_profiles.get().unwrap_or_default();
                                    zones_by_id.with(|map| {
                                        let zone = map.get(&zid)?;
                                        // A generic ARGB controller is just raw channels until
                                        // the user attaches a component, so an unattached channel
                                        // stays out of the layout entirely — only configured
                                        // hardware draws. Fixed devices (keyboards, AIOs) always
                                        // render; they have meaningful LEDs without a component.
                                        if zone.attachment.is_none()
                                            && devices
                                                .iter()
                                                .find(|device| {
                                                    device.layout_device_id == zone.device_id
                                                })
                                                .is_some_and(|device| {
                                                    classify_device(device) == DeviceClass::Controller
                                                })
                                        {
                                            return None;
                                        }
                                        let x_pct = zone.position.x * 100.0;
                                        let y_pct = zone.position.y * 100.0;
                                        let w_pct = zone.size.x * 100.0;
                                        let h_pct = zone.size.y * 100.0;
                                        let rotation = zone.rotation.to_degrees();
                                        let scale = zone.scale;

                                        let (primary, secondary) = device_accent_colors(&zone.device_id);
                                        let display = layout_utils::effective_zone_display(
                                            zone,
                                            &devices,
                                            &attachment_profiles,
                                        );

                                        // For Ring/Arc zones, omit explicit height and use
                                        // aspect-ratio: 1 so the browser enforces a perfect
                                        // circle regardless of canvas aspect ratio.
                                        let is_circular = matches!(
                                            zone.shape,
                                            Some(ZoneShape::Ring) | Some(ZoneShape::Arc { .. })
                                        );
                                        let position_style = if is_circular {
                                            format!(
                                                "left: {x_pct:.2}%; top: {y_pct:.2}%; width: {w_pct:.2}%; aspect-ratio: 1; \
                                                 transform: translate(-50%, -50%) rotate({rotation:.1}deg) scale({scale:.3})"
                                            )
                                        } else {
                                            format!(
                                                "left: {x_pct:.2}%; top: {y_pct:.2}%; width: {w_pct:.2}%; height: {h_pct:.2}%; \
                                                 transform: translate(-50%, -50%) rotate({rotation:.1}deg) scale({scale:.3})"
                                            )
                                        };

                                        Some(ZoneRenderData {
                                            position_style,
                                            primary_rgb: primary,
                                            secondary_rgb: secondary,
                                            name: display.label,
                                            led_count: zone.topology.led_count(),
                                            shape: zone.shape.clone(),
                                        })
                                    })
                                }
                            });

                            let is_selected = {
                                let zid = zid.clone();
                                Signal::derive(move || selected_zone_ids.with(|ids| ids.contains(&zid)))
                            };

                            let is_hovered = {
                                let zid = zid.clone();
                                Signal::derive(move || hovered_zone_ids.with(|ids| ids.contains(&zid)))
                            };

                            let is_hidden = {
                                let zid = zid.clone();
                                Signal::derive(move || hidden_zones.get().contains(&zid))
                            };

                            let is_interacting = {
                                let zid = zid.clone();
                                Signal::derive(move || interacting_zone_id.with(|active| {
                                    active.as_deref() == Some(&zid)
                                }))
                            };

                            view! {
                                <div
                                    data-zone-id=zid.clone()
                                    class=move || if is_interacting.get() {
                                        "absolute rounded-md cursor-move group"
                                    } else {
                                        "absolute rounded-md cursor-move group transition-[border-color,box-shadow,background,opacity,filter] duration-300"
                                    }
                                    style=move || {
                                        let Some(zd) = zone_style.get() else {
                                            return "display: none".to_string();
                                        };
                                        let hidden = is_hidden.get();
                                        let selected = is_selected.get();
                                        // Hover only lifts a box that is not already selected, so
                                        // the persistent selection always reads stronger.
                                        let hovered = is_hovered.get() && !selected;
                                        let border = if selected {
                                            format!("border: 2px solid rgba({}, 0.85)", zd.primary_rgb)
                                        } else if hovered {
                                            format!("border: 2px solid rgba({}, 0.62)", zd.primary_rgb)
                                        } else {
                                            format!("border: 1.5px solid rgba({}, 0.35)", zd.primary_rgb)
                                        };
                                        let bg = if selected {
                                            format!(
                                                "background: linear-gradient(135deg, rgba({}, 0.14), rgba({}, 0.08))",
                                                zd.primary_rgb, zd.secondary_rgb
                                            )
                                        } else if hovered {
                                            format!(
                                                "background: linear-gradient(135deg, rgba({}, 0.11), rgba({}, 0.05))",
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
                                        } else if hovered {
                                            format!(
                                                "box-shadow: 0 0 16px rgba({}, 0.3), inset 0 1px 0 rgba(255,255,255,0.04)",
                                                zd.primary_rgb
                                            )
                                        } else {
                                            "box-shadow: 0 2px 8px rgba(0,0,0,0.3), inset 0 1px 0 rgba(255,255,255,0.03)"
                                                .to_string()
                                        };
                                        let shape = zone_shape_style(&zd.shape);
                                        let z = if selected || hovered { elevated_z_index } else { base_z_index };
                                        // A box not in focus while something else is recedes,
                                        // so a dense canvas reads around the selection/hover.
                                        let dimmed_by_focus = has_focus.get() && !selected && !hovered;
                                        let visibility = if hidden {
                                            "opacity: 0.08; pointer-events: none; filter: grayscale(1)".to_string()
                                        } else if dimmed_by_focus {
                                            "opacity: 0.26".to_string()
                                        } else {
                                            "opacity: 1".to_string()
                                        };
                                        format!(
                                            "{}; {}; {}; {}; {}; z-index: {z}; backdrop-filter: blur(4px) saturate(120%); {}",
                                            zd.position_style, border, bg, shadow, shape, visibility
                                        )
                                    }
                                    on:mousedown=move |ev| {
                                        ev.stop_propagation();
                                        ev.prevent_default();

                                        // Compound-aware selection
                                        let depth = compound_depth.get_untracked();
                                        let (ids, clicked_different_device) = layout.with_untracked(|l| {
                                            let Some(l) = l.as_ref() else {
                                                return (std::collections::HashSet::new(), false);
                                            };
                                            let ids = compound_selection::resolve_click(l, &zid_select, &depth);
                                            // Check if clicked zone is from a different device than entered
                                            let different = match &depth {
                                                CompoundDepth::Device { device_id } | CompoundDepth::Slot { device_id, .. } => {
                                                    l.zones.iter()
                                                        .find(|z| z.id == zid_select)
                                                        .is_some_and(|z| z.device_id != *device_id)
                                                }
                                                CompoundDepth::Root => false,
                                            };
                                            (ids, different)
                                        });

                                        // Reset depth if clicked outside entered compound
                                        if clicked_different_device {
                                            set_compound_depth.set(CompoundDepth::Root);
                                        }

                                        let is_shift = ev.shift_key();
                                        if is_shift {
                                            // Shift+click: toggle compound in/out of selection (no drag)
                                            let mut current = selected_zone_ids.get_untracked();
                                            for id in &ids {
                                                if !current.remove(id) {
                                                    current.insert(id.clone());
                                                }
                                            }
                                            set_selected_zone_ids.set(current);
                                            return; // Don't start drag on shift+click
                                        }
                                        set_selected_zone_ids.set(ids);

                                        let Some(viewport) = viewport_ref.try_get_untracked().flatten() else {
                                            return;
                                        };
                                        let viewport_el: web_sys::HtmlElement = (*viewport).clone();
                                        let Some(mouse_norm) = pointer_to_normalized(
                                            &viewport_el,
                                            ev.client_x(),
                                            ev.client_y(),
                                        ) else {
                                            return;
                                        };

                                        // Snapshot the layout once and capture every zone we need
                                        // so the drag can run entirely against an in-flight copy.
                                        let Some(snapshot) = layout.get_untracked() else {
                                            return;
                                        };
                                        let primary_zone_id = zid_drag.clone();
                                        let Some(primary_zone) = snapshot
                                            .zones
                                            .iter()
                                            .find(|z| z.id == primary_zone_id)
                                        else {
                                            return;
                                        };
                                        let zx = primary_zone.position.x;
                                        let zy = primary_zone.position.y;

                                        let dragged_ids: std::collections::HashSet<String> =
                                            selected_zone_ids.get_untracked();
                                        let initial_positions: Vec<(String, NormalizedPosition)> = snapshot
                                            .zones
                                            .iter()
                                            .filter(|z| dragged_ids.contains(&z.id))
                                            .map(|z| (z.id.clone(), z.position))
                                            .collect();

                                        let elements = collect_zone_elements(
                                            &viewport_el,
                                            initial_positions.iter().map(|(id, _)| id.clone()),
                                        );

                                        set_layout.begin_interaction();
                                        interacting_zone_id.set(Some(zid_drag2.clone()));

                                        let runtime = DragRuntime {
                                            kind: InteractionKind::Drag {
                                                primary_zone_id,
                                                offset_x: mouse_norm.x - zx,
                                                offset_y: mouse_norm.y - zy,
                                                initial_positions,
                                            },
                                            current_zones: snapshot.zones,
                                            elements,
                                            pending_mouse: Cell::new(None),
                                            moved: Cell::new(false),
                                            last_preview_push_ms: Cell::new(0.0),
                                        };
                                        drag_runtime.set_value(Some(runtime));
                                    }
                                    on:dblclick=move |ev| {
                                        ev.stop_propagation();
                                        let depth = compound_depth.get_untracked();
                                        layout.with_untracked(|l| {
                                            let Some(l) = l.as_ref() else { return; };
                                            let Some(zone) = l.zones.iter().find(|z| z.id == zid_dblclick) else { return; };
                                            match &depth {
                                                CompoundDepth::Root => {
                                                    // Enter device
                                                    let device_id = zone.device_id.clone();
                                                    set_compound_depth.set(CompoundDepth::Device { device_id: device_id.clone() });
                                                    // Select the clicked zone's slot compound or individual zone
                                                    let inner_depth = CompoundDepth::Device { device_id };
                                                    set_selected_zone_ids.set(compound_selection::resolve_click(l, &zid_dblclick, &inner_depth));
                                                }
                                                CompoundDepth::Device { device_id } => {
                                                    if zone.device_id != *device_id {
                                                        // Different device — enter that device instead
                                                        let new_did = zone.device_id.clone();
                                                        set_compound_depth.set(CompoundDepth::Device { device_id: new_did.clone() });
                                                        let inner = CompoundDepth::Device { device_id: new_did };
                                                        set_selected_zone_ids.set(compound_selection::resolve_click(l, &zid_dblclick, &inner));
                                                    } else if let Some(slot_id) = zone.attachment.as_ref().map(|a| a.slot_id.clone()) {
                                                        // Enter slot
                                                        set_compound_depth.set(CompoundDepth::Slot {
                                                            device_id: device_id.clone(),
                                                            slot_id,
                                                        });
                                                        set_selected_zone_ids.set(std::collections::HashSet::from([zid_dblclick.clone()]));
                                                    }
                                                    // No attachment → already at individual level, no-op
                                                }
                                                CompoundDepth::Slot { device_id, .. } => {
                                                    if zone.device_id != *device_id {
                                                        // Different device — enter that device
                                                        let new_did = zone.device_id.clone();
                                                        set_compound_depth.set(CompoundDepth::Device { device_id: new_did.clone() });
                                                        let inner = CompoundDepth::Device { device_id: new_did };
                                                        set_selected_zone_ids.set(compound_selection::resolve_click(l, &zid_dblclick, &inner));
                                                    }
                                                    // Same device, same/different slot — already at deepest level
                                                }
                                            }
                                        });
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
                                    {
                                        let zid_for_resize = zid.clone();
                                        let zid_resize_nw = zid_resize_nw.clone();
                                        let _ = (&zid_resize_ne, &zid_resize_sw, &zid_resize_se);
                                        move || is_selected.get().then(|| {
                                        let zid_resize_nw = zid_resize_nw.clone();
                                        let zid_for_rotation = zid_for_resize.clone();

                                        let handle_class = "absolute w-3 h-3 rounded-full border-2 transition-[box-shadow,transform] duration-150 \
                                                           hover:scale-125";

                                        // Shared resize starter — builds a `DragRuntime` keyed to the
                                        // requested handle and seeds it with the current zone snapshot.
                                        let start_resize: Rc<dyn Fn(ResizeHandle, i32, i32)> = {
                                            let zone_id_template = zid_resize_nw.clone();
                                            Rc::new(move |handle, client_x, client_y| {
                                                let Some(viewport) = viewport_ref.try_get_untracked().flatten() else {
                                                    return;
                                                };
                                                let viewport_el: web_sys::HtmlElement =
                                                    (*viewport).clone();
                                                let Some(mouse_norm) = pointer_to_normalized(
                                                    &viewport_el,
                                                    client_x,
                                                    client_y,
                                                ) else {
                                                    return;
                                                };
                                                let Some(snapshot) = layout.get_untracked() else {
                                                    return;
                                                };
                                                let zone_id = zone_id_template.clone();
                                                let Some(zone) = snapshot.zones.iter().find(|z| z.id == zone_id) else {
                                                    return;
                                                };
                                                let start_center = zone.position;
                                                let start_size = zone.size;
                                                let rotation = zone.rotation;

                                                set_selected_zone_ids
                                                    .set(std::collections::HashSet::from([zone_id.clone()]));
                                                set_layout.begin_interaction();
                                                interacting_zone_id.set(Some(zone_id.clone()));

                                                let elements = collect_zone_elements(
                                                    &viewport_el,
                                                    std::iter::once(zone_id.clone()),
                                                );

                                                let runtime = DragRuntime {
                                                    kind: InteractionKind::Resize {
                                                        zone_id,
                                                        handle,
                                                        start_mouse: mouse_norm,
                                                        start_center,
                                                        start_size,
                                                        rotation,
                                                        keep_aspect_ratio: keep_aspect_ratio.get_untracked(),
                                                    },
                                                    current_zones: snapshot.zones,
                                                    elements,
                                                    pending_mouse: Cell::new(None),
                                                    moved: Cell::new(false),
                                                    last_preview_push_ms: Cell::new(0.0),
                                                };
                                                drag_runtime.set_value(Some(runtime));
                                            })
                                        };
                                        let start_resize_nw = Rc::clone(&start_resize);
                                        let start_resize_ne = Rc::clone(&start_resize);
                                        let start_resize_sw = Rc::clone(&start_resize);
                                        let start_resize_se = Rc::clone(&start_resize);

                                        // Derive rotation for counter-rotate + cursor — O(1) via zones_by_id
                                        let zone_rotation_deg = {
                                            let zid = zid_for_rotation;
                                            Signal::derive(move || {
                                                zones_by_id.with(|map| {
                                                    map.get(&zid)
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
                                                    start_resize_nw(ResizeHandle::NorthWest, ev.client_x(), ev.client_y());
                                                }
                                            />
                                            <div
                                                class=format!("{handle_class} -top-1.5 -right-1.5")
                                                style=handle_style_ne
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    start_resize_ne(ResizeHandle::NorthEast, ev.client_x(), ev.client_y());
                                                }
                                            />
                                            <div
                                                class=format!("{handle_class} -bottom-1.5 -left-1.5")
                                                style=handle_style_sw
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    start_resize_sw(ResizeHandle::SouthWest, ev.client_x(), ev.client_y());
                                                }
                                            />
                                            <div
                                                class=format!("{handle_class} -bottom-1.5 -right-1.5")
                                                style=handle_style_se
                                                on:mousedown=move |ev| {
                                                    ev.stop_propagation();
                                                    ev.prevent_default();
                                                    start_resize_se(ResizeHandle::SouthEast, ev.client_x(), ev.client_y());
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
                                            class="text-[11px] font-semibold leading-tight tracking-tight text-center max-w-full select-none break-words line-clamp-2 shrink-0"
                                            style=move || {
                                                zone_style.get()
                                                    .map(|zd| format!(
                                                        "color: rgba({}, 0.96); text-shadow: 0 1px 2px rgba(0,0,0,0.85), 0 0 10px rgba({}, 0.4)",
                                                        zd.primary_rgb, zd.primary_rgb
                                                    ))
                                                    .unwrap_or_default()
                                            }
                                        >
                                            {move || zone_style.get().map(|zd| zd.name.clone()).unwrap_or_default()}
                                        </div>
                                        <div
                                            class="text-[9px] font-mono select-none tabular-nums mt-1 shrink min-h-0 overflow-hidden"
                                            style="color: rgba(255, 255, 255, 0.68); text-shadow: 0 1px 2px rgba(0,0,0,0.7)"
                                        >
                                            {move || zone_style.get().map(|zd| format!("{} LEDs", zd.led_count)).unwrap_or_default()}
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}

                    <CompoundBoundingBoxOutline
                        layout=layout
                        selected_zone_ids=selected_zone_ids
                        compound_depth=compound_depth
                    />

                    <CanvasDepthBreadcrumb
                        compound_depth=compound_depth
                        devices_resource=devices_ctx.devices_resource
                    />

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
