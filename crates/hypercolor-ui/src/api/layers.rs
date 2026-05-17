//! Scene layer-stack API client.

use serde::{Deserialize, Serialize};

use hypercolor_types::layer::{
    LayerAdjust, LayerBinding, LayerBlendMode, LayerSource, LayerTransform, SceneLayer,
    SceneLayerId,
};

use super::client;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LayerStackResponse {
    pub items: Vec<SceneLayer>,
    pub layers_version: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateLayerRequest {
    pub name: Option<String>,
    pub source: LayerSource,
    pub blend: LayerBlendMode,
    pub opacity: f32,
    pub transform: LayerTransform,
    pub adjust: LayerAdjust,
    pub bindings: Vec<LayerBinding>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateLayerRequest {
    pub id: SceneLayerId,
    pub name: Option<String>,
    pub source: LayerSource,
    pub blend: LayerBlendMode,
    pub opacity: f32,
    pub transform: LayerTransform,
    pub adjust: LayerAdjust,
    pub bindings: Vec<LayerBinding>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerOrderRequest {
    pub layer_ids: Vec<SceneLayerId>,
}

impl From<&SceneLayer> for UpdateLayerRequest {
    fn from(layer: &SceneLayer) -> Self {
        Self {
            id: layer.id,
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

pub async fn list_layers(scene_id: &str, group_id: &str) -> Result<LayerStackResponse, String> {
    client::fetch_json(&format!(
        "/api/v1/scenes/{scene_id}/groups/{group_id}/layers"
    ))
    .await
    .map_err(Into::into)
}

pub async fn create_layer(
    scene_id: &str,
    group_id: &str,
    request: &CreateLayerRequest,
) -> Result<LayerStackResponse, String> {
    client::post_json(
        &format!("/api/v1/scenes/{scene_id}/groups/{group_id}/layers"),
        request,
    )
    .await
    .map_err(Into::into)
}

pub async fn update_layer(
    scene_id: &str,
    group_id: &str,
    layer_id: &str,
    request: &UpdateLayerRequest,
) -> Result<LayerStackResponse, String> {
    client::put_json(
        &format!("/api/v1/scenes/{scene_id}/groups/{group_id}/layers/{layer_id}"),
        request,
    )
    .await
    .map_err(Into::into)
}

pub async fn delete_layer(
    scene_id: &str,
    group_id: &str,
    layer_id: &str,
) -> Result<LayerStackResponse, String> {
    client::delete_json(&format!(
        "/api/v1/scenes/{scene_id}/groups/{group_id}/layers/{layer_id}"
    ))
    .await
    .map_err(Into::into)
}

pub async fn reorder_layers(
    scene_id: &str,
    group_id: &str,
    layer_ids: Vec<SceneLayerId>,
) -> Result<LayerStackResponse, String> {
    client::patch_json(
        &format!("/api/v1/scenes/{scene_id}/groups/{group_id}/layers/order"),
        &LayerOrderRequest { layer_ids },
    )
    .await
    .map_err(Into::into)
}
