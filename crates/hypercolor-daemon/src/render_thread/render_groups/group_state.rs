use std::collections::HashSet;
use std::sync::Arc;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::asset::AssetId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::Zone;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

pub(super) fn group_is_active(group: &Zone) -> bool {
    enabled_layer_count(group) > 0
}

pub(super) fn group_contributes_to_scene_canvas(group: &Zone) -> bool {
    group_is_active(group) && group.display_target.is_none()
}

pub(super) fn group_publishes_direct_canvas(group: &Zone) -> bool {
    group.enabled && group.display_target.is_some() && enabled_layer_count(group) > 0
}

pub(super) fn enabled_layer_count(group: &Zone) -> u32 {
    if !group.enabled {
        return 0;
    }
    u32::try_from(
        group
            .effective_layers()
            .into_iter()
            .filter(|layer| layer.enabled)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

pub(super) fn desired_media_asset_ids(groups: &[Zone]) -> HashSet<AssetId> {
    groups
        .iter()
        .filter(|group| group.enabled)
        .flat_map(Zone::effective_layers)
        .filter_map(|layer| match layer.source {
            LayerSource::Media { asset_id, .. } if layer.enabled => Some(asset_id),
            _ => None,
        })
        .collect()
}

pub(super) fn scene_logical_layer_count(groups: &[Zone]) -> u32 {
    groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .map(enabled_layer_count)
        .fold(0_u32, u32::saturating_add)
}

pub(super) fn empty_group_layout(width: u32, height: u32) -> SpatialLayout {
    SpatialLayout {
        id: "scene-groups".into(),
        name: "Scene Groups".into(),
        description: Some("Combined render-group routing layout".into()),
        canvas_width: width,
        canvas_height: height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

pub(super) fn combine_led_group_layouts(groups: &[Zone], width: u32, height: u32) -> SpatialLayout {
    let mut layout = empty_group_layout(width, height);
    let zone_count = groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .map(|group| group.layout.zones.len())
        .sum();
    let mut zones = Vec::with_capacity(zone_count);
    for group in groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
    {
        zones.extend_from_slice(&group.layout.zones);
    }
    layout.zones = zones;
    layout
}

pub(super) fn combined_led_state(layout: SpatialLayout) -> (Arc<SpatialLayout>, SpatialEngine) {
    let engine = SpatialEngine::new(layout);
    (engine.layout(), engine)
}
