use wasm_bindgen::Clamped;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

use crate::ws::{CanvasFrame, CanvasPixelFormat};

use super::PreviewRenderOutcome;

pub(crate) fn expand_rgb_to_rgba_bytes(source: &[u8], destination: &mut Vec<u8>) {
    let pixel_count = source.len() / 3;
    destination.resize(pixel_count.saturating_mul(4), 0);

    for (rgb, rgba) in source.chunks_exact(3).zip(destination.chunks_exact_mut(4)) {
        rgba[0] = rgb[0];
        rgba[1] = rgb[1];
        rgba[2] = rgb[2];
        rgba[3] = 255;
    }
}

pub(super) struct Canvas2dPreviewRuntime {
    ctx: CanvasRenderingContext2d,
    scratch_rgba: Vec<u8>,
    scratch_source: Vec<u8>,
}

impl Canvas2dPreviewRuntime {
    pub(super) fn new(canvas: &HtmlCanvasElement) -> Option<Self> {
        let ctx = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|ctx| ctx.dyn_into::<CanvasRenderingContext2d>().ok())?;

        Some(Self {
            ctx,
            scratch_rgba: Vec::new(),
            scratch_source: Vec::new(),
        })
    }

    fn ensure_canvas_size(&mut self, canvas: &HtmlCanvasElement, width: u32, height: u32) {
        if canvas.width() != width {
            canvas.set_width(width);
        }
        if canvas.height() != height {
            canvas.set_height(height);
        }
    }

    fn copy_frame_into_rgba(&mut self, frame: &CanvasFrame) {
        match frame.pixel_format() {
            CanvasPixelFormat::Rgba => {
                let rgba_len = frame.pixel_count().saturating_mul(4);
                self.scratch_rgba.resize(rgba_len, 0);
                frame.pixels_js().copy_to(&mut self.scratch_rgba);
            }
            CanvasPixelFormat::Rgb => {
                let rgb_len = frame.pixel_count().saturating_mul(3);
                self.scratch_source.resize(rgb_len, 0);
                frame.pixels_js().copy_to(&mut self.scratch_source);
                expand_rgb_to_rgba_bytes(&self.scratch_source, &mut self.scratch_rgba);
            }
        }
    }

    pub(super) fn render(
        &mut self,
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
    ) -> PreviewRenderOutcome {
        self.ensure_canvas_size(canvas, frame.width, frame.height);
        self.copy_frame_into_rgba(frame);

        let Ok(image_data) = ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(self.scratch_rgba.as_slice()),
            frame.width,
            frame.height,
        ) else {
            return PreviewRenderOutcome::Reinitialize;
        };

        if self.ctx.put_image_data(&image_data, 0.0, 0.0).is_err() {
            return PreviewRenderOutcome::Reinitialize;
        }

        PreviewRenderOutcome::Presented
    }
}
