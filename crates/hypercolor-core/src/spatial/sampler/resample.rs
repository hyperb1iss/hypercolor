//! Pixel resampling primitives — nearest, bilinear, and area-average sampling.
//!
//! All interpolation is performed in linear light using the LUT from [`super::lut`].
//! The prepared sample structs pre-compute byte offsets and weights at layout time
//! so that the per-frame hot path is pure arithmetic on the canvas byte buffer.

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, SamplingMethod};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SamplingMode};

use super::super::plan::{
    PreparedAreaSample, PreparedBilinearSample, PreparedGaussianSample, PreparedGaussianSamples,
    PreparedNearestSample, PreparedZoneSamples,
};
use super::lut::{
    ATTENUATION_ONE, BILINEAR_ONE, BILINEAR_SHIFT, decode_srgb_byte, encode_linear_byte,
};

// ── Sample preparation (layout-time) ───────────────────────────────────────

#[must_use]
pub(super) fn prepare_nearest_sample(
    position: NormalizedPosition,
    edge_behavior: EdgeBehavior,
    canvas_width: u32,
    canvas_height: u32,
) -> PreparedNearestSample {
    let attenuation = attenuation_for_position(position, edge_behavior);
    let clamped = NormalizedPosition::new(position.x.clamp(0.0, 1.0), position.y.clamp(0.0, 1.0));

    PreparedNearestSample {
        offset: nearest_pixel_offset(clamped, canvas_width, canvas_height),
        attenuation,
    }
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(super) fn prepare_bilinear_sample_for_position(
    position: NormalizedPosition,
    edge_behavior: EdgeBehavior,
    canvas_width: u32,
    canvas_height: u32,
) -> PreparedBilinearSample {
    let attenuation = attenuation_for_position(position, edge_behavior);
    let clamped = NormalizedPosition::new(position.x.clamp(0.0, 1.0), position.y.clamp(0.0, 1.0));
    let fx = clamped.x * (canvas_width - 1) as f32;
    let fy = clamped.y * (canvas_height - 1) as f32;

    let x0 = fx.floor() as u32;
    let y0 = fy.floor() as u32;
    let x1 = (x0 + 1).min(canvas_width - 1);
    let y1 = (y0 + 1).min(canvas_height - 1);
    let frac_x = (fx.fract() * BILINEAR_ONE as f32).clamp(0.0, BILINEAR_ONE as f32) as u32;
    let frac_y = (fy.fract() * BILINEAR_ONE as f32).clamp(0.0, BILINEAR_ONE as f32) as u32;

    PreparedBilinearSample {
        offsets: [
            pixel_offset(canvas_width, x0, y0),
            pixel_offset(canvas_width, x1, y0),
            pixel_offset(canvas_width, x0, y1),
            pixel_offset(canvas_width, x1, y1),
        ],
        x_upper_weight: u8::try_from(frac_x).expect("bilinear x upper weight must fit in u8"),
        y_upper_weight: u8::try_from(frac_y).expect("bilinear y upper weight must fit in u8"),
        attenuation,
    }
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
pub(super) fn prepare_area_sample_for_position(
    position: NormalizedPosition,
    edge_behavior: EdgeBehavior,
    radius: f32,
    canvas_width: u32,
    canvas_height: u32,
) -> PreparedAreaSample {
    let attenuation = attenuation_for_position(position, edge_behavior);
    let clamped = NormalizedPosition::new(position.x.clamp(0.0, 1.0), position.y.clamp(0.0, 1.0));
    let cx = clamped.x * (canvas_width - 1) as f32;
    let cy = clamped.y * (canvas_height - 1) as f32;
    let radius = radius.ceil() as i32;

    PreparedAreaSample {
        center_x: cx as i32,
        center_y: cy as i32,
        radius,
        canvas_width: canvas_width as i32,
        canvas_height: canvas_height as i32,
        attenuation,
    }
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
pub(super) fn prepare_gaussian_sample_for_position(
    position: NormalizedPosition,
    edge_behavior: EdgeBehavior,
    radius: u32,
    canvas_width: u32,
    canvas_height: u32,
) -> PreparedGaussianSample {
    let attenuation = attenuation_for_position(position, edge_behavior);
    let clamped = NormalizedPosition::new(position.x.clamp(0.0, 1.0), position.y.clamp(0.0, 1.0));
    let cx = clamped.x * (canvas_width - 1) as f32;
    let cy = clamped.y * (canvas_height - 1) as f32;

    PreparedGaussianSample {
        center_x: cx as i32,
        center_y: cy as i32,
        radius: i32::try_from(radius).unwrap_or(i32::MAX),
        canvas_width: canvas_width as i32,
        canvas_height: canvas_height as i32,
        attenuation,
    }
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(super) fn prepare_gaussian_kernel(sigma: f32, radius: u32) -> (Vec<u16>, u32) {
    if radius == 0 || sigma <= f32::EPSILON {
        return (vec![u16::MAX], u32::from(u16::MAX));
    }

    let radius_i32 = i32::try_from(radius).unwrap_or(i32::MAX);
    let diameter = radius.saturating_mul(2).saturating_add(1);
    let capacity = usize::try_from(diameter.saturating_mul(diameter)).unwrap_or(usize::MAX);
    let mut weights = Vec::with_capacity(capacity);
    let sigma = f64::from(sigma.max(f32::EPSILON));
    let denominator = 2.0 * sigma * sigma;
    let mut weight_sum = 0_u32;

    for dy in -radius_i32..=radius_i32 {
        for dx in -radius_i32..=radius_i32 {
            let dx = i64::from(dx);
            let dy = i64::from(dy);
            let distance_squared = (dx * dx + dy * dy) as f64;
            let weight = (-distance_squared / denominator).exp();
            let fixed = (weight * f64::from(u16::MAX))
                .round()
                .clamp(1.0, f64::from(u16::MAX));
            let fixed = fixed as u16;
            weights.push(fixed);
            weight_sum = weight_sum.saturating_add(u32::from(fixed));
        }
    }

    (weights, weight_sum.max(1))
}

// ── Per-frame sampling (hot path) ──────────────────────────────────────────

#[must_use]
#[allow(
    clippy::as_conversions,
    reason = "canvas dimensions are already bounded by in-memory image sizes before widening to usize"
)]
pub(super) fn sample_prepared_canvas_pixels(
    canvas: &Canvas,
    samples: &PreparedZoneSamples,
    has_attenuation: bool,
) -> Vec<[u8; 3]> {
    let bytes = canvas.as_rgba_bytes();
    let row_stride = canvas.width() as usize * BYTES_PER_PIXEL;
    match samples {
        PreparedZoneSamples::Nearest(samples) => {
            sample_prepared_nearest_pixels(bytes, samples, has_attenuation)
        }
        PreparedZoneSamples::Bilinear(samples) => {
            sample_prepared_bilinear_pixels(bytes, samples, has_attenuation)
        }
        PreparedZoneSamples::Area(samples) => {
            sample_prepared_area_pixels(bytes, row_stride, samples, has_attenuation)
        }
        PreparedZoneSamples::Gaussian(samples) => {
            sample_prepared_gaussian_pixels(bytes, row_stride, samples, has_attenuation)
        }
    }
}

#[allow(
    clippy::as_conversions,
    reason = "canvas dimensions are already bounded by in-memory image sizes before widening to usize"
)]
pub(super) fn sample_prepared_canvas_pixels_into(
    canvas: &Canvas,
    samples: &PreparedZoneSamples,
    colors: &mut Vec<[u8; 3]>,
    has_attenuation: bool,
) {
    let bytes = canvas.as_rgba_bytes();
    let row_stride = canvas.width() as usize * BYTES_PER_PIXEL;
    match samples {
        PreparedZoneSamples::Nearest(samples) => {
            sample_prepared_nearest_pixels_into(bytes, samples, colors, has_attenuation);
        }
        PreparedZoneSamples::Bilinear(samples) => {
            sample_prepared_bilinear_pixels_into(bytes, samples, colors, has_attenuation);
        }
        PreparedZoneSamples::Area(samples) => {
            sample_prepared_area_pixels_into(bytes, row_stride, samples, colors, has_attenuation);
        }
        PreparedZoneSamples::Gaussian(samples) => {
            sample_prepared_gaussian_pixels_into(
                bytes,
                row_stride,
                samples,
                colors,
                has_attenuation,
            );
        }
    }
}

pub(super) fn sample_positions_into_buffer(
    canvas: &Canvas,
    positions: &[NormalizedPosition],
    sampling_method: SamplingMethod,
    edge_behavior: EdgeBehavior,
    colors: &mut Vec<[u8; 3]>,
) {
    colors.clear();
    colors.reserve(positions.len());
    let bytes = canvas.as_rgba_bytes();
    #[allow(
        clippy::as_conversions,
        reason = "canvas dimensions are already bounded by in-memory image sizes before widening to usize"
    )]
    let row_stride = canvas.width() as usize * BYTES_PER_PIXEL;

    for &pos in positions {
        colors.push(sample_srgb_rgb(
            canvas,
            bytes,
            row_stride,
            pos,
            sampling_method,
            edge_behavior,
        ));
    }
}

pub(super) fn sample_positions_for_mode_into_buffer(
    canvas: &Canvas,
    positions: &[NormalizedPosition],
    mode: &SamplingMode,
    edge_behavior: EdgeBehavior,
    colors: &mut Vec<[u8; 3]>,
) {
    let SamplingMode::GaussianArea { sigma, radius } = mode else {
        let method = match mode {
            SamplingMode::Nearest => SamplingMethod::Nearest,
            SamplingMode::Bilinear => SamplingMethod::Bilinear,
            SamplingMode::AreaAverage { radius_x, radius_y } => SamplingMethod::Area {
                radius: (*radius_x).max(*radius_y),
            },
            SamplingMode::GaussianArea { .. } => unreachable!("gaussian mode handled above"),
        };
        sample_positions_into_buffer(canvas, positions, method, edge_behavior, colors);
        return;
    };

    colors.clear();
    colors.reserve(positions.len());
    let bytes = canvas.as_rgba_bytes();
    #[allow(
        clippy::as_conversions,
        reason = "canvas dimensions are already bounded by in-memory image sizes before widening to usize"
    )]
    let row_stride = canvas.width() as usize * BYTES_PER_PIXEL;
    let (weights, weight_sum) = prepare_gaussian_kernel(*sigma, *radius);

    for &position in positions {
        let sample = prepare_gaussian_sample_for_position(
            position,
            edge_behavior,
            *radius,
            canvas.width(),
            canvas.height(),
        );
        colors.push(encode_linear_rgb(attenuate_rgb(
            sample_gaussian_linear_rgb(bytes, row_stride, &sample, &weights, weight_sum),
            sample.attenuation,
        )));
    }
}

#[must_use]
#[allow(
    clippy::as_conversions,
    reason = "canvas dimensions are already bounded by in-memory image sizes before widening to usize"
)]
pub(super) fn sample_srgb_rgb(
    canvas: &Canvas,
    bytes: &[u8],
    row_stride: usize,
    position: NormalizedPosition,
    sampling_method: SamplingMethod,
    edge_behavior: EdgeBehavior,
) -> [u8; 3] {
    let linear = match sampling_method {
        SamplingMethod::Nearest => {
            let sample =
                prepare_nearest_sample(position, edge_behavior, canvas.width(), canvas.height());
            attenuate_rgb(
                read_linear_rgb_at(bytes, prepared_offset(sample.offset)),
                sample.attenuation,
            )
        }
        SamplingMethod::Bilinear => {
            let sample = prepare_bilinear_sample_for_position(
                position,
                edge_behavior,
                canvas.width(),
                canvas.height(),
            );
            attenuate_rgb(
                sample_bilinear_linear_rgb(bytes, &sample),
                sample.attenuation,
            )
        }
        SamplingMethod::Area { radius } => {
            let sample = prepare_area_sample_for_position(
                position,
                edge_behavior,
                radius,
                canvas.width(),
                canvas.height(),
            );
            attenuate_rgb(
                sample_area_linear_rgb(bytes, row_stride, &sample),
                sample.attenuation,
            )
        }
    };

    encode_linear_rgb(linear)
}

// ── Internal helpers ───────────────────────────────────────────────────────

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the attenuation curve is clamped into the valid u16 range before narrowing"
)]
fn attenuation_for_position(position: NormalizedPosition, edge_behavior: EdgeBehavior) -> u16 {
    let EdgeBehavior::FadeToBlack { falloff } = edge_behavior else {
        return ATTENUATION_ONE;
    };

    let dx = if position.x < 0.0 {
        -position.x
    } else if position.x > 1.0 {
        position.x - 1.0
    } else {
        0.0
    };
    let dy = if position.y < 0.0 {
        -position.y
    } else if position.y > 1.0 {
        position.y - 1.0
    } else {
        0.0
    };

    let distance = (dx * dx + dy * dy).sqrt();
    if distance <= 0.0 {
        return ATTENUATION_ONE;
    }

    ((-distance * falloff).exp().clamp(0.0, 1.0) * f32::from(ATTENUATION_ONE)).round() as u16
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn nearest_pixel_offset(
    position: NormalizedPosition,
    canvas_width: u32,
    canvas_height: u32,
) -> u32 {
    let x = (position.x * (canvas_width - 1) as f32).round() as u32;
    let y = (position.y * (canvas_height - 1) as f32).round() as u32;
    pixel_offset(
        canvas_width,
        x.min(canvas_width - 1),
        y.min(canvas_height - 1),
    )
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "prepared canvas byte offsets are bounded by in-memory image sizes"
)]
fn pixel_offset(canvas_width: u32, x: u32, y: u32) -> u32 {
    ((y as usize * canvas_width as usize) + x as usize) as u32 * BYTES_PER_PIXEL as u32
}

#[must_use]
#[inline]
fn read_linear_rgb_at(bytes: &[u8], offset: usize) -> [u16; 3] {
    [
        decode_srgb_byte(bytes[offset]),
        decode_srgb_byte(bytes[offset + 1]),
        decode_srgb_byte(bytes[offset + 2]),
    ]
}

#[must_use]
#[inline]
#[allow(
    clippy::as_conversions,
    reason = "prepared sample offsets are constrained to the in-memory canvas byte range"
)]
fn prepared_offset(offset: u32) -> usize {
    offset as usize
}

#[must_use]
fn sample_bilinear_linear_rgb(bytes: &[u8], sample: &PreparedBilinearSample) -> [u16; 3] {
    let [top_left, top_right, bottom_left, bottom_right] = sample.offsets.map(prepared_offset);
    let x_upper_weight = u32::from(sample.x_upper_weight);
    let x_lower_weight = BILINEAR_ONE - x_upper_weight;
    let y_upper_weight = u64::from(sample.y_upper_weight);
    let y_lower_weight = u64::from(BILINEAR_ONE) - y_upper_weight;

    [
        bilinear_channel(
            decode_srgb_byte(bytes[top_left]),
            decode_srgb_byte(bytes[top_right]),
            decode_srgb_byte(bytes[bottom_left]),
            decode_srgb_byte(bytes[bottom_right]),
            x_lower_weight,
            x_upper_weight,
            y_lower_weight,
            y_upper_weight,
        ),
        bilinear_channel(
            decode_srgb_byte(bytes[top_left + 1]),
            decode_srgb_byte(bytes[top_right + 1]),
            decode_srgb_byte(bytes[bottom_left + 1]),
            decode_srgb_byte(bytes[bottom_right + 1]),
            x_lower_weight,
            x_upper_weight,
            y_lower_weight,
            y_upper_weight,
        ),
        bilinear_channel(
            decode_srgb_byte(bytes[top_left + 2]),
            decode_srgb_byte(bytes[top_right + 2]),
            decode_srgb_byte(bytes[bottom_left + 2]),
            decode_srgb_byte(bytes[bottom_right + 2]),
            x_lower_weight,
            x_upper_weight,
            y_lower_weight,
            y_upper_weight,
        ),
    ]
}

#[must_use]
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    reason = "bilinear interpolation stays within the 16-bit fixed-point color domain"
)]
fn bilinear_channel(
    top_left: u16,
    top_right: u16,
    bottom_left: u16,
    bottom_right: u16,
    x_lower_weight: u32,
    x_upper_weight: u32,
    y_lower_weight: u64,
    y_upper_weight: u64,
) -> u16 {
    let top = u32::from(top_left) * x_lower_weight + u32::from(top_right) * x_upper_weight;
    let bottom = u32::from(bottom_left) * x_lower_weight + u32::from(bottom_right) * x_upper_weight;
    ((u64::from(top) * y_lower_weight + u64::from(bottom) * y_upper_weight) >> BILINEAR_SHIFT)
        as u16
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "area sampling clamps coordinates and averages byte channels before narrowing"
)]
fn sample_area_linear_rgb(
    bytes: &[u8],
    row_stride: usize,
    sample: &PreparedAreaSample,
) -> [u16; 3] {
    let mut sum_r = 0u64;
    let mut sum_g = 0u64;
    let mut sum_b = 0u64;
    let mut count = 0u64;

    for dy in -sample.radius..=sample.radius {
        let y = (sample.center_y + dy).clamp(0, sample.canvas_height - 1) as usize;
        let row_offset = y * row_stride;
        for dx in -sample.radius..=sample.radius {
            let x = (sample.center_x + dx).clamp(0, sample.canvas_width - 1) as usize;
            let offset = row_offset + x * BYTES_PER_PIXEL;
            sum_r += u64::from(decode_srgb_byte(bytes[offset]));
            sum_g += u64::from(decode_srgb_byte(bytes[offset + 1]));
            sum_b += u64::from(decode_srgb_byte(bytes[offset + 2]));
            count += 1;
        }
    }

    [
        (sum_r / count) as u16,
        (sum_g / count) as u16,
        (sum_b / count) as u16,
    ]
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "gaussian sampling clamps coordinates and normalizes fixed-point weights before narrowing"
)]
fn sample_gaussian_linear_rgb(
    bytes: &[u8],
    row_stride: usize,
    sample: &PreparedGaussianSample,
    weights: &[u16],
    weight_sum: u32,
) -> [u16; 3] {
    let mut sum_r = 0u64;
    let mut sum_g = 0u64;
    let mut sum_b = 0u64;
    let mut weight_index = 0usize;

    for dy in -sample.radius..=sample.radius {
        let y = (sample.center_y + dy).clamp(0, sample.canvas_height - 1) as usize;
        let row_offset = y * row_stride;
        for dx in -sample.radius..=sample.radius {
            let weight = u64::from(weights[weight_index]);
            weight_index += 1;
            let x = (sample.center_x + dx).clamp(0, sample.canvas_width - 1) as usize;
            let offset = row_offset + x * BYTES_PER_PIXEL;
            sum_r += u64::from(decode_srgb_byte(bytes[offset])) * weight;
            sum_g += u64::from(decode_srgb_byte(bytes[offset + 1])) * weight;
            sum_b += u64::from(decode_srgb_byte(bytes[offset + 2])) * weight;
        }
    }

    let weight_sum = u64::from(weight_sum);
    [
        (sum_r / weight_sum) as u16,
        (sum_g / weight_sum) as u16,
        (sum_b / weight_sum) as u16,
    ]
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "attenuation math keeps channel values within the 0-65535 fixed-point range"
)]
fn attenuate_rgb(color: [u16; 3], attenuation: u16) -> [u16; 3] {
    if attenuation >= ATTENUATION_ONE {
        return color;
    }

    let attenuation = u32::from(attenuation);
    [
        ((u32::from(color[0]) * attenuation + 128) / u32::from(ATTENUATION_ONE)) as u16,
        ((u32::from(color[1]) * attenuation + 128) / u32::from(ATTENUATION_ONE)) as u16,
        ((u32::from(color[2]) * attenuation + 128) / u32::from(ATTENUATION_ONE)) as u16,
    ]
}

#[must_use]
fn encode_linear_rgb(color: [u16; 3]) -> [u8; 3] {
    [
        encode_linear_byte(color[0]),
        encode_linear_byte(color[1]),
        encode_linear_byte(color[2]),
    ]
}

#[must_use]
fn sample_prepared_nearest_pixels(
    bytes: &[u8],
    samples: &[PreparedNearestSample],
    has_attenuation: bool,
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_nearest_pixels_into(bytes, samples, &mut colors, has_attenuation);
    colors
}

fn sample_prepared_nearest_pixels_into(
    bytes: &[u8],
    samples: &[PreparedNearestSample],
    colors: &mut Vec<[u8; 3]>,
    has_attenuation: bool,
) {
    colors.resize(samples.len(), [0, 0, 0]);
    if has_attenuation {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = encode_linear_rgb(attenuate_rgb(
                read_linear_rgb_at(bytes, prepared_offset(sample.offset)),
                sample.attenuation,
            ));
        }
    } else {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = encode_linear_rgb(read_linear_rgb_at(bytes, prepared_offset(sample.offset)));
        }
    }
}

#[must_use]
fn sample_prepared_bilinear_pixels(
    bytes: &[u8],
    samples: &[PreparedBilinearSample],
    has_attenuation: bool,
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_bilinear_pixels_into(bytes, samples, &mut colors, has_attenuation);
    colors
}

fn sample_prepared_bilinear_pixels_into(
    bytes: &[u8],
    samples: &[PreparedBilinearSample],
    colors: &mut Vec<[u8; 3]>,
    has_attenuation: bool,
) {
    colors.resize(samples.len(), [0, 0, 0]);
    if has_attenuation {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = sample_prepared_bilinear_srgb_rgb(bytes, sample);
        }
    } else {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = encode_linear_rgb(sample_bilinear_linear_rgb(bytes, sample));
        }
    }
}

#[must_use]
fn sample_prepared_area_pixels(
    bytes: &[u8],
    row_stride: usize,
    samples: &[PreparedAreaSample],
    has_attenuation: bool,
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_area_pixels_into(bytes, row_stride, samples, &mut colors, has_attenuation);
    colors
}

fn sample_prepared_area_pixels_into(
    bytes: &[u8],
    row_stride: usize,
    samples: &[PreparedAreaSample],
    colors: &mut Vec<[u8; 3]>,
    has_attenuation: bool,
) {
    colors.resize(samples.len(), [0, 0, 0]);
    if has_attenuation {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = encode_linear_rgb(attenuate_rgb(
                sample_area_linear_rgb(bytes, row_stride, sample),
                sample.attenuation,
            ));
        }
    } else {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = encode_linear_rgb(sample_area_linear_rgb(bytes, row_stride, sample));
        }
    }
}

#[must_use]
fn sample_prepared_gaussian_pixels(
    bytes: &[u8],
    row_stride: usize,
    samples: &PreparedGaussianSamples,
    has_attenuation: bool,
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_gaussian_pixels_into(bytes, row_stride, samples, &mut colors, has_attenuation);
    colors
}

fn sample_prepared_gaussian_pixels_into(
    bytes: &[u8],
    row_stride: usize,
    samples: &PreparedGaussianSamples,
    colors: &mut Vec<[u8; 3]>,
    has_attenuation: bool,
) {
    colors.resize(samples.samples.len(), [0, 0, 0]);
    if has_attenuation {
        for (color, sample) in colors.iter_mut().zip(&samples.samples) {
            *color = encode_linear_rgb(attenuate_rgb(
                sample_gaussian_linear_rgb(
                    bytes,
                    row_stride,
                    sample,
                    &samples.weights,
                    samples.weight_sum,
                ),
                sample.attenuation,
            ));
        }
    } else {
        for (color, sample) in colors.iter_mut().zip(&samples.samples) {
            *color = encode_linear_rgb(sample_gaussian_linear_rgb(
                bytes,
                row_stride,
                sample,
                &samples.weights,
                samples.weight_sum,
            ));
        }
    }
}

#[must_use]
#[inline]
fn sample_prepared_bilinear_srgb_rgb(bytes: &[u8], sample: &PreparedBilinearSample) -> [u8; 3] {
    encode_linear_rgb(attenuate_rgb(
        sample_bilinear_linear_rgb(bytes, sample),
        sample.attenuation,
    ))
}
