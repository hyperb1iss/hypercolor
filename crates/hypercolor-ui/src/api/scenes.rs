//! Scene API client for the live `/api/v1/scene` tree and saved scenes.

use std::collections::HashMap;

pub use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::api::scene::ZoneResource;
use hypercolor_types::effect::{ControlBinding, ControlValue, EffectId};
use hypercolor_types::layer::LayerSource;
use hypercolor_types::layer::SceneLayer;
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    DisplayFaceTarget, SceneKind, SceneMutationMode, UnassignedBehavior, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};

use super::client;
use super::http_transport::HttpMethod;

pub use hypercolor_types::api::scenes::{
    CreateSceneRequest, ReplaceSceneRequest, SceneListResponse, SceneSummary,
};

#[derive(Debug, Clone, PartialEq)]
pub struct LiveSceneView {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub priority: u8,
    pub kind: SceneKind,
    pub mutation_mode: SceneMutationMode,
    pub zones: Vec<LiveZoneView>,
    pub revision: u64,
    pub unassigned_behavior: UnassignedBehavior,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveZoneView {
    pub id: ZoneId,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: Option<EffectId>,
    pub controls: HashMap<String, ControlValue>,
    pub control_bindings: HashMap<String, ControlBinding>,
    pub preset_id: Option<PresetId>,
    pub layers: Vec<SceneLayer>,
    pub layout: SpatialLayout,
    pub brightness: f32,
    pub enabled: bool,
    pub color: Option<String>,
    pub display_target: Option<DisplayFaceTarget>,
    pub role: ZoneRole,
}

pub async fn fetch_active_scene() -> Result<Option<LiveSceneView>, String> {
    let document: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let summary = list_scenes()
        .await?
        .into_iter()
        .find(|scene| scene.id == document.id.to_string());
    Ok(Some(active_scene_projection(document, summary.as_ref())))
}

pub async fn deactivate_scene() -> Result<(), String> {
    client::post_empty("/api/v1/scene/deactivate")
        .await
        .map_err(Into::into)
}

fn active_scene_projection(
    document: SceneDocument,
    summary: Option<&SceneSummary>,
) -> LiveSceneView {
    LiveSceneView {
        id: document.id.to_string(),
        name: document.name,
        description: summary.and_then(|scene| scene.description.clone()),
        enabled: summary.is_none_or(|scene| scene.enabled),
        priority: summary.map_or(0, |scene| scene.priority),
        kind: document.kind,
        mutation_mode: summary.map_or(SceneMutationMode::Live, |scene| scene.mutation_mode),
        zones: document.zones.into_iter().map(zone_projection).collect(),
        revision: document.revision,
        unassigned_behavior: document.unassigned_behavior,
    }
}

pub(crate) fn zone_projection(zone: ZoneResource) -> LiveZoneView {
    let effect = zone.layers.iter().rev().find_map(|layer| {
        let LayerSource::Effect {
            effect_id,
            controls,
            control_bindings,
            preset_id,
        } = &layer.source
        else {
            return None;
        };
        Some((
            Some(*effect_id),
            controls.clone(),
            control_bindings.clone(),
            *preset_id,
        ))
    });
    let outputs = zone.layout.as_ref().map_or_else(Vec::new, |layout| {
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
    });
    let (effect_id, controls, control_bindings, preset_id) =
        effect.unwrap_or_else(|| (None, HashMap::new(), HashMap::new(), None));
    LiveZoneView {
        id: zone.id,
        name: zone.name.clone(),
        description: None,
        effect_id,
        controls,
        control_bindings,
        preset_id,
        layers: zone.layers,
        layout: SpatialLayout {
            id: zone.id.to_string(),
            name: zone.name,
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: outputs,
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        },
        brightness: zone.brightness,
        enabled: zone.enabled,
        color: zone.color,
        display_target: zone.display_target,
        role: zone.role,
    }
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
        HttpMethod::Put,
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
    client::post_empty(&format!("/api/v1/scenes/{scene_id}/activate"))
        .await
        .map_err(Into::into)
}
