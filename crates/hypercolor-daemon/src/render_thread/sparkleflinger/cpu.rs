use hypercolor_core::blend_math::blend_rgba_pixels_in_place;
use hypercolor_core::types::canvas::Canvas;
use hypercolor_types::overlay::OverlayBlendMode;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, publish_composed_frame,
};

#[derive(Debug, Default)]
pub(super) struct CpuSparkleFlinger;

impl CpuSparkleFlinger {
    pub(super) const fn new() -> Self {
        Self
    }

    #[allow(
        clippy::unused_self,
        reason = "the CPU compositor keeps an instance method to match the GPU flinger API"
    )]
    pub(super) fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
        let CompositionPlan {
            width,
            height,
            mut layers,
        } = plan;

        if layers.len() == 1
            && let Some(layer) = layers.pop()
            && layer.is_bypass_candidate()
        {
            return publish_composed_frame(layer.frame.into_render_frame(), true);
        }

        let mut layers = layers.into_iter();
        let mut sampling_canvas = if let Some(first_layer) = layers.next() {
            take_base_canvas(first_layer, width, height)
        } else {
            Canvas::new(width, height)
        };
        for layer in layers {
            compose_layer(&mut sampling_canvas, layer);
        }

        publish_composed_frame((sampling_canvas, None), false)
    }
}

fn take_base_canvas(layer: CompositionLayer, width: u32, height: u32) -> Canvas {
    if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        let (canvas, _) = layer.frame.into_render_frame();
        return canvas;
    }

    let mut canvas = Canvas::new(width, height);
    compose_layer(&mut canvas, layer);
    canvas
}

fn compose_layer(target: &mut Canvas, layer: CompositionLayer) {
    let (source_canvas, _) = layer.frame.into_render_frame();
    if target.width() != source_canvas.width() || target.height() != source_canvas.height() {
        *target = Canvas::new(source_canvas.width(), source_canvas.height());
    }

    let opacity = layer.opacity.clamp(0.0, 1.0);
    if layer.mode == CompositionMode::Replace && opacity >= 1.0 {
        target
            .as_rgba_bytes_mut()
            .copy_from_slice(source_canvas.as_rgba_bytes());
        return;
    }

    if opacity <= 0.0 {
        return;
    }

    let blend_mode = match layer.mode {
        CompositionMode::Replace | CompositionMode::Alpha => OverlayBlendMode::Normal,
        CompositionMode::Add => OverlayBlendMode::Add,
        CompositionMode::Screen => OverlayBlendMode::Screen,
    };
    blend_rgba_pixels_in_place(
        target.as_rgba_bytes_mut(),
        source_canvas.as_rgba_bytes(),
        blend_mode,
        opacity,
    );
}
