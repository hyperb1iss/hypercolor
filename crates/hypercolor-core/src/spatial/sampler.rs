//! Sampling algorithms — extracting LED colors from the canvas.
//!
//! Three sampling strategies with different quality/performance tradeoffs:
//! - **Nearest**: O(1), 1 pixel read — fast but aliased.
//! - **Bilinear**: O(1), 4 pixel reads — smooth gradients, default.
//! - **Area Average**: O(area), brute-force rectangle — mood/ambient lighting.
//!
//! The canvas already provides `sample_nearest`, `sample_bilinear`, and `sample_area`
//! methods. This module wraps them with the zone-level [`SamplingMode`] dispatch
//! and coordinate transformation pipeline.

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, Rgba, SamplingMethod};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, NormalizedPosition, SamplingMode, SpatialLayout,
};

const BILINEAR_ONE: u32 = 256;
const BILINEAR_SHIFT: u32 = 16;
const ATTENUATION_ONE: u16 = 256;

/// Per-zone sampling plan prepared when the layout changes.
#[derive(Debug, Clone)]
pub(crate) struct PreparedZone {
    pub(crate) zone_id: String,
    sampling_method: SamplingMethod,
    edge_behavior: EdgeBehavior,
    sample_positions: Vec<NormalizedPosition>,
    prepared_canvas_width: u32,
    prepared_canvas_height: u32,
    prepared_samples: PreparedZoneSamples,
}

#[derive(Debug, Clone)]
enum PreparedZoneSamples {
    Nearest(Vec<PreparedNearestSample>),
    Bilinear(Vec<PreparedBilinearSample>),
    Area(Vec<PreparedAreaSample>),
}

#[derive(Debug, Clone, Copy)]
struct PreparedNearestSample {
    offset: usize,
    attenuation: u16,
}

#[derive(Debug, Clone, Copy)]
struct PreparedBilinearSample {
    offsets: [u32; 4],
    weights: [u32; 4],
    attenuation: u16,
}

#[derive(Debug, Clone, Copy)]
struct PreparedAreaSample {
    center_x: i32,
    center_y: i32,
    radius: i32,
    canvas_width: i32,
    canvas_height: i32,
    attenuation: u16,
}

/// Transform a zone-local LED position to a normalized canvas position.
///
/// Applies the full affine chain: center at origin, scale by zone dimensions,
/// rotate by `zone.rotation`, then translate to `zone.position`.
///
/// The result is a position in the normalized `[0.0, 1.0]` canvas space,
/// with edge behavior applied for out-of-bounds coordinates.
#[must_use]
fn zone_local_to_canvas(
    local: NormalizedPosition,
    zone: &DeviceZone,
    edge: EdgeBehavior,
) -> NormalizedPosition {
    let s = zone.scale;

    // Step 1: Center at origin and scale to zone dimensions
    let sx = (local.x - 0.5) * zone.size.x * s;
    let sy = (local.y - 0.5) * zone.size.y * s;

    // Step 2: Rotate around zone center
    let cos_t = zone.rotation.cos();
    let sin_t = zone.rotation.sin();
    let rx = sx.mul_add(cos_t, -sy * sin_t);
    let ry = sx.mul_add(sin_t, sy * cos_t);

    // Step 3: Translate to zone position (still normalized canvas space)
    let cx = zone.position.x + rx;
    let cy = zone.position.y + ry;

    // Step 4: Apply edge behavior
    let nx = apply_edge_normalized(cx, edge);
    let ny = apply_edge_normalized(cy, edge);

    NormalizedPosition::new(nx, ny)
}

/// Apply edge behavior to a single normalized coordinate.
///
/// All math operates in `[0.0, 1.0]` normalized space — the canvas dimensions
/// are irrelevant here because `NormalizedPosition` is resolution-independent.
fn apply_edge_normalized(value: f32, edge: EdgeBehavior) -> f32 {
    match edge {
        EdgeBehavior::Clamp => value.clamp(0.0, 1.0),
        EdgeBehavior::Wrap => value.rem_euclid(1.0),
        EdgeBehavior::Mirror => {
            let p = value.rem_euclid(2.0);
            if p >= 1.0 { 2.0 - p } else { p }
        }
        // Fade-to-black leaves coordinates as-is; fading is applied post-sample.
        EdgeBehavior::FadeToBlack { .. } => value,
    }
}

/// Resolve the effective sampling mode for a zone, falling back to the layout default.
fn resolve_sampling_mode(zone: &DeviceZone, layout: &SpatialLayout) -> SamplingMode {
    zone.sampling_mode
        .clone()
        .unwrap_or_else(|| layout.default_sampling_mode.clone())
}

/// Resolve the effective edge behavior for a zone, falling back to the layout default.
fn resolve_edge_behavior(zone: &DeviceZone, layout: &SpatialLayout) -> EdgeBehavior {
    zone.edge_behavior.unwrap_or(layout.default_edge_behavior)
}

/// Convert a [`SamplingMode`] to the canvas's [`SamplingMethod`] for dispatch.
fn to_sampling_method(mode: &SamplingMode) -> SamplingMethod {
    match mode {
        SamplingMode::Nearest => SamplingMethod::Nearest,
        // Gaussian falls back to bilinear until the PrecomputedSampler / SamplingLut
        // implements full kernel sampling.
        SamplingMode::Bilinear | SamplingMode::GaussianArea { .. } => SamplingMethod::Bilinear,
        SamplingMode::AreaAverage { radius_x, .. } => SamplingMethod::Area { radius: *radius_x },
    }
}

/// Build the immutable sampling plan for a zone.
#[must_use]
pub(crate) fn prepare_zone(zone: &DeviceZone, layout: &SpatialLayout) -> PreparedZone {
    let mode = resolve_sampling_mode(zone, layout);
    let edge = resolve_edge_behavior(zone, layout);
    let sampling_method = match mode {
        SamplingMode::AreaAverage { radius_x, radius_y } => SamplingMethod::Area {
            radius: radius_x.max(radius_y),
        },
        other => to_sampling_method(&other),
    };
    let sample_positions = zone
        .led_positions
        .iter()
        .map(|&pos| zone_local_to_canvas(pos, zone, edge))
        .collect::<Vec<_>>();
    let prepared_samples = match sampling_method {
        SamplingMethod::Nearest => PreparedZoneSamples::Nearest(
            sample_positions
                .iter()
                .copied()
                .map(|position| {
                    prepare_nearest_sample(
                        position,
                        edge,
                        layout.canvas_width,
                        layout.canvas_height,
                    )
                })
                .collect(),
        ),
        SamplingMethod::Bilinear => PreparedZoneSamples::Bilinear(
            sample_positions
                .iter()
                .copied()
                .map(|position| {
                    prepare_bilinear_sample_for_position(
                        position,
                        edge,
                        layout.canvas_width,
                        layout.canvas_height,
                    )
                })
                .collect(),
        ),
        SamplingMethod::Area { radius } => PreparedZoneSamples::Area(
            sample_positions
                .iter()
                .copied()
                .map(|position| {
                    prepare_area_sample_for_position(
                        position,
                        edge,
                        radius,
                        layout.canvas_width,
                        layout.canvas_height,
                    )
                })
                .collect(),
        ),
    };

    PreparedZone {
        zone_id: zone.id.clone(),
        sampling_method,
        edge_behavior: edge,
        sample_positions,
        prepared_canvas_width: layout.canvas_width,
        prepared_canvas_height: layout.canvas_height,
        prepared_samples,
    }
}

#[must_use]
fn prepare_nearest_sample(
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
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn prepare_bilinear_sample_for_position(
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
            u32::try_from(pixel_offset(canvas_width, x0, y0))
                .expect("prepared bilinear sample offset should fit within u32"),
            u32::try_from(pixel_offset(canvas_width, x1, y0))
                .expect("prepared bilinear sample offset should fit within u32"),
            u32::try_from(pixel_offset(canvas_width, x0, y1))
                .expect("prepared bilinear sample offset should fit within u32"),
            u32::try_from(pixel_offset(canvas_width, x1, y1))
                .expect("prepared bilinear sample offset should fit within u32"),
        ],
        weights: [
            inv_frac_x * inv_frac_y,
            frac_x * inv_frac_y,
            inv_frac_x * frac_y,
            frac_x * frac_y,
        ],
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
fn prepare_area_sample_for_position(
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
#[allow(clippy::as_conversions)]
fn pixel_offset(canvas_width: u32, x: u32, y: u32) -> usize {
    ((y as usize * canvas_width as usize) + x as usize) * BYTES_PER_PIXEL
}

#[must_use]
fn sample_positions(
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

fn sample_positions_into_buffer(
    canvas: &Canvas,
    positions: &[NormalizedPosition],
    sampling_method: SamplingMethod,
    edge_behavior: EdgeBehavior,
    colors: &mut Vec<[u8; 3]>,
) {
    colors.clear();
    colors.reserve(positions.len());

    for &pos in positions {
        let color = canvas.sample(pos.x, pos.y, sampling_method);
        let color = apply_fade_to_black(color, pos, edge_behavior);
        colors.push([color.r, color.g, color.b]);
    }
}

/// Sample a prepared zone without redoing zone transform math.
#[must_use]
pub(crate) fn sample_prepared_zone(canvas: &Canvas, zone: &PreparedZone) -> Vec<[u8; 3]> {
    if canvas.width() == zone.prepared_canvas_width
        && canvas.height() == zone.prepared_canvas_height
    {
        return sample_prepared_canvas_pixels(canvas, &zone.prepared_samples);
    }

    sample_positions(
        canvas,
        &zone.sample_positions,
        zone.sampling_method,
        zone.edge_behavior,
    )
}

pub(crate) fn sample_prepared_zone_into(
    canvas: &Canvas,
    zone: &PreparedZone,
    colors: &mut Vec<[u8; 3]>,
) {
    if canvas.width() == zone.prepared_canvas_width
        && canvas.height() == zone.prepared_canvas_height
    {
        sample_prepared_canvas_pixels_into(canvas, &zone.prepared_samples, colors);
        return;
    }

    sample_positions_into_buffer(
        canvas,
        &zone.sample_positions,
        zone.sampling_method,
        zone.edge_behavior,
        colors,
    )
}

#[must_use]
fn sample_prepared_canvas_pixels(canvas: &Canvas, samples: &PreparedZoneSamples) -> Vec<[u8; 3]> {
    let bytes = canvas.as_rgba_bytes();
    let row_stride = canvas.width() as usize * BYTES_PER_PIXEL;
    match samples {
        PreparedZoneSamples::Nearest(samples) => sample_prepared_nearest_pixels(bytes, samples),
        PreparedZoneSamples::Bilinear(samples) => sample_prepared_bilinear_pixels(bytes, samples),
        PreparedZoneSamples::Area(samples) => {
            sample_prepared_area_pixels(bytes, row_stride, samples)
        }
    }
}

fn sample_prepared_canvas_pixels_into(
    canvas: &Canvas,
    samples: &PreparedZoneSamples,
    colors: &mut Vec<[u8; 3]>,
) {
    let bytes = canvas.as_rgba_bytes();
    let row_stride = canvas.width() as usize * BYTES_PER_PIXEL;
    match samples {
        PreparedZoneSamples::Nearest(samples) => {
            sample_prepared_nearest_pixels_into(bytes, samples, colors);
        }
        PreparedZoneSamples::Bilinear(samples) => {
            sample_prepared_bilinear_pixels_into(bytes, samples, colors);
        }
        PreparedZoneSamples::Area(samples) => {
            sample_prepared_area_pixels_into(bytes, row_stride, samples, colors);
        }
    }
}

#[must_use]
fn sample_prepared_nearest_pixels(bytes: &[u8], samples: &[PreparedNearestSample]) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_nearest_pixels_into(bytes, samples, &mut colors);
    colors
}

fn sample_prepared_nearest_pixels_into(
    bytes: &[u8],
    samples: &[PreparedNearestSample],
    colors: &mut Vec<[u8; 3]>,
) {
    colors.resize(samples.len(), [0, 0, 0]);
    for (color, sample) in colors.iter_mut().zip(samples) {
        *color = attenuate_rgb(read_rgb_at(bytes, sample.offset), sample.attenuation);
    }
}

#[must_use]
fn sample_prepared_bilinear_pixels(
    bytes: &[u8],
    samples: &[PreparedBilinearSample],
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_bilinear_pixels_into(bytes, samples, &mut colors);
    colors
}

fn sample_prepared_bilinear_pixels_into(
    bytes: &[u8],
    samples: &[PreparedBilinearSample],
    colors: &mut Vec<[u8; 3]>,
) {
    colors.resize(samples.len(), [0, 0, 0]);
    for (color, sample) in colors.iter_mut().zip(samples) {
        *color = attenuate_rgb(sample_bilinear_rgb(bytes, sample), sample.attenuation);
    }
}

#[must_use]
fn sample_prepared_area_pixels(
    bytes: &[u8],
    row_stride: usize,
    samples: &[PreparedAreaSample],
) -> Vec<[u8; 3]> {
    let mut colors = Vec::new();
    sample_prepared_area_pixels_into(bytes, row_stride, samples, &mut colors);
    colors
}

fn sample_prepared_area_pixels_into(
    bytes: &[u8],
    row_stride: usize,
    samples: &[PreparedAreaSample],
    colors: &mut Vec<[u8; 3]>,
) {
    colors.resize(samples.len(), [0, 0, 0]);
    for (color, sample) in colors.iter_mut().zip(samples) {
        *color = attenuate_rgb(sample_area_rgb(bytes, row_stride, sample), sample.attenuation);
    }
}

#[must_use]
fn read_rgb_at(bytes: &[u8], offset: usize) -> [u8; 3] {
    [bytes[offset], bytes[offset + 1], bytes[offset + 2]]
}

#[must_use]
#[allow(clippy::as_conversions)]
fn sample_bilinear_rgb(bytes: &[u8], sample: &PreparedBilinearSample) -> [u8; 3] {
    let [top_left, top_right, bottom_left, bottom_right] = sample.offsets;
    let [top_left_weight, top_right_weight, bottom_left_weight, bottom_right_weight] =
        sample.weights;
    let top_left = top_left as usize;
    let top_right = top_right as usize;
    let bottom_left = bottom_left as usize;
    let bottom_right = bottom_right as usize;

    let channel = |index: usize| {
        let blended = u32::from(bytes[top_left + index]) * top_left_weight
            + u32::from(bytes[top_right + index]) * top_right_weight
            + u32::from(bytes[bottom_left + index]) * bottom_left_weight
            + u32::from(bytes[bottom_right + index]) * bottom_right_weight;
        (blended >> BILINEAR_SHIFT) as u8
    };

    [channel(0), channel(1), channel(2)]
}

#[must_use]
fn sample_area_rgb(bytes: &[u8], row_stride: usize, sample: &PreparedAreaSample) -> [u8; 3] {
    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;
    let mut count = 0u32;

    for dy in -sample.radius..=sample.radius {
        let y = (sample.center_y + dy).clamp(0, sample.canvas_height - 1) as usize;
        let row_offset = y * row_stride;
        for dx in -sample.radius..=sample.radius {
            let x = (sample.center_x + dx).clamp(0, sample.canvas_width - 1) as usize;
            let offset = row_offset + x * BYTES_PER_PIXEL;
            sum_r += u32::from(bytes[offset]);
            sum_g += u32::from(bytes[offset + 1]);
            sum_b += u32::from(bytes[offset + 2]);
            count += 1;
        }
    }

    [
        (sum_r / count) as u8,
        (sum_g / count) as u8,
        (sum_b / count) as u8,
    ]
}

#[must_use]
fn attenuate_rgb(color: [u8; 3], attenuation: u16) -> [u8; 3] {
    if attenuation >= ATTENUATION_ONE {
        return color;
    }

    let attenuation = u32::from(attenuation);
    [
        ((u32::from(color[0]) * attenuation + 128) / u32::from(ATTENUATION_ONE)) as u8,
        ((u32::from(color[1]) * attenuation + 128) / u32::from(ATTENUATION_ONE)) as u8,
        ((u32::from(color[2]) * attenuation + 128) / u32::from(ATTENUATION_ONE)) as u8,
    ]
}

/// Sample a single LED position from the canvas.
///
/// Transforms the zone-local position to canvas space, then delegates
/// to the canvas's built-in sampling methods.
#[must_use]
pub fn sample_led(
    canvas: &Canvas,
    local_pos: NormalizedPosition,
    zone: &DeviceZone,
    mode: &SamplingMode,
    edge: EdgeBehavior,
) -> Rgba {
    let canvas_pos = zone_local_to_canvas(local_pos, zone, edge);

    // For area average with distinct X/Y radii, use the canvas area sampler
    // with the larger radius (the canvas `sample_area` uses a square kernel).
    let method = match mode {
        SamplingMode::AreaAverage { radius_x, radius_y } => SamplingMethod::Area {
            radius: radius_x.max(*radius_y),
        },
        other => to_sampling_method(other),
    };

    let color = canvas.sample(canvas_pos.x, canvas_pos.y, method);

    // Apply fade-to-black attenuation for out-of-bounds positions.
    apply_fade_to_black(color, canvas_pos, edge)
}

/// Sample every LED in a zone, returning `[u8; 3]` RGB triplets.
///
/// Each LED position from the zone's `led_positions` is transformed
/// through the zone's affine placement and sampled from the canvas.
#[must_use]
pub fn sample_zone(canvas: &Canvas, zone: &DeviceZone, layout: &SpatialLayout) -> Vec<[u8; 3]> {
    let prepared = prepare_zone(zone, layout);
    sample_prepared_zone(canvas, &prepared)
}

/// Apply fade-to-black post-processing for positions outside `[0.0, 1.0]`.
///
/// When edge behavior is `FadeToBlack`, pixels outside the canvas bounds
/// are attenuated toward black based on how far they are from the edge.
#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_fade_to_black(color: Rgba, canvas_pos: NormalizedPosition, edge: EdgeBehavior) -> Rgba {
    let EdgeBehavior::FadeToBlack { falloff } = edge else {
        return color;
    };

    // Compute distance from the [0, 1] bounds.
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

    let distance = (dx * dx + dy * dy).sqrt();
    if distance <= 0.0 {
        return color;
    }

    // Exponential fade based on distance and falloff rate.
    let attenuation = (-distance * falloff).exp().clamp(0.0, 1.0);

    Rgba::new(
        (f32::from(color.r) * attenuation).round() as u8,
        (f32::from(color.g) * attenuation).round() as u8,
        (f32::from(color.b) * attenuation).round() as u8,
        color.a,
    )
}
