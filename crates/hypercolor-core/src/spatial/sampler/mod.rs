//! Sampling algorithms — extracting LED colors from the canvas.
//!
//! Three sampling strategies with different quality/performance tradeoffs:
//! - **Nearest**: O(1), 1 pixel read — fast but aliased.
//! - **Bilinear**: O(1), 4 pixel reads — smooth gradients, default.
//! - **Area Average**: O(1) per query after one summed-area build per canvas.
//!
//! The canvas already provides `sample_nearest`, `sample_bilinear`, and `sample_area`
//! methods. This module wraps them with the zone-level [`SamplingMode`] dispatch
//! and coordinate transformation pipeline.

mod integral;
mod lut;
mod resample;

use std::sync::Arc;

use hypercolor_types::canvas::{Canvas, Rgba, SamplingMethod};
use hypercolor_types::spatial::{
    EdgeBehavior, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};

use super::plan::{PreparedZonePlan, PreparedZoneSamples};
use super::{
    SpatialPlanError, SpatialSamplingCapacity, SpatialSamplingError, validate_canvas_descriptor,
};
#[cfg(feature = "spatial-workspace-test-hooks")]
pub use integral::SpatialWorkspaceAllocationTestHook;
pub(crate) use integral::{AreaWorkspacePool, SummedAreaWorkspace};
use resample::{
    prepare_area_sample_for_position, prepare_bilinear_sample_for_position,
    prepare_gaussian_kernel, prepare_gaussian_sample_for_position, prepare_nearest_sample,
    sample_positions_for_mode_into_buffer, sample_prepared_canvas_pixels,
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
    zone: &Output,
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
fn resolve_sampling_mode(zone: &Output, layout: &SpatialLayout) -> SamplingMode {
    zone.sampling_mode
        .clone()
        .unwrap_or_else(|| layout.default_sampling_mode.clone())
}

/// Resolve the effective edge behavior for a zone, falling back to the layout default.
fn resolve_edge_behavior(zone: &Output, layout: &SpatialLayout) -> EdgeBehavior {
    zone.edge_behavior.unwrap_or(layout.default_edge_behavior)
}

/// Convert a [`SamplingMode`] to the canvas's [`SamplingMethod`] for dispatch.
fn to_sampling_method(mode: &SamplingMode) -> SamplingMethod {
    match mode {
        SamplingMode::Nearest => SamplingMethod::Nearest,
        SamplingMode::Bilinear => SamplingMethod::Bilinear,
        SamplingMode::AreaAverage { .. } => {
            unreachable!("area sampling uses the summed-area workspace path")
        }
        SamplingMode::GaussianArea { .. } => {
            unreachable!("gaussian sampling uses the prepared kernel path")
        }
    }
}

// ── Zone preparation ───────────────────────────────────────────────────────

/// Build the immutable sampling plan for a zone.
pub(crate) fn prepare_zone(
    zone: &Output,
    layout: &SpatialLayout,
    plan_generation: u64,
) -> Result<PreparedZonePlan, SpatialPlanError> {
    let mode = resolve_sampling_mode(zone, layout);
    let edge = resolve_edge_behavior(zone, layout);
    let sample_positions = zone
        .led_positions
        .iter()
        .map(|&pos| zone_local_to_canvas(pos, zone, edge))
        .collect::<Vec<_>>();
    let (prepared_samples, has_attenuation) = match mode {
        SamplingMode::Nearest => {
            let samples = sample_positions
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
                .collect::<Result<Vec<_>, _>>()?;
            let has_attenuation = samples
                .iter()
                .any(|sample| sample.attenuation < lut::ATTENUATION_ONE);
            (PreparedZoneSamples::Nearest(samples), has_attenuation)
        }
        SamplingMode::Bilinear => {
            let samples = sample_positions
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
                .collect::<Result<Vec<_>, _>>()?;
            let has_attenuation = samples
                .iter()
                .any(|sample| sample.attenuation < lut::ATTENUATION_ONE);
            (PreparedZoneSamples::Bilinear(samples), has_attenuation)
        }
        SamplingMode::AreaAverage { radius_x, radius_y } => {
            let samples = sample_positions
                .iter()
                .copied()
                .map(|position| {
                    prepare_area_sample_for_position(
                        position,
                        edge,
                        radius_x,
                        radius_y,
                        layout.canvas_width,
                        layout.canvas_height,
                    )
                })
                .collect::<Vec<_>>();
            let has_attenuation = samples
                .iter()
                .any(|sample| sample.attenuation < lut::ATTENUATION_ONE);
            (PreparedZoneSamples::Area(samples), has_attenuation)
        }
        SamplingMode::GaussianArea { sigma, radius } => {
            let (weights, weight_sum, effective_radius) = prepare_gaussian_kernel(sigma, radius)?;
            let samples = sample_positions
                .iter()
                .copied()
                .map(|position| {
                    prepare_gaussian_sample_for_position(
                        position,
                        edge,
                        effective_radius,
                        layout.canvas_width,
                        layout.canvas_height,
                    )
                })
                .collect::<Vec<_>>();
            let has_attenuation = samples
                .iter()
                .any(|sample| sample.attenuation < lut::ATTENUATION_ONE);
            (
                PreparedZoneSamples::Gaussian(super::plan::PreparedGaussianSamples {
                    samples,
                    weights,
                    weight_sum,
                }),
                has_attenuation,
            )
        }
    };

    Ok(PreparedZonePlan {
        plan_generation,
        zone_id: zone.id.clone(),
        sampling_mode: mode,
        edge_behavior: edge,
        sample_positions,
        has_attenuation,
        prepared_canvas_width: layout.canvas_width,
        prepared_canvas_height: layout.canvas_height,
        prepared_samples,
    })
}

pub(crate) fn prepare_area_workspace_pool(
    zones: &[PreparedZonePlan],
    width: u32,
    height: u32,
    capacity: SpatialSamplingCapacity,
) -> Result<Option<Arc<AreaWorkspacePool>>, SpatialSamplingError> {
    if zones.iter().any(|zone| {
        matches!(&zone.prepared_samples, PreparedZoneSamples::Area(samples) if !samples.is_empty())
    }) {
        AreaWorkspacePool::try_new(width, height, capacity).map(Some)
    } else {
        Ok(None)
    }
}

// ── Public sampling API ────────────────────────────────────────────────────

/// Sample a prepared zone without redoing zone transform math.
#[must_use]
pub(crate) fn sample_prepared_zone(
    canvas: &Canvas,
    zone: &PreparedZonePlan,
    area_workspace: Option<&SummedAreaWorkspace>,
) -> Vec<[u8; 3]> {
    if canvas.width() == zone.prepared_canvas_width
        && canvas.height() == zone.prepared_canvas_height
    {
        return sample_prepared_canvas_pixels(
            canvas,
            &zone.prepared_samples,
            area_workspace,
            zone.has_attenuation,
        );
    }

    let mut colors = Vec::new();
    sample_positions_for_mode_into_buffer(
        canvas,
        &zone.sample_positions,
        &zone.sampling_mode,
        zone.edge_behavior,
        area_workspace,
        &mut colors,
    );
    colors
}

pub(crate) fn sample_prepared_zone_into(
    canvas: &Canvas,
    zone: &PreparedZonePlan,
    colors: &mut Vec<[u8; 3]>,
    area_workspace: Option<&SummedAreaWorkspace>,
) {
    if canvas.width() == zone.prepared_canvas_width
        && canvas.height() == zone.prepared_canvas_height
    {
        sample_prepared_canvas_pixels_into(
            canvas,
            &zone.prepared_samples,
            area_workspace,
            colors,
            zone.has_attenuation,
        );
        return;
    }

    sample_positions_for_mode_into_buffer(
        canvas,
        &zone.sample_positions,
        &zone.sampling_mode,
        zone.edge_behavior,
        area_workspace,
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
    zone: &Output,
    mode: &SamplingMode,
    edge: EdgeBehavior,
) -> Rgba {
    let canvas_pos = zone_local_to_canvas(local_pos, zone, edge);

    sample_canvas_position(canvas, canvas_pos, mode, edge)
}

/// Sample one normalized canvas position without applying zone placement.
#[must_use]
pub fn sample_canvas_position(
    canvas: &Canvas,
    canvas_pos: NormalizedPosition,
    mode: &SamplingMode,
    edge: EdgeBehavior,
) -> Rgba {
    if matches!(
        mode,
        SamplingMode::AreaAverage { .. } | SamplingMode::GaussianArea { .. }
    ) {
        let prepared_samples = match mode {
            SamplingMode::AreaAverage { radius_x, radius_y } => {
                let sample = prepare_area_sample_for_position(
                    canvas_pos,
                    edge,
                    *radius_x,
                    *radius_y,
                    canvas.width(),
                    canvas.height(),
                );
                PreparedZoneSamples::Area(vec![sample])
            }
            SamplingMode::GaussianArea { sigma, radius } => {
                let Ok((weights, weight_sum, effective_radius)) =
                    prepare_gaussian_kernel(*sigma, *radius)
                else {
                    return Rgba::new(0, 0, 0, 255);
                };
                PreparedZoneSamples::Gaussian(super::plan::PreparedGaussianSamples {
                    samples: vec![prepare_gaussian_sample_for_position(
                        canvas_pos,
                        edge,
                        effective_radius,
                        canvas.width(),
                        canvas.height(),
                    )],
                    weights,
                    weight_sum,
                })
            }
            SamplingMode::Nearest | SamplingMode::Bilinear => unreachable!(),
        };
        let workspace_pool = if matches!(prepared_samples, PreparedZoneSamples::Area(_)) {
            match AreaWorkspacePool::try_new(
                canvas.width(),
                canvas.height(),
                SpatialSamplingCapacity::UNBOUNDED,
            ) {
                Ok(pool) => Some(pool),
                Err(_) => return Rgba::new(0, 0, 0, 255),
            }
        } else {
            None
        };
        let workspace = match workspace_pool.as_ref() {
            Some(pool) => match pool.try_checkout(canvas) {
                Ok(workspace) => Some(workspace),
                Err(_) => return Rgba::new(0, 0, 0, 255),
            },
            None => None,
        };
        let mut colors = Vec::new();
        sample_prepared_canvas_pixels_into(
            canvas,
            &prepared_samples,
            workspace.as_deref(),
            &mut colors,
            false,
        );
        let [r, g, b] = colors
            .into_iter()
            .next()
            .expect("single prepared sample should produce one color");
        return Rgba::new(r, g, b, 255);
    }

    let method = to_sampling_method(mode);

    let bytes = canvas.as_rgba_bytes();
    let Some(row_stride) = usize::try_from(canvas.width())
        .ok()
        .and_then(|width| width.checked_mul(hypercolor_types::canvas::BYTES_PER_PIXEL))
    else {
        return Rgba::new(0, 0, 0, 255);
    };
    let color = sample_srgb_rgb(canvas, bytes, row_stride, canvas_pos, method, edge);
    Rgba::new(color[0], color[1], color[2], 255)
}

/// Sample every LED in a zone, returning `[u8; 3]` RGB triplets.
///
/// Each LED position from the zone's `led_positions` is transformed
/// through the zone's affine placement and sampled from the canvas.
#[must_use]
pub fn sample_zone(canvas: &Canvas, zone: &Output, layout: &SpatialLayout) -> Vec<[u8; 3]> {
    if validate_canvas_descriptor(layout.canvas_width, layout.canvas_height).is_err() {
        return Vec::new();
    }
    let Ok(prepared) = prepare_zone(zone, layout, 0) else {
        return Vec::new();
    };
    let Ok(pool) = prepare_area_workspace_pool(
        std::slice::from_ref(&prepared),
        canvas.width(),
        canvas.height(),
        SpatialSamplingCapacity::UNBOUNDED,
    ) else {
        return Vec::new();
    };
    let workspace = match pool.as_ref() {
        Some(pool) => match pool.try_checkout(canvas) {
            Ok(workspace) => Some(workspace),
            Err(_) => return Vec::new(),
        },
        None => None,
    };
    sample_prepared_zone(canvas, &prepared, workspace.as_deref())
}
