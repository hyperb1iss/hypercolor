use std::collections::{HashMap, TryReserveError};

use hypercolor_core::spatial::sample_canvas_position;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::scene::{Zone, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};
use hypercolor_types::viewport::FitMode;

use super::super::producer_queue::ProducerFrame;
use super::super::sparkleflinger::{CompositionLayer, CompositionTransform};
use super::group_state::group_contributes_to_scene_canvas;

pub(super) struct CachedGroupProjection {
    pub(super) scene_width: u32,
    pub(super) scene_height: u32,
    pub(super) layout: SpatialLayout,
    pub(super) zones: Vec<CachedZoneProjection>,
}

pub(super) struct CachedZoneProjection {
    zone: Output,
    sampling_mode: SamplingMode,
    edge_behavior: EdgeBehavior,
    pub(super) bounds: Option<ProjectionBounds>,
    affine: Option<ProjectionAffine>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProjectionRasterWork {
    pub(super) affine_setups: usize,
    pub(super) rows: u64,
    pub(super) pixels: u64,
}

#[cfg(test)]
impl CachedGroupProjection {
    pub(super) fn raster_work(&self) -> ProjectionRasterWork {
        self.zones.iter().fold(
            ProjectionRasterWork {
                affine_setups: 0,
                rows: 0,
                pixels: 0,
            },
            |work, zone| {
                let Some(bounds) = zone.bounds else {
                    return work;
                };
                ProjectionRasterWork {
                    affine_setups: work.affine_setups + usize::from(zone.affine.is_some()),
                    rows: work.rows + u64::from(bounds.y1 - bounds.y0),
                    pixels: work.pixels
                        + u64::from(bounds.x1 - bounds.x0) * u64::from(bounds.y1 - bounds.y0),
                }
            },
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ProjectionAffine {
    local_origin: [f32; 2],
    local_x_step: [f32; 2],
    local_y_step: [f32; 2],
    cos: f32,
    sin: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProjectionBounds {
    pub(super) x0: u32,
    pub(super) y0: u32,
    pub(super) x1: u32,
    pub(super) y1: u32,
}

pub(super) fn compose_authoritative_scene_canvas(
    scene_canvas: &mut Canvas,
    groups: &[Zone],
    target_canvases: &HashMap<ZoneId, Canvas>,
    scene_width: u32,
    scene_height: u32,
    scene_projection_cache: &HashMap<ZoneId, CachedGroupProjection>,
) {
    scene_canvas.clear();

    for group in groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
    {
        let Some(source) = target_canvases.get(&group.id) else {
            continue;
        };

        let Some(projection) = scene_projection_cache.get(&group.id) else {
            for zone in &group.layout.zones {
                blit_zone_projection(
                    scene_canvas,
                    source,
                    zone,
                    &group.layout,
                    scene_width,
                    scene_height,
                );
            }
            continue;
        };

        if copy_full_scene_identity_projection(scene_canvas, source, projection) {
            continue;
        }

        for zone_projection in &projection.zones {
            rasterize_zone_projection(scene_canvas, source, zone_projection);
        }
    }
}

pub(super) fn groups_support_projection_composition(
    groups: &[Zone],
    scene_projection_cache: &HashMap<ZoneId, CachedGroupProjection>,
) -> bool {
    let mut scene_group_count = 0_usize;
    for group in groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
    {
        scene_group_count = scene_group_count.saturating_add(1);
        let Some(projection) = scene_projection_cache.get(&group.id) else {
            return false;
        };
        if !projection_supports_composition(projection) {
            return false;
        }
    }
    scene_group_count > 0
}

pub(super) fn projection_supports_composition(projection: &CachedGroupProjection) -> bool {
    !projection.zones.is_empty()
        && projection.zones.iter().all(|zone| {
            zone.affine.is_some()
                && zone.sampling_mode == SamplingMode::Nearest
                && zone.edge_behavior == EdgeBehavior::Clamp
        })
}

pub(super) fn append_projection_composition_layers_for_group(
    layers: &mut Vec<CompositionLayer>,
    frame: &ProducerFrame,
    group: &Zone,
    projection: &CachedGroupProjection,
    scene_width: u32,
    scene_height: u32,
) -> bool {
    if projection.scene_width != scene_width
        || projection.scene_height != scene_height
        || projection.layout != group.layout
        || !projection_supports_composition(projection)
    {
        return false;
    }
    if layers.capacity().saturating_sub(layers.len()) < projection.zones.len() {
        return false;
    }
    for zone in &projection.zones {
        layers.push(CompositionLayer::alpha(frame.clone(), 1.0).with_transform(
            CompositionTransform {
                anchor: zone.zone.position,
                scale: [
                    zone.zone.size.x * zone.zone.scale,
                    zone.zone.size.y * zone.zone.scale,
                ],
                rotation: zone.zone.rotation,
                fit: FitMode::Stretch,
                sample_target_space: true,
            },
        ));
    }
    true
}

#[cfg(test)]
pub(super) fn projection_composition_layers_for_group(
    frame: &ProducerFrame,
    group: &Zone,
    projection: &CachedGroupProjection,
    scene_width: u32,
    scene_height: u32,
) -> Option<Vec<CompositionLayer>> {
    let mut layers = Vec::new();
    layers.try_reserve_exact(projection.zones.len()).ok()?;
    append_projection_composition_layers_for_group(
        &mut layers,
        frame,
        group,
        projection,
        scene_width,
        scene_height,
    )
    .then_some(layers)
}

pub(super) fn copy_full_scene_identity_projection(
    scene_canvas: &mut Canvas,
    source: &Canvas,
    projection: &CachedGroupProjection,
) -> bool {
    if scene_canvas.width() != source.width()
        || scene_canvas.height() != source.height()
        || !full_scene_identity_projection(source, projection)
    {
        return false;
    }

    scene_canvas
        .as_rgba_bytes_mut()
        .copy_from_slice(source.as_rgba_bytes());
    true
}

fn full_scene_identity_projection(source: &Canvas, projection: &CachedGroupProjection) -> bool {
    if projection.scene_width != source.width()
        || projection.scene_height != source.height()
        || projection.layout.canvas_width != source.width()
        || projection.layout.canvas_height != source.height()
    {
        return false;
    }

    full_scene_identity_projection_shape(projection)
}

fn full_scene_identity_projection_shape(projection: &CachedGroupProjection) -> bool {
    if projection.layout.canvas_width != projection.scene_width
        || projection.layout.canvas_height != projection.scene_height
    {
        return false;
    }

    let [zone_projection] = projection.zones.as_slice() else {
        return false;
    };
    if zone_projection.sampling_mode != SamplingMode::Nearest {
        return false;
    }
    if !zone_is_full_scene_identity(&zone_projection.zone) {
        return false;
    }
    matches!(
        zone_projection.bounds,
        Some(ProjectionBounds {
            x0: 0,
            y0: 0,
            x1,
            y1,
        }) if x1 == projection.scene_width && y1 == projection.scene_height
    )
}

fn zone_is_full_scene_identity(zone: &Output) -> bool {
    zone.position == NormalizedPosition::new(0.5, 0.5)
        && zone.size == NormalizedPosition::new(1.0, 1.0)
        && (zone.scale - 1.0).abs() <= f32::EPSILON
        && zone.rotation.abs() <= f32::EPSILON
}

pub(super) fn build_group_projection(
    group: &Zone,
    scene_width: u32,
    scene_height: u32,
) -> Result<CachedGroupProjection, TryReserveError> {
    let mut zones = Vec::new();
    zones.try_reserve_exact(group.layout.zones.len())?;
    zones.extend(
        group
            .layout
            .zones
            .iter()
            .map(|zone| build_zone_projection(zone, &group.layout, scene_width, scene_height)),
    );
    Ok(CachedGroupProjection {
        scene_width,
        scene_height,
        layout: group.layout.clone(),
        zones,
    })
}

fn build_zone_projection(
    zone: &Output,
    layout: &SpatialLayout,
    target_width: u32,
    target_height: u32,
) -> CachedZoneProjection {
    let sampling_mode = zone
        .sampling_mode
        .clone()
        .unwrap_or_else(|| layout.default_sampling_mode.clone());
    let edge_behavior = zone.edge_behavior.unwrap_or(layout.default_edge_behavior);
    let affine = ProjectionAffine::new(zone, target_width, target_height);
    CachedZoneProjection {
        zone: zone.clone(),
        sampling_mode,
        edge_behavior,
        bounds: affine
            .and_then(|affine| zone_projection_bounds(zone, target_width, target_height, affine)),
        affine,
    }
}

pub(super) fn blit_zone_projection(
    target: &mut Canvas,
    source: &Canvas,
    zone: &Output,
    layout: &SpatialLayout,
    target_width: u32,
    target_height: u32,
) {
    let projection = build_zone_projection(zone, layout, target_width, target_height);
    rasterize_zone_projection(target, source, &projection);
}

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "scene raster coordinates are represented as normalized f32 values"
)]
fn rasterize_zone_projection(
    target: &mut Canvas,
    source: &Canvas,
    projection: &CachedZoneProjection,
) {
    let Some(bounds) = projection.bounds else {
        return;
    };
    let Some(affine) = projection.affine else {
        return;
    };

    let source_x_step = 1.0 / target.width() as f32;
    let mut source_y = (bounds.y0 as f32 + 0.5) / target.height() as f32;

    for y in bounds.y0..bounds.y1 {
        let mut local = affine.local_position(bounds.x0, y);
        let mut source_x = (bounds.x0 as f32 + 0.5) / target.width() as f32;
        for x in bounds.x0..bounds.x1 {
            if (0.0..=1.0).contains(&local.x) && (0.0..=1.0).contains(&local.y) {
                target.set_pixel(
                    x,
                    y,
                    sample_canvas_position(
                        source,
                        NormalizedPosition::new(source_x, source_y),
                        &projection.sampling_mode,
                        projection.edge_behavior,
                    ),
                );
            }
            local.x += affine.local_x_step[0];
            local.y += affine.local_x_step[1];
            source_x += source_x_step;
        }
        source_y += 1.0 / target.height() as f32;
    }
}

impl ProjectionAffine {
    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "scene raster coordinates are represented as normalized f32 values"
    )]
    fn new(zone: &Output, target_width: u32, target_height: u32) -> Option<Self> {
        let span_x = zone.size.x * zone.scale;
        let span_y = zone.size.y * zone.scale;
        if target_width == 0
            || target_height == 0
            || !span_x.is_finite()
            || !span_y.is_finite()
            || span_x <= 0.0
            || span_y <= 0.0
            || !zone.position.x.is_finite()
            || !zone.position.y.is_finite()
            || !zone.rotation.is_finite()
        {
            return None;
        }

        let (sin, cos) = zone.rotation.sin_cos();
        let inverse_width = 1.0 / target_width as f32;
        let inverse_height = 1.0 / target_height as f32;
        let delta_x = 0.5f32.mul_add(inverse_width, -zone.position.x);
        let delta_y = 0.5f32.mul_add(inverse_height, -zone.position.y);
        Some(Self {
            local_origin: [
                delta_x.mul_add(cos, delta_y * sin) / span_x + 0.5,
                delta_y.mul_add(cos, -delta_x * sin) / span_y + 0.5,
            ],
            local_x_step: [cos * inverse_width / span_x, -sin * inverse_width / span_y],
            local_y_step: [sin * inverse_height / span_x, cos * inverse_height / span_y],
            cos,
            sin,
        })
    }

    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "bounded scene raster coordinates are represented as normalized f32 values"
    )]
    fn local_position(self, x: u32, y: u32) -> NormalizedPosition {
        NormalizedPosition::new(
            (x as f32).mul_add(
                self.local_x_step[0],
                (y as f32).mul_add(self.local_y_step[0], self.local_origin[0]),
            ),
            (x as f32).mul_add(
                self.local_x_step[1],
                (y as f32).mul_add(self.local_y_step[1], self.local_origin[1]),
            ),
        )
    }
}

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "zone projection rasterizes bounded normalized geometry into scene pixels"
)]
fn zone_projection_bounds(
    zone: &Output,
    target_width: u32,
    target_height: u32,
    affine: ProjectionAffine,
) -> Option<ProjectionBounds> {
    let span_x = zone.size.x * zone.scale;
    let span_y = zone.size.y * zone.scale;
    if span_x <= 0.0 || span_y <= 0.0 || target_width == 0 || target_height == 0 {
        return None;
    }

    let half_x = span_x * 0.5;
    let half_y = span_y * 0.5;
    let cos_t = affine.cos;
    let sin_t = affine.sin;
    let corners = [
        (-half_x, -half_y),
        (half_x, -half_y),
        (half_x, half_y),
        (-half_x, half_y),
    ];
    let (mut start_x, mut end_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut start_y, mut end_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for (corner_x, corner_y) in corners {
        let rotated_x = corner_x.mul_add(cos_t, -corner_y * sin_t);
        let rotated_y = corner_x.mul_add(sin_t, corner_y * cos_t);
        let scene_x = zone.position.x + rotated_x;
        let scene_y = zone.position.y + rotated_y;
        start_x = start_x.min(scene_x);
        end_x = end_x.max(scene_x);
        start_y = start_y.min(scene_y);
        end_y = end_y.max(scene_y);
    }

    start_x = start_x.clamp(0.0, 1.0);
    start_y = start_y.clamp(0.0, 1.0);
    end_x = end_x.clamp(0.0, 1.0);
    end_y = end_y.clamp(0.0, 1.0);
    if end_x <= start_x || end_y <= start_y {
        return None;
    }

    let x0 = ((start_x * target_width as f32).floor() as u32).min(target_width);
    let x1 = ((end_x * target_width as f32).ceil() as u32).min(target_width);
    let y0 = ((start_y * target_height as f32).floor() as u32).min(target_height);
    let y1 = ((end_y * target_height as f32).ceil() as u32).min(target_height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    Some(ProjectionBounds { x0, y0, x1, y1 })
}

#[cfg(test)]
pub(super) fn zone_local_position_for_scene_pixel(
    x: u32,
    y: u32,
    target_width: u32,
    target_height: u32,
    zone: &Output,
) -> Option<NormalizedPosition> {
    let local = ProjectionAffine::new(zone, target_width, target_height)?.local_position(x, y);
    if !(0.0..=1.0).contains(&local.x) || !(0.0..=1.0).contains(&local.y) {
        return None;
    }

    Some(local)
}
