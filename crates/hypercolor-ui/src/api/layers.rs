//! Live scene layer-stack API client.

use std::collections::BTreeMap;

use serde::Deserialize;

use hypercolor_types::api::scene::{
    PatchControlsRequest, ReorderLayersRequest, SceneDocument, ZoneResource,
};
use hypercolor_types::control::ControlValue;
use hypercolor_types::layer::{LayerScope, SceneLayer, SceneLayerId};

use super::client::{Fence, KeyedResponse, MutationOutcome};
use super::http_transport::HttpMethod;
use super::{ApiError, ApiResult, client};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LayerStackResponse {
    pub items: Vec<SceneLayer>,
    pub revision: u64,
}

pub use hypercolor_types::api::scene::{CreateLayerRequest, ReplaceLayerRequest};

/// The capability a daemon advertises once its layers can be limited to
/// some of a zone's outputs.
pub const LAYER_SCOPE_CAPABILITY: &str = "layer-scope";

/// Whether a replace names the layer's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeField {
    /// The request states the scope: the layer's outputs, or `null` for
    /// the whole zone.
    Stated,
    /// The request leaves the field out. A daemon from before layers could
    /// be limited refuses a field it does not know, `null` included.
    Omitted,
}

impl ScopeField {
    /// The field a replace sends a daemon that advertises
    /// [`LAYER_SCOPE_CAPABILITY`] (`limits_layers`), or does not.
    #[must_use]
    pub fn for_daemon(limits_layers: bool) -> Self {
        if limits_layers {
            Self::Stated
        } else {
            Self::Omitted
        }
    }

    /// The request field for a layer that holds `scope`.
    #[must_use]
    #[allow(
        clippy::option_option,
        reason = "the request field's own tri-state: missing, null, or a scope"
    )]
    pub fn of(self, scope: Option<&LayerScope>) -> Option<Option<LayerScope>> {
        match self {
            Self::Stated => Some(scope.cloned()),
            Self::Omitted => None,
        }
    }
}

/// Build a whole-layer replacement request from the fields the canonical
/// resource accepts. Replacement mints a new layer identity.
///
/// Toward a daemon that limits layers the request states the layer's
/// scope ([`ScopeField::Stated`]): a replace that left it out would keep
/// whatever scope the daemon holds, which is the same thing only while
/// nobody else has changed it. Toward any other daemon it must leave the
/// field out ([`ScopeField::Omitted`]), or the replace is refused.
#[must_use]
pub fn update_request_from_layer(layer: &SceneLayer, scope: ScopeField) -> ReplaceLayerRequest {
    ReplaceLayerRequest {
        name: layer.name.clone(),
        source: layer.source.clone(),
        blend: Some(layer.blend),
        opacity: Some(layer.opacity),
        transform: Some(layer.transform),
        adjust: Some(layer.adjust),
        bindings: Some(layer.bindings.clone()),
        enabled: Some(layer.enabled),
        scope: scope.of(layer.scope.as_ref()),
    }
}

/// [`update_request_from_layer`] with the layer limited to `scope`, or to
/// the whole zone for `None`: the request a change of target sends. Only a
/// daemon that limits layers is ever asked for one.
#[must_use]
pub fn scope_request(layer: &SceneLayer, scope: Option<LayerScope>) -> ReplaceLayerRequest {
    ReplaceLayerRequest {
        scope: Some(scope),
        ..update_request_from_layer(layer, ScopeField::Stated)
    }
}

pub type LayerStackOutcome = MutationOutcome<LayerStackResponse>;

pub async fn create_layer(
    zone_id: &str,
    request: &CreateLayerRequest,
    fence: Option<&Fence>,
) -> ApiResult<LayerStackOutcome> {
    let outcome = client::send_json_versioned_with_revision::<_, ZoneResource>(
        HttpMethod::Post,
        &format!("/api/v1/scene/zones/{zone_id}/layers"),
        Some(request),
        fence,
    )
    .await?;
    stack_outcome(outcome).await
}

/// [`create_layer`] under an idempotency key, fenced at `fence`. The
/// stack's revision is the one the daemon reported in `ETag`.
pub async fn create_layer_keyed(
    zone_id: &str,
    request: &CreateLayerRequest,
    fence: &Fence,
    key: &str,
) -> KeyedResponse<LayerStackResponse> {
    let KeyedResponse {
        run,
        status,
        replayed,
        outcome,
    } = client::send_json_keyed::<_, ZoneResource>(
        HttpMethod::Post,
        &format!("/api/v1/scene/zones/{zone_id}/layers"),
        Some(request),
        Some(fence),
        key,
    )
    .await;
    let outcome = match outcome {
        Ok(outcome) => stack_outcome(outcome).await,
        Err(error) => Err(error),
    };
    KeyedResponse {
        run,
        status,
        replayed,
        outcome,
    }
}

pub async fn update_layer(
    zone_id: &str,
    layer_id: &str,
    request: &ReplaceLayerRequest,
    fence: Option<&Fence>,
) -> ApiResult<LayerStackOutcome> {
    let outcome = client::send_json_versioned_with_revision::<_, ZoneResource>(
        HttpMethod::Put,
        &format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}"),
        Some(request),
        fence,
    )
    .await?;
    stack_outcome(outcome).await
}

pub async fn delete_layer(
    zone_id: &str,
    layer_id: &str,
    fence: Option<&Fence>,
) -> ApiResult<LayerStackOutcome> {
    let outcome = client::send_json_versioned_with_revision::<(), ZoneResource>(
        HttpMethod::Delete,
        &format!("/api/v1/scene/zones/{zone_id}/layers/{layer_id}"),
        None,
        fence,
    )
    .await?;
    stack_outcome(outcome).await
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
    fence: Option<&Fence>,
) -> ApiResult<LayerStackOutcome> {
    let request = ReorderLayersRequest { order: layer_ids };
    let outcome = client::send_json_versioned_with_revision::<_, ZoneResource>(
        HttpMethod::Patch,
        &format!("/api/v1/scene/zones/{zone_id}/layers/order"),
        Some(&request),
        fence,
    )
    .await?;
    stack_outcome(outcome).await
}

/// The layer stack a layer write left, at the revision its `ETag` names.
/// A reply that names none is read back from the live scene rather than
/// guessed at.
async fn stack_outcome(
    outcome: MutationOutcome<(ZoneResource, Option<u64>)>,
) -> ApiResult<LayerStackOutcome> {
    match outcome {
        MutationOutcome::Applied((zone, landed)) => {
            let stack = match landed {
                Some(revision) => layer_stack(zone, revision),
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
