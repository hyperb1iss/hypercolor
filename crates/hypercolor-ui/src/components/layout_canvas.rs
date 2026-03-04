//! Layout canvas — live effect preview with draggable zone overlays.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::device_card::backend_accent_rgb;
use hypercolor_types::spatial::SpatialLayout;

/// Canvas viewport with zone overlay divs.
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

    let container_ref = NodeRef::<leptos::html::Div>::new();

    // Drag state
    let (dragging, set_dragging) = signal(None::<DragState>);

    // Derive just the zone IDs — only re-renders the zone list when zones are added/removed,
    // NOT when positions change during drag.
    let zone_ids = Memo::new(move |_| {
        layout
            .get()
            .map(|l| l.zones.iter().map(|z| z.id.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    });

    view! {
        <div
            class="relative w-full h-full bg-black"
            node_ref=container_ref
            on:mouseup=move |_| {
                set_dragging.set(None);
            }
            on:mouseleave=move |_| {
                set_dragging.set(None);
            }
            on:mousemove=move |ev| {
                let Some(drag) = dragging.get() else { return };
                let Some(container) = container_ref.get() else { return };
                let rect = container.get_bounding_client_rect();
                let cw = rect.width();
                let ch = rect.height();
                if cw <= 0.0 || ch <= 0.0 { return; }

                let mouse_x = f64::from(ev.client_x()) - rect.left();
                let mouse_y = f64::from(ev.client_y()) - rect.top();

                #[allow(clippy::cast_possible_truncation)]
                let norm_x = ((mouse_x / cw) as f32 - drag.offset_x).clamp(0.0, 1.0);
                #[allow(clippy::cast_possible_truncation)]
                let norm_y = ((mouse_y / ch) as f32 - drag.offset_y).clamp(0.0, 1.0);

                let zone_id = drag.zone_id.clone();
                set_layout.update(|l| {
                    if let Some(layout) = l {
                        if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                            zone.position.x = norm_x;
                            zone.position.y = norm_y;
                        }
                    }
                });
                set_is_dirty.set(true);
            }
            on:click=move |ev| {
                // Click on background deselects
                if ev.target() == ev.current_target() {
                    set_selected_zone_id.set(None);
                }
            }
        >
            // Live effect canvas background
            <div class="absolute inset-0 pointer-events-none">
                <CanvasPreview
                    frame=canvas_frame
                    fps=ws_fps
                    show_fps=false
                />
            </div>

            // Zone overlays — keyed on zone IDs, only re-renders when zones are added/removed
            {move || {
                zone_ids.get().into_iter().map(|zone_id| {
                    let zid = zone_id.clone();
                    let zid_select = zone_id.clone();
                    let zid_drag = zone_id.clone();
                    let zid_drag2 = zone_id.clone();

                    // Derive per-zone position/style reactively from the layout signal
                    let zone_style = Signal::derive({
                        let zid = zid.clone();
                        move || {
                            let l = layout.get()?;
                            let zone = l.zones.iter().find(|z| z.id == zid)?;
                            let x_pct = zone.position.x * 100.0;
                            let y_pct = zone.position.y * 100.0;
                            let w_pct = zone.size.x * 100.0;
                            let h_pct = zone.size.y * 100.0;
                            let rotation = zone.rotation.to_degrees();
                            let backend = zone.device_id.split(':').next().unwrap_or("");
                            let rgb = backend_accent_rgb(backend).to_string();
                            Some((
                                format!(
                                    "left: {x_pct:.2}%; top: {y_pct:.2}%; width: {w_pct:.2}%; height: {h_pct:.2}%; \
                                     transform: translate(-50%, -50%) rotate({rotation:.1}deg)"
                                ),
                                rgb,
                                zone.name.clone(),
                                zone.topology.led_count(),
                            ))
                        }
                    });

                    let is_selected = {
                        let zid = zid.clone();
                        Signal::derive(move || selected_zone_id.get().as_deref() == Some(&zid))
                    };

                    view! {
                        <div
                            class="absolute border-2 rounded cursor-move group"
                            style=move || {
                                let Some((base, rgb, _, _)) = zone_style.get() else {
                                    return "display: none".to_string();
                                };
                                let selected = is_selected.get();
                                let border_opacity = if selected { "1.0" } else { "0.6" };
                                let shadow = if selected {
                                    format!("box-shadow: 0 0 16px rgba({rgb}, 0.4)")
                                } else {
                                    String::new()
                                };
                                format!("{base}; border-color: rgba({rgb}, {border_opacity}); {shadow}")
                            }
                            on:mousedown=move |ev| {
                                ev.stop_propagation();
                                ev.prevent_default();
                                set_selected_zone_id.set(Some(zid_select.clone()));

                                let Some(container) = container_ref.get() else { return };
                                let rect = container.get_bounding_client_rect();
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
                                    set_dragging.set(Some(DragState {
                                        zone_id: zid_drag2.clone(),
                                        offset_x: mouse_norm_x - zx,
                                        offset_y: mouse_norm_y - zy,
                                    }));
                                }
                            }
                            on:click=move |ev| {
                                ev.stop_propagation();
                            }
                        >
                            // Zone label
                            <div class="absolute -top-5 left-0 text-[9px] font-mono text-fg whitespace-nowrap
                                        opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none
                                        bg-black/70 px-1.5 py-0.5 rounded">
                                {move || zone_style.get().map(|(_, _, name, count)| format!("{name} · {count} LEDs")).unwrap_or_default()}
                            </div>

                            // Resize handles (selected only)
                            {move || is_selected.get().then(|| view! {
                                <div class="absolute -top-1 -left-1 w-2.5 h-2.5 bg-white/80 rounded-sm border border-white/40 cursor-nw-resize" />
                                <div class="absolute -top-1 -right-1 w-2.5 h-2.5 bg-white/80 rounded-sm border border-white/40 cursor-ne-resize" />
                                <div class="absolute -bottom-1 -left-1 w-2.5 h-2.5 bg-white/80 rounded-sm border border-white/40 cursor-sw-resize" />
                                <div class="absolute -bottom-1 -right-1 w-2.5 h-2.5 bg-white/80 rounded-sm border border-white/40 cursor-se-resize" />
                            })}

                            // LED count indicator
                            <div class="absolute inset-0 flex items-center justify-center pointer-events-none">
                                <div class="text-[8px] font-mono text-white/30 select-none">
                                    {move || zone_style.get().map(|(_, _, _, count)| count).unwrap_or(0)}
                                </div>
                            </div>
                        </div>
                    }
                }).collect::<Vec<_>>()
            }}
        </div>
    }
}

#[derive(Clone, Debug)]
struct DragState {
    zone_id: String,
    offset_x: f32,
    offset_y: f32,
}
