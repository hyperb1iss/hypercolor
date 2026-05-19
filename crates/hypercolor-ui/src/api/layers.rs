//! Scene layer-stack API client.

use gloo_net::http::{Request, RequestBuilder};
use serde::{Deserialize, Serialize};

use hypercolor_types::layer::{
    LayerAdjust, LayerBinding, LayerBlendMode, LayerSource, LayerTransform, SceneLayer,
    SceneLayerId,
};

use super::{ApiEnvelope, client};

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

/// Outcome of a layer mutation guarded by an `If-Match` precondition.
///
/// The daemon honors `If-Match: "<layers_version>"` on every layer route
/// (see `hypercolor-daemon/src/api/layers.rs`): it applies the mutation
/// only when the version still matches, otherwise replies `412` with the
/// authoritative `current` version. Modeled as a real type — not an HTTP
/// string match — so callers can drive a clean refetch path off `Stale`,
/// mirroring `effects::UpdateControlsOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerStackOutcome {
    /// The mutation applied; carries the fresh stack and its new version.
    Applied(LayerStackResponse),
    /// The `If-Match` precondition failed. `current` is the daemon's
    /// authoritative `layers_version` to rebase on before retrying.
    Stale { current: u64 },
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
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let body = serde_json::to_string(request).map_err(|error| error.to_string())?;
    send_layer_mutation(
        Request::post(&format!(
            "/api/v1/scenes/{scene_id}/groups/{group_id}/layers"
        )),
        Some(body),
        expected_version,
    )
    .await
}

pub async fn update_layer(
    scene_id: &str,
    group_id: &str,
    layer_id: &str,
    request: &UpdateLayerRequest,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let body = serde_json::to_string(request).map_err(|error| error.to_string())?;
    send_layer_mutation(
        Request::put(&format!(
            "/api/v1/scenes/{scene_id}/groups/{group_id}/layers/{layer_id}"
        )),
        Some(body),
        expected_version,
    )
    .await
}

pub async fn delete_layer(
    scene_id: &str,
    group_id: &str,
    layer_id: &str,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    send_layer_mutation(
        Request::delete(&format!(
            "/api/v1/scenes/{scene_id}/groups/{group_id}/layers/{layer_id}"
        )),
        None,
        expected_version,
    )
    .await
}

/// Patch the effect controls of one layer. `controls` is a JSON object of
/// `{ control_id: raw_value }` pairs — the daemon normalizes each against
/// the effect's control schema and merges them into the layer's stored
/// controls, so a partial payload is fine. Guarded by `If-Match`.
pub async fn patch_layer_controls(
    scene_id: &str,
    group_id: &str,
    layer_id: &str,
    controls: &serde_json::Value,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let body = serde_json::to_string(&serde_json::json!({ "controls": controls }))
        .map_err(|error| error.to_string())?;
    send_layer_mutation(
        Request::patch(&format!(
            "/api/v1/scenes/{scene_id}/groups/{group_id}/layers/{layer_id}/controls"
        )),
        Some(body),
        expected_version,
    )
    .await
}

pub async fn reorder_layers(
    scene_id: &str,
    group_id: &str,
    layer_ids: Vec<SceneLayerId>,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let body = serde_json::to_string(&LayerOrderRequest { layer_ids })
        .map_err(|error| error.to_string())?;
    send_layer_mutation(
        Request::patch(&format!(
            "/api/v1/scenes/{scene_id}/groups/{group_id}/layers/order"
        )),
        Some(body),
        expected_version,
    )
    .await
}

/// Issue one layer mutation, attaching the `If-Match` precondition when a
/// version is supplied and classifying a `412` reply as [`LayerStackOutcome::Stale`].
///
/// Hand-rolls the request rather than using `client::post_json` and friends
/// because those have no header hook; `client::with_auth` is still applied so
/// the daemon's network API key requirement is honored.
async fn send_layer_mutation(
    builder: RequestBuilder,
    body: Option<String>,
    expected_version: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let mut builder = client::with_auth(builder);
    if let Some(version) = expected_version {
        builder = builder.header("If-Match", &version.to_string());
    }

    let response = match body {
        Some(body) => builder
            .header("Content-Type", "application/json")
            .body(body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?,
        None => builder.send().await.map_err(|error| error.to_string())?,
    };

    match response.status() {
        200..=299 => {
            let envelope: ApiEnvelope<LayerStackResponse> =
                response.json().await.map_err(|error| error.to_string())?;
            Ok(LayerStackOutcome::Applied(envelope.data))
        }
        412 => {
            let body: serde_json::Value =
                response.json().await.map_err(|error| error.to_string())?;
            let current = body["current"]
                .as_u64()
                .ok_or_else(|| "412 response missing `current` layers_version".to_owned())?;
            Ok(LayerStackOutcome::Stale { current })
        }
        status => Err(format!("HTTP {status}")),
    }
}
