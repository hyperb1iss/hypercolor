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
