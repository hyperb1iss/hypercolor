//! Canvas preview — presents authoritative daemon frames in the browser via WebGL.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hypercolor_leptos_ext::prelude::now_ms;
use hypercolor_leptos_ext::raf::Scheduler;
use leptos::ev::Custom;
use leptos::html::Canvas;
use leptos::prelude::*;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};

use crate::app::WsContext;
use crate::preview_telemetry::{PreviewPresenterTelemetry, PreviewTelemetryContext};
use crate::ws::CanvasFrame;

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
) -> impl IntoView {
    let canvas_ref = NodeRef::<Canvas>::new();
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
    let consumer_count = if register_main_preview_consumer {
        consumer_count.or_else(|| ws.map(|ws| ws.set_preview_consumers))
    } else {
        consumer_count
    };

    {
        let schedule_canvas_ref = canvas_ref;
        let latest_frame = Rc::clone(&latest_frame);
        let latest_frame_received_at = Rc::clone(&latest_frame_received_at);
        let presenter = Rc::clone(&presenter);
        let presenter_scheduler_handle = Rc::clone(&presenter_scheduler);
        let last_presented_frame = Rc::clone(&last_presented_frame);
        let last_presented_at = Rc::clone(&last_presented_at);
        let skipped_frames = Rc::clone(&skipped_frames);
        let last_published_telemetry = Rc::clone(&last_published_telemetry);
        let last_telemetry_published_at = Rc::clone(&last_telemetry_published_at);

        let scheduler = Scheduler::new(move |frame_info| {
            let raf_time_ms = frame_info.time_ms;

            let Some(canvas_handle) = schedule_canvas_ref.get() else {
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
            move || scheduler.schedule()
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
            let next_frame = frame.get();
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

        move |_| {
            if canvas_ref.get().is_none() {
                return;
            }

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
        move || {
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

    let canvas_style = format!("max-width: {max_width}; image-rendering: {image_rendering};");
    let resolved_aspect_ratio = Memo::new(move |_| {
        aspect_ratio.clone().unwrap_or_else(|| {
            frame.with(|frame| {
                frame
                    .as_ref()
                    .map(|frame| format!("{} / {}", frame.width.max(1), frame.height.max(1)))
                    .unwrap_or_else(|| "320 / 200".to_string())
            })
        })
    });
    let wrapper_style = Signal::derive(move || {
        let ratio = resolved_aspect_ratio.get();
        format!("max-width: {max_width}; width: 100%; height: 100%; aspect-ratio: {ratio};")
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
                style=canvas_style
                role="img"
                aria-label=aria_label
            />
            {if show_fps {
                Some(view! {
                    <div class="absolute top-2 right-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] font-mono text-fg-tertiary
                                transition-all duration-300 animate-fade-in">
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
