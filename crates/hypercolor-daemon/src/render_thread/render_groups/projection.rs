use std::collections::HashMap;

use hypercolor_core::spatial::sample_led;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::scene::{Zone, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};

use super::super::producer_queue::ProducerFrame;
use super::super::sparkleflinger::CompositionLayer;
use super::group_contributes_to_scene_canvas;

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
    pub(super) samples: Vec<ProjectionSample>,
}

#[derive(Clone, Copy)]
pub(super) struct ProjectionSample {
    x: u32,
    y: u32,
    local_position: NormalizedPosition,
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
            for sample in &zone_projection.samples {
                scene_canvas.set_pixel(
                    sample.x,
                    sample.y,
                    sample_led(
                        source,
                        sample.local_position,
                        &zone_projection.zone,
                        &zone_projection.sampling_mode,
                        zone_projection.edge_behavior,
                    ),
                );
            }
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

fn projection_supports_composition(projection: &CachedGroupProjection) -> bool {
    full_scene_identity_projection_shape(projection)
}

pub(super) fn projection_composition_layers_for_group(
    frame: &ProducerFrame,
    group: &Zone,
    projection: &CachedGroupProjection,
    scene_width: u32,
    scene_height: u32,
) -> Option<Vec<CompositionLayer>> {
    if projection.scene_width != scene_width
        || projection.scene_height != scene_height
        || projection.layout != group.layout
        || !projection_supports_composition(projection)
    {
        return None;
    }

    Some(vec![CompositionLayer::replace_opaque(frame.clone())])
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
    let expected_samples = u64::from(projection.scene_width) * u64::from(projection.scene_height);
    u64::try_from(zone_projection.samples.len()) == Ok(expected_samples)
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
) -> CachedGroupProjection {
    CachedGroupProjection {
        scene_width,
        scene_height,
        layout: group.layout.clone(),
        zones: group
            .layout
            .zones
            .iter()
            .map(|zone| build_zone_projection(zone, &group.layout, scene_width, scene_height))
            .collect(),
    }
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
    let Some((x0, y0, x1, y1)) = zone_projection_bounds(zone, target_width, target_height) else {
        return CachedZoneProjection {
            zone: zone.clone(),
            sampling_mode,
            edge_behavior,
            samples: Vec::new(),
        };
    };

    let mut samples = Vec::with_capacity(
        usize::try_from(u64::from(x1 - x0) * u64::from(y1 - y0)).unwrap_or(usize::MAX),
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let Some(local_position) =
                zone_local_position_for_scene_pixel(x, y, target_width, target_height, zone)
            else {
                continue;
            };
            samples.push(ProjectionSample {
                x,
                y,
                local_position,
            });
        }
    }

    CachedZoneProjection {
        zone: zone.clone(),
        sampling_mode,
        edge_behavior,
        samples,
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
    for sample in projection.samples {
        target.set_pixel(
            sample.x,
            sample.y,
            sample_led(
                source,
                sample.local_position,
                &projection.zone,
                &projection.sampling_mode,
                projection.edge_behavior,
            ),
        );
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
) -> Option<(u32, u32, u32, u32)> {
    let span_x = zone.size.x * zone.scale;
    let span_y = zone.size.y * zone.scale;
    if span_x <= 0.0 || span_y <= 0.0 || target_width == 0 || target_height == 0 {
        return None;
    }

    let half_x = span_x * 0.5;
    let half_y = span_y * 0.5;
    let cos_t = zone.rotation.cos();
    let sin_t = zone.rotation.sin();
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

    Some((x0, y0, x1, y1))
}

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "scene pixel centers are rasterized into normalized scene coordinates"
)]
pub(super) fn zone_local_position_for_scene_pixel(
    x: u32,
    y: u32,
    target_width: u32,
    target_height: u32,
    zone: &Output,
) -> Option<NormalizedPosition> {
    if target_width == 0 || target_height == 0 {
        return None;
    }

    let span_x = zone.size.x * zone.scale;
    let span_y = zone.size.y * zone.scale;
    if span_x <= 0.0 || span_y <= 0.0 {
        return None;
    }

    let scene_x = (x as f32 + 0.5) / target_width as f32;
    let scene_y = (y as f32 + 0.5) / target_height as f32;
    let delta_x = scene_x - zone.position.x;
    let delta_y = scene_y - zone.position.y;
    let cos_t = zone.rotation.cos();
    let sin_t = zone.rotation.sin();
    let local_x = (delta_x.mul_add(cos_t, delta_y * sin_t) / span_x) + 0.5;
    let local_y = (delta_y.mul_add(cos_t, -delta_x * sin_t) / span_y) + 0.5;
    if !(0.0..=1.0).contains(&local_x) || !(0.0..=1.0).contains(&local_y) {
        return None;
    }

    Some(NormalizedPosition::new(local_x, local_y))
}
