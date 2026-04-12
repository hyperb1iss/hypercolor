//! Pixel resampling primitives — nearest, bilinear, and area-average sampling.
//!
//! All interpolation is performed in linear light using the LUT from [`super::lut`].
//! The prepared sample structs pre-compute byte offsets and weights at layout time
//! so that the per-frame hot path is pure arithmetic on the canvas byte buffer.

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, SamplingMethod};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

use super::super::plan::{
    PreparedAreaSample, PreparedBilinearSample, PreparedNearestSample, PreparedZoneSamples,
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
    let inv_frac_x = BILINEAR_ONE - frac_x;
    let inv_frac_y = BILINEAR_ONE - frac_y;

    PreparedBilinearSample {
        offsets: [
            pixel_offset(canvas_width, x0, y0),
            pixel_offset(canvas_width, x1, y0),
            pixel_offset(canvas_width, x0, y1),
            pixel_offset(canvas_width, x1, y1),
        ],
        x_lower_weight: u16::try_from(inv_frac_x).expect("bilinear x lower weight must fit in u16"),
        x_upper_weight: u16::try_from(frac_x).expect("bilinear x upper weight must fit in u16"),
        y_lower_weight: u16::try_from(inv_frac_y).expect("bilinear y lower weight must fit in u16"),
        y_upper_weight: u16::try_from(frac_y).expect("bilinear y upper weight must fit in u16"),
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
            sample_prepared_area_pixels_into(
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

#[must_use]
pub(super) fn sample_positions(
    canvas: &Canvas,
    positions: &[NormalizedPosition],
    sampling_method: SamplingMethod,
    edge_behavior: EdgeBehavior,
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_positions_into_buffer(
        canvas,
        positions,
        sampling_method,
        edge_behavior,
        &mut colors,
    );
    colors
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
            attenuate_rgb(read_linear_rgb_at(bytes, sample.offset), sample.attenuation)
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
) -> usize {
    let x = (position.x * (canvas_width - 1) as f32).round() as u32;
    let y = (position.y * (canvas_height - 1) as f32).round() as u32;
    pixel_offset(
        canvas_width,
        x.min(canvas_width - 1),
        y.min(canvas_height - 1),
    )
}

#[must_use]
#[allow(clippy::as_conversions)]
fn pixel_offset(canvas_width: u32, x: u32, y: u32) -> usize {
    ((y as usize * canvas_width as usize) + x as usize) * BYTES_PER_PIXEL
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
fn sample_bilinear_linear_rgb(bytes: &[u8], sample: &PreparedBilinearSample) -> [u16; 3] {
    let [top_left, top_right, bottom_left, bottom_right] = sample.offsets;
    let x_lower_weight = u32::from(sample.x_lower_weight);
    let x_upper_weight = u32::from(sample.x_upper_weight);
    let y_lower_weight = u64::from(sample.y_lower_weight);
    let y_upper_weight = u64::from(sample.y_upper_weight);

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
    let top =
        u32::from(top_left) * x_lower_weight + u32::from(top_right) * x_upper_weight;
    let bottom =
        u32::from(bottom_left) * x_lower_weight + u32::from(bottom_right) * x_upper_weight;
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
                read_linear_rgb_at(bytes, sample.offset),
                sample.attenuation,
            ));
        }
    } else {
        for (color, sample) in colors.iter_mut().zip(samples) {
            *color = encode_linear_rgb(read_linear_rgb_at(bytes, sample.offset));
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
#[inline]
fn sample_prepared_bilinear_srgb_rgb(bytes: &[u8], sample: &PreparedBilinearSample) -> [u8; 3] {
    encode_linear_rgb(attenuate_rgb(
        sample_bilinear_linear_rgb(bytes, sample),
        sample.attenuation,
    ))
}
