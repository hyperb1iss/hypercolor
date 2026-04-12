//! Viewport-to-canvas rendering with bilinear sampling.

use hypercolor_core::bus::CanvasFrame;
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

use super::{DisplayViewport, DisplayViewportSignature};

pub(super) const BILINEAR_WEIGHT_SCALE: u32 = 256;
const BILINEAR_WEIGHT_ROUNDING: u32 = (BILINEAR_WEIGHT_SCALE * BILINEAR_WEIGHT_SCALE) / 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreparedDisplayPlanKey {
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub edge_behavior: u8,
    pub start_x_bits: u32,
    pub start_y_bits: u32,
    pub span_x_bits: u32,
    pub span_y_bits: u32,
}

#[derive(Clone, Debug)]
pub(super) struct PreparedDisplayPlan {
    pub key: PreparedDisplayPlanKey,
    pub samples: Vec<PreparedDisplaySample>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PreparedDisplaySample {
    pub offsets: [usize; 4],
    pub corner_weights: [u32; 4],
}

#[derive(Clone, Copy)]
struct AxisSample {
    lower: usize,
    upper: usize,
    lower_weight: u16,
    upper_weight: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FastDisplayCrop {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(super) fn render_display_view(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    width: u32,
    height: u32,
    rendered_rgb: &mut Vec<u8>,
    axis_plan: &mut Option<PreparedDisplayPlan>,
    brightness_lut: Option<&[u8; 256]>,
) {
    let Some(render_len) = rgb_buffer_len(width, height) else {
        rendered_rgb.clear();
        return;
    };
    if rendered_rgb.len() != render_len {
        rendered_rgb.resize(render_len, 0);
    }

    if width == 0 || height == 0 || source.width == 0 || source.height == 0 {
        rendered_rgb.fill(0);
        return;
    }

    if viewport.rotation.abs() <= f32::EPSILON
        && !matches!(viewport.edge_behavior, EdgeBehavior::FadeToBlack { .. })
    {
        render_display_view_axis_aligned(
            source,
            viewport,
            width,
            height,
            rendered_rgb,
            axis_plan,
            brightness_lut,
        );
        return;
    }

    let width_f32 = width as f32;
    let height_f32 = height as f32;

    for y in 0..height {
        for x in 0..width {
            let local = NormalizedPosition::new(
                (x as f32 + 0.5) / width_f32,
                (y as f32 + 0.5) / height_f32,
            );
            let canvas_pos = viewport_local_to_canvas(local, viewport);
            let pixel = sample_image_bilinear(source, canvas_pos, viewport.edge_behavior);
            write_rgb_pixel(
                rendered_rgb,
                width,
                x,
                y,
                apply_display_brightness(pixel, brightness_lut),
            );
        }
    }
}

fn render_display_view_axis_aligned(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    width: u32,
    height: u32,
    rendered_rgb: &mut [u8],
    axis_plan: &mut Option<PreparedDisplayPlan>,
    brightness_lut: Option<&[u8; 256]>,
) {
    let rgba = source.rgba_bytes();
    let plan_key = prepared_display_plan_key(source, viewport, width, height);
    if axis_plan.as_ref().is_none_or(|plan| plan.key != plan_key) {
        *axis_plan = Some(prepare_display_plan(
            source, viewport, width, height, plan_key,
        ));
    }

    if let Some(plan) = axis_plan.as_ref() {
        let mut output_offset = 0usize;
        for sample in &plan.samples {
            let pixel = sample_prepared_display_rgb(rgba, sample, brightness_lut);
            rendered_rgb[output_offset] = pixel[0];
            rendered_rgb[output_offset + 1] = pixel[1];
            rendered_rgb[output_offset + 2] = pixel[2];
            output_offset += 3;
        }
    }
}

fn viewport_local_to_canvas(
    local: NormalizedPosition,
    viewport: &DisplayViewport,
) -> NormalizedPosition {
    let sx = (local.x - 0.5) * viewport.size.x * viewport.scale;
    let sy = (local.y - 0.5) * viewport.size.y * viewport.scale;

    let cos_t = viewport.rotation.cos();
    let sin_t = viewport.rotation.sin();
    let rx = sx.mul_add(cos_t, -sy * sin_t);
    let ry = sx.mul_add(sin_t, sy * cos_t);

    NormalizedPosition::new(viewport.position.x + rx, viewport.position.y + ry)
}

fn sample_image_bilinear(
    source: &CanvasFrame,
    canvas_pos: NormalizedPosition,
    edge_behavior: EdgeBehavior,
) -> [u8; 3] {
    let sample_x = apply_edge_normalized(canvas_pos.x, edge_behavior).clamp(0.0, 1.0);
    let sample_y = apply_edge_normalized(canvas_pos.y, edge_behavior).clamp(0.0, 1.0);

    let sampled = bilinear_sample(source, sample_x, sample_y);
    apply_fade_to_black(sampled, canvas_pos, edge_behavior)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "display crop boxes are defined in source-pixel space"
)]
pub(super) fn fast_display_crop(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
) -> Option<FastDisplayCrop> {
    if viewport.rotation.abs() > f32::EPSILON || viewport.edge_behavior != EdgeBehavior::Clamp {
        return None;
    }

    let span_x = viewport.size.x * viewport.scale;
    let span_y = viewport.size.y * viewport.scale;
    if span_x <= 0.0 || span_y <= 0.0 {
        return None;
    }

    let start_x = viewport.position.x - (span_x * 0.5);
    let start_y = viewport.position.y - (span_y * 0.5);
    let end_x = start_x + span_x;
    let end_y = start_y + span_y;
    if start_x < 0.0 || start_y < 0.0 || end_x > 1.0 || end_y > 1.0 {
        return None;
    }

    Some(FastDisplayCrop {
        left: f64::from(start_x) * f64::from(source.width),
        top: f64::from(start_y) * f64::from(source.height),
        width: f64::from(span_x) * f64::from(source.width),
        height: f64::from(span_y) * f64::from(source.height),
    })
}

pub(super) fn apply_edge_normalized(value: f32, edge_behavior: EdgeBehavior) -> f32 {
    match edge_behavior {
        EdgeBehavior::Clamp => value.clamp(0.0, 1.0),
        EdgeBehavior::Wrap => value.rem_euclid(1.0),
        EdgeBehavior::Mirror => {
            let period = value.rem_euclid(2.0);
            if period >= 1.0 { 2.0 - period } else { period }
        }
        EdgeBehavior::FadeToBlack { .. } => value,
    }
}

fn bilinear_sample(source: &CanvasFrame, nx: f32, ny: f32) -> [u8; 3] {
    let x_sample = axis_sample(nx, source.width);
    let y_sample = axis_sample(ny, source.height);
    bilinear_sample_rgb(source, x_sample, y_sample)
}

fn prepared_display_plan_key(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    output_width: u32,
    output_height: u32,
) -> PreparedDisplayPlanKey {
    let start_x = viewport.position.x - (viewport.size.x * viewport.scale * 0.5);
    let start_y = viewport.position.y - (viewport.size.y * viewport.scale * 0.5);
    let span_x = viewport.size.x * viewport.scale;
    let span_y = viewport.size.y * viewport.scale;

    PreparedDisplayPlanKey {
        source_width: source.width,
        source_height: source.height,
        output_width,
        output_height,
        edge_behavior: match viewport.edge_behavior {
            EdgeBehavior::Clamp => 0,
            EdgeBehavior::Wrap => 1,
            EdgeBehavior::Mirror => 2,
            EdgeBehavior::FadeToBlack { .. } => 3,
        },
        start_x_bits: start_x.to_bits(),
        start_y_bits: start_y.to_bits(),
        span_x_bits: span_x.to_bits(),
        span_y_bits: span_y.to_bits(),
    }
}

fn prepare_display_plan(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    output_width: u32,
    output_height: u32,
    key: PreparedDisplayPlanKey,
) -> PreparedDisplayPlan {
    let start_x = viewport.position.x - (viewport.size.x * viewport.scale * 0.5);
    let start_y = viewport.position.y - (viewport.size.y * viewport.scale * 0.5);
    let span_x = viewport.size.x * viewport.scale;
    let span_y = viewport.size.y * viewport.scale;
    let x_samples = precompute_axis_samples(
        output_width,
        start_x,
        span_x,
        source.width,
        viewport.edge_behavior,
    );
    let y_samples = precompute_axis_samples(
        output_height,
        start_y,
        span_y,
        source.height,
        viewport.edge_behavior,
    );
    let source_width = usize::try_from(source.width).unwrap_or_default();
    let mut samples = Vec::with_capacity(x_samples.len().saturating_mul(y_samples.len()));
    for y_sample in &y_samples {
        for x_sample in &x_samples {
            samples.push(PreparedDisplaySample {
                offsets: [
                    rgba_offset(source_width, x_sample.lower, y_sample.lower),
                    rgba_offset(source_width, x_sample.upper, y_sample.lower),
                    rgba_offset(source_width, x_sample.lower, y_sample.upper),
                    rgba_offset(source_width, x_sample.upper, y_sample.upper),
                ],
                corner_weights: [
                    u32::from(x_sample.lower_weight) * u32::from(y_sample.lower_weight),
                    u32::from(x_sample.upper_weight) * u32::from(y_sample.lower_weight),
                    u32::from(x_sample.lower_weight) * u32::from(y_sample.upper_weight),
                    u32::from(x_sample.upper_weight) * u32::from(y_sample.upper_weight),
                ],
            });
        }
    }

    PreparedDisplayPlan { key, samples }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_fade_to_black(
    pixel: [u8; 3],
    canvas_pos: NormalizedPosition,
    edge_behavior: EdgeBehavior,
) -> [u8; 3] {
    let EdgeBehavior::FadeToBlack { falloff } = edge_behavior else {
        return pixel;
    };

    let dx = if canvas_pos.x < 0.0 {
        -canvas_pos.x
    } else if canvas_pos.x > 1.0 {
        canvas_pos.x - 1.0
    } else {
        0.0
    };
    let dy = if canvas_pos.y < 0.0 {
        -canvas_pos.y
    } else if canvas_pos.y > 1.0 {
        canvas_pos.y - 1.0
    } else {
        0.0
    };

    let distance = (dx.mul_add(dx, dy * dy)).sqrt();
    if distance <= 0.0 {
        return pixel;
    }

    let attenuation = (-distance * falloff).exp().clamp(0.0, 1.0);
    [
        round_to_u8(f32::from(pixel[0]) * attenuation),
        round_to_u8(f32::from(pixel[1]) * attenuation),
        round_to_u8(f32::from(pixel[2]) * attenuation),
    ]
}

pub(super) fn apply_circular_mask(image: &mut [u8], width: u32, height: u32) {
    apply_circular_mask_with_stride(image, width, height, 3);
}

pub(super) fn apply_circular_mask_rgba(image: &mut [u8], width: u32, height: u32) {
    apply_circular_mask_with_stride(image, width, height, 4);
}

fn apply_circular_mask_with_stride(image: &mut [u8], width: u32, height: u32, stride: usize) {
    let width = i64::from(width);
    let height = i64::from(height);
    let radius = width.min(height);
    let radius_sq = radius.saturating_mul(radius);

    for y in 0..height {
        for x in 0..width {
            let dx = x.saturating_mul(2).saturating_add(1) - width;
            let dy = y.saturating_mul(2).saturating_add(1) - height;
            let distance_sq = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
            if distance_sq > radius_sq {
                let index = pixel_offset_with_stride(
                    usize::try_from(width).unwrap_or_default(),
                    usize::try_from(x).unwrap_or_default(),
                    usize::try_from(y).unwrap_or_default(),
                    stride,
                );
                image[index..index + stride].fill(0);
            }
        }
    }
}

pub(super) fn rgb_buffer_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(3)
}

pub(super) fn rgba_buffer_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)
}

#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "display resampling math operates in normalized float space before producing bounded indices"
)]
fn precompute_axis_samples(
    output_len: u32,
    start: f32,
    span: f32,
    source_len: u32,
    edge_behavior: EdgeBehavior,
) -> Vec<AxisSample> {
    let output_len_f32 = output_len.max(1) as f32;
    (0..output_len)
        .map(|index| {
            let position = start + ((index as f32 + 0.5) / output_len_f32) * span;
            let normalized = apply_edge_normalized(position, edge_behavior).clamp(0.0, 1.0);
            axis_sample(normalized, source_len)
        })
        .collect()
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "axis sampling clamps coordinates and weights into valid output ranges before narrowing"
)]
fn axis_sample(normalized: f32, source_len: u32) -> AxisSample {
    let max_index = source_len.saturating_sub(1);
    let coordinate = normalized * max_index as f32;
    let lower = coordinate as usize;
    let upper = lower
        .saturating_add(1)
        .min(usize::try_from(max_index).unwrap_or_default());
    let upper_weight = (((coordinate - lower as f32) * BILINEAR_WEIGHT_SCALE as f32) + 0.5)
        .clamp(0.0, BILINEAR_WEIGHT_SCALE as f32) as u16;
    let lower_weight = u16::try_from(BILINEAR_WEIGHT_SCALE).unwrap_or(u16::MAX) - upper_weight;
    AxisSample {
        lower,
        upper,
        lower_weight,
        upper_weight,
    }
}

fn bilinear_sample_rgb(
    source: &CanvasFrame,
    x_sample: AxisSample,
    y_sample: AxisSample,
) -> [u8; 3] {
    bilinear_sample_rgba(
        source.rgba_bytes(),
        usize::try_from(source.width).unwrap_or_default(),
        x_sample,
        y_sample,
    )
}

fn bilinear_sample_rgba(
    rgba: &[u8],
    source_width: usize,
    x_sample: AxisSample,
    y_sample: AxisSample,
) -> [u8; 3] {
    let top_left = rgba_offset(source_width, x_sample.lower, y_sample.lower);
    let top_right = rgba_offset(source_width, x_sample.upper, y_sample.lower);
    let bottom_left = rgba_offset(source_width, x_sample.lower, y_sample.upper);
    let bottom_right = rgba_offset(source_width, x_sample.upper, y_sample.upper);

    [
        bilinear_channel(
            rgba[top_left],
            rgba[top_right],
            rgba[bottom_left],
            rgba[bottom_right],
            x_sample,
            y_sample,
        ),
        bilinear_channel(
            rgba[top_left + 1],
            rgba[top_right + 1],
            rgba[bottom_left + 1],
            rgba[bottom_right + 1],
            x_sample,
            y_sample,
        ),
        bilinear_channel(
            rgba[top_left + 2],
            rgba[top_right + 2],
            rgba[bottom_left + 2],
            rgba[bottom_right + 2],
            x_sample,
            y_sample,
        ),
    ]
}

fn sample_prepared_display_rgb(
    rgba: &[u8],
    sample: &PreparedDisplaySample,
    brightness_lut: Option<&[u8; 256]>,
) -> [u8; 3] {
    apply_display_brightness(prepared_display_rgb(rgba, sample), brightness_lut)
}

fn prepared_display_rgb(rgba: &[u8], sample: &PreparedDisplaySample) -> [u8; 3] {
    let [top_left, top_right, bottom_left, bottom_right] = sample.offsets;
    let [
        top_left_weight,
        top_right_weight,
        bottom_left_weight,
        bottom_right_weight,
    ] = sample.corner_weights;

    [
        prepared_display_channel(
            rgba[top_left],
            rgba[top_right],
            rgba[bottom_left],
            rgba[bottom_right],
            top_left_weight,
            top_right_weight,
            bottom_left_weight,
            bottom_right_weight,
        ),
        prepared_display_channel(
            rgba[top_left + 1],
            rgba[top_right + 1],
            rgba[bottom_left + 1],
            rgba[bottom_right + 1],
            top_left_weight,
            top_right_weight,
            bottom_left_weight,
            bottom_right_weight,
        ),
        prepared_display_channel(
            rgba[top_left + 2],
            rgba[top_right + 2],
            rgba[bottom_left + 2],
            rgba[bottom_right + 2],
            top_left_weight,
            top_right_weight,
            bottom_left_weight,
            bottom_right_weight,
        ),
    ]
}

fn prepared_display_channel(
    top_left: u8,
    top_right: u8,
    bottom_left: u8,
    bottom_right: u8,
    top_left_weight: u32,
    top_right_weight: u32,
    bottom_left_weight: u32,
    bottom_right_weight: u32,
) -> u8 {
    let blended = u32::from(top_left) * top_left_weight
        + u32::from(top_right) * top_right_weight
        + u32::from(bottom_left) * bottom_left_weight
        + u32::from(bottom_right) * bottom_right_weight;
    let rounded = blended.saturating_add(BILINEAR_WEIGHT_ROUNDING) >> 16;
    u8::try_from(rounded).expect("bilinear interpolation should remain within byte range")
}

fn bilinear_channel(
    top_left: u8,
    top_right: u8,
    bottom_left: u8,
    bottom_right: u8,
    x_sample: AxisSample,
    y_sample: AxisSample,
) -> u8 {
    let top = blend_channel(top_left, top_right, x_sample);
    let bottom = blend_channel(bottom_left, bottom_right, x_sample);
    let blended =
        top * u32::from(y_sample.lower_weight) + bottom * u32::from(y_sample.upper_weight);
    let rounded = blended.saturating_add(BILINEAR_WEIGHT_ROUNDING) >> 16;
    u8::try_from(rounded).expect("bilinear interpolation should remain within byte range")
}

fn blend_channel(lower: u8, upper: u8, sample: AxisSample) -> u32 {
    u32::from(lower) * u32::from(sample.lower_weight)
        + u32::from(upper) * u32::from(sample.upper_weight)
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the helper bounds finite values to the 0-255 display byte range before narrowing"
)]
fn round_to_u8(value: f32) -> u8 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= 255.0 {
        return u8::MAX;
    }

    (value + 0.5) as u8
}

fn apply_display_brightness(pixel: [u8; 3], brightness_lut: Option<&[u8; 256]>) -> [u8; 3] {
    [
        apply_display_brightness_channel(pixel[0], brightness_lut),
        apply_display_brightness_channel(pixel[1], brightness_lut),
        apply_display_brightness_channel(pixel[2], brightness_lut),
    ]
}

fn apply_display_brightness_channel(channel: u8, brightness_lut: Option<&[u8; 256]>) -> u8 {
    brightness_lut.map_or(channel, |lut| lut[usize::from(channel)])
}

fn write_rgb_pixel(image: &mut [u8], width: u32, x: u32, y: u32, pixel: [u8; 3]) {
    let offset = rgb_offset(
        usize::try_from(width).unwrap_or_default(),
        usize::try_from(x).unwrap_or_default(),
        usize::try_from(y).unwrap_or_default(),
    );
    image[offset] = pixel[0];
    image[offset + 1] = pixel[1];
    image[offset + 2] = pixel[2];
}

pub(super) fn rgba_offset(width: usize, x: usize, y: usize) -> usize {
    (y * width + x) * 4
}

fn rgb_offset(width: usize, x: usize, y: usize) -> usize {
    pixel_offset_with_stride(width, x, y, 3)
}

fn pixel_offset_with_stride(width: usize, x: usize, y: usize, stride: usize) -> usize {
    (y * width + x) * stride
}

pub(super) fn display_viewport_signature(viewport: &DisplayViewport) -> DisplayViewportSignature {
    let (edge_behavior, fade_falloff_bits) = match viewport.edge_behavior {
        EdgeBehavior::Clamp => (0, 0),
        EdgeBehavior::Wrap => (1, 0),
        EdgeBehavior::Mirror => (2, 0),
        EdgeBehavior::FadeToBlack { falloff } => (3, falloff.to_bits()),
    };

    DisplayViewportSignature {
        position_x_bits: viewport.position.x.to_bits(),
        position_y_bits: viewport.position.y.to_bits(),
        size_x_bits: viewport.size.x.to_bits(),
        size_y_bits: viewport.size.y.to_bits(),
        rotation_bits: viewport.rotation.to_bits(),
        scale_bits: viewport.scale.to_bits(),
        edge_behavior,
        fade_falloff_bits,
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_types::canvas::{Canvas, Rgba};
    use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

    use super::{DisplayViewport, FastDisplayCrop, fast_display_crop};

    fn sample_frame() -> CanvasFrame {
        let mut canvas = Canvas::new(320, 200);
        canvas.fill(Rgba::new(255, 0, 0, 255));
        CanvasFrame::from_canvas(&canvas, 1, 16)
    }

    #[test]
    fn fast_display_crop_maps_axis_aligned_clamp_viewport_to_source_pixels() {
        let frame = sample_frame();
        let viewport = DisplayViewport {
            position: NormalizedPosition::new(0.25, 0.5),
            size: NormalizedPosition::new(0.5, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: EdgeBehavior::Clamp,
        };

        assert_eq!(
            fast_display_crop(&frame, &viewport),
            Some(FastDisplayCrop {
                left: 0.0,
                top: 0.0,
                width: 160.0,
                height: 200.0,
            })
        );
    }

    #[test]
    fn fast_display_crop_rejects_non_clamp_or_out_of_bounds_viewports() {
        let frame = sample_frame();
        let rotated = DisplayViewport {
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.1,
            scale: 1.0,
            edge_behavior: EdgeBehavior::Clamp,
        };
        let wrapped = DisplayViewport {
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: EdgeBehavior::Wrap,
        };
        let out_of_bounds = DisplayViewport {
            position: NormalizedPosition::new(0.1, 0.5),
            size: NormalizedPosition::new(0.4, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: EdgeBehavior::Clamp,
        };

        assert!(fast_display_crop(&frame, &rotated).is_none());
        assert!(fast_display_crop(&frame, &wrapped).is_none());
        assert!(fast_display_crop(&frame, &out_of_bounds).is_none());
    }
}
