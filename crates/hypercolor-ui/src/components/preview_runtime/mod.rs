use web_sys::HtmlCanvasElement;

use crate::ws::{CanvasFrame, CanvasPixelFormat};

mod canvas2d;
mod webgl;
mod worker;

use canvas2d::Canvas2dPreviewRuntime;
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
    WebGl(WebGlPreviewRuntime),
    Canvas2d(Canvas2dPreviewRuntime),
}

pub(super) struct PreviewRuntime(PreviewRuntimeBackend);

impl PreviewRuntime {
    pub(super) fn new(
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
        allow_canvas2d_fallback: bool,
        smooth_scaling: bool,
    ) -> Result<Self, PreviewRuntimeInitError> {
        prepare_canvas(canvas, frame);

        if let Ok(runtime) = PreviewWorkerRuntime::new(canvas, frame) {
            return Ok(Self(PreviewRuntimeBackend::Worker(runtime)));
        }

        if frame.pixel_format() == CanvasPixelFormat::Jpeg {
            return Err(PreviewRuntimeInitError::WebGlUnavailable);
        }

        match WebGlPreviewRuntime::new(canvas, smooth_scaling) {
            Ok(runtime) => Ok(Self(PreviewRuntimeBackend::WebGl(runtime))),
            Err(WebGlInitError::InitializationFailed) => {
                Err(PreviewRuntimeInitError::WebGlInitializationFailed)
            }
            Err(WebGlInitError::ContextUnavailable) if allow_canvas2d_fallback => {
                Canvas2dPreviewRuntime::new(canvas)
                    .map(PreviewRuntimeBackend::Canvas2d)
                    .map(Self)
                    .ok_or(PreviewRuntimeInitError::WebGlUnavailable)
            }
            Err(WebGlInitError::ContextUnavailable) => {
                Err(PreviewRuntimeInitError::WebGlUnavailable)
            }
        }
    }

    pub(super) fn render(
        &mut self,
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
    ) -> PreviewRenderOutcome {
        match &mut self.0 {
            PreviewRuntimeBackend::Worker(runtime) => runtime.render(frame),
            PreviewRuntimeBackend::WebGl(runtime) => runtime.render(canvas, frame),
            PreviewRuntimeBackend::Canvas2d(runtime) => runtime.render(canvas, frame),
        }
    }

    pub(super) fn preserves_webgl_unavailable_streak(&self) -> bool {
        matches!(self.0, PreviewRuntimeBackend::Canvas2d(_))
    }

    pub(super) fn mode_label(&self) -> &'static str {
        match self.0 {
            PreviewRuntimeBackend::Worker(_) => "worker",
            PreviewRuntimeBackend::WebGl(_) => "webgl",
            PreviewRuntimeBackend::Canvas2d(_) => "canvas2d",
        }
    }
}

fn prepare_canvas(canvas: &HtmlCanvasElement, frame: &CanvasFrame) {
    if canvas.width() != frame.width {
        canvas.set_width(frame.width);
    }
    if canvas.height() != frame.height {
        canvas.set_height(frame.height);
    }
}
