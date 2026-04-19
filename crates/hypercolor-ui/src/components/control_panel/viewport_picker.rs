//! Shared viewport picker for rect-based effect controls.

use leptos::ev;
use leptos::prelude::*;
use serde_json::json;

use hypercolor_types::effect::ControlValue;
use hypercolor_types::viewport::{FitMode, MIN_VIEWPORT_EDGE, ViewportRect};

use crate::api::effects::fetch_active_effect;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::viewport_designer::{
    ModeDraft, ViewportDesignerContext, ViewportDesignerModal, ViewportDesignerMode,
    ViewportDesignerResult, ViewportDraft, ViewportDraftCommon,
};
use crate::control_geometry::{
    FrameHandle, FrameRect, clamp_frame_rect, drag_frame_rect, resize_frame_rect,
};
use crate::toasts::toast_error;
use crate::ws::CanvasFrame;

#[derive(Clone)]
pub(super) struct UrlInputBinding {
    pub label: String,
    pub value: Signal<String>,
    pub on_commit: Callback<String>,
    pub placeholder: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportInteractionState {
    handle: FrameHandle,
    start_rect: FrameRect,
    start_client_x: f64,
    start_client_y: f64,
}

#[component]
pub(super) fn ViewportPicker(
    control_id: String,
    label: String,
    #[prop(into)] value: Signal<ViewportRect>,
    on_change: Callback<(String, serde_json::Value)>,
    #[prop(into)] preview_source: Signal<Option<CanvasFrame>>,
    #[prop(optional_no_strip)] preview_consumer_count: Option<WriteSignal<u32>>,
    accent_rgb: String,
    #[prop(optional_no_strip)] aspect_lock: Option<f32>,
    #[prop(optional_no_strip)] url_input: Option<UrlInputBinding>,
    #[prop(optional_no_strip)] aspect_ratio: Option<String>,
) -> impl IntoView {
    let viewport_ref = NodeRef::<leptos::html::Div>::new();
    let (interaction, set_interaction) = signal(None::<ViewportInteractionState>);
    let preview_rect = Signal::derive(move || frame_rect_from_viewport(value.get().clamp()));
    let readout_rect = Signal::derive(move || preview_rect.get());
    let frame_style = Signal::derive({
        let accent_rgb = accent_rgb.clone();
        move || {
            let frame = preview_rect.get();
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
    let description = if url_input.is_some() {
        "Drag the frame over the live page render to choose what the effect samples."
    } else {
        "Drag the crop box or pull its corners to aim the viewport."
    };
    let preview_image_rendering = if url_input.is_some() {
        "auto".to_string()
    } else {
        "pixelated".to_string()
    };
    let preview_shell_class = if url_input.is_some() {
        "mx-auto w-full max-w-[420px]"
    } else {
        "mx-auto w-full max-w-[280px]"
    };

    let initial_url = url_input
        .as_ref()
        .map(|binding| binding.value.get_untracked())
        .unwrap_or_default();
    let (url_text, set_url_text) = signal(initial_url);

    // Viewport Designer modal state. We populate it on demand via one
    // GET /api/v1/effects/active so the inline picker doesn't need
    // extra props — the Edit button is self-contained.
    let designer_context = RwSignal::<Option<ViewportDesignerContext>>::new(None);
    let designer_opening = RwSignal::new(false);
    if let Some(binding) = url_input.clone() {
        Effect::new(move |_| {
            let next = binding.value.get();
            if url_text.get_untracked() != next {
                set_url_text.set(next);
            }
        });
    }

    let _drag_move = window_event_listener(ev::mousemove, {
        let control_id = control_id.clone();
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
                    MIN_VIEWPORT_EDGE,
                    MIN_VIEWPORT_EDGE,
                ),
                handle => resize_viewport_rect(
                    state.start_rect,
                    handle,
                    delta_x as f32,
                    delta_y as f32,
                    aspect_lock,
                ),
            };

            emit_viewport_update(&on_change, &control_id, next_rect);
        }
    });

    let _drag_end = window_event_listener(ev::mouseup, move |_| {
        if interaction.get_untracked().is_some() {
            set_interaction.set(None);
        }
    });

    let start_interaction =
        Callback::new(move |(handle, ev): (FrameHandle, web_sys::MouseEvent)| {
            let start_rect = preview_rect.get_untracked();
            ev.prevent_default();
            ev.stop_propagation();
            set_interaction.set(Some(ViewportInteractionState {
                handle,
                start_rect,
                start_client_x: f64::from(ev.client_x()),
                start_client_y: f64::from(ev.client_y()),
            }));
        });

    let reset_viewport = {
        let control_id = control_id.clone();
        move |_| emit_viewport_update(&on_change, &control_id, FrameRect::new(0.0, 0.0, 1.0, 1.0))
    };

    // Fetch active-effect metadata + seed a draft when the user clicks
    // Edit. The GET happens on click (not on picker mount) so the
    // extra round trip only fires when the user actually wants the
    // modal. A failure surfaces as a toast and re-arms the button.
    let open_designer = move |_ev: ev::MouseEvent| {
        if designer_opening.get_untracked() || designer_context.get_untracked().is_some() {
            return;
        }
        designer_opening.set(true);
        let seed_viewport = value.get_untracked().clamp();
        leptos::task::spawn_local(async move {
            match fetch_active_effect().await {
                Ok(Some(effect)) => {
                    let version = effect.controls_version.unwrap_or(0);
                    let mode = detect_designer_mode(&effect);
                    let fit = parse_fit_from_values(&effect.control_values);
                    let brightness = effect
                        .control_values
                        .get("brightness")
                        .and_then(ControlValue::as_f32)
                        .unwrap_or(1.0);
                    let mode_draft = match mode {
                        ViewportDesignerMode::WebViewport => ModeDraft::WebViewport {
                            url: effect
                                .control_values
                                .get("url")
                                .and_then(|v| match v {
                                    ControlValue::Text(t) => Some(t.clone()),
                                    ControlValue::Enum(t) => Some(t.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default(),
                            scroll_x: effect
                                .control_values
                                .get("scroll_x")
                                .and_then(ControlValue::as_f32)
                                .map(|v| v.round() as i32)
                                .unwrap_or(0),
                            scroll_y: effect
                                .control_values
                                .get("scroll_y")
                                .and_then(ControlValue::as_f32)
                                .map(|v| v.round() as i32)
                                .unwrap_or(0),
                            render_width: effect
                                .control_values
                                .get("render_width")
                                .and_then(ControlValue::as_f32)
                                .map(|v| v.round() as u32)
                                .unwrap_or(1280),
                            render_height: effect
                                .control_values
                                .get("render_height")
                                .and_then(ControlValue::as_f32)
                                .map(|v| v.round() as u32)
                                .unwrap_or(720),
                        },
                        ViewportDesignerMode::ScreenCast => ModeDraft::ScreenCast,
                    };
                    let context = ViewportDesignerContext {
                        effect_id: effect.id.clone(),
                        effect_name: effect.name.clone(),
                        canvas_aspect: 16.0 / 9.0,
                        initial_draft: ViewportDraft {
                            common: ViewportDraftCommon {
                                viewport: seed_viewport,
                                fit_mode: fit,
                                brightness,
                                controls_version: version,
                            },
                            mode: mode_draft,
                        },
                    };
                    designer_context.set(Some(context));
                }
                Ok(None) => {
                    toast_error("No active effect to edit.");
                }
                Err(error) => {
                    toast_error(&format!("Couldn't load effect metadata: {error}"));
                }
            }
            designer_opening.set(false);
        });
    };

    let designer_closed = Callback::new(move |_result: ViewportDesignerResult| {
        designer_context.set(None);
    });

    let url_section = url_input.clone().map(|binding| {
        let commit = binding.on_commit;
        let placeholder = binding.placeholder;
        let label = binding.label;
        view! {
            <div class="space-y-1.5">
                <div class="text-[9px] font-mono uppercase" style=label_style.clone()>{label}</div>
                <input
                    type="text"
                    class="w-full bg-surface-sunken border border-edge-subtle rounded-xl px-3 py-2 text-xs text-fg-primary
                           focus:outline-none focus:border-accent-muted glow-ring placeholder-fg-tertiary/40 transition-all duration-150"
                    placeholder=placeholder
                    prop:value=move || url_text.get()
                    on:change=move |ev| {
                        use wasm_bindgen::JsCast;
                        let target = ev
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(input) = target {
                            let next = input.value();
                            set_url_text.set(next.clone());
                            commit.run(next);
                        }
                    }
                />
            </div>
        }
    });
    let viewport_style = aspect_ratio
        .as_ref()
        .map(|ratio| format!("aspect-ratio: {ratio};"))
        .unwrap_or_default();
    let preview_canvas = match (aspect_ratio.clone(), preview_consumer_count) {
        (Some(aspect_ratio), Some(consumer_count)) => view! {
            <CanvasPreview
                frame=preview_source
                fps=Signal::derive(|| 0.0_f32)
                fps_target=Signal::derive(|| 0_u32)
                max_width="100%".to_string()
                image_rendering=preview_image_rendering.clone()
                aspect_ratio=aspect_ratio
                consumer_count=consumer_count
            />
        }
        .into_any(),
        (Some(aspect_ratio), None) => view! {
            <CanvasPreview
                frame=preview_source
                fps=Signal::derive(|| 0.0_f32)
                fps_target=Signal::derive(|| 0_u32)
                max_width="100%".to_string()
                image_rendering=preview_image_rendering.clone()
                aspect_ratio=aspect_ratio
            />
        }
        .into_any(),
        (None, Some(consumer_count)) => view! {
            <CanvasPreview
                frame=preview_source
                fps=Signal::derive(|| 0.0_f32)
                fps_target=Signal::derive(|| 0_u32)
                max_width="100%".to_string()
                image_rendering=preview_image_rendering.clone()
                consumer_count=consumer_count
            />
        }
        .into_any(),
        (None, None) => view! {
            <CanvasPreview
                frame=preview_source
                fps=Signal::derive(|| 0.0_f32)
                fps_target=Signal::derive(|| 0_u32)
                max_width="100%".to_string()
                image_rendering=preview_image_rendering
            />
        }
        .into_any(),
    };

    view! {
        <div class="mb-2 rounded-2xl border p-3 space-y-3" style=card_style>
            <div class="flex items-start gap-2">
                <div>
                    <div class="text-[9px] font-mono uppercase" style=label_style.clone()>{label.clone()}</div>
                    <div class="text-[11px] text-fg-tertiary/70 mt-1">{description}</div>
                </div>
                <div class="flex-1" />
                <button
                    type="button"
                    class="rounded-lg border px-2.5 py-1 text-[10px] font-medium uppercase tracking-[0.12em]
                           transition-all duration-150 hover:scale-[1.02]
                           disabled:opacity-60 disabled:cursor-wait disabled:hover:scale-100"
                    style=reset_button_style.clone()
                    disabled=move || designer_opening.get()
                    title="Open pixel-accurate editor"
                    on:click=open_designer
                >
                    {move || if designer_opening.get() { "Loading…" } else { "Edit…" }}
                </button>
                <button
                    type="button"
                    class="rounded-lg border px-2.5 py-1 text-[10px] font-medium uppercase tracking-[0.12em] transition-all duration-150 hover:scale-[1.02]"
                    style=reset_button_style
                    on:click=reset_viewport
                >
                    "Reset"
                </button>
            </div>

            {url_section}

            <div class="rounded-[1.5rem] border border-white/[0.06] bg-[#09070f]/90 p-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]">
                <div class="flex items-center gap-2 text-[10px] text-fg-tertiary/55 font-mono uppercase tracking-[0.12em] mb-3">
                    <span class="inline-block h-1.5 w-1.5 rounded-full" style=move || format!("background: rgba({}, 0.85)", preview_dot_rgb) />
                    <span>
                        {if url_input.is_some() {
                            "Live page preview"
                        } else {
                            "Live viewport preview"
                        }}
                    </span>
                </div>
                <div class=preview_shell_class>
                    <div
                        node_ref=viewport_ref
                        class="relative overflow-hidden rounded-[1.25rem] border border-white/[0.08] shadow-[inset_0_1px_0_rgba(255,255,255,0.05)] select-none"
                        style=viewport_style
                    >
                        <div class="absolute inset-0" style=preview_style.clone() />
                        <div class="absolute inset-0">
                            {preview_canvas}
                        </div>
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
                                {label.clone()}
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

        // Viewport Designer modal — sibling of the card so its
        // `fixed inset-0 z-50` overlay covers the whole viewport, not
        // just this card's flex column. Only rendered once the open
        // click has finished fetching effect context.
        {move || designer_context.get().map(|ctx| view! {
            <ViewportDesignerModal context=ctx on_close=designer_closed />
        })}
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

fn frame_rect_from_viewport(viewport: ViewportRect) -> FrameRect {
    FrameRect::new(viewport.x, viewport.y, viewport.width, viewport.height)
}

fn viewport_from_frame_rect(rect: FrameRect) -> ViewportRect {
    ViewportRect::new(rect.x, rect.y, rect.width, rect.height).clamp()
}

fn emit_viewport_update(
    on_change: &Callback<(String, serde_json::Value)>,
    control_id: &str,
    rect: FrameRect,
) {
    let viewport = viewport_from_frame_rect(rect);
    on_change.run((
        control_id.to_owned(),
        json!({
            "x": viewport.x,
            "y": viewport.y,
            "width": viewport.width,
            "height": viewport.height,
        }),
    ));
}

fn resize_viewport_rect(
    start: FrameRect,
    handle: FrameHandle,
    delta_x: f32,
    delta_y: f32,
    aspect_lock: Option<f32>,
) -> FrameRect {
    let Some(aspect_lock) = aspect_lock.filter(|aspect| aspect.is_finite() && *aspect > 0.0) else {
        return resize_frame_rect(
            start,
            handle,
            delta_x,
            delta_y,
            MIN_VIEWPORT_EDGE,
            MIN_VIEWPORT_EDGE,
        );
    };

    if matches!(handle, FrameHandle::Move) {
        return drag_frame_rect(
            start,
            delta_x,
            delta_y,
            MIN_VIEWPORT_EDGE,
            MIN_VIEWPORT_EDGE,
        );
    }

    let width_delta = match handle {
        FrameHandle::NorthWest | FrameHandle::SouthWest => -delta_x,
        FrameHandle::NorthEast | FrameHandle::SouthEast => delta_x,
        FrameHandle::Move => 0.0,
    };
    let height_delta = match handle {
        FrameHandle::NorthWest | FrameHandle::NorthEast => -delta_y,
        FrameHandle::SouthWest | FrameHandle::SouthEast => delta_y,
        FrameHandle::Move => 0.0,
    };

    let min_width = MIN_VIEWPORT_EDGE
        .max(MIN_VIEWPORT_EDGE * aspect_lock)
        .min(1.0);
    let min_height = (min_width / aspect_lock).max(MIN_VIEWPORT_EDGE).min(1.0);
    let use_width = width_delta.abs() >= height_delta.abs();
    let mut width = (start.width + width_delta).max(min_width);
    let mut height = (start.height + height_delta).max(min_height);

    if use_width {
        height = width / aspect_lock;
    } else {
        width = height * aspect_lock;
    }

    let (max_width, max_height) = match handle {
        FrameHandle::NorthWest => (start.right(), start.bottom()),
        FrameHandle::NorthEast => (1.0 - start.x, start.bottom()),
        FrameHandle::SouthWest => (start.right(), 1.0 - start.y),
        FrameHandle::SouthEast => (1.0 - start.x, 1.0 - start.y),
        FrameHandle::Move => (1.0, 1.0),
    };
    let scale = (max_width / width).min(max_height / height).min(1.0);
    width *= scale;
    height *= scale;

    let rect = match handle {
        FrameHandle::NorthWest => FrameRect::new(
            start.right() - width,
            start.bottom() - height,
            width,
            height,
        ),
        FrameHandle::NorthEast => FrameRect::new(start.x, start.bottom() - height, width, height),
        FrameHandle::SouthWest => FrameRect::new(start.right() - width, start.y, width, height),
        FrameHandle::SouthEast => FrameRect::new(start.x, start.y, width, height),
        FrameHandle::Move => start,
    };

    clamp_frame_rect(rect, min_width.min(max_width), min_height.min(max_height))
}

/// Best-effort discriminator between Web Viewport and Screen Cast
/// modes. The active-effect response doesn't carry a structured
/// discriminator, so we sniff control ids — Web Viewport is the only
/// built-in that exposes `url` / `scroll_y`. A future spec can add a
/// proper marker to the response; today this matches the built-in
/// shapes.
fn detect_designer_mode(
    effect: &crate::api::effects::ActiveEffectResponse,
) -> ViewportDesignerMode {
    if effect.control_values.contains_key("url") || effect.control_values.contains_key("scroll_y") {
        ViewportDesignerMode::WebViewport
    } else {
        ViewportDesignerMode::ScreenCast
    }
}

fn parse_fit_from_values(values: &std::collections::HashMap<String, ControlValue>) -> FitMode {
    match values.get("fit_mode") {
        Some(ControlValue::Enum(s) | ControlValue::Text(s)) => match s.as_str() {
            "Contain" => FitMode::Contain,
            "Stretch" => FitMode::Stretch,
            _ => FitMode::Cover,
        },
        _ => FitMode::Cover,
    }
}
