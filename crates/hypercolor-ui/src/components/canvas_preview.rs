//! Canvas preview — presents authoritative daemon frames in the browser via WebGL.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hypercolor_leptos_ext::events::document as browser_document;
use hypercolor_leptos_ext::prelude::now_ms;
use hypercolor_leptos_ext::raf::Scheduler;
use leptos::ev::Custom;
use leptos::html::Canvas;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos_icons::Icon;
use leptos_use::{
    UseEventListenerOptions, use_document, use_event_listener, use_event_listener_with_options,
};

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::icons::LuMousePointerClick;
use crate::preview_telemetry::{PreviewPresenterTelemetry, PreviewTelemetryContext};
use crate::ws::CanvasFrame;
use crate::ws::input::{InputEdgeButton, InputEdgeState, InputInjectEdge};

use super::preview_runtime::{PreviewRenderOutcome, PreviewRuntime, PreviewRuntimeInitError};

type PresentCallback = Rc<dyn Fn()>;
type PresentScheduler = Rc<RefCell<Option<PresentCallback>>>;
const PREVIEW_RUNTIME_RETRY_DELAY_FRAMES: u32 = 30;
const CANVAS2D_FALLBACK_THRESHOLD: u8 = 3;
const PREVIEW_TELEMETRY_INTERVAL_MS: f64 = 250.0;

fn quantize_present_fps(value: f32) -> f32 {
    (value * 10.0).round() / 10.0
}

fn quantize_arrival_to_present_ms(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn should_publish_preview_telemetry(
    previous: &PreviewPresenterTelemetry,
    next: &PreviewPresenterTelemetry,
    last_published_at_ms: Option<f64>,
    now_ms: f64,
) -> bool {
    if previous.runtime_mode != next.runtime_mode || previous.skipped_frames != next.skipped_frames
    {
        return true;
    }

    if last_published_at_ms.is_none() {
        return true;
    }

    previous != next
        && now_ms - last_published_at_ms.unwrap_or_default() >= PREVIEW_TELEMETRY_INTERVAL_MS
}

fn has_preview_extent(frame: &CanvasFrame) -> bool {
    frame.width > 0 && frame.height > 0
}

/// Browser-side mirror of `EffectMetadata::requires_interaction` over the
/// REST `EffectSummary` shape. `EffectSummary` does not carry the explicit
/// `input_reactive` capability flag, so the category and legacy-tag legs
/// are the whole gate here.
pub fn effect_wants_interaction(effect: &api::EffectSummary) -> bool {
    effect.category.eq_ignore_ascii_case("interactive")
        || effect.tags.iter().any(|tag| {
            tag.eq_ignore_ascii_case("interactive")
                || tag.eq_ignore_ascii_case("input")
                || tag.eq_ignore_ascii_case("mouse")
                || tag.eq_ignore_ascii_case("keyboard")
        })
}

/// Map a `KeyboardEvent.code` to the daemon's canonical browser-style key
/// name (`canonical_evdev_key_name` in hypercolor-core): single lowercase
/// letters, bare digits, literal punctuation, and pass-through for the
/// named keys (`Space`, `ArrowLeft`, `ShiftLeft`, ...), which `event.code`
/// already spells identically.
pub fn canonical_injection_key(code: &str) -> Option<String> {
    if let Some(letter) = code.strip_prefix("Key")
        && letter.len() == 1
        && letter.bytes().all(|byte| byte.is_ascii_uppercase())
    {
        return Some(letter.to_ascii_lowercase());
    }
    if let Some(digit) = code.strip_prefix("Digit")
        && digit.len() == 1
        && digit.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Some(digit.to_owned());
    }
    let name = match code {
        "" => return None,
        "Minus" => "-",
        "Equal" => "=",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Backquote" => "`",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        other => other,
    };
    Some(name.to_owned())
}

/// Convert a `WheelEvent` delta into the daemon's hi-res wheel units
/// (120 per notch). Pixel deltas assume the common ~100px notch; line and
/// page modes scale through conventional pixel equivalents. Sign flips so
/// scrolling up (negative `deltaY`) is a positive notch, matching evdev's
/// `REL_WHEEL_HI_RES`.
pub fn wheel_delta_hi_res(delta_y: f64, delta_mode: u32) -> i32 {
    const LINE_HEIGHT_PX: f64 = 40.0;
    const PAGE_HEIGHT_PX: f64 = 400.0;
    const NOTCH_PX: f64 = 100.0;
    let pixels = match delta_mode {
        1 => delta_y * LINE_HEIGHT_PX,
        2 => delta_y * PAGE_HEIGHT_PX,
        _ => delta_y,
    };
    let hi_res = (-pixels * 120.0 / NOTCH_PX).round();
    #[allow(clippy::cast_possible_truncation)]
    {
        hi_res.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    }
}

/// Normalize a client-space pointer position against the canvas content
/// rect, clamped to `[0, 1]`. Returns `None` for a degenerate rect (not
/// yet laid out).
pub fn normalized_canvas_position(
    client_x: f64,
    client_y: f64,
    rect_left: f64,
    rect_top: f64,
    rect_width: f64,
    rect_height: f64,
) -> Option<(f32, f32)> {
    if rect_width <= 0.0 || rect_height <= 0.0 {
        return None;
    }
    let nx = ((client_x - rect_left) / rect_width).clamp(0.0, 1.0);
    let ny = ((client_y - rect_top) / rect_height).clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation)]
    {
        Some((nx as f32, ny as f32))
    }
}

enum PresenterState {
    Uninitialized {
        webgl_unavailable_streak: u8,
    },
    Ready {
        runtime: PreviewRuntime,
        webgl_unavailable_streak: u8,
    },
    CoolingDown {
        retry_at_frame: u32,
        webgl_unavailable_streak: u8,
    },
}

impl PresenterState {
    fn schedule_retry(&mut self, frame_number: u32, webgl_unavailable_streak: u8) {
        *self = Self::CoolingDown {
            retry_at_frame: frame_number.saturating_add(PREVIEW_RUNTIME_RETRY_DELAY_FRAMES),
            webgl_unavailable_streak,
        };
    }

    fn retry_state(&self, frame_number: u32) -> Option<u8> {
        match self {
            Self::Uninitialized {
                webgl_unavailable_streak,
            } => Some(*webgl_unavailable_streak),
            Self::CoolingDown {
                retry_at_frame,
                webgl_unavailable_streak,
            } if frame_number >= *retry_at_frame => Some(*webgl_unavailable_streak),
            Self::CoolingDown { .. } | Self::Ready { .. } => None,
        }
    }

    fn ensure_runtime(
        &mut self,
        canvas: &web_sys::HtmlCanvasElement,
        frame: &CanvasFrame,
        smooth_scaling: bool,
    ) -> bool {
        let Some(webgl_unavailable_streak) = self.retry_state(frame.frame_number) else {
            return matches!(self, Self::Ready { .. });
        };

        match PreviewRuntime::new(
            canvas,
            frame,
            webgl_unavailable_streak >= CANVAS2D_FALLBACK_THRESHOLD,
            smooth_scaling,
        ) {
            Ok(runtime) => {
                let ready_streak = if runtime.preserves_webgl_unavailable_streak() {
                    webgl_unavailable_streak
                } else {
                    0
                };
                *self = Self::Ready {
                    runtime,
                    webgl_unavailable_streak: ready_streak,
                };
                true
            }
            Err(PreviewRuntimeInitError::WebGlUnavailable) => {
                self.schedule_retry(
                    frame.frame_number,
                    webgl_unavailable_streak.saturating_add(1),
                );
                false
            }
            Err(PreviewRuntimeInitError::WebGlInitializationFailed) => {
                self.schedule_retry(frame.frame_number, 0);
                false
            }
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn mode_label(&self) -> Option<&'static str> {
        match self {
            Self::Ready { runtime, .. } => Some(runtime.mode_label()),
            Self::Uninitialized { .. } | Self::CoolingDown { .. } => None,
        }
    }
}

impl Default for PresenterState {
    fn default() -> Self {
        Self::Uninitialized {
            webgl_unavailable_streak: 0,
        }
    }
}

/// Live canvas preview that paints authoritative canvas pixels from WebSocket frames.
#[component]
pub fn CanvasPreview(
    #[prop(into)] frame: Signal<Option<CanvasFrame>>,
    #[prop(into)] fps: Signal<f32>,
    #[prop(default = false)] show_fps: bool,
    #[prop(default = "Preview".to_string())] fps_label: String,
    #[prop(into)] fps_target: Signal<u32>,
    #[prop(default = "100%".to_string())] max_width: String,
    #[prop(default = "pixelated".to_string())] image_rendering: String,
    #[prop(optional)] aspect_ratio: Option<String>,
    #[prop(default = "Live effect canvas preview".to_string())] aria_label: String,
    #[prop(default = true)] register_main_preview_consumer: bool,
    #[prop(default = false)] report_presenter_telemetry: bool,
    #[prop(optional)] consumer_count: Option<WriteSignal<u32>>,
    /// Opt-in host flag for the interactive-injection toggle (spec 71 W4).
    /// When `true` and the active effect wants interaction, the preview
    /// offers a toggle that forwards pointer/keyboard/wheel input to the
    /// daemon as `input_inject` messages. Off for passive surfaces
    /// (sidebar mini-preview, pickers, layout ghosts).
    #[prop(default = false)] allow_interactive: bool,
) -> impl IntoView {
    let canvas_ref = NodeRef::<Canvas>::new();
    let mounted_canvas = Rc::new(RefCell::new(None::<web_sys::HtmlCanvasElement>));
    let latest_frame = Rc::new(RefCell::new(None::<CanvasFrame>));
    let latest_frame_received_at = Rc::new(RefCell::new(None::<f64>));
    let presenter = Rc::new(RefCell::new(PresenterState::default()));
    let presenter_scheduler = Rc::new(RefCell::new(None::<Scheduler>));
    let last_presented_frame = Rc::new(RefCell::new(None::<u32>));
    let last_presented_at = Rc::new(RefCell::new(None::<f64>));
    let skipped_frames = Rc::new(RefCell::new(0_u32));
    let schedule_present: PresentScheduler = Rc::new(RefCell::new(None));
    let presented_fps = RwSignal::new(0.0_f32);
    let runtime_mode = RwSignal::new(None::<&'static str>);
    let last_published_telemetry = Rc::new(RefCell::new(PreviewPresenterTelemetry::default()));
    let last_telemetry_published_at = Rc::new(RefCell::new(None::<f64>));
    let ws = use_context::<WsContext>();
    let preview_telemetry = use_context::<PreviewTelemetryContext>()
        .filter(|_| report_presenter_telemetry)
        .map(|context| context.set_presenter);
    let smooth_scaling = image_rendering != "pixelated";
    let preview_registered = Arc::new(AtomicBool::new(false));
    let disposed = Arc::new(AtomicBool::new(false));
    let consumer_count = if register_main_preview_consumer {
        consumer_count.or_else(|| ws.map(|ws| ws.set_preview_consumers))
    } else {
        consumer_count
    };

    {
        let mounted_canvas = Rc::clone(&mounted_canvas);
        let latest_frame = Rc::clone(&latest_frame);
        let latest_frame_received_at = Rc::clone(&latest_frame_received_at);
        let presenter = Rc::clone(&presenter);
        let presenter_scheduler_handle = Rc::clone(&presenter_scheduler);
        let last_presented_frame = Rc::clone(&last_presented_frame);
        let last_presented_at = Rc::clone(&last_presented_at);
        let skipped_frames = Rc::clone(&skipped_frames);
        let last_published_telemetry = Rc::clone(&last_published_telemetry);
        let last_telemetry_published_at = Rc::clone(&last_telemetry_published_at);
        let scheduler_disposed = Arc::clone(&disposed);

        let scheduler = Scheduler::new(move |frame_info| {
            if scheduler_disposed.load(Ordering::Relaxed) {
                return;
            }

            let raf_time_ms = frame_info.time_ms;

            let Some(canvas_handle) = mounted_canvas.borrow().as_ref().cloned() else {
                return;
            };

            if !canvas_handle.is_connected() {
                presenter.borrow_mut().reset();
                last_presented_frame.borrow_mut().take();
                last_presented_at.borrow_mut().take();
                *skipped_frames.borrow_mut() = 0;
                if show_fps {
                    presented_fps.set(0.0);
                }
                if runtime_mode.get_untracked().is_some() {
                    runtime_mode.set(None);
                }
                *last_telemetry_published_at.borrow_mut() = None;
                *last_published_telemetry.borrow_mut() = PreviewPresenterTelemetry::default();
                if let Some(telemetry) = preview_telemetry {
                    telemetry.set(PreviewPresenterTelemetry::default());
                }
                return;
            }

            let latest_frame_ref = latest_frame.borrow();
            if let Some(frame) = latest_frame_ref.as_ref()
                && Some(frame.frame_number) != *last_presented_frame.borrow()
            {
                let mut presenter_state = presenter.borrow_mut();
                if presenter_state.ensure_runtime(&canvas_handle, frame, smooth_scaling) {
                    let mode = presenter_state.mode_label();
                    if runtime_mode.get_untracked() != mode {
                        runtime_mode.set(mode);
                    }
                    if let PresenterState::Ready {
                        runtime: presenter,
                        webgl_unavailable_streak,
                    } = &mut *presenter_state
                    {
                        match presenter.render(&canvas_handle, frame) {
                            PreviewRenderOutcome::Presented => {
                                let skipped =
                                    last_presented_frame.borrow().map_or(0, |previous_frame| {
                                        frame
                                            .frame_number
                                            .saturating_sub(previous_frame.saturating_add(1))
                                    });
                                if skipped > 0 {
                                    let mut skipped_total = skipped_frames.borrow_mut();
                                    *skipped_total = skipped_total.saturating_add(skipped);
                                }

                                let next_present_fps = if let Some(previous_presented_at) =
                                    last_presented_at.borrow_mut().replace(raf_time_ms)
                                {
                                    let elapsed_ms = (raf_time_ms - previous_presented_at).max(0.0);
                                    if elapsed_ms > 0.0 {
                                        let max_present_fps = {
                                            let target = fps_target.get_untracked();
                                            if target > 0 { f64::from(target) } else { 120.0 }
                                        };
                                        let instant_fps =
                                            (1000.0 / elapsed_ms).clamp(0.0, max_present_fps);
                                        let previous_fps = f64::from(presented_fps.get_untracked());
                                        let next_fps = if previous_fps <= 0.0 {
                                            instant_fps
                                        } else {
                                            previous_fps * 0.82 + instant_fps * 0.18
                                        }
                                        .clamp(0.0, max_present_fps);
                                        #[allow(clippy::cast_possible_truncation)]
                                        {
                                            next_fps as f32
                                        }
                                    } else {
                                        presented_fps.get_untracked()
                                    }
                                } else {
                                    0.0
                                };
                                if show_fps {
                                    presented_fps.set(next_present_fps);
                                }

                                if let Some(telemetry) = preview_telemetry {
                                    let arrival_to_present_ms = latest_frame_received_at
                                        .borrow()
                                        .map_or(0.0, |received_at_ms| {
                                            (raf_time_ms - received_at_ms).max(0.0)
                                        });
                                    let next_telemetry = PreviewPresenterTelemetry {
                                        runtime_mode: mode,
                                        present_fps: quantize_present_fps(next_present_fps),
                                        arrival_to_present_ms: quantize_arrival_to_present_ms(
                                            arrival_to_present_ms,
                                        ),
                                        skipped_frames: *skipped_frames.borrow(),
                                        last_frame_number: Some(frame.frame_number),
                                    };
                                    let previous_telemetry =
                                        last_published_telemetry.borrow().clone();
                                    let last_published_at_ms =
                                        *last_telemetry_published_at.borrow();
                                    if should_publish_preview_telemetry(
                                        &previous_telemetry,
                                        &next_telemetry,
                                        last_published_at_ms,
                                        raf_time_ms,
                                    ) {
                                        telemetry.set(next_telemetry.clone());
                                        *last_published_telemetry.borrow_mut() = next_telemetry;
                                        *last_telemetry_published_at.borrow_mut() =
                                            Some(raf_time_ms);
                                    }
                                }
                                *last_presented_frame.borrow_mut() = Some(frame.frame_number);
                            }
                            PreviewRenderOutcome::Reinitialize => {
                                let retry_streak = *webgl_unavailable_streak;
                                presenter_state.schedule_retry(frame.frame_number, retry_streak);
                                last_presented_frame.borrow_mut().take();
                            }
                        }
                    }
                }
            }
        });

        let schedule = Rc::new({
            let scheduler = scheduler.clone();
            let disposed = Arc::clone(&disposed);
            move || {
                if !disposed.load(Ordering::Relaxed) {
                    scheduler.schedule();
                }
            }
        });

        presenter_scheduler_handle.borrow_mut().replace(scheduler);
        *schedule_present.borrow_mut() = Some(schedule);
    }

    // Stash the newest frame immediately and queue a single browser-timed present.
    Effect::new({
        let latest_frame = Rc::clone(&latest_frame);
        let latest_frame_received_at = Rc::clone(&latest_frame_received_at);
        let schedule_present = Rc::clone(&schedule_present);
        move |_| {
            let next_frame = frame.get().filter(has_preview_extent);
            let has_next_frame = next_frame.is_some();
            let received_at_ms = has_next_frame.then(now_ms);
            *latest_frame.borrow_mut() = next_frame;
            *latest_frame_received_at.borrow_mut() = received_at_ms;

            if has_next_frame && let Some(schedule) = schedule_present.borrow().as_ref() {
                schedule();
            }
        }
    });

    // If the canvas mounts after frames have already started arriving, present immediately.
    Effect::new({
        let latest_frame = Rc::clone(&latest_frame);
        let schedule_present = Rc::clone(&schedule_present);
        let mounted_canvas = Rc::clone(&mounted_canvas);

        move |_| {
            let Some(canvas_handle) = canvas_ref.get() else {
                mounted_canvas.borrow_mut().take();
                return;
            };
            mounted_canvas.borrow_mut().replace(canvas_handle);

            if latest_frame.borrow().is_some()
                && let Some(schedule) = schedule_present.borrow().as_ref()
            {
                schedule();
            }
        }
    });

    Effect::new({
        let preview_registered = Arc::clone(&preview_registered);
        move |_| {
            if canvas_ref.get().is_some()
                && let Some(counter) = consumer_count
                && !preview_registered.load(Ordering::Relaxed)
            {
                counter.update(|count| *count = count.saturating_add(1));
                preview_registered.store(true, Ordering::Relaxed);
            }
        }
    });

    on_cleanup({
        let preview_registered = Arc::clone(&preview_registered);
        let disposed = Arc::clone(&disposed);
        move || {
            disposed.store(true, Ordering::Relaxed);
            if let Some(counter) = consumer_count
                && preview_registered.load(Ordering::Relaxed)
            {
                counter.update(|count| *count = count.saturating_sub(1));
            }
            if let Some(telemetry) = preview_telemetry {
                telemetry.set(PreviewPresenterTelemetry::default());
            }
        }
    });

    let _ = use_event_listener_with_options(
        canvas_ref,
        Custom::new("webglcontextlost"),
        {
            let presenter = Rc::clone(&presenter);
            let presenter_scheduler = Rc::clone(&presenter_scheduler);
            let last_presented_frame = Rc::clone(&last_presented_frame);
            let last_presented_at = Rc::clone(&last_presented_at);
            let skipped_frames = Rc::clone(&skipped_frames);
            let last_published_telemetry = Rc::clone(&last_published_telemetry);
            let last_telemetry_published_at = Rc::clone(&last_telemetry_published_at);

            move |event: web_sys::Event| {
                event.prevent_default();
                presenter.borrow_mut().reset();
                last_presented_frame.borrow_mut().take();
                last_presented_at.borrow_mut().take();
                *skipped_frames.borrow_mut() = 0;
                if show_fps {
                    presented_fps.set(0.0);
                }
                if runtime_mode.get_untracked().is_some() {
                    runtime_mode.set(None);
                }
                *last_telemetry_published_at.borrow_mut() = None;
                *last_published_telemetry.borrow_mut() = PreviewPresenterTelemetry::default();
                if let Some(telemetry) = preview_telemetry {
                    telemetry.set(PreviewPresenterTelemetry::default());
                }
                if let Some(scheduler) = presenter_scheduler.borrow().as_ref() {
                    scheduler.pause();
                }
            }
        },
        UseEventListenerOptions::default().passive(false),
    );

    let _ = use_event_listener_with_options(
        canvas_ref,
        Custom::new("webglcontextrestored"),
        {
            let presenter = Rc::clone(&presenter);
            let last_presented_frame = Rc::clone(&last_presented_frame);
            let last_presented_at = Rc::clone(&last_presented_at);
            let latest_frame = Rc::clone(&latest_frame);
            let schedule_present = Rc::clone(&schedule_present);
            let skipped_frames = Rc::clone(&skipped_frames);
            let last_published_telemetry = Rc::clone(&last_published_telemetry);
            let last_telemetry_published_at = Rc::clone(&last_telemetry_published_at);

            move |_: web_sys::Event| {
                presenter.borrow_mut().reset();
                last_presented_frame.borrow_mut().take();
                last_presented_at.borrow_mut().take();
                *skipped_frames.borrow_mut() = 0;
                if show_fps {
                    presented_fps.set(0.0);
                }
                if runtime_mode.get_untracked().is_some() {
                    runtime_mode.set(None);
                }
                *last_telemetry_published_at.borrow_mut() = None;
                *last_published_telemetry.borrow_mut() = PreviewPresenterTelemetry::default();
                if let Some(telemetry) = preview_telemetry {
                    telemetry.set(PreviewPresenterTelemetry::default());
                }

                if latest_frame.borrow().is_some()
                    && let Some(schedule) = schedule_present.borrow().as_ref()
                {
                    schedule();
                }
            }
        },
        UseEventListenerOptions::default(),
    );

    // ── Interactive input injection (spec 71 W4) ────────────────────────
    //
    // A control-authorized upstream `input_inject` message carries pointer
    // and keyboard edges; the daemon stamps a per-connection source id and
    // synthesizes releases on socket close. Local held state is released
    // explicitly on blur/visibility-loss/toggle-off/unmount because none
    // of those close the socket.
    let effects_ctx = use_context::<EffectsContext>();
    let send_input_inject = ws.map(|ws| ws.send_input_inject);
    let interactive_available = Memo::new(move |_| {
        allow_interactive
            && send_input_inject.is_some()
            && effects_ctx.is_some_and(|fx| {
                fx.active_effect_id.get().is_some_and(|id| {
                    fx.effects_index.with(|effects| {
                        effects
                            .iter()
                            .find(|entry| entry.effect.id == id)
                            .is_some_and(|entry| effect_wants_interaction(&entry.effect))
                    })
                })
            })
    });
    let interactive_on = RwSignal::new(false);
    let capture_focused = RwSignal::new(false);
    let interactive_active =
        Signal::derive(move || interactive_available.get() && interactive_on.get());

    let pressed_keys = Rc::new(RefCell::new(HashSet::<String>::new()));
    let pressed_buttons = Rc::new(RefCell::new(HashSet::<InputEdgeButton>::new()));
    let pending_edges = Rc::new(RefCell::new(Vec::<InputInjectEdge>::new()));
    let pending_move = Rc::new(RefCell::new(None::<(f32, f32)>));

    // Coalesce to animation-frame cadence: discrete edges queue in order,
    // the newest move rides along at the end of each flush.
    let inject_scheduler = {
        let pending_edges = Rc::clone(&pending_edges);
        let pending_move = Rc::clone(&pending_move);
        let disposed = Arc::clone(&disposed);
        Scheduler::new(move |_| {
            if disposed.load(Ordering::Relaxed) {
                return;
            }
            let mut edges = pending_edges.borrow_mut().split_off(0);
            if let Some((nx, ny)) = pending_move.borrow_mut().take() {
                edges.push(InputInjectEdge::Move { nx, ny });
            }
            if !edges.is_empty()
                && let Some(send) = send_input_inject
            {
                send.run(edges);
            }
        })
    };
    let queue_edge = Rc::new({
        let pending_edges = Rc::clone(&pending_edges);
        let scheduler = inject_scheduler.clone();
        move |edge: InputInjectEdge| {
            pending_edges.borrow_mut().push(edge);
            scheduler.schedule();
        }
    });
    // Flush everything still pending plus a release edge for every held
    // key and button, bypassing the rAF scheduler — blur and visibility
    // loss can precede throttled frames.
    let release_all_inputs = Rc::new({
        let pressed_keys = Rc::clone(&pressed_keys);
        let pressed_buttons = Rc::clone(&pressed_buttons);
        let pending_edges = Rc::clone(&pending_edges);
        let pending_move = Rc::clone(&pending_move);
        move || {
            let mut edges = pending_edges.borrow_mut().split_off(0);
            if let Some((nx, ny)) = pending_move.borrow_mut().take() {
                edges.push(InputInjectEdge::Move { nx, ny });
            }
            for key in pressed_keys.borrow_mut().drain() {
                edges.push(InputInjectEdge::Key {
                    key,
                    state: InputEdgeState::Released,
                });
            }
            for button in pressed_buttons.borrow_mut().drain() {
                edges.push(InputInjectEdge::Button {
                    button,
                    state: InputEdgeState::Released,
                });
            }
            if !edges.is_empty()
                && let Some(send) = send_input_inject
            {
                send.run(edges);
            }
        }
    });

    // Switching to a non-interactive effect (or losing the host gate)
    // resets the toggle so it never sticks on across effect swaps.
    Effect::new(move |_| {
        if !interactive_available.get() && interactive_on.get_untracked() {
            interactive_on.set(false);
        }
    });
    // Turning capture off — by toggle, effect swap, or gate loss —
    // releases held inputs and drops canvas focus.
    Effect::new({
        let release_all = Rc::clone(&release_all_inputs);
        move |_| {
            if !interactive_active.get() {
                release_all();
                capture_focused.set(false);
                if let Some(canvas) = canvas_ref.get_untracked() {
                    let _ = canvas.blur();
                }
            }
        }
    });

    let _ = use_event_listener(use_document(), Custom::new("visibilitychange"), {
        let release_all = Rc::clone(&release_all_inputs);
        move |_: web_sys::Event| {
            if browser_document().is_some_and(|doc| doc.hidden()) {
                release_all();
            }
        }
    });

    // `on_cleanup` demands `Send + Sync`; `StoredValue::new_local` keeps
    // the `Rc`-flavored release closure and scheduler reachable from it
    // (same pattern as the modal's focus restore).
    let interactive_release: StoredValue<Rc<dyn Fn()>, LocalStorage> =
        StoredValue::new_local(Rc::clone(&release_all_inputs) as Rc<dyn Fn()>);
    let interactive_scheduler = StoredValue::new_local(inject_scheduler.clone());
    on_cleanup(move || {
        interactive_release.with_value(|release| release());
        interactive_scheduler.with_value(Scheduler::pause);
    });

    let canvas_style = format!("max-width: {max_width}; image-rendering: {image_rendering};");
    // Staged: the dimension memo tracks the (up to 60 Hz) frame stream but
    // dedupes to actual size changes, so the string memo below only
    // re-formats when the canvas is genuinely resized.
    let frame_dimensions = Memo::new(move |_| {
        frame.with(|frame| {
            frame
                .as_ref()
                .map(|frame| (frame.width.max(1), frame.height.max(1)))
        })
    });
    let resolved_aspect_ratio = Memo::new(move |_| {
        aspect_ratio.clone().unwrap_or_else(|| {
            frame_dimensions
                .get()
                .map(|(width, height)| format!("{width} / {height}"))
                .unwrap_or_else(|| "320 / 200".to_string())
        })
    });
    // `aspect-ratio` plus `width: 100%` lets the wrapper grow to fill the
    // parent's content box while preserving the canvas's true aspect ratio.
    // When the parent is bounded vertically (e.g., the dashboard hero row),
    // `max-height: 100%` forces the browser to shrink the width too so the
    // wrapper letterboxes/pillarboxes correctly. Centering happens at the
    // host (PreviewCabinet adds `flex items-center justify-center`).
    let wrapper_style = Signal::derive(move || {
        let ratio = resolved_aspect_ratio.get();
        format!("max-width: {max_width}; width: 100%; max-height: 100%; aspect-ratio: {ratio};")
    });

    view! {
        <div
            class="relative bg-black"
            style=move || wrapper_style.get()
            data-preview-runtime=move || runtime_mode.get().unwrap_or("pending")
        >
            <canvas
                node_ref=canvas_ref
                class="w-full h-full block bg-black"
                class:cursor-crosshair=move || interactive_active.get()
                style=canvas_style
                role=move || if interactive_active.get() { "application" } else { "img" }
                aria-label={
                    let aria_label = aria_label.clone();
                    move || if interactive_active.get() {
                        format!(
                            "{aria_label} — interactive mode on: click to focus, \
                             then pointer, keyboard, and wheel input drives the effect"
                        )
                    } else {
                        aria_label.clone()
                    }
                }
                tabindex=move || interactive_active.get().then_some("0")
                on:pointermove={
                    let pending_move = Rc::clone(&pending_move);
                    let scheduler = inject_scheduler.clone();
                    move |ev| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        let Some(canvas) = canvas_ref.get_untracked() else {
                            return;
                        };
                        let rect = canvas.get_bounding_client_rect();
                        let Some((nx, ny)) = normalized_canvas_position(
                            f64::from(ev.client_x()),
                            f64::from(ev.client_y()),
                            rect.left(),
                            rect.top(),
                            rect.width(),
                            rect.height(),
                        ) else {
                            return;
                        };
                        pending_move.borrow_mut().replace((nx, ny));
                        scheduler.schedule();
                    }
                }
                on:pointerdown={
                    let pressed_buttons = Rc::clone(&pressed_buttons);
                    let queue_edge = Rc::clone(&queue_edge);
                    move |ev| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        ev.prevent_default();
                        if let Some(canvas) = canvas_ref.get_untracked() {
                            let _ = canvas.focus();
                            let _ = canvas.set_pointer_capture(ev.pointer_id());
                        }
                        let Some(button) = InputEdgeButton::from_pointer_button(ev.button())
                        else {
                            return;
                        };
                        pressed_buttons.borrow_mut().insert(button);
                        queue_edge(InputInjectEdge::Button {
                            button,
                            state: InputEdgeState::Pressed,
                        });
                    }
                }
                on:pointerup={
                    let pressed_buttons = Rc::clone(&pressed_buttons);
                    let queue_edge = Rc::clone(&queue_edge);
                    move |ev| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        let Some(button) = InputEdgeButton::from_pointer_button(ev.button())
                        else {
                            return;
                        };
                        pressed_buttons.borrow_mut().remove(&button);
                        queue_edge(InputInjectEdge::Button {
                            button,
                            state: InputEdgeState::Released,
                        });
                    }
                }
                on:pointercancel={
                    let pressed_buttons = Rc::clone(&pressed_buttons);
                    let queue_edge = Rc::clone(&queue_edge);
                    move |_| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        let released: Vec<_> = pressed_buttons.borrow_mut().drain().collect();
                        for button in released {
                            queue_edge(InputInjectEdge::Button {
                                button,
                                state: InputEdgeState::Released,
                            });
                        }
                    }
                }
                on:contextmenu=move |ev| {
                    if interactive_active.get_untracked() {
                        ev.prevent_default();
                    }
                }
                on:keydown={
                    let pressed_keys = Rc::clone(&pressed_keys);
                    let queue_edge = Rc::clone(&queue_edge);
                    move |ev| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        // Captured typing must never reach the shell's
                        // global shortcuts (Ctrl+K palette, `/` search,
                        // Ctrl+digit nav) — the shell listens on a
                        // wrapping div, so stopping propagation here is
                        // the whole coordination contract.
                        ev.stop_propagation();
                        if ev.key() == "Escape" {
                            ev.prevent_default();
                            if let Some(canvas) = canvas_ref.get_untracked() {
                                let _ = canvas.blur();
                            }
                            return;
                        }
                        let Some(key) = canonical_injection_key(&ev.code()) else {
                            return;
                        };
                        ev.prevent_default();
                        let state = if ev.repeat() {
                            InputEdgeState::Repeated
                        } else {
                            pressed_keys.borrow_mut().insert(key.clone());
                            InputEdgeState::Pressed
                        };
                        queue_edge(InputInjectEdge::Key { key, state });
                    }
                }
                on:keyup={
                    let pressed_keys = Rc::clone(&pressed_keys);
                    let queue_edge = Rc::clone(&queue_edge);
                    move |ev| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        ev.stop_propagation();
                        let Some(key) = canonical_injection_key(&ev.code()) else {
                            return;
                        };
                        ev.prevent_default();
                        pressed_keys.borrow_mut().remove(&key);
                        queue_edge(InputInjectEdge::Key {
                            key,
                            state: InputEdgeState::Released,
                        });
                    }
                }
                on:wheel={
                    let queue_edge = Rc::clone(&queue_edge);
                    move |ev| {
                        if !interactive_active.get_untracked() {
                            return;
                        }
                        ev.prevent_default();
                        ev.stop_propagation();
                        let delta = wheel_delta_hi_res(ev.delta_y(), ev.delta_mode());
                        if delta != 0 {
                            queue_edge(InputInjectEdge::Wheel { delta_hi_res: delta });
                        }
                    }
                }
                on:focus=move |_| {
                    if interactive_active.get_untracked() {
                        capture_focused.set(true);
                    }
                }
                on:blur={
                    let release_all = Rc::clone(&release_all_inputs);
                    move |_| {
                        capture_focused.set(false);
                        release_all();
                    }
                }
            />

            // Interactive-mode chrome: toggle button (top-left), focus glow
            // ring, and a capture-state hint chip. Rendered only while the
            // active effect wants interaction, so passive effects keep a
            // clean frame.
            {move || interactive_available.get().then(|| view! {
                <button
                    type="button"
                    class=move || if interactive_on.get() {
                        "absolute top-2 left-2 z-20 p-1.5 rounded-lg backdrop-blur-sm border \
                         transition-colors duration-200 btn-press \
                         bg-accent/20 border-accent-muted text-accent"
                    } else {
                        "absolute top-2 left-2 z-20 p-1.5 rounded-lg backdrop-blur-sm border \
                         transition-colors duration-200 btn-press \
                         bg-black/45 border-edge-subtle/60 text-fg-secondary \
                         hover:text-fg-primary hover:bg-black/65 hover:border-edge-default"
                    }
                    title=move || if interactive_on.get() {
                        "Stop driving the effect from the preview"
                    } else {
                        "Drive the effect — send preview clicks, keys, and wheel to the daemon"
                    }
                    aria-label=move || if interactive_on.get() {
                        "Disable interactive preview input"
                    } else {
                        "Enable interactive preview input"
                    }
                    aria-pressed=move || interactive_on.get().to_string()
                    on:click=move |ev| {
                        ev.stop_propagation();
                        interactive_on.update(|on| *on = !*on);
                    }
                >
                    <Icon icon=LuMousePointerClick width="13px" height="13px" />
                </button>

                {move || interactive_active.get().then(|| view! {
                    <div
                        class="absolute inset-0 pointer-events-none z-10"
                        style=move || capture_focused.get().then_some(
                            "box-shadow: inset 0 0 0 2px var(--glow-focus), \
                             inset 0 0 12px var(--glow-focus-soft)"
                        )
                    />
                    <div class="absolute top-2 left-1/2 -translate-x-1/2 z-20 pointer-events-none \
                                px-2 py-0.5 rounded-full bg-black/60 backdrop-blur-sm \
                                text-[9px] font-mono uppercase tracking-wider">
                        {move || if capture_focused.get() {
                            view! { <span class="text-accent">"Input live — Esc releases"</span> }
                                .into_any()
                        } else {
                            view! { <span class="text-fg-tertiary">"Click canvas to capture input"</span> }
                                .into_any()
                        }}
                    </div>
                })}
            })}
            {if show_fps {
                Some(view! {
                    <div class="absolute top-2 right-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] font-mono text-fg-tertiary
                                transition-opacity duration-300 animate-enter-fade">
                        {move || {
                            let target = fps_target.get();
                            let mode = runtime_mode.get().unwrap_or("pending");
                            let display_fps = {
                                let present = presented_fps.get();
                                if present > 0.0 { present } else { fps.get() }
                            };
                            if target > 0 {
                                format!("{fps_label} {:.0}/{target} fps [{mode}]", display_fps)
                            } else {
                                format!("{fps_label} {:.0} fps [{mode}]", display_fps)
                            }
                        }}
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}
