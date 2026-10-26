//! Scene API client for the live `/api/v1/scene` tree and saved scenes.

pub use hypercolor_types::api::scene::{SceneDocument, ZoneResource};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::SceneMutationMode;
use hypercolor_types::spatial::Output;

use super::http_transport::HttpMethod;
use super::{ApiError, ApiResult, client};

pub use hypercolor_types::api::scenes::{
    ActivateSceneRequest, CreateSceneRequest, ReplaceSceneRequest, SceneListResponse, SceneSummary,
    SnapshotSceneRequest,
};

/// The selected topmost effect layer inside one canonical zone resource.
#[derive(Debug, Clone, Copy)]
pub struct ZoneEffectRef<'a> {
    pub effect_id: EffectId,
    pub controls: &'a std::collections::HashMap<String, ControlValue>,
    pub preset_id: Option<PresetId>,
}

/// Read the active scene, waiting no longer than
/// [`client::SCENE_READ_DEADLINE`], so a stalled daemon cannot hold up
/// whatever waits on the read.
pub async fn fetch_active_scene() -> ApiResult<SceneDocument> {
    client::fetch_json_within("/api/v1/scene", client::SCENE_READ_DEADLINE).await
}

pub async fn deactivate_scene() -> ApiResult<()> {
    client::post_empty("/api/v1/scene/deactivate").await
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
pub async fn list_scenes() -> ApiResult<Vec<SceneSummary>> {
    client::fetch_json::<SceneListResponse>("/api/v1/scenes")
        .await
        .map(|response| response.items)
}

/// Create a scene. The daemon seeds it with a primary zone (§5.2).
pub async fn create_scene(name: &str) -> ApiResult<SceneSummary> {
    let request = CreateSceneRequest {
        name: name.to_owned(),
        ..CreateSceneRequest::default()
    };
    client::post_json("/api/v1/scenes", &request).await
}

/// Save the running scene as a new scene through the snapshot route,
/// which copies the live zones, layers, and layout. The daemon stores the
/// copy snapshot-locked, and a locked scene refuses every runtime edit, so
/// a duplicate meant for editing follows this with [`unlock_scene`].
pub async fn snapshot_active_scene(name: &str) -> ApiResult<SceneSummary> {
    let request = SnapshotSceneRequest {
        name: name.to_owned(),
        description: None,
    };
    client::post_json("/api/v1/scenes/snapshot", &request).await
}

/// Switch a stored scene to live so it accepts runtime edits.
pub async fn unlock_scene(scene_id: &str) -> ApiResult<()> {
    replace_stored_scene(scene_id, |document| {
        document.mutation_mode = SceneMutationMode::Live;
    })
    .await
}

/// Rename a scene through the guarded whole-document replacement route.
pub async fn rename_scene(scene_id: &str, name: &str) -> ApiResult<()> {
    replace_stored_scene(scene_id, |document| name.clone_into(&mut document.name)).await
}

/// Read one stored scene, edit it, and write it back at the revision it
/// was read at, so a concurrent change is refused rather than clobbered.
async fn replace_stored_scene(
    scene_id: &str,
    edit: impl FnOnce(&mut SceneDocument),
) -> ApiResult<()> {
    let url = format!("/api/v1/scenes/{scene_id}");
    let mut document = client::fetch_json::<SceneDocument>(&url).await?;
    edit(&mut document);
    let request = ReplaceSceneRequest::from(&document);
    match client::send_json_versioned::<_, SceneDocument>(
        HttpMethod::Put,
        &url,
        Some(&request),
        Some(document.revision),
    )
    .await?
    {
        client::MutationOutcome::Applied(_) => Ok(()),
        client::MutationOutcome::Stale { current } => Err(ApiError::precondition_failed(format!(
            "Scene changed while it was being saved; reload revision {current}"
        ))),
    }
}

/// Delete a scene.
pub async fn delete_scene(scene_id: &str) -> ApiResult<()> {
    client::delete_empty(&format!("/api/v1/scenes/{scene_id}")).await
}

/// Activate a scene, making it the one the render loop composes.
pub async fn activate_scene(scene_id: &str) -> ApiResult<()> {
    client::post_json_discard(
        &format!("/api/v1/scenes/{scene_id}/activate"),
        &ActivateSceneRequest::default(),
    )
    .await
}
