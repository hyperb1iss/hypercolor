//! Live scene layer-stack API client.

use std::collections::BTreeMap;

use gloo_net::http::Method;
use serde::Deserialize;

use hypercolor_types::api::scene::{
    PatchControlsRequest, ReorderLayersRequest, SceneDocument, ZoneResource,
};
use hypercolor_types::control::ControlValue;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};

use super::client;
use super::client::MutationOutcome;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LayerStackResponse {
    pub items: Vec<SceneLayer>,
    pub revision: u64,
}

pub use hypercolor_types::api::scene::{CreateLayerRequest, ReplaceLayerRequest};

/// Build a whole-layer replacement request from the fields the canonical
/// resource accepts. Replacement mints a new layer identity.
#[must_use]
pub fn update_request_from_layer(layer: &SceneLayer) -> ReplaceLayerRequest {
    ReplaceLayerRequest {
        name: layer.name.clone(),
        source: layer.source.clone(),
        blend: Some(layer.blend),
        opacity: Some(layer.opacity),
        transform: Some(layer.transform),
        adjust: Some(layer.adjust),
        bindings: Some(layer.bindings.clone()),
        enabled: Some(layer.enabled),
    }
}

pub type LayerStackOutcome = MutationOutcome<LayerStackResponse>;

pub async fn list_layers(zone_id: &str) -> Result<LayerStackResponse, String> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let zone = scene
        .zones
        .into_iter()
        .find(|zone| zone.id.to_string() == zone_id)
        .ok_or_else(|| format!("Zone {zone_id} is not present in the live scene"))?;
    Ok(layer_stack(zone, scene.revision))
}

pub async fn create_layer(
    zone_id: &str,
    request: &CreateLayerRequest,
    expected_revision: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        Method::POST,
        &format!("/api/v1/scene/zones/{zone_id}/layers"),
        Some(request),
        expected_revision,
    )
    .await?;
    stack_outcome(outcome, expected_revision).await
}

pub async fn update_layer(
    zone_id: &str,
    layer_id: &str,
    request: &ReplaceLayerRequest,
    expected_revision: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        Method::PUT,
        &format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}"),
        Some(request),
        expected_revision,
    )
    .await?;
    stack_outcome(outcome, expected_revision).await
}

pub async fn delete_layer(
    zone_id: &str,
    layer_id: &str,
    expected_revision: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let outcome = client::send_json_versioned::<(), ZoneResource>(
        Method::DELETE,
        &format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}"),
        None,
        expected_revision,
    )
    .await?;
    stack_outcome(outcome, expected_revision).await
}

/// Patch one real effect layer. Control writes are unguarded by contract;
/// a replacement fences stale writes by retiring the addressed layer id.
pub async fn patch_layer_controls(
    zone_id: &str,
    layer_id: &str,
    controls: &std::collections::HashMap<String, ControlValue>,
) -> Result<(), String> {
    client::patch_json_discard(
        &format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls"),
        &control_patch_request(controls, Vec::new()),
    )
    .await
    .map_err(Into::into)
}

#[must_use]
pub fn control_patch_request(
    controls: &std::collections::HashMap<String, ControlValue>,
    clear_bindings: Vec<String>,
) -> PatchControlsRequest {
    PatchControlsRequest {
        values: controls
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        clear_bindings,
    }
}

pub async fn reorder_layers(
    zone_id: &str,
    layer_ids: Vec<SceneLayerId>,
    expected_revision: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    let request = ReorderLayersRequest { order: layer_ids };
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        Method::PATCH,
        &format!("/api/v1/scene/zones/{zone_id}/layers/order"),
        Some(&request),
        expected_revision,
    )
    .await?;
    stack_outcome(outcome, expected_revision).await
}

async fn stack_outcome(
    outcome: MutationOutcome<ZoneResource>,
    expected_revision: Option<u64>,
) -> Result<LayerStackOutcome, String> {
    match outcome {
        MutationOutcome::Applied(zone) => {
            let stack = match expected_revision {
                Some(revision) => layer_stack(zone, revision.saturating_add(1)),
                None => {
                    let zone_id = zone.id;
                    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
                    let current = scene
                        .zones
                        .into_iter()
                        .find(|candidate| candidate.id == zone_id)
                        .ok_or_else(|| "The written zone left the live scene".to_owned())?;
                    layer_stack(current, scene.revision)
                }
            };
            Ok(MutationOutcome::Applied(stack))
        }
        MutationOutcome::Stale { current } => Ok(MutationOutcome::Stale { current }),
    }
}

fn layer_stack(zone: ZoneResource, revision: u64) -> LayerStackResponse {
    LayerStackResponse {
        items: zone.layers,
        revision,
    }
}
