use hypercolor_leptos_ext::canvas::set_canvas_size;
use web_sys::HtmlCanvasElement;

use crate::ws::{CanvasFrame, CanvasPixelFormat};

pub mod canvas2d;
mod main_thread;
mod webgl;
mod worker;

use canvas2d::Canvas2dPreviewRuntime;
use main_thread::MainThreadJpegRuntime;
use webgl::WebGlInitError;
use webgl::WebGlPreviewRuntime;
use worker::PreviewWorkerRuntime;

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
}

impl PreviewRuntime {
    pub(super) fn new(
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
        allow_canvas2d_fallback: bool,
        smooth_scaling: bool,
    ) -> Result<Self, PreviewRuntimeInitError> {
        prepare_canvas(canvas, frame);

        if frame.pixel_format() == CanvasPixelFormat::Jpeg {
            // JPEG never falls back to raw pixels: without the worker it
            // decodes on the main thread, and a page that can do neither
            // leaves the canvas as it is.
            return PreviewWorkerRuntime::new(canvas, frame)
                .map(PreviewRuntimeBackend::Worker)
                .or_else(|()| {
                    MainThreadJpegRuntime::new(canvas).map(PreviewRuntimeBackend::MainThreadJpeg)
                })
                .map(|backend| Self { backend })
                .map_err(|()| PreviewRuntimeInitError::WebGlUnavailable);
        }

        let backend = match WebGlPreviewRuntime::new(canvas, smooth_scaling) {
            Ok(runtime) => PreviewRuntimeBackend::WebGl(runtime),
            Err(WebGlInitError::InitializationFailed) => {
                return Err(PreviewRuntimeInitError::WebGlInitializationFailed);
            }
            Err(WebGlInitError::ContextUnavailable) => PreviewWorkerRuntime::new(canvas, frame)
                .map(PreviewRuntimeBackend::Worker)
                .or_else(|()| {
                    allow_canvas2d_fallback
                        .then(|| Canvas2dPreviewRuntime::new(canvas))
                        .flatten()
                        .map(PreviewRuntimeBackend::Canvas2d)
                        .ok_or(PreviewRuntimeInitError::WebGlUnavailable)
                })?,
        };
        Ok(Self { backend })
    }

    pub(super) fn render(
        &mut self,
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
    ) -> PreviewRenderOutcome {
        match &mut self.backend {
            PreviewRuntimeBackend::Worker(runtime) => runtime.render(frame),
            PreviewRuntimeBackend::MainThreadJpeg(runtime) => runtime.render(frame),
            PreviewRuntimeBackend::WebGl(runtime) => runtime.render(canvas, frame),
            PreviewRuntimeBackend::Canvas2d(runtime) => runtime.render(canvas, frame),
        }
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
