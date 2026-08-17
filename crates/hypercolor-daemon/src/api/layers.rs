//! Layer-stack endpoints for `/api/v1/scenes/{id}/zones/{zone_id}/layers/*`.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use hypercolor_core::scene::{LayerMutationError, SceneGroupLayerInsert, SceneManager};
use hypercolor_types::asset::AssetId;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, MediaPlayback, SceneLayer,
    SceneLayerId,
};
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use crate::api::control_values::json_to_control_value;
use crate::api::effects::normalize_control_payload;
use crate::api::envelope::ApiResponse;
use crate::api::{AppState, scenes};
use crate::domain::layer;
use crate::domain::{DomainError, MutationContext, ResourceKind};

pub use hypercolor_types::api::layers::{
    BroadcastMediaLayerRequest, BroadcastMediaLayerTarget, CreateLayerQuery, CreateLayerRequest,
    LayerOrderRequest, PatchLayerControlsRequest, UpdateLayerRequest,
};

#[derive(Debug, Serialize, ToSchema)]
pub struct LayerStackResponse {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<SceneLayer>,
    pub layers_version: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BroadcastMediaLayerZoneResponse {
    #[schema(value_type = String)]
    pub zone_id: ZoneId,
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<SceneLayer>,
    pub layers_version: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BroadcastMediaLayerResponse {
    pub zones: Vec<BroadcastMediaLayerZoneResponse>,
}

pub async fn list_layers(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };
    let Some(zone) = find_zone(&manager, scene_id, zone_id) else {
        return DomainError::not_found(ResourceKind::Zone, zone_id).into_response();
    };
    layer_stack_response(zone, StatusKind::Ok)
}

pub async fn create_layer(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    Query(query): Query<CreateLayerQuery>,
    headers: HeaderMap,
    Json(body): Json<CreateLayerRequest>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let layer = body.into_layer(SceneLayerId::new());
    if let Err(error) =
        validate_livestream_layer_insert(&state, &scene_id_raw, layer.source.media_asset_id()).await
    {
        return error.into_response();
    }
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match layer::insert_layer(
        state.as_ref(),
        scene_id,
        zone_id,
        layer,
        query.index,
        expected_version,
        MutationContext::api(),
    )
    .await
    {
        Ok(Ok(written)) => layer_stack_response(written.zone(), StatusKind::Created),
        Ok(Err(refusal)) => layer_mutation_error(
            refusal,
            LayerTarget {
                scene: &scene_id_raw,
                zone: Some(zone_id),
            },
        ),
        Err(error) => error.into_response(),
    }
}

pub async fn broadcast_media_layer(
    State(state): State<Arc<AppState>>,
    Path(scene_id_raw): Path<String>,
    Json(body): Json<BroadcastMediaLayerRequest>,
) -> Response {
    if body.targets.is_empty() {
        return DomainError::validation_field("targets", "targets must include at least one zone")
            .into_response();
    }
    {
        let library = state.asset_library.read().await;
        if !library.contains(body.asset_id) {
            return DomainError::not_found(ResourceKind::Asset, body.asset_id).into_response();
        }
    }
    if let Err(error) =
        validate_livestream_layer_insert(&state, &scene_id_raw, Some(body.asset_id)).await
    {
        return error.into_response();
    }
    let inserts = {
        let manager = state.scene_manager.read().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
        };
        if let Some(zone_id) = body.targets.iter().find_map(|target| {
            find_zone(&manager, scene_id, target.zone_id)
                .is_none()
                .then_some(target.zone_id)
        }) {
            return DomainError::not_found(ResourceKind::Zone, zone_id).into_response();
        }
        (scene_id, broadcast_layer_inserts(body))
    };

    match layer::insert_layers(state.as_ref(), inserts.0, inserts.1, MutationContext::api()).await {
        Ok(Ok(written)) => broadcast_media_layer_response(&written.zones),
        Ok(Err(refusal)) => layer_mutation_error(
            refusal,
            LayerTarget {
                scene: &scene_id_raw,
                zone: None,
            },
        ),
        Err(error) => error.into_response(),
    }
}

pub async fn update_layer(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw, layer_id_raw)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateLayerRequest>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let Ok(layer_id) = SceneLayerId::from_str(&layer_id_raw) else {
        return DomainError::malformed("layer_id must be a valid UUID").into_response();
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    if let Err(error) = validate_livestream_layer_update(
        &state,
        &scene_id_raw,
        zone_id,
        layer_id,
        body.source.media_asset_id(),
    )
    .await
    {
        return error.into_response();
    }
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match layer::update_layer(
        state.as_ref(),
        scene_id,
        zone_id,
        layer_id,
        body.into_layer(),
        expected_version,
        MutationContext::api(),
    )
    .await
    {
        Ok(Ok(written)) => layer_stack_response(written.zone(), StatusKind::Ok),
        Ok(Err(refusal)) => layer_mutation_error(
            refusal,
            LayerTarget {
                scene: &scene_id_raw,
                zone: Some(zone_id),
            },
        ),
        Err(error) => error.into_response(),
    }
}

pub async fn delete_layer(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw, layer_id_raw)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let Ok(layer_id) = SceneLayerId::from_str(&layer_id_raw) else {
        return DomainError::malformed("layer_id must be a valid UUID").into_response();
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match layer::remove_layer(
        state.as_ref(),
        scene_id,
        zone_id,
        layer_id,
        expected_version,
        MutationContext::api(),
    )
    .await
    {
        Ok(Ok(written)) => layer_stack_response(written.zone(), StatusKind::Ok),
        Ok(Err(refusal)) => layer_mutation_error(
            refusal,
            LayerTarget {
                scene: &scene_id_raw,
                zone: Some(zone_id),
            },
        ),
        Err(error) => error.into_response(),
    }
}

pub async fn reorder_layers(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerOrderRequest>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match layer::reorder_layers(
        state.as_ref(),
        scene_id,
        zone_id,
        body.layer_ids,
        expected_version,
        MutationContext::api(),
    )
    .await
    {
        Ok(Ok(written)) => layer_stack_response(written.zone(), StatusKind::Ok),
        Ok(Err(refusal)) => layer_mutation_error(
            refusal,
            LayerTarget {
                scene: &scene_id_raw,
                zone: Some(zone_id),
            },
        ),
        Err(error) => error.into_response(),
    }
}

pub async fn patch_layer_controls(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw, layer_id_raw)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<PatchLayerControlsRequest>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let Ok(layer_id) = SceneLayerId::from_str(&layer_id_raw) else {
        return DomainError::malformed("layer_id must be a valid UUID").into_response();
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let controls = body
        .controls
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if controls.is_empty() {
        return DomainError::validation_field(
            "controls",
            "controls payload must include at least one key",
        )
        .into_response();
    }

    let (scene_id, effect_id) = {
        let manager = state.scene_manager.read().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
        };
        let Some(zone) = find_zone(&manager, scene_id, zone_id) else {
            return DomainError::not_found(ResourceKind::Zone, zone_id).into_response();
        };
        let Some(layer) = zone
            .effective_layers()
            .into_iter()
            .find(|layer| layer.id == layer_id)
        else {
            return DomainError::not_found(ResourceKind::Layer, layer_id).into_response();
        };
        let LayerSource::Effect { effect_id, .. } = layer.source else {
            return DomainError::validation("layer source has no controls").into_response();
        };
        (scene_id, effect_id)
    };

    let (normalized, invalid) =
        normalize_layer_controls(state.as_ref(), effect_id, &controls).await;
    if normalized.is_empty() && !invalid.is_empty() {
        return DomainError::validation_details(
            "no valid controls to apply",
            serde_json::json!({ "rejected": invalid }),
        )
        .into_response();
    }

    match layer::patch_layer_controls(
        state.as_ref(),
        scene_id,
        zone_id,
        layer_id,
        normalized,
        expected_version,
        MutationContext::api(),
    )
    .await
    {
        Ok(Ok(written)) => layer_stack_response(written.zone(), StatusKind::Ok),
        Ok(Err(refusal)) => layer_mutation_error(
            refusal,
            LayerTarget {
                scene: &scene_id_raw,
                zone: Some(zone_id),
            },
        ),
        Err(error) => error.into_response(),
    }
}

/// Expand one broadcast request into a per-zone layer insert list.
fn broadcast_layer_inserts(request: BroadcastMediaLayerRequest) -> Vec<SceneGroupLayerInsert> {
    let source = LayerSource::Media {
        asset_id: request.asset_id,
        playback: request.playback,
    };
    request
        .targets
        .into_iter()
        .map(|target| SceneGroupLayerInsert {
            group_id: target.zone_id,
            layer: SceneLayer {
                id: SceneLayerId::new(),
                name: request.name.clone(),
                source: source.clone(),
                blend: request.blend,
                opacity: request.opacity,
                transform: target.transform,
                adjust: target.adjust,
                bindings: request.bindings.clone(),
                enabled: request.enabled,
            },
            index: target.index,
            expected_version: target.expected_layers_version,
        })
        .collect()
}

enum StatusKind {
    Ok,
    Created,
}

fn layer_stack_response(zone: &Zone, status: StatusKind) -> Response {
    let body = LayerStackResponse {
        items: zone.effective_layers(),
        layers_version: zone.layers_version,
    };
    let response = match status {
        StatusKind::Ok => ApiResponse::ok(body),
        StatusKind::Created => ApiResponse::created(body),
    };
    attach_layers_version_headers(response, zone.layers_version)
}

fn broadcast_media_layer_response(zones: &[Zone]) -> Response {
    ApiResponse::created(BroadcastMediaLayerResponse {
        zones: zones
            .iter()
            .map(|zone| BroadcastMediaLayerZoneResponse {
                zone_id: zone.id,
                items: zone.effective_layers(),
                layers_version: zone.layers_version,
            })
            .collect(),
    })
}

async fn resolve_scene(state: &AppState, scene_id_raw: &str) -> Option<SceneId> {
    let manager = state.scene_manager.read().await;
    scenes::resolve_scene_id(&manager, scene_id_raw)
}

fn find_zone(manager: &SceneManager, scene_id: SceneId, zone_id: ZoneId) -> Option<&Zone> {
    manager
        .get(&scene_id)?
        .groups
        .iter()
        .find(|zone| zone.id == zone_id)
}

async fn normalize_layer_controls(
    state: &AppState,
    effect_id: EffectId,
    controls: &serde_json::Map<String, serde_json::Value>,
) -> (HashMap<String, ControlValue>, Vec<String>) {
    let registry = state.effect_registry.read().await;
    if let Some(entry) = registry.get(&effect_id) {
        return normalize_control_payload(&entry.metadata, controls);
    }

    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();
    for (name, value) in controls {
        if let Some(parsed) = json_to_control_value(value) {
            normalized.insert(name.clone(), parsed);
        } else {
            rejected.push(format!("{name} (unsupported JSON shape)"));
        }
    }
    (normalized, rejected)
}

fn parse_zone_id(raw: &str) -> Result<ZoneId, uuid::Error> {
    raw.parse::<uuid::Uuid>().map(ZoneId)
}

fn parse_if_match_layers_version(headers: &HeaderMap) -> Result<Option<u64>, &'static str> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| "If-Match header must be ASCII")?;
    let trimmed = raw.trim().trim_matches('"');
    if trimmed == "*" {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| "If-Match must be a non-negative integer layers_version")
}

/// What a layer mutation addressed, so a miss names the resource the
/// caller asked for instead of a placeholder.
struct LayerTarget<'a> {
    scene: &'a str,
    zone: Option<ZoneId>,
}

/// Project a layer-stack refusal onto the canonical error envelope.
fn layer_mutation_error(error: LayerMutationError, target: LayerTarget<'_>) -> Response {
    match error {
        LayerMutationError::NoActiveScene => {
            DomainError::not_found(ResourceKind::Scene, "active").into_response()
        }
        LayerMutationError::SceneMissing => {
            DomainError::not_found(ResourceKind::Scene, target.scene).into_response()
        }
        LayerMutationError::GroupMissing => match target.zone {
            Some(zone) => DomainError::not_found(ResourceKind::Zone, zone).into_response(),
            // The broadcast route validates every target zone before it
            // commits, so reaching here means one was removed mid-flight.
            None => DomainError::not_found(ResourceKind::Zone, "requested").into_response(),
        },
        LayerMutationError::LayerMissing { layer_id } => {
            DomainError::not_found(ResourceKind::Layer, layer_id).into_response()
        }
        LayerMutationError::DuplicateLayer { layer_id } => {
            DomainError::validation(format!("Layer already exists: {layer_id}")).into_response()
        }
        LayerMutationError::Stale { expected, current } => DomainError::PreconditionFailed {
            resource: ResourceKind::Layer,
            expected,
            current,
        }
        .into_response(),
        LayerMutationError::InvalidLayer { errors } => DomainError::validation_details(
            "Layer payload is invalid",
            serde_json::json!({ "errors": errors }),
        )
        .into_response(),
        LayerMutationError::InvalidIndex { index, len } => DomainError::validation(format!(
            "Layer insertion index {index} is out of range for stack length {len}"
        ))
        .into_response(),
        LayerMutationError::InvalidOrder => {
            DomainError::validation("layer_ids must be an exact permutation of current layer IDs")
                .into_response()
        }
    }
}

async fn validate_livestream_layer_insert(
    state: &Arc<AppState>,
    scene_id_raw: &str,
    asset_id: Option<AssetId>,
) -> Result<(), DomainError> {
    let Some(asset_id) = asset_id else {
        return Ok(());
    };
    validate_livestream_admission(state, scene_id_raw, Some((None, asset_id))).await
}

async fn validate_livestream_layer_update(
    state: &Arc<AppState>,
    scene_id_raw: &str,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    next_asset_id: Option<AssetId>,
) -> Result<(), DomainError> {
    validate_livestream_admission(
        state,
        scene_id_raw,
        next_asset_id.map(|id| (Some((zone_id, layer_id)), id)),
    )
    .await
}

async fn validate_livestream_admission(
    state: &Arc<AppState>,
    scene_id_raw: &str,
    pending_asset: Option<(Option<(ZoneId, SceneLayerId)>, AssetId)>,
) -> Result<(), DomainError> {
    let asset_mime_types = scenes::asset_mime_types(state.as_ref()).await;
    let media_config = scenes::current_media_config(state.as_ref());
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = scenes::resolve_scene_id(&manager, scene_id_raw) else {
        return Ok(());
    };
    let Some(scene) = manager.get(&scene_id) else {
        return Ok(());
    };
    if manager.active_scene_id().copied() != Some(scene_id) {
        return Ok(());
    }
    let mut candidate = scene.clone();
    if let Some((replacement, asset_id)) = pending_asset {
        if let Some((zone_id, layer_id)) = replacement {
            if let Some(layer) = candidate
                .groups
                .iter_mut()
                .find(|zone| zone.id == zone_id)
                .and_then(|zone| zone.layers.iter_mut().find(|layer| layer.id == layer_id))
            {
                layer.source = LayerSource::Media {
                    asset_id,
                    playback: MediaPlayback::default(),
                };
            }
        } else if let Some(zone) = candidate.groups.first_mut() {
            zone.layers.push(media_layer_for_validation(asset_id));
        }
    }
    let counts = scenes::scene_media_admission_counts(&candidate, &asset_mime_types);
    if let Some(error) = scenes::validate_scene_media_admission(&counts, &media_config) {
        return Err(error);
    }
    Ok(())
}

fn media_layer_for_validation(asset_id: AssetId) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::Media {
            asset_id,
            playback: MediaPlayback::default(),
        },
        blend: LayerBlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

fn attach_layers_version_headers(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

trait LayerSourceExt {
    fn media_asset_id(&self) -> Option<AssetId>;
}

impl LayerSourceExt for LayerSource {
    fn media_asset_id(&self) -> Option<AssetId> {
        match self {
            LayerSource::Media { asset_id, .. } => Some(*asset_id),
            _ => None,
        }
    }
}
