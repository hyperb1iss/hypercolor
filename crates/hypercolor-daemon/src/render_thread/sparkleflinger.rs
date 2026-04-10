use std::array;
use std::sync::LazyLock;

use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, linear_to_srgb_u8, srgb_u8_to_linear,
};

use super::producer_queue::ProducerFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompositionMode {
    Replace,
    Alpha,
    Add,
    Screen,
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

    pub(crate) fn from_parts(frame: ProducerFrame, mode: CompositionMode, opacity: f32) -> Self {
        Self {
            frame,
            mode,
            opacity,
        }
    }

    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Alpha,
            opacity,
        }
    }

    pub(crate) fn add(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Add,
            opacity,
        }
    }

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

const LINEAR_ENCODE_LUT_SCALE: f32 = 65_535.0;
const LINEAR_ENCODE_LUT_LAST_INDEX: usize = 65_535;

static SRGB_TO_LINEAR_LUT: LazyLock<[f32; 256]> = LazyLock::new(|| {
    array::from_fn(|index| {
        let channel = u8::try_from(index).expect("LUT index must fit in u8");
        srgb_u8_to_linear(channel)
    })
});
static LINEAR_TO_SRGB_LUT: LazyLock<Vec<u8>> = LazyLock::new(|| {
    (0..=LINEAR_ENCODE_LUT_LAST_INDEX)
        .map(|index| linear_to_srgb_u8(index as f32 / LINEAR_ENCODE_LUT_SCALE))
        .collect()
});

impl SparkleFlinger {
    pub const fn new() -> Self {
        Self
    }

    pub fn compose(&mut self, plan: CompositionPlan) -> ComposedFrameSet {
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

fn publish_composed_frame(
    frame: (Canvas, Option<PublishedSurface>),
    bypassed: bool,
) -> ComposedFrameSet {
    let (sampling_canvas, sampling_surface) = frame;
    if let Some(sampling_surface) = sampling_surface {
        return ComposedFrameSet {
            sampling_canvas,
            sampling_surface: Some(sampling_surface),
            preview_surface: None,
            bypassed,
        };
    }

    let sampling_surface = PublishedSurface::from_owned_canvas(sampling_canvas, 0, 0);
    let sampling_canvas = Canvas::from_published_surface(&sampling_surface);
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface: Some(sampling_surface),
        preview_surface: None,
        bypassed,
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

    let target_pixels = target.as_rgba_bytes_mut();
    let source_pixels = source_canvas.as_rgba_bytes();
    match layer.mode {
        CompositionMode::Replace | CompositionMode::Alpha => {
            compose_normal_layer(target_pixels, source_pixels, opacity)
        }
        CompositionMode::Add => compose_add_layer(target_pixels, source_pixels, opacity),
        CompositionMode::Screen => compose_screen_layer(target_pixels, source_pixels, opacity),
    }
}

fn compose_normal_layer(target_pixels: &mut [u8], source_pixels: &[u8], opacity: f32) {
    let fully_opaque_layer = opacity >= 1.0 - f32::EPSILON;
    let opaque_dst_weights: Option<[f32; 256]> = (!fully_opaque_layer).then(|| {
        let inverse_alpha = 1.0 - opacity;
        array::from_fn(|channel| decode_srgb_channel(channel as u8) * inverse_alpha)
    });
    let opaque_src_weights: Option<[f32; 256]> = (!fully_opaque_layer)
        .then(|| array::from_fn(|channel| decode_srgb_channel(channel as u8) * opacity));
    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_pixels.chunks_exact(4))
    {
        let source_alpha_channel = src_px[3];
        if source_alpha_channel == 0 {
            continue;
        }

        if source_alpha_channel == 255 {
            if fully_opaque_layer {
                dst_px.copy_from_slice(src_px);
                continue;
            }

            if dst_px[3] == 255 {
                let dst_weights = opaque_dst_weights
                    .as_ref()
                    .expect("non-opaque layers should precompute dst weights");
                let src_weights = opaque_src_weights
                    .as_ref()
                    .expect("non-opaque layers should precompute src weights");
                dst_px[0] = encode_srgb_channel(
                    dst_weights[usize::from(dst_px[0])] + src_weights[usize::from(src_px[0])],
                );
                dst_px[1] = encode_srgb_channel(
                    dst_weights[usize::from(dst_px[1])] + src_weights[usize::from(src_px[1])],
                );
                dst_px[2] = encode_srgb_channel(
                    dst_weights[usize::from(dst_px[2])] + src_weights[usize::from(src_px[2])],
                );
                continue;
            }
        }

        let source_alpha = alpha_weight(source_alpha_channel, opacity);
        if source_alpha <= 0.0 {
            continue;
        }

        let inverse_alpha = 1.0 - source_alpha;
        dst_px[0] = encode_srgb_channel(
            decode_srgb_channel(dst_px[0])
                .mul_add(inverse_alpha, decode_srgb_channel(src_px[0]) * source_alpha),
        );
        dst_px[1] = encode_srgb_channel(
            decode_srgb_channel(dst_px[1])
                .mul_add(inverse_alpha, decode_srgb_channel(src_px[1]) * source_alpha),
        );
        dst_px[2] = encode_srgb_channel(
            decode_srgb_channel(dst_px[2])
                .mul_add(inverse_alpha, decode_srgb_channel(src_px[2]) * source_alpha),
        );
        dst_px[3] = encode_alpha_channel(composite_alpha(dst_px[3], source_alpha));
    }
}

fn compose_add_layer(target_pixels: &mut [u8], source_pixels: &[u8], opacity: f32) {
    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_pixels.chunks_exact(4))
    {
        let source_alpha_channel = src_px[3];
        if source_alpha_channel == 0 {
            continue;
        }

        let source_alpha = alpha_weight(source_alpha_channel, opacity);
        if source_alpha <= 0.0 {
            continue;
        }

        let inverse_alpha = 1.0 - source_alpha;
        let dst_red = decode_srgb_channel(dst_px[0]);
        let dst_green = decode_srgb_channel(dst_px[1]);
        let dst_blue = decode_srgb_channel(dst_px[2]);
        let src_red = decode_srgb_channel(src_px[0]);
        let src_green = decode_srgb_channel(src_px[1]);
        let src_blue = decode_srgb_channel(src_px[2]);
        dst_px[0] = encode_srgb_channel(
            dst_red.mul_add(inverse_alpha, (dst_red + src_red).min(1.0) * source_alpha),
        );
        dst_px[1] = encode_srgb_channel(dst_green.mul_add(
            inverse_alpha,
            (dst_green + src_green).min(1.0) * source_alpha,
        ));
        dst_px[2] = encode_srgb_channel(
            dst_blue.mul_add(inverse_alpha, (dst_blue + src_blue).min(1.0) * source_alpha),
        );
        if source_alpha_channel == 255 && dst_px[3] == 255 {
            continue;
        }
        dst_px[3] = encode_alpha_channel(composite_alpha(dst_px[3], source_alpha));
    }
}

fn compose_screen_layer(target_pixels: &mut [u8], source_pixels: &[u8], opacity: f32) {
    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_pixels.chunks_exact(4))
    {
        let source_alpha_channel = src_px[3];
        if source_alpha_channel == 0 {
            continue;
        }

        let source_alpha = alpha_weight(source_alpha_channel, opacity);
        if source_alpha <= 0.0 {
            continue;
        }

        let inverse_alpha = 1.0 - source_alpha;
        let dst_red = decode_srgb_channel(dst_px[0]);
        let dst_green = decode_srgb_channel(dst_px[1]);
        let dst_blue = decode_srgb_channel(dst_px[2]);
        let src_red = decode_srgb_channel(src_px[0]);
        let src_green = decode_srgb_channel(src_px[1]);
        let src_blue = decode_srgb_channel(src_px[2]);
        dst_px[0] = encode_srgb_channel(
            dst_red.mul_add(inverse_alpha, screen_blend(dst_red, src_red) * source_alpha),
        );
        dst_px[1] = encode_srgb_channel(dst_green.mul_add(
            inverse_alpha,
            screen_blend(dst_green, src_green) * source_alpha,
        ));
        dst_px[2] = encode_srgb_channel(dst_blue.mul_add(
            inverse_alpha,
            screen_blend(dst_blue, src_blue) * source_alpha,
        ));
        if source_alpha_channel == 255 && dst_px[3] == 255 {
            continue;
        }
        dst_px[3] = encode_alpha_channel(composite_alpha(dst_px[3], source_alpha));
    }
}

fn alpha_weight(source_alpha: u8, opacity: f32) -> f32 {
    (f32::from(source_alpha) / 255.0) * opacity
}

fn composite_alpha(target_alpha: u8, source_alpha: f32) -> f32 {
    let target_alpha = f32::from(target_alpha) / 255.0;
    (target_alpha + source_alpha - target_alpha * source_alpha).min(1.0)
}

fn screen_blend(dst: f32, src: f32) -> f32 {
    1.0 - (1.0 - dst) * (1.0 - src)
}

fn decode_srgb_channel(channel: u8) -> f32 {
    SRGB_TO_LINEAR_LUT[channel as usize]
}

fn encode_srgb_channel(channel: f32) -> u8 {
    let index = (channel.clamp(0.0, 1.0) * LINEAR_ENCODE_LUT_SCALE).round() as usize;
    LINEAR_TO_SRGB_LUT[index.min(LINEAR_ENCODE_LUT_LAST_INDEX)]
}

fn encode_alpha_channel(channel: f32) -> u8 {
    (channel * 255.0).round().clamp(0.0, 255.0) as u8
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
        assert!(composed.preview_surface.is_none());
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
        let composed_surface = composed
            .sampling_surface
            .expect("composed frame should publish an immutable sampling surface");
        assert_eq!(
            composed.sampling_canvas.as_rgba_bytes().as_ptr(),
            composed_surface.rgba_bytes().as_ptr()
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

    #[test]
    fn sparkleflinger_reuses_first_replace_canvas_for_multi_layer_plans() {
        let base = solid_canvas(Rgba::new(255, 0, 0, 255));
        let base_ptr = base.as_rgba_bytes().as_ptr();
        let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
        let mut sparkleflinger = SparkleFlinger::new();
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(base)),
                CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
            ],
        ));

        assert_eq!(composed.sampling_canvas.as_rgba_bytes().as_ptr(), base_ptr);
    }
}
