//! Scene API client for the live `/api/v1/scene` tree and saved scenes.

pub use hypercolor_types::api::scene::{SceneDocument, ZoneResource};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::PresetId;
use hypercolor_types::spatial::Output;

use super::client;
use gloo_net::http::Method;

pub use hypercolor_types::api::scenes::{
    ActivateSceneRequest, CreateSceneRequest, ReplaceSceneRequest, SceneListResponse, SceneSummary,
};

/// The selected topmost effect layer inside one canonical zone resource.
#[derive(Debug, Clone, Copy)]
pub struct ZoneEffectRef<'a> {
    pub effect_id: EffectId,
    pub controls: &'a std::collections::HashMap<String, ControlValue>,
    pub preset_id: Option<PresetId>,
}

pub async fn fetch_active_scene() -> Result<SceneDocument, String> {
    client::fetch_json("/api/v1/scene")
        .await
        .map_err(Into::into)
}

pub async fn deactivate_scene() -> Result<(), String> {
    client::post_empty("/api/v1/scene/deactivate")
        .await
        .map_err(Into::into)
}

#[must_use]
pub fn zone_effect(zone: &ZoneResource) -> Option<ZoneEffectRef<'_>> {
    zone.layers.iter().rev().find_map(|layer| {
        let LayerSource::Effect {
            effect_id,
            controls,
            preset_id,
            ..
        } = &layer.source
        else {
            return None;
        };
        Some(ZoneEffectRef {
            effect_id: *effect_id,
            controls,
            preset_id: *preset_id,
        })
    })
}

/// Select editor-facing outputs from canonical members and placements.
#[must_use]
pub fn zone_outputs(zone: &ZoneResource) -> Vec<Output> {
    zone.layout.as_ref().map_or_else(Vec::new, |layout| {
        layout
            .placements
            .iter()
            .enumerate()
            .filter_map(|(index, placement)| {
                let member = zone
                    .members
                    .iter()
                    .find(|member| member.id == placement.member)?;
                Some(Output {
                    id: member.id.to_string(),
                    name: member.name.clone(),
                    device_id: member.device_id.clone(),
                    zone_name: member.segment.clone(),
                    position: placement.position,
                    size: placement.size,
                    rotation: placement.rotation,
                    scale: placement.scale,
                    display_order: i32::try_from(index).unwrap_or(i32::MAX),
                    orientation: placement.orientation,
                    topology: placement.topology.clone(),
                    led_positions: Vec::new(),
                    led_mapping: None,
                    sampling_mode: None,
                    edge_behavior: None,
                    shape: None,
                    shape_preset: None,
                    attachment: None,
                    brightness: None,
                })
            })
            .collect()
    })
}

/// List every user-facing scene (the daemon omits the ephemeral default).
pub async fn list_scenes() -> Result<Vec<SceneSummary>, String> {
    client::fetch_json::<SceneListResponse>("/api/v1/scenes")
        .await
        .map(|response| response.items)
        .map_err(Into::into)
}

/// Create a scene. The daemon seeds it with a Default zone (§5.2).
pub async fn create_scene(name: &str) -> Result<SceneSummary, String> {
    let request = CreateSceneRequest {
        name: name.to_owned(),
        ..CreateSceneRequest::default()
    };
    client::post_json("/api/v1/scenes", &request)
        .await
        .map_err(Into::into)
}

/// Rename a scene through the guarded whole-document replacement route.
pub async fn rename_scene(scene_id: &str, name: &str) -> Result<(), String> {
    let url = format!("/api/v1/scenes/{scene_id}");
    let mut document = client::fetch_json::<SceneDocument>(&url).await?;
    document.name = name.to_owned();
    let request = ReplaceSceneRequest::from(&document);
    match client::send_json_versioned::<_, SceneDocument>(
        Method::PUT,
        &url,
        Some(&request),
        Some(document.revision),
    )
    .await?
    {
        client::MutationOutcome::Applied(_) => Ok(()),
        client::MutationOutcome::Stale { current } => Err(format!(
            "Scene changed while it was being renamed; reload revision {current}"
        )),
    }
}

/// Delete a scene.
pub async fn delete_scene(scene_id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/scenes/{scene_id}"))
        .await
        .map_err(Into::into)
}

/// Activate a scene, making it the one the render loop composes.
pub async fn activate_scene(scene_id: &str) -> Result<(), String> {
    client::post_json_discard(
        &format!("/api/v1/scenes/{scene_id}/activate"),
        &ActivateSceneRequest::default(),
    )
    .await
    .map_err(Into::into)
}
