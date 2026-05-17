//! Layer-stack endpoints for `/api/v1/scenes/{id}/groups/{group_id}/layers/*`.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use hypercolor_core::scene::{LayerMutationError, SceneManager};
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::event::RenderGroupChangeKind;
use hypercolor_types::layer::{
    LayerAdjust, LayerBinding, LayerBlendMode, LayerSource, LayerTransform, SceneLayer,
    SceneLayerId,
};
use hypercolor_types::scene::{RenderGroup, RenderGroupId, SceneId};

use crate::api::control_values::json_to_control_value;
use crate::api::effects::normalize_control_payload;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{
    AppState, persist_runtime_session, publish_render_group_changed, save_scene_store_snapshot,
    scenes,
};

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateLayerRequest {
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateLayerRequest {
    #[schema(value_type = String)]
    pub id: SceneLayerId,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct LayerOrderRequest {
    #[schema(value_type = Vec<String>)]
    pub layer_ids: Vec<SceneLayerId>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchLayerControlsRequest {
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLayerQuery {
    pub index: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LayerStackResponse {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<SceneLayer>,
    pub layers_version: u64,
}

pub async fn list_layers(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, group_id_raw)): Path<(String, String)>,
) -> Response {
    let Ok(group_id) = parse_group_id(&group_id_raw) else {
        return ApiError::bad_request("group_id must be a valid UUID");
    };
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };
    let Some(group) = find_group(&manager, scene_id, group_id) else {
        return ApiError::not_found(format!("Render group not found: {group_id}"));
    };
    layer_stack_response(group, StatusKind::Ok)
}

pub async fn create_layer(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, group_id_raw)): Path<(String, String)>,
    Query(query): Query<CreateLayerQuery>,
    headers: HeaderMap,
    Json(body): Json<CreateLayerRequest>,
) -> Response {
    let Ok(group_id) = parse_group_id(&group_id_raw) else {
        return ApiError::bad_request("group_id must be a valid UUID");
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let layer = body.into_layer(SceneLayerId::new());
    let (scene_id, group) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        match manager.insert_scene_group_layer(
            scene_id,
            group_id,
            layer,
            query.index,
            expected_version,
        ) {
            Ok((group, _version)) => (scene_id, group.clone()),
            Err(error) => return layer_mutation_error(error),
        }
    };
    if let Err(response) =
        finalize_layer_mutation(&state, scene_id, &group, RenderGroupChangeKind::Updated).await
    {
        return response;
    }
    layer_stack_response(&group, StatusKind::Created)
}

pub async fn update_layer(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, group_id_raw, layer_id_raw)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateLayerRequest>,
) -> Response {
    let Ok(group_id) = parse_group_id(&group_id_raw) else {
        return ApiError::bad_request("group_id must be a valid UUID");
    };
    let Ok(layer_id) = SceneLayerId::from_str(&layer_id_raw) else {
        return ApiError::bad_request("layer_id must be a valid UUID");
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let (scene_id, group) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        match manager.update_scene_group_layer(
            scene_id,
            group_id,
            layer_id,
            body.into_layer(),
            expected_version,
        ) {
            Ok((group, _version)) => (scene_id, group.clone()),
            Err(error) => return layer_mutation_error(error),
        }
    };
    if let Err(response) =
        finalize_layer_mutation(&state, scene_id, &group, RenderGroupChangeKind::Updated).await
    {
        return response;
    }
    layer_stack_response(&group, StatusKind::Ok)
}

pub async fn delete_layer(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, group_id_raw, layer_id_raw)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Ok(group_id) = parse_group_id(&group_id_raw) else {
        return ApiError::bad_request("group_id must be a valid UUID");
    };
    let Ok(layer_id) = SceneLayerId::from_str(&layer_id_raw) else {
        return ApiError::bad_request("layer_id must be a valid UUID");
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let (scene_id, group) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        match manager.remove_scene_group_layer(scene_id, group_id, layer_id, expected_version) {
            Ok((group, _version)) => (scene_id, group.clone()),
            Err(error) => return layer_mutation_error(error),
        }
    };
    if let Err(response) =
        finalize_layer_mutation(&state, scene_id, &group, RenderGroupChangeKind::Updated).await
    {
        return response;
    }
    layer_stack_response(&group, StatusKind::Ok)
}

pub async fn reorder_layers(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, group_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerOrderRequest>,
) -> Response {
    let Ok(group_id) = parse_group_id(&group_id_raw) else {
        return ApiError::bad_request("group_id must be a valid UUID");
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let (scene_id, group) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        match manager.reorder_scene_group_layers(
            scene_id,
            group_id,
            body.layer_ids,
            expected_version,
        ) {
            Ok((group, _version)) => (scene_id, group.clone()),
            Err(error) => return layer_mutation_error(error),
        }
    };
    if let Err(response) =
        finalize_layer_mutation(&state, scene_id, &group, RenderGroupChangeKind::Updated).await
    {
        return response;
    }
    layer_stack_response(&group, StatusKind::Ok)
}

pub async fn patch_layer_controls(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, group_id_raw, layer_id_raw)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<PatchLayerControlsRequest>,
) -> Response {
    let Ok(group_id) = parse_group_id(&group_id_raw) else {
        return ApiError::bad_request("group_id must be a valid UUID");
    };
    let Ok(layer_id) = SceneLayerId::from_str(&layer_id_raw) else {
        return ApiError::bad_request("layer_id must be a valid UUID");
    };
    let expected_version = match parse_if_match_layers_version(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let controls = body
        .controls
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if controls.is_empty() {
        return ApiError::bad_request("controls payload must include at least one key");
    }

    let (scene_id, effect_id) = {
        let manager = state.scene_manager.read().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        let Some(group) = find_group(&manager, scene_id, group_id) else {
            return ApiError::not_found(format!("Render group not found: {group_id}"));
        };
        let Some(layer) = group
            .effective_layers()
            .into_iter()
            .find(|layer| layer.id == layer_id)
        else {
            return ApiError::not_found(format!("Layer not found: {layer_id}"));
        };
        let LayerSource::Effect { effect_id, .. } = layer.source else {
            return ApiError::validation("layer source has no controls");
        };
        (scene_id, effect_id)
    };

    let (normalized, invalid) =
        normalize_layer_controls(state.as_ref(), effect_id, &controls).await;
    if normalized.is_empty() && !invalid.is_empty() {
        return ApiError::validation_with_details(
            "no valid controls to apply",
            serde_json::json!({ "rejected": invalid }),
        );
    }

    let group = {
        let mut manager = state.scene_manager.write().await;
        match manager.patch_scene_layer_effect_controls(
            scene_id,
            group_id,
            layer_id,
            normalized,
            expected_version,
        ) {
            Ok((group, _version)) => group.clone(),
            Err(error) => return layer_mutation_error(error),
        }
    };
    if let Err(response) = finalize_layer_mutation(
        &state,
        scene_id,
        &group,
        RenderGroupChangeKind::ControlsPatched,
    )
    .await
    {
        return response;
    }
    layer_stack_response(&group, StatusKind::Ok)
}

impl CreateLayerRequest {
    fn into_layer(self, id: SceneLayerId) -> SceneLayer {
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
    fn into_layer(self) -> SceneLayer {
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

enum StatusKind {
    Ok,
    Created,
}

fn layer_stack_response(group: &RenderGroup, status: StatusKind) -> Response {
    let body = LayerStackResponse {
        items: group.effective_layers(),
        layers_version: group.layers_version,
    };
    let response = match status {
        StatusKind::Ok => ApiResponse::ok(body),
        StatusKind::Created => ApiResponse::created(body),
    };
    attach_layers_version_headers(response, group.layers_version)
}

fn find_group(
    manager: &SceneManager,
    scene_id: SceneId,
    group_id: RenderGroupId,
) -> Option<&RenderGroup> {
    manager
        .get(&scene_id)?
        .groups
        .iter()
        .find(|group| group.id == group_id)
}

async fn finalize_layer_mutation(
    state: &Arc<AppState>,
    scene_id: SceneId,
    group: &RenderGroup,
    kind: RenderGroupChangeKind,
) -> Result<(), Response> {
    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return Err(ApiError::internal(format!(
            "Failed to persist layer stack: {error}"
        )));
    }
    persist_runtime_session(state).await;
    publish_render_group_changed(state.as_ref(), scene_id, group, kind);
    Ok(())
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

fn parse_group_id(raw: &str) -> Result<RenderGroupId, uuid::Error> {
    raw.parse::<uuid::Uuid>().map(RenderGroupId)
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

fn layer_mutation_error(error: LayerMutationError) -> Response {
    match error {
        LayerMutationError::NoActiveScene | LayerMutationError::SceneMissing => {
            ApiError::not_found("Scene not found")
        }
        LayerMutationError::GroupMissing => ApiError::not_found("Render group not found"),
        LayerMutationError::LayerMissing { layer_id } => {
            ApiError::not_found(format!("Layer not found: {layer_id}"))
        }
        LayerMutationError::DuplicateLayer { layer_id } => {
            ApiError::validation(format!("Layer already exists: {layer_id}"))
        }
        LayerMutationError::Stale { current } => layers_version_mismatch_response(current),
        LayerMutationError::InvalidLayer { errors } => ApiError::validation_with_details(
            "Layer payload is invalid",
            serde_json::json!({ "errors": errors }),
        ),
        LayerMutationError::InvalidIndex { index, len } => ApiError::validation(format!(
            "Layer insertion index {index} is out of range for stack length {len}"
        )),
        LayerMutationError::InvalidOrder => {
            ApiError::validation("layer_ids must be an exact permutation of current layer IDs")
        }
    }
}

fn layers_version_mismatch_response(current: u64) -> Response {
    let body = serde_json::json!({
        "error": "layers_version mismatch",
        "current": current,
    });
    let mut response = (StatusCode::PRECONDITION_FAILED, Json(body)).into_response();
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{current}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn attach_layers_version_headers(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn default_layer_opacity() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}
