use std::collections::HashSet;
use std::sync::Arc;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::asset::AssetId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::Zone;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

pub(super) fn zone_is_active(zone: &Zone) -> bool {
    enabled_layer_count(zone) > 0
}

pub(super) fn zone_contributes_to_scene_canvas(zone: &Zone) -> bool {
    zone_is_active(zone) && zone.display_target.is_none()
}

pub(super) fn zone_publishes_direct_canvas(zone: &Zone) -> bool {
    zone.enabled && zone.display_target.is_some() && enabled_layer_count(zone) > 0
}

pub(super) fn enabled_layer_count(zone: &Zone) -> u32 {
    if !zone.enabled {
        return 0;
    }
    u32::try_from(zone.layers.iter().filter(|layer| layer.enabled).count()).unwrap_or(u32::MAX)
}

pub(super) fn desired_media_asset_ids(zones: &[Zone]) -> HashSet<AssetId> {
    zones
        .iter()
        .filter(|zone| zone.enabled)
        .flat_map(|zone| zone.layers.iter())
        .filter_map(|layer| match &layer.source {
            LayerSource::Media { asset_id, .. } if layer.enabled => Some(*asset_id),
            _ => None,
        })
        .collect()
}

pub(super) fn scene_logical_layer_count(zones: &[Zone]) -> u32 {
    zones
        .iter()
        .filter(|zone| zone_contributes_to_scene_canvas(zone))
        .map(enabled_layer_count)
        .fold(0_u32, u32::saturating_add)
}

pub(super) fn empty_zone_layout(width: u32, height: u32) -> SpatialLayout {
    SpatialLayout {
        id: "scene-zones".into(),
        name: "Scene Zones".into(),
        description: Some("Combined render-zone routing layout".into()),
        canvas_width: width,
        canvas_height: height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

pub(super) fn combine_led_zone_layouts(zones: &[Zone], width: u32, height: u32) -> SpatialLayout {
    let mut layout = empty_zone_layout(width, height);
    let zone_count = zones
        .iter()
        .filter(|zone| zone_contributes_to_scene_canvas(zone))
        .map(|zone| zone.layout.zones.len())
        .sum();
    let mut spatial_zones = Vec::with_capacity(zone_count);
    for zone in zones
        .iter()
        .filter(|zone| zone_contributes_to_scene_canvas(zone))
    {
        spatial_zones.extend_from_slice(&zone.layout.zones);
    }
    layout.zones = spatial_zones;
    layout
}

pub(super) fn combined_led_state(
    layout: SpatialLayout,
) -> Result<(Arc<SpatialLayout>, SpatialEngine), hypercolor_core::spatial::SpatialPlanError> {
    let engine = SpatialEngine::try_new(layout)?;
    Ok((engine.layout(), engine))
}
