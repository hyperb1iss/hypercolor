//! Canvas preview — presents authoritative daemon frames in the browser via WebGL.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html::Canvas;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::app::WsContext;
use crate::ws::CanvasFrame;

use super::preview_runtime::{PreviewRenderOutcome, WebGlPreviewRuntime};

type PresentCallback = Rc<dyn Fn()>;
type PresentScheduler = Rc<RefCell<Option<PresentCallback>>>;
const WEBGL_RETRY_DELAY_FRAMES: u32 = 30;

enum PresenterState {
    Uninitialized,
    Ready(WebGlPreviewRuntime),
    CoolingDown { retry_at_frame: u32 },
}

impl PresenterState {
    fn schedule_retry(&mut self, frame_number: u32) {
        *self = Self::CoolingDown {
            retry_at_frame: frame_number.saturating_add(WEBGL_RETRY_DELAY_FRAMES),
        };
    }

    fn ready_for_retry(&self, frame_number: u32) -> bool {
        match self {
            Self::CoolingDown { retry_at_frame } => frame_number >= *retry_at_frame,
            Self::Uninitialized | Self::Ready(_) => true,
        }
    }

    fn ensure_runtime(&mut self, canvas: &web_sys::HtmlCanvasElement, frame_number: u32) -> bool {
        if !self.ready_for_retry(frame_number) {
            return false;
        }

        if matches!(self, Self::Ready(_)) {
            return true;
        }

        match WebGlPreviewRuntime::new(canvas) {
            Some(runtime) => {
                *self = Self::Ready(runtime);
                true
            }
            None => {
                self.schedule_retry(frame_number);
                false
            }
        }
    }

    fn reset(&mut self) {
        *self = Self::Uninitialized;
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
    #[prop(optional)] aspect_ratio: Option<String>,
    #[prop(optional)] consumer_count: Option<WriteSignal<u32>>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<Canvas>::new();
    let latest_frame = Rc::new(RefCell::new(None::<CanvasFrame>));
    let presenter = Rc::new(RefCell::new(PresenterState::Uninitialized));
    let animation = Rc::new(RefCell::new(None::<Closure<dyn FnMut(f64)>>));
    let last_presented_frame = Rc::new(RefCell::new(None::<u32>));
    let schedule_present: PresentScheduler = Rc::new(RefCell::new(None));
    let ws = use_context::<WsContext>();
    let preview_registered = Arc::new(AtomicBool::new(false));
    let consumer_count = consumer_count.or_else(|| ws.map(|ws| ws.set_preview_consumers));

    {
        let schedule_canvas_ref = canvas_ref;
        let latest_frame = Rc::clone(&latest_frame);
        let presenter = Rc::clone(&presenter);
        let animation = Rc::clone(&animation);
        let last_presented_frame = Rc::clone(&last_presented_frame);

        let schedule = Rc::new(move || {
            if animation.borrow().is_some() {
                return;
            }

            let Some(canvas) = schedule_canvas_ref.get() else {
                return;
            };

            let Some(window) = web_sys::window() else {
                return;
            };

            let animation_handle = Rc::clone(&animation);
            let presenter_handle = Rc::clone(&presenter);
            let latest_frame = Rc::clone(&latest_frame);
            let last_presented_frame = Rc::clone(&last_presented_frame);
            let canvas_handle = canvas.clone();

            let callback = Closure::<dyn FnMut(f64)>::new(move |_| {
                if !canvas_handle.is_connected() {
                    presenter_handle.borrow_mut().reset();
                    last_presented_frame.borrow_mut().take();
                    animation_handle.borrow_mut().take();
                    return;
                }

                if let Some(frame) = latest_frame.borrow().clone()
                    && Some(frame.frame_number) != *last_presented_frame.borrow()
                {
                    let mut presenter_state = presenter_handle.borrow_mut();
                    if presenter_state.ensure_runtime(&canvas_handle, frame.frame_number)
                        && let PresenterState::Ready(presenter) = &mut *presenter_state
                    {
                        match presenter.render(&canvas_handle, &frame) {
                            PreviewRenderOutcome::Presented => {
                                *last_presented_frame.borrow_mut() = Some(frame.frame_number);
                            }
                            PreviewRenderOutcome::Reinitialize => {
                                presenter_state.schedule_retry(frame.frame_number);
                                last_presented_frame.borrow_mut().take();
                            }
                        }
                    }
                }

                animation_handle.borrow_mut().take();
            });

            if window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .is_ok()
            {
                *animation.borrow_mut() = Some(callback);
            }
        });

        *schedule_present.borrow_mut() = Some(schedule);
    }

    // Stash the newest frame immediately and queue a single browser-timed present.
    Effect::new({
        let latest_frame = Rc::clone(&latest_frame);
        let schedule_present = Rc::clone(&schedule_present);
        move |_| {
            let next_frame = frame.get();
            *latest_frame.borrow_mut() = next_frame.clone();

            if next_frame.is_some()
                && let Some(schedule) = schedule_present.borrow().as_ref()
            {
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
        }
    });

    let canvas_style = format!("max-width: {max_width}; image-rendering: pixelated;");
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
        <div class="relative bg-black" style=move || wrapper_style.get()>
            <canvas
                node_ref=canvas_ref
                class="w-full h-full block bg-black"
                style=canvas_style
            />
            {if show_fps {
                Some(view! {
                    <div class="absolute top-2 right-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] font-mono text-fg-tertiary
                                transition-all duration-300 animate-fade-in">
                        {move || {
                            let target = fps_target.get();
                            if target > 0 {
                                format!("{fps_label} {:.0}/{target} fps", fps.get())
                            } else {
                                format!("{fps_label} {:.0} fps", fps.get())
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
