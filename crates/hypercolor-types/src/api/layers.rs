//! Scene layer API contracts — `/api/v1/scenes/{id}/zones/*/layers/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::asset::AssetId;
use crate::layer::{
    LayerAdjust, LayerBinding, LayerBlendMode, LayerSource, LayerTransform, MediaPlayback,
    SceneLayer, SceneLayerId, default_layer_opacity, default_true,
};
use crate::scene::ZoneId;

/// Query parameters for
/// `POST /api/v1/scenes/{id}/zones/{zone_id}/layers`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLayerQuery {
    /// Stack position for the new layer; omitted appends on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// Request body for
/// `POST /api/v1/scenes/{id}/zones/{zone_id}/layers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateLayerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[schema(value_type = Object)]
    pub source: LayerSource,
    #[serde(default)]
    #[schema(value_type = String)]
    pub blend: LayerBlendMode,
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub transform: LayerTransform,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub adjust: LayerAdjust,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub bindings: Vec<LayerBinding>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Request body for
/// `PUT /api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}`.
///
/// The whole layer is replaced, so every field the caller wants to keep
/// must be echoed back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateLayerRequest {
    #[schema(value_type = String)]
    pub id: SceneLayerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[schema(value_type = Object)]
    pub source: LayerSource,
    #[serde(default)]
    #[schema(value_type = String)]
    pub blend: LayerBlendMode,
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub transform: LayerTransform,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub adjust: LayerAdjust,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub bindings: Vec<LayerBinding>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl CreateLayerRequest {
    /// Build the scene layer this request describes under a fresh id.
    #[must_use]
    pub fn into_layer(self, id: SceneLayerId) -> SceneLayer {
        SceneLayer {
            id,
            name: self.name,
            source: self.source,
            blend: self.blend,
            opacity: self.opacity,
            transform: self.transform,
            adjust: self.adjust,
            bindings: self.bindings,
            enabled: self.enabled,
        }
    }
}

impl UpdateLayerRequest {
    /// Build the replacement scene layer this request describes.
    #[must_use]
    pub fn into_layer(self) -> SceneLayer {
        SceneLayer {
            id: self.id,
            name: self.name,
            source: self.source,
            blend: self.blend,
            opacity: self.opacity,
            transform: self.transform,
            adjust: self.adjust,
            bindings: self.bindings,
            enabled: self.enabled,
        }
    }
}

/// Request body for
/// `PUT /api/v1/scenes/{id}/zones/{zone_id}/layers/order`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LayerOrderRequest {
    /// The zone's layers, bottom to top.
    #[schema(value_type = Vec<String>)]
    pub layer_ids: Vec<SceneLayerId>,
}

/// Request body for
/// `PATCH /api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}/controls`.
/// `controls` carries no `#[serde(default)]` on purpose: the schema this
/// route publishes marks it required, and serde still admits an absent
/// field through `Option`'s own default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PatchLayerControlsRequest {
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
}

/// One zone targeted by a broadcast media layer create.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BroadcastMediaLayerTarget {
    #[schema(value_type = String)]
    pub zone_id: ZoneId,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub transform: LayerTransform,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub adjust: LayerAdjust,
    /// Stack position within this zone; omitted appends on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// Per-zone optimistic-concurrency precondition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_layers_version: Option<u64>,
}

/// Request body for `POST /api/v1/scenes/{id}/layers/broadcast-media`.
///
/// Creates one media layer per target zone in a single transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BroadcastMediaLayerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[schema(value_type = String)]
    pub asset_id: AssetId,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub playback: MediaPlayback,
    #[serde(default)]
    #[schema(value_type = String)]
    pub blend: LayerBlendMode,
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub bindings: Vec<LayerBinding>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub targets: Vec<BroadcastMediaLayerTarget>,
}
