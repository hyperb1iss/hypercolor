//! Scene API contracts — `/api/v1/scenes/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::api::scene::{SceneDocument, SideEffectOutcome, ZoneLayoutResource, ZoneMember};
use crate::identity::LayoutId;
use crate::layer::{
    BlendMode, LayerAdjust, LayerBinding, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use crate::scene::{
    DisplayFaceTarget, SceneId, SceneKind, SceneMutationMode, ScenePriority, TransitionSpec,
    UnassignedBehavior, ZoneId, ZoneRole,
};

/// Response for `GET /api/v1/scenes`.
pub type SceneListResponse = ListResponse<SceneSummary>;

/// One saved scene as listed by `GET /api/v1/scenes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SceneSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the scene participates in activation. Defaults true for
    /// daemons that predate the field.
    #[serde(default = "default_scene_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: u8,
    /// Live vs snapshot-locked. Lets scene pickers mark locked scenes
    /// without inferring lock state from the live scene kind.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub mutation_mode: SceneMutationMode,
    /// Named layout the scene applies on activation, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_id: Option<LayoutId>,
    /// Brightness applied on activation, when the scene sets one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_brightness: Option<f32>,
}

/// Response for `DELETE /api/v1/scenes/{id}`.
///
/// `id` echoes the identifier the caller sent, which may be a scene name
/// rather than the resolved id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeleteSceneResponse {
    pub id: String,
    pub deleted: bool,
}

/// Response for `POST /api/v1/scenes/{id}/activate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ActivateSceneResponse {
    pub scene: ActivatedSceneRef,
    pub activated: bool,
    pub layout: SceneLayoutActivationOutcome,
    pub brightness: SideEffectOutcome,
}

/// Request for `POST /api/v1/scenes/{id}/activate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ActivateSceneRequest {
    /// Override the scene's authored transition duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u64>,
}

/// Post-commit outcome for a scene's optional named layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SceneLayoutActivationOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_id: Option<LayoutId>,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The scene an activation resolved to, by id and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ActivatedSceneRef {
    pub id: String,
    pub name: String,
}

/// Request body for `POST /api/v1/scenes`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CreateSceneRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(value_type = Option<String>))]
    pub mutation_mode: Option<SceneMutationMode>,
}

/// Request body for `POST /api/v1/scenes/snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SnapshotSceneRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Whole-document replacement body for `PUT /api/v1/scenes/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ReplaceSceneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(value_type = Option<String>))]
    pub id: Option<SceneId>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub kind: SceneKind,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub unassigned_behavior: UnassignedBehavior,
    #[serde(default)]
    pub layout_id: Option<LayoutId>,
    #[serde(default)]
    pub activation_brightness: Option<f32>,
    #[cfg_attr(feature = "schema", schema(value_type = Object))]
    pub transition: TransitionSpec,
    #[cfg_attr(feature = "schema", schema(value_type = u8))]
    pub priority: ScenePriority,
    pub enabled: bool,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub mutation_mode: SceneMutationMode,
    #[serde(default)]
    pub zones: Vec<ReplaceZoneRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ReplaceZoneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(value_type = Option<String>))]
    pub id: Option<ZoneId>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub role: ZoneRole,
    pub enabled: bool,
    pub brightness: f32,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(value_type = Option<Object>))]
    pub display_target: Option<DisplayFaceTarget>,
    #[serde(default)]
    pub members: Vec<ZoneMember>,
    #[serde(default)]
    pub layout: Option<ZoneLayoutResource>,
    #[serde(default)]
    pub layers: Vec<ReplaceSceneLayerRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ReplaceSceneLayerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<SceneLayerId>,
    #[serde(default)]
    pub name: Option<String>,
    pub source: LayerSource,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub transform: LayerTransform,
    #[serde(default)]
    pub adjust: LayerAdjust,
    #[serde(default)]
    pub bindings: Vec<LayerBinding>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl From<&SceneDocument> for ReplaceSceneRequest {
    fn from(document: &SceneDocument) -> Self {
        Self {
            id: Some(document.id),
            name: document.name.clone(),
            description: document.description.clone(),
            kind: document.kind,
            unassigned_behavior: document.unassigned_behavior.clone(),
            layout_id: document.layout_id.clone(),
            activation_brightness: document.activation_brightness,
            transition: document.transition.clone(),
            priority: document.priority,
            enabled: document.enabled,
            metadata: document.metadata.clone(),
            mutation_mode: document.mutation_mode,
            zones: document
                .zones
                .iter()
                .map(ReplaceZoneRequest::from)
                .collect(),
        }
    }
}

impl From<&crate::api::scene::ZoneResource> for ReplaceZoneRequest {
    fn from(zone: &crate::api::scene::ZoneResource) -> Self {
        Self {
            id: Some(zone.id),
            name: zone.name.clone(),
            description: zone.description.clone(),
            role: zone.role,
            enabled: zone.enabled,
            brightness: zone.brightness,
            color: zone.color.clone(),
            display_target: zone.display_target.clone(),
            members: zone.members.clone(),
            layout: zone.layout.clone(),
            layers: zone
                .layers
                .iter()
                .map(ReplaceSceneLayerRequest::from)
                .collect(),
        }
    }
}

impl From<&SceneLayer> for ReplaceSceneLayerRequest {
    fn from(layer: &SceneLayer) -> Self {
        Self {
            id: Some(layer.id),
            name: layer.name.clone(),
            source: layer.source.clone(),
            blend: layer.blend,
            opacity: layer.opacity,
            transform: layer.transform,
            adjust: layer.adjust,
            bindings: layer.bindings.clone(),
            enabled: layer.enabled,
        }
    }
}

/// Hard media-producer caps a scene is measured against.
///
/// Rides the canonical error envelope's `error.details` when a scene
/// exceeds them, beside the counts and the offending layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct MediaProducerCaps {
    /// How many distinct video assets a scene may drive at once.
    pub video: usize,
    /// How many distinct livestream assets a scene may drive at once.
    pub livestream: usize,
}

/// What a scene candidate actually asks of the media producers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct MediaProducerCounts {
    /// Distinct video assets the scene drives.
    pub video: usize,
    /// Distinct livestream assets the scene drives.
    pub livestream: usize,
}

/// One layer that contributes to a media-producer count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct MediaLayerDetail {
    /// The zone holding the layer.
    pub zone_id: String,
    /// That zone's name, so a client can name it without a second read.
    pub zone_name: String,
    /// The layer itself.
    pub layer_id: String,
    /// That layer's name, when the layer carries one.
    #[serde(default)]
    pub layer_name: Option<String>,
    /// The media asset the layer sources.
    pub asset_id: String,
    /// The asset's MIME type, which decides which cap it counts against.
    pub mime_type: String,
}

/// The layers behind each media-producer count, grouped by producer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct MediaProducerLayers {
    /// Layers counting against the video cap.
    pub video: Vec<MediaLayerDetail>,
    /// Layers counting against the livestream cap.
    pub livestream: Vec<MediaLayerDetail>,
}

const fn default_layer_opacity() -> f32 {
    1.0
}

const fn default_true() -> bool {
    true
}

const fn default_scene_enabled() -> bool {
    true
}
