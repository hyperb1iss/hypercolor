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

mod lut;
mod resample;

use hypercolor_types::canvas::{Canvas, Rgba, SamplingMethod};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, NormalizedPosition, SamplingMode, SpatialLayout,
};

use super::plan::{PreparedZonePlan, PreparedZoneSamples};
use resample::{
    prepare_area_sample_for_position, prepare_bilinear_sample_for_position, prepare_nearest_sample,
    sample_positions, sample_positions_into_buffer, sample_prepared_canvas_pixels,
    sample_prepared_canvas_pixels_into, sample_srgb_rgb,
};

// ── Coordinate transforms ──────────────────────────────────────────────────

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

// ── Resolution helpers ─────────────────────────────────────────────────────

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

// ── Zone preparation ───────────────────────────────────────────────────────

/// Build the immutable sampling plan for a zone.
#[must_use]
pub(crate) fn prepare_zone(zone: &DeviceZone, layout: &SpatialLayout) -> PreparedZonePlan {
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

    PreparedZonePlan {
        zone_id: zone.id.clone(),
        sampling_method,
        edge_behavior: edge,
        sample_positions,
        prepared_canvas_width: layout.canvas_width,
        prepared_canvas_height: layout.canvas_height,
        prepared_samples,
    }
}

// ── Public sampling API ────────────────────────────────────────────────────

/// Sample a prepared zone without redoing zone transform math.
#[must_use]
pub(crate) fn sample_prepared_zone(canvas: &Canvas, zone: &PreparedZonePlan) -> Vec<[u8; 3]> {
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
    zone: &PreparedZonePlan,
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
    );
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

    let bytes = canvas.as_rgba_bytes();
    #[allow(
        clippy::as_conversions,
        reason = "canvas dimensions are already bounded by in-memory image sizes before widening to usize"
    )]
    let row_stride = canvas.width() as usize * hypercolor_types::canvas::BYTES_PER_PIXEL;
    let color = sample_srgb_rgb(canvas, bytes, row_stride, canvas_pos, method, edge);
    Rgba::new(color[0], color[1], color[2], 255)
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
