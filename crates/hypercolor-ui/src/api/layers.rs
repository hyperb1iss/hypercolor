//! Live scene layer-stack API client.

use std::collections::BTreeMap;

use serde::Deserialize;

use hypercolor_types::api::scene::{
    PatchControlsRequest, ReorderLayersRequest, SceneDocument, ZoneResource,
};
use hypercolor_types::control::ControlValue;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};

use super::client::MutationOutcome;
use super::http_transport::HttpMethod;
use super::{ApiError, ApiResult, client};

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

pub async fn list_layers(zone_id: &str) -> ApiResult<LayerStackResponse> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let zone = scene
        .zones
        .into_iter()
        .find(|zone| zone.id.to_string() == zone_id)
        .ok_or_else(|| {
            ApiError::Parse(format!("Zone {zone_id} is not present in the live scene"))
        })?;
    Ok(layer_stack(zone, scene.revision))
}

pub async fn create_layer(
    zone_id: &str,
    request: &CreateLayerRequest,
    expected_revision: Option<u64>,
) -> ApiResult<LayerStackOutcome> {
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        HttpMethod::Post,
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
) -> ApiResult<LayerStackOutcome> {
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        HttpMethod::Put,
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
) -> ApiResult<LayerStackOutcome> {
    let outcome = client::send_json_versioned::<(), ZoneResource>(
        HttpMethod::Delete,
        &format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}"),
        None,
        expected_revision,
    )
    .await?;
    stack_outcome(outcome, expected_revision).await
}

/// Patch one real effect layer. Control writes are unguarded by contract;
/// a replacement fences stale writes by retiring the addressed layer id.
///
/// A key an input binding owns comes back `409 control_bound`, and the
/// daemon names the offending keys in `error.details.bound`. Writing a
/// control directly is the user taking manual command of it, so the same
/// patch is resent with those bindings cleared: removal and the new values
/// land in one atomic commit, which is exactly what `clear_bindings` exists
/// for.
pub async fn patch_layer_controls(
    zone_id: &str,
    layer_id: &str,
    controls: &std::collections::HashMap<String, ControlValue>,
) -> ApiResult<()> {
    let url = format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}/controls");
    let error = match client::patch_json_discard(&url, &control_patch_request(controls, Vec::new()))
        .await
    {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };

    let bound = error.bound_control_keys();
    if bound.is_empty() {
        return Err(error);
    }
    client::patch_json_discard(&url, &control_patch_request(controls, bound)).await
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
) -> ApiResult<LayerStackOutcome> {
    let request = ReorderLayersRequest { order: layer_ids };
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        HttpMethod::Patch,
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
) -> ApiResult<LayerStackOutcome> {
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
                        .ok_or_else(|| {
                            ApiError::Parse("The written zone left the live scene".to_owned())
                        })?;
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
