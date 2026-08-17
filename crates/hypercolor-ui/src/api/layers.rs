//! Scene layer-stack API client.

use gloo_net::http::Method;
use serde::Deserialize;

use hypercolor_types::layer::{SceneLayer, SceneLayerId};

use super::client;
use super::client::MutationOutcome;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LayerStackResponse {
    pub items: Vec<SceneLayer>,
    pub layers_version: u64,
}

pub use hypercolor_types::api::layers::{
    CreateLayerRequest, LayerOrderRequest, PatchLayerControlsRequest, UpdateLayerRequest,
};

/// Build a whole-layer replacement request that preserves every field of
/// the layer as it stands.
#[must_use]
pub fn update_request_from_layer(layer: &SceneLayer) -> UpdateLayerRequest {
    UpdateLayerRequest {
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

/// Outcome of a layer mutation guarded by an `If-Match` precondition.
///
/// The daemon honors `If-Match: "<layers_version>"` on every layer route
/// (see `hypercolor-daemon/src/api/layers.rs`): it applies the mutation
/// only when the version still matches, otherwise replies `412` with the
/// authoritative `current` `layers_version` (surfaced as `Stale`).
/// `Applied` carries the fresh stack and its new version.
pub type LayerStackOutcome = MutationOutcome<LayerStackResponse>;

pub async fn list_layers(scene_id: &str, zone_id: &str) -> Result<LayerStackResponse, String> {
    client::fetch_json(&format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layers"))
        .await
        .map_err(Into::into)
}

pub async fn create_layer(
    scene_id: &str,
    zone_id: &str,
    request: &CreateLayerRequest,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    client::send_json_versioned(
        Method::POST,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layers"),
        Some(request),
        expected_version,
    )
    .await
    .map_err(Into::into)
}

pub async fn update_layer(
    scene_id: &str,
    zone_id: &str,
    layer_id: &str,
    request: &UpdateLayerRequest,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    client::send_json_versioned(
        Method::PUT,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layers/{layer_id}"),
        Some(request),
        expected_version,
    )
    .await
    .map_err(Into::into)
}

pub async fn delete_layer(
    scene_id: &str,
    zone_id: &str,
    layer_id: &str,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    client::send_json_versioned::<(), _>(
        Method::DELETE,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layers/{layer_id}"),
        None,
        expected_version,
    )
    .await
    .map_err(Into::into)
}

/// Patch the effect controls of one layer. `controls` is a JSON object of
/// `{ control_id: raw_value }` pairs — the daemon normalizes each against
/// the effect's control schema and merges them into the layer's stored
/// controls, so a partial payload is fine. Guarded by `If-Match`.
pub async fn patch_layer_controls(
    scene_id: &str,
    zone_id: &str,
    layer_id: &str,
    controls: &serde_json::Value,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let body = PatchLayerControlsRequest {
        controls: Some(controls.clone()),
    };
    client::send_json_versioned(
        Method::PATCH,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layers/{layer_id}/controls"),
        Some(&body),
        expected_version,
    )
    .await
    .map_err(Into::into)
}

pub async fn reorder_layers(
    scene_id: &str,
    zone_id: &str,
    layer_ids: Vec<SceneLayerId>,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let request = LayerOrderRequest { layer_ids };
    client::send_json_versioned(
        Method::PATCH,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layers/order"),
        Some(&request),
        expected_version,
    )
    .await
    .map_err(Into::into)
}
