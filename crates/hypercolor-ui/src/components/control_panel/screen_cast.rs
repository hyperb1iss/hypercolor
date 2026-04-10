//! Screen cast frame widget — crop-box picker for the screen source effect.

use std::collections::HashMap;

use leptos::ev;
use leptos::prelude::*;
use serde_json::json;

use hypercolor_types::effect::{ControlDefinition, ControlValue};

use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::control_geometry::{
    FrameHandle, FrameRect, clamp_frame_rect, drag_frame_rect, resize_frame_rect,
};

use super::SCREEN_CAST_FRAME_CONTROL_IDS;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScreenCastFrameConfig {
    min_width: f32,
    min_height: f32,
    x_step: f32,
    y_step: f32,
    width_step: f32,
    height_step: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenCastInteractionState {
    handle: FrameHandle,
    start_rect: FrameRect,
    start_client_x: f64,
    start_client_y: f64,
}

#[component]
pub(super) fn ScreenCastFrameWidget(
    #[prop(into)] control_values: Signal<HashMap<String, ControlValue>>,
    accent_rgb: String,
    on_change: Callback<(String, serde_json::Value)>,
    frame_config: ScreenCastFrameConfig,
) -> impl IntoView {
    let viewport_ref = NodeRef::<leptos::html::Div>::new();
    let (interaction, set_interaction) = signal(None::<ScreenCastInteractionState>);
    let ws = use_context::<WsContext>();

    let frame_rect = Signal::derive(move || {
        control_values.with(|values| screen_cast_frame_rect(values, frame_config))
    });
    let readout_rect = Signal::derive(move || frame_rect.get());
    let frame_style = Signal::derive({
        let accent_rgb = accent_rgb.clone();
        move || {
            let frame = frame_rect.get();
            format!(
                "left: {:.2}%; top: {:.2}%; width: {:.2}%; height: {:.2}%; \
                 border-color: rgba({}, 0.86); \
                 box-shadow: 0 0 0 999px rgba(5, 6, 12, 0.52), inset 0 0 0 1px rgba(255,255,255,0.06), 0 0 24px rgba({}, 0.18);",
                frame.x * 100.0,
                frame.y * 100.0,
                frame.width * 100.0,
                frame.height * 100.0,
                accent_rgb,
                accent_rgb,
            )
        }
    });
    let preview_style = format!(
        "background:
            radial-gradient(circle at 18% 20%, rgba({0}, 0.22), transparent 30%),
            radial-gradient(circle at 76% 72%, rgba(255, 106, 193, 0.18), transparent 34%),
            linear-gradient(145deg, rgba({0}, 0.12), rgba(10, 9, 16, 0.94) 60%),
            linear-gradient(rgba(255,255,255,0.035) 1px, transparent 1px),
            linear-gradient(90deg, rgba(255,255,255,0.035) 1px, transparent 1px);
         background-size: auto, auto, auto, 12% 12%, 12% 12%;",
        accent_rgb
    );
    let card_style = format!(
        "border-color: rgba({}, 0.12); background: linear-gradient(180deg, rgba({}, 0.08), rgba(10, 9, 16, 0.7));",
        accent_rgb, accent_rgb
    );
    let reset_button_style = format!(
        "border-color: rgba({0}, 0.16); color: rgba({0}, 0.88); background: rgba({0}, 0.08);",
        accent_rgb
    );
    let preview_dot_rgb = accent_rgb.clone();
    let label_style = format!("color: rgba({}, 0.56); letter-spacing: 0.14em;", accent_rgb);
    let pill_style = format!(
        "border-color: rgba({0}, 0.12); background: rgba({0}, 0.08); box-shadow: inset 0 1px 0 rgba(255,255,255,0.03);",
        accent_rgb
    );

    let _drag_move = window_event_listener(ev::mousemove, {
        move |ev| {
            let Some(state) = interaction.get_untracked() else {
                return;
            };
            let Some(viewport) = viewport_ref.get_untracked() else {
                return;
            };
            let rect = viewport.get_bounding_client_rect();
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return;
            }

            ev.prevent_default();
            let delta_x = (f64::from(ev.client_x()) - state.start_client_x) / rect.width();
            let delta_y = (f64::from(ev.client_y()) - state.start_client_y) / rect.height();
            let next_rect = match state.handle {
                FrameHandle::Move => drag_frame_rect(
                    state.start_rect,
                    delta_x as f32,
                    delta_y as f32,
                    frame_config.min_width,
                    frame_config.min_height,
                ),
                handle => resize_frame_rect(
                    state.start_rect,
                    handle,
                    delta_x as f32,
                    delta_y as f32,
                    frame_config.min_width,
                    frame_config.min_height,
                ),
            };

            emit_screen_cast_frame_update(
                &on_change,
                snap_screen_cast_frame_rect(next_rect, frame_config),
            );
        }
    });

    let _drag_end = window_event_listener(ev::mouseup, move |_| {
        if interaction.get_untracked().is_some() {
            set_interaction.set(None);
        }
    });

    let start_interaction =
        Callback::new(move |(handle, ev): (FrameHandle, web_sys::MouseEvent)| {
            let start_rect = frame_rect.get_untracked();
            ev.prevent_default();
            ev.stop_propagation();
            set_interaction.set(Some(ScreenCastInteractionState {
                handle,
                start_rect,
                start_client_x: f64::from(ev.client_x()),
                start_client_y: f64::from(ev.client_y()),
            }));
        });

    let reset_frame = {
        move |_| {
            emit_screen_cast_frame_update(&on_change, FrameRect::new(0.0, 0.0, 1.0, 1.0));
        }
    };

    view! {
        <div class="mb-2 rounded-2xl border p-3 space-y-3" style=card_style>
            <div class="flex items-center gap-2">
                <div>
                    <div class="text-[9px] font-mono uppercase" style=label_style>"Screen Frame"</div>
                    <div class="text-[11px] text-fg-tertiary/70 mt-1">
                        "Drag the crop box or pull its corners to aim the cast."
                    </div>
                </div>
                <div class="flex-1" />
                <button
                    type="button"
                    class="rounded-lg border px-2.5 py-1 text-[10px] font-medium uppercase tracking-[0.12em] transition-all duration-150 hover:scale-[1.02]"
                    style=reset_button_style
                    on:click=reset_frame
                >
                    "Reset"
                </button>
            </div>

            <div class="rounded-[1.5rem] border border-white/[0.06] bg-[#09070f]/90 p-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]">
                <div class="flex items-center gap-2 text-[10px] text-fg-tertiary/55 font-mono uppercase tracking-[0.12em] mb-3">
                    <span class="inline-block h-1.5 w-1.5 rounded-full" style=move || format!("background: rgba({}, 0.85)", preview_dot_rgb) />
                    <span>"Live crop preview"</span>
                </div>
                <div class="mx-auto w-full max-w-[280px]">
                    <div
                        node_ref=viewport_ref
                        class="relative overflow-hidden rounded-[1.25rem] border border-white/[0.08] shadow-[inset_0_1px_0_rgba(255,255,255,0.05)] select-none"
                        style="aspect-ratio: 16 / 9;"
                    >
                        <div class="absolute inset-0" style=preview_style />
                        {ws.map(|ws| {
                            view! {
                                <div class="absolute inset-0">
                                    <CanvasPreview
                                        frame=Signal::derive(move || ws.screen_canvas_frame.get())
                                        fps=Signal::derive(|| 0.0_f32)
                                        fps_target=Signal::derive(|| 0_u32)
                                        max_width="100%".to_string()
                                        aspect_ratio="16 / 9".to_string()
                                        consumer_count=ws.set_screen_preview_consumers
                                    />
                                </div>
                            }
                        })}
                        <div class="absolute inset-[7%] rounded-[1rem] border border-white/[0.05]" />
                        <div class="absolute left-1/2 top-0 bottom-0 w-px bg-white/[0.05] -translate-x-1/2" />
                        <div class="absolute top-1/2 left-0 right-0 h-px bg-white/[0.05] -translate-y-1/2" />

                        <div
                            class="absolute rounded-[1rem] border cursor-grab"
                            class:cursor-grabbing=move || interaction.get().is_some()
                            style=move || frame_style.get()
                            on:mousedown=move |ev| start_interaction.run((FrameHandle::Move, ev))
                        >
                            <div class="absolute inset-0 rounded-[0.95rem] bg-white/[0.035] backdrop-blur-[1px]" />
                            <div class="absolute left-2 top-2 rounded-full bg-black/45 px-2 py-1 text-[9px] font-semibold uppercase tracking-[0.14em] text-white/80">
                                "Screen Cast"
                            </div>

                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="left-0 top-0 -translate-x-1/2 -translate-y-1/2 cursor-nwse-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::NorthWest, ev)))
                            />
                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="right-0 top-0 translate-x-1/2 -translate-y-1/2 cursor-nesw-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::NorthEast, ev)))
                            />
                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="left-0 bottom-0 -translate-x-1/2 translate-y-1/2 cursor-nesw-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::SouthWest, ev)))
                            />
                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="right-0 bottom-0 translate-x-1/2 translate-y-1/2 cursor-nwse-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::SouthEast, ev)))
                            />
                        </div>
                    </div>
                </div>
            </div>

            <div class="grid grid-cols-4 gap-2">
                <FrameReadout label="X" value=Signal::derive(move || readout_rect.get().x) pill_style=pill_style.clone() />
                <FrameReadout label="Y" value=Signal::derive(move || readout_rect.get().y) pill_style=pill_style.clone() />
                <FrameReadout label="W" value=Signal::derive(move || readout_rect.get().width) pill_style=pill_style.clone() />
                <FrameReadout label="H" value=Signal::derive(move || readout_rect.get().height) pill_style=pill_style />
            </div>
        </div>
    }
}

#[component]
fn FrameHandleGrip(
    accent_rgb: String,
    class: &'static str,
    on_mousedown: Callback<web_sys::MouseEvent>,
) -> impl IntoView {
    let grip_style = format!(
        "background: rgba({0}, 0.92); box-shadow: 0 0 0 2px rgba(9, 7, 15, 0.88), 0 0 16px rgba({0}, 0.3);",
        accent_rgb
    );

    view! {
        <button
            type="button"
            class=format!("absolute h-3.5 w-3.5 rounded-full border border-white/20 transition-transform duration-150 hover:scale-110 {class}")
            style=grip_style
            on:mousedown=move |ev| on_mousedown.run(ev)
        />
    }
}

#[component]
fn FrameReadout(
    label: &'static str,
    #[prop(into)] value: Signal<f32>,
    pill_style: String,
) -> impl IntoView {
    view! {
        <div class="rounded-xl border px-2.5 py-2" style=pill_style>
            <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/55">{label}</div>
            <div class="mt-1 text-xs font-semibold tabular-nums text-fg-secondary">
                {move || format!("{:.2}", value.get())}
            </div>
        </div>
    }
}

pub(super) fn is_screen_cast_frame_control(control_id: &str) -> bool {
    SCREEN_CAST_FRAME_CONTROL_IDS.contains(&control_id)
}

pub(super) fn screen_cast_frame_config(
    items: &[(ControlDefinition, String)],
) -> Option<ScreenCastFrameConfig> {
    let find = |control_id: &str| {
        items
            .iter()
            .find_map(|(def, _)| (def.control_id() == control_id).then_some(def))
    };

    let frame_x = find("frame_x")?;
    let frame_y = find("frame_y")?;
    let frame_width = find("frame_width")?;
    let frame_height = find("frame_height")?;

    Some(ScreenCastFrameConfig {
        min_width: frame_width.min.unwrap_or(0.05),
        min_height: frame_height.min.unwrap_or(0.05),
        x_step: frame_x.step.unwrap_or(0.01),
        y_step: frame_y.step.unwrap_or(0.01),
        width_step: frame_width.step.unwrap_or(0.01),
        height_step: frame_height.step.unwrap_or(0.01),
    })
}

fn screen_cast_frame_rect(
    values: &HashMap<String, ControlValue>,
    frame_config: ScreenCastFrameConfig,
) -> FrameRect {
    let x = values
        .get("frame_x")
        .and_then(ControlValue::as_f32)
        .unwrap_or(0.0);
    let y = values
        .get("frame_y")
        .and_then(ControlValue::as_f32)
        .unwrap_or(0.0);
    let width = values
        .get("frame_width")
        .and_then(ControlValue::as_f32)
        .unwrap_or(1.0);
    let height = values
        .get("frame_height")
        .and_then(ControlValue::as_f32)
        .unwrap_or(1.0);

    clamp_frame_rect(
        FrameRect::new(x, y, width, height),
        frame_config.min_width,
        frame_config.min_height,
    )
}

fn emit_screen_cast_frame_update(
    on_change: &Callback<(String, serde_json::Value)>,
    rect: FrameRect,
) {
    on_change.run(("frame_x".to_string(), json!(rect.x)));
    on_change.run(("frame_y".to_string(), json!(rect.y)));
    on_change.run(("frame_width".to_string(), json!(rect.width)));
    on_change.run(("frame_height".to_string(), json!(rect.height)));
}

fn snap_screen_cast_frame_rect(rect: FrameRect, frame_config: ScreenCastFrameConfig) -> FrameRect {
    clamp_frame_rect(
        FrameRect::new(
            snap_to_step(rect.x, frame_config.x_step),
            snap_to_step(rect.y, frame_config.y_step),
            snap_to_step(rect.width, frame_config.width_step),
            snap_to_step(rect.height, frame_config.height_step),
        ),
        frame_config.min_width,
        frame_config.min_height,
    )
}

fn snap_to_step(value: f32, step: f32) -> f32 {
    if step <= f32::EPSILON {
        value
    } else {
        (value / step).round() * step
    }
}
