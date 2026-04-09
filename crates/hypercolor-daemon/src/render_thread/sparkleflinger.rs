use hypercolor_core::types::canvas::{BlendMode, Canvas, PublishedSurface, Rgba, RgbaF32};

use super::producer_queue::ProducerFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "Wave 3 lands the compositor surface now; more layer modes get wired into plans in Wave 4"
)]
pub(crate) enum CompositionMode {
    Replace,
    Alpha,
    Add,
    Screen,
}

impl CompositionMode {
    const fn blend_mode(self) -> Option<BlendMode> {
        match self {
            Self::Replace => None,
            Self::Alpha => Some(BlendMode::Normal),
            Self::Add => Some(BlendMode::Add),
            Self::Screen => Some(BlendMode::Screen),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompositionLayer {
    frame: ProducerFrame,
    mode: CompositionMode,
    opacity: f32,
}

impl CompositionLayer {
    pub fn replace_canvas(canvas: Canvas) -> Self {
        Self::replace(ProducerFrame::Canvas(canvas))
    }

    pub fn replace_surface(surface: PublishedSurface) -> Self {
        Self::replace(ProducerFrame::Surface(surface))
    }

    pub fn alpha_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::alpha(ProducerFrame::Canvas(canvas), opacity)
    }

    pub fn add_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::add(ProducerFrame::Canvas(canvas), opacity)
    }

    pub fn screen_canvas(canvas: Canvas, opacity: f32) -> Self {
        Self::screen(ProducerFrame::Canvas(canvas), opacity)
    }

    pub(crate) fn replace(frame: ProducerFrame) -> Self {
        Self {
            frame,
            mode: CompositionMode::Replace,
            opacity: 1.0,
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 3 proves blend math in unit tests before live multi-layer plans arrive"
    )]
    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Alpha,
            opacity,
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 3 proves blend math in unit tests before live multi-layer plans arrive"
    )]
    pub(crate) fn add(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Add,
            opacity,
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 3 proves blend math in unit tests before live multi-layer plans arrive"
    )]
    pub(crate) fn screen(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Screen,
            opacity,
        }
    }

    fn is_bypass_candidate(&self) -> bool {
        self.mode == CompositionMode::Replace && self.opacity >= 1.0
    }
}

#[derive(Debug, Clone)]
pub struct CompositionPlan {
    width: u32,
    height: u32,
    layers: Vec<CompositionLayer>,
}

impl CompositionPlan {
    pub fn single(width: u32, height: u32, layer: CompositionLayer) -> Self {
        Self {
            width,
            height,
            layers: vec![layer],
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 3 exercises layered plans in unit tests ahead of render-group wiring"
    )]
    pub fn with_layers(width: u32, height: u32, layers: Vec<CompositionLayer>) -> Self {
        Self {
            width,
            height,
            layers,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComposedFrameSet {
    pub sampling_canvas: Canvas,
    pub sampling_surface: Option<PublishedSurface>,
    pub preview_surface: Option<PublishedSurface>,
    pub bypassed: bool,
}

#[derive(Debug, Default)]
pub struct SparkleFlinger;

impl SparkleFlinger {
    pub const fn new() -> Self {
        Self
    }

    pub fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
        if plan.layers.len() == 1
            && let Some(layer) = plan.layers.first()
            && layer.is_bypass_candidate()
        {
            let (sampling_canvas, sampling_surface) = layer.frame.clone().into_render_frame();
            return ComposedFrameSet {
                sampling_canvas,
                sampling_surface,
                preview_surface: None,
                bypassed: true,
            };
        }

        let mut sampling_canvas = Canvas::new(plan.width, plan.height);
        for layer in plan.layers {
            compose_layer(&mut sampling_canvas, layer);
        }

        ComposedFrameSet {
            sampling_canvas,
            sampling_surface: None,
            preview_surface: None,
            bypassed: false,
        }
    }
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

    let blend_mode = layer.mode.blend_mode().unwrap_or(BlendMode::Normal);
    let target_pixels = target.as_rgba_bytes_mut();
    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_canvas.as_rgba_bytes().chunks_exact(4))
    {
        let dst = Rgba::new(dst_px[0], dst_px[1], dst_px[2], dst_px[3]).to_linear_f32();
        let src = Rgba::new(src_px[0], src_px[1], src_px[2], src_px[3]).to_linear_f32();
        let blended = blend_mode.blend(
            [dst.r, dst.g, dst.b, dst.a],
            [src.r, src.g, src.b, src.a],
            opacity,
        );
        let out = RgbaF32::new(blended[0], blended[1], blended[2], blended[3]).to_srgba();
        dst_px[0] = out.r;
        dst_px[1] = out.g;
        dst_px[2] = out.b;
        dst_px[3] = out.a;
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{BlendMode, Canvas, PublishedSurface, Rgba, RgbaF32};

    use super::{CompositionLayer, CompositionPlan, SparkleFlinger};
    use crate::render_thread::producer_queue::ProducerFrame;

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(color);
        canvas
    }

    fn expected_blend(dst: Rgba, src: Rgba, mode: BlendMode, opacity: f32) -> Rgba {
        let dst = dst.to_linear_f32();
        let src = src.to_linear_f32();
        let blended = mode.blend(
            [dst.r, dst.g, dst.b, dst.a],
            [src.r, src.g, src.b, src.a],
            opacity,
        );
        RgbaF32::new(blended[0], blended[1], blended[2], blended[3]).to_srgba()
    }

    #[test]
    fn sparkleflinger_bypasses_single_replace_surface() {
        let source =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
        let mut sparkleflinger = SparkleFlinger::new();
        let composed = sparkleflinger.compose(CompositionPlan::single(
            2,
            2,
            CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
        ));

        let surface = composed
            .sampling_surface
            .expect("single replace layer should bypass into a surface");
        assert_eq!(surface.rgba_bytes().as_ptr(), source.rgba_bytes().as_ptr());
        assert_eq!(
            composed.sampling_canvas.as_rgba_bytes().as_ptr(),
            source.rgba_bytes().as_ptr()
        );
    }

    #[test]
    fn sparkleflinger_alpha_layers_respect_order() {
        let base = Rgba::new(255, 0, 0, 255);
        let overlay = Rgba::new(0, 0, 255, 255);
        let opacity = 0.25;
        let mut sparkleflinger = SparkleFlinger::new();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::alpha(ProducerFrame::Canvas(solid_canvas(overlay)), opacity),
            ],
        ));
        let reversed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(overlay))),
                CompositionLayer::alpha(ProducerFrame::Canvas(solid_canvas(base)), opacity),
            ],
        ));

        assert_eq!(
            composed.sampling_canvas.get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Normal, opacity)
        );
        assert_ne!(
            composed.sampling_canvas.get_pixel(0, 0),
            reversed.sampling_canvas.get_pixel(0, 0)
        );
    }

    #[test]
    fn sparkleflinger_add_layers_use_additive_blend() {
        let base = Rgba::new(64, 0, 0, 255);
        let glow = Rgba::new(0, 96, 64, 255);
        let mut sparkleflinger = SparkleFlinger::new();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::add(ProducerFrame::Canvas(solid_canvas(glow)), 1.0),
            ],
        ));

        assert_eq!(
            composed.sampling_canvas.get_pixel(0, 0),
            expected_blend(base, glow, BlendMode::Add, 1.0)
        );
    }

    #[test]
    fn sparkleflinger_screen_layers_use_screen_blend() {
        let base = Rgba::new(32, 64, 96, 255);
        let overlay = Rgba::new(96, 64, 32, 255);
        let mut sparkleflinger = SparkleFlinger::new();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
                CompositionLayer::screen(ProducerFrame::Canvas(solid_canvas(overlay)), 1.0),
            ],
        ));

        assert_eq!(
            composed.sampling_canvas.get_pixel(0, 0),
            expected_blend(base, overlay, BlendMode::Screen, 1.0)
        );
    }
}
