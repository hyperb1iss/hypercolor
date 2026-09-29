use std::rc::Rc;

use hypercolor_leptos_ext::canvas::set_canvas_size;
use web_sys::HtmlCanvasElement;

use crate::ws::{CanvasFrame, CanvasPixelFormat, PreviewTag};

pub mod canvas2d;
mod main_thread;
mod webgl;
mod worker;

use canvas2d::Canvas2dPreviewRuntime;
use main_thread::MainThreadJpegRuntime;
use webgl::WebGlInitError;
use webgl::WebGlPreviewRuntime;
use worker::PreviewWorkerRuntime;

/// Called once a tagged frame's pixels are on screen, with the tag its
/// `render` call carried. For the asynchronous presenters that is later
/// than `render` returns; a frame that never lands never reports.
pub(super) type PresentedHook = Rc<dyn Fn(&CanvasFrame, PreviewTag)>;

/// A frame on its way to the screen, with the tag to report once it lands.
#[derive(Clone)]
pub(super) struct SubmittedFrame {
    pub(super) frame: CanvasFrame,
    pub(super) tag: Option<PreviewTag>,
}

impl SubmittedFrame {
    fn report(&self, hook: Option<&PresentedHook>) {
        if let (Some(hook), Some(tag)) = (hook, self.tag) {
            hook(&self.frame, tag);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextureShape {
    width: u32,
    height: u32,
    format: CanvasPixelFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewRenderOutcome {
    Presented,
    Reinitialize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewRuntimeInitError {
    WebGlUnavailable,
    WebGlInitializationFailed,
}

enum PreviewRuntimeBackend {
    Worker(PreviewWorkerRuntime),
    MainThreadJpeg(MainThreadJpegRuntime),
    WebGl(WebGlPreviewRuntime),
    Canvas2d(Canvas2dPreviewRuntime),
}

pub(super) struct PreviewRuntime {
    backend: PreviewRuntimeBackend,
    presented: Option<PresentedHook>,
}

impl PreviewRuntime {
    pub(super) fn new(
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
        allow_canvas2d_fallback: bool,
        smooth_scaling: bool,
        presented: Option<PresentedHook>,
    ) -> Result<Self, PreviewRuntimeInitError> {
        prepare_canvas(canvas, frame);

        if frame.pixel_format() == CanvasPixelFormat::Jpeg {
            // JPEG never falls back to raw pixels: without the worker it
            // decodes on the main thread, and a page that can do neither
            // leaves the canvas as it is.
            return PreviewWorkerRuntime::new(canvas, frame, presented.clone())
                .map(PreviewRuntimeBackend::Worker)
                .or_else(|()| {
                    MainThreadJpegRuntime::new(canvas, presented.clone())
                        .map(PreviewRuntimeBackend::MainThreadJpeg)
                })
                .map(|backend| Self { backend, presented })
                .map_err(|()| PreviewRuntimeInitError::WebGlUnavailable);
        }

        let backend = match WebGlPreviewRuntime::new(canvas, smooth_scaling) {
            Ok(runtime) => PreviewRuntimeBackend::WebGl(runtime),
            Err(WebGlInitError::InitializationFailed) => {
                return Err(PreviewRuntimeInitError::WebGlInitializationFailed);
            }
            Err(WebGlInitError::ContextUnavailable) => {
                PreviewWorkerRuntime::new(canvas, frame, presented.clone())
                    .map(PreviewRuntimeBackend::Worker)
                    .or_else(|()| {
                        allow_canvas2d_fallback
                            .then(|| Canvas2dPreviewRuntime::new(canvas))
                            .flatten()
                            .map(PreviewRuntimeBackend::Canvas2d)
                            .ok_or(PreviewRuntimeInitError::WebGlUnavailable)
                    })?
            }
        };
        Ok(Self { backend, presented })
    }

    /// Present `frame`. A `tag` is reported through the presented hook once
    /// the frame's pixels land; an untagged frame is never reported.
    pub(super) fn render(
        &mut self,
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
        tag: Option<PreviewTag>,
    ) -> PreviewRenderOutcome {
        // A worker that broke after it started cannot recover by being
        // recreated, so JPEG moves to the main thread instead.
        if let PreviewRuntimeBackend::Worker(worker) = &self.backend
            && worker.is_broken()
            && frame.pixel_format() == CanvasPixelFormat::Jpeg
            && let Ok(fallback) = MainThreadJpegRuntime::new(canvas, self.presented.clone())
        {
            self.backend = PreviewRuntimeBackend::MainThreadJpeg(fallback);
        }

        let submitted = SubmittedFrame {
            frame: frame.clone(),
            tag,
        };
        let outcome = match &mut self.backend {
            PreviewRuntimeBackend::Worker(runtime) => return runtime.render(submitted),
            PreviewRuntimeBackend::MainThreadJpeg(runtime) => return runtime.render(submitted),
            PreviewRuntimeBackend::WebGl(runtime) => runtime.render(canvas, frame),
            PreviewRuntimeBackend::Canvas2d(runtime) => runtime.render(canvas, frame),
        };
        if outcome == PreviewRenderOutcome::Presented {
            submitted.report(self.presented.as_ref());
        }
        outcome
    }

    pub(super) fn preserves_webgl_unavailable_streak(&self) -> bool {
        matches!(self.backend, PreviewRuntimeBackend::Canvas2d(_))
    }

    pub(super) fn mode_label(&self) -> &'static str {
        match self.backend {
            PreviewRuntimeBackend::Worker(_) => "worker",
            PreviewRuntimeBackend::MainThreadJpeg(_) => "main-thread",
            PreviewRuntimeBackend::WebGl(_) => "webgl",
            PreviewRuntimeBackend::Canvas2d(_) => "canvas2d",
        }
    }
}

fn prepare_canvas(canvas: &HtmlCanvasElement, frame: &CanvasFrame) {
    set_canvas_size(canvas, frame.width, frame.height);
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use hypercolor_leptos_ext::ws::PreviewFrameChannel;
    use js_sys::Promise;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use wasm_bindgen_test::*;
    use web_sys::HtmlCanvasElement;

    use super::{PresentedHook, PreviewRuntime};
    use crate::ws::{CanvasFrame, CanvasPixelFormat, PreviewCounters, PreviewTag};

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen(inline_js = r##"
export function jpegBytes() {
  const canvas = document.createElement("canvas");
  canvas.width = 8;
  canvas.height = 6;
  const context = canvas.getContext("2d");
  context.fillStyle = "#e135ff";
  context.fillRect(0, 0, 8, 6);
  const base64 = canvas.toDataURL("image/jpeg").split(",")[1];
  return Uint8Array.from(atob(base64), (character) => character.charCodeAt(0));
}
export function sleep(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
"##)]
    extern "C" {
        #[wasm_bindgen(js_name = jpegBytes)]
        fn jpeg_bytes() -> js_sys::Uint8Array;
        fn sleep(ms: u32) -> Promise;
    }

    fn frame(frame_number: u32, payload: js_sys::Uint8Array) -> CanvasFrame {
        CanvasFrame {
            channel: PreviewFrameChannel::Canvas,
            frame_number,
            timestamp_ms: frame_number,
            width: 8,
            height: 6,
            format: CanvasPixelFormat::Jpeg,
            payload,
        }
    }

    fn garbage() -> js_sys::Uint8Array {
        js_sys::Uint8Array::from(&b"not a jpeg"[..])
    }

    fn canvas() -> HtmlCanvasElement {
        hypercolor_leptos_ext::canvas::create_canvas().expect("canvas")
    }

    /// Frame numbers and tags a presenter reported, in order.
    type Reports = Rc<RefCell<Vec<(u32, PreviewTag)>>>;

    /// The runtime carries a tag without reading it, so one tag stands in
    /// for every frame these tests submit.
    fn submitted_tag() -> PreviewTag {
        let mut counters = PreviewCounters::default();
        counters.record_received(0);
        counters.tag(0).expect("tag")
    }

    fn recording_hook() -> (PresentedHook, Reports) {
        let shown = Rc::new(RefCell::new(Vec::new()));
        let record = Rc::clone(&shown);
        let hook: PresentedHook = Rc::new(move |frame: &CanvasFrame, tag: PreviewTag| {
            record.borrow_mut().push((frame.frame_number, tag));
        });
        (hook, shown)
    }

    /// Render frames until `done`, giving the presenters time to answer.
    async fn render_until(
        runtime: &mut PreviewRuntime,
        canvas: &HtmlCanvasElement,
        mut next: impl FnMut(u32) -> CanvasFrame,
        done: impl Fn(&PreviewRuntime) -> bool,
    ) -> u32 {
        for frame_number in 1..=60 {
            let _ = runtime.render(canvas, &next(frame_number), Some(submitted_tag()));
            JsFuture::from(sleep(40)).await.expect("sleep");
            if done(runtime) {
                return frame_number;
            }
        }
        panic!("presenter never reached the expected state");
    }

    #[wasm_bindgen_test]
    async fn the_worker_skips_an_undecodable_frame_and_keeps_presenting() {
        let canvas = canvas();
        let (hook, shown) = recording_hook();
        let mut runtime =
            PreviewRuntime::new(&canvas, &frame(0, garbage()), false, false, Some(hook))
                .unwrap_or_else(|_| panic!("worker runtime"));
        assert_eq!(runtime.mode_label(), "worker");

        let _ = runtime.render(&canvas, &frame(1, garbage()), Some(submitted_tag()));
        JsFuture::from(sleep(80)).await.expect("sleep");
        let last = render_until(
            &mut runtime,
            &canvas,
            |number| frame(number + 1, jpeg_bytes()),
            |_| !shown.borrow().is_empty(),
        )
        .await;

        assert_eq!(runtime.mode_label(), "worker");
        assert!(
            shown
                .borrow()
                .iter()
                .all(|(_, tag)| *tag == submitted_tag())
        );
        assert!(
            shown
                .borrow()
                .iter()
                .all(|(number, _)| (2..=last + 1).contains(number))
        );
        assert_eq!(canvas.width(), 8);
    }

    #[wasm_bindgen_test]
    async fn a_worker_that_decodes_nothing_hands_jpeg_to_the_main_thread() {
        let canvas = canvas();
        let (hook, shown) = recording_hook();
        let mut runtime =
            PreviewRuntime::new(&canvas, &frame(0, garbage()), false, false, Some(hook))
                .unwrap_or_else(|_| panic!("worker runtime"));

        render_until(&mut runtime, &canvas, |number| frame(number, garbage()), |runtime| {
            matches!(&runtime.backend, super::PreviewRuntimeBackend::Worker(worker) if worker.is_broken())
        })
        .await;
        assert!(shown.borrow().is_empty());

        render_until(
            &mut runtime,
            &canvas,
            |number| frame(100 + number, jpeg_bytes()),
            |_| !shown.borrow().is_empty(),
        )
        .await;
        assert_eq!(runtime.mode_label(), "main-thread");
        assert!(
            shown
                .borrow()
                .iter()
                .all(|(number, tag)| *number > 100 && *tag == submitted_tag())
        );
    }

    #[wasm_bindgen_test]
    async fn untagged_frames_land_without_reporting() {
        let canvas = canvas();
        let (hook, shown) = recording_hook();
        let mut runtime =
            PreviewRuntime::new(&canvas, &frame(0, jpeg_bytes()), false, false, Some(hook))
                .unwrap_or_else(|_| panic!("worker runtime"));
        for number in 1..=5 {
            let _ = runtime.render(&canvas, &frame(number, jpeg_bytes()), None);
            JsFuture::from(sleep(40)).await.expect("sleep");
        }
        assert!(shown.borrow().is_empty());
        assert_eq!(canvas.width(), 8);
    }
}
