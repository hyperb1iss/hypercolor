//! Zone lifecycle endpoints for `/api/v1/scenes/{id}/zones/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use hypercolor_core::scene::{RenderGroupMetaPatch, SceneManager, ZoneMutationError};
use hypercolor_types::event::{HypercolorEvent, RenderGroupChangeKind, SceneSettingsChangeKind};
use hypercolor_types::scene::{
    RenderGroup, RenderGroupId, RenderGroupRole, SceneId, UnassignedBehavior,
};
use hypercolor_types::spatial::DeviceZone;

use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{
    AppState, persist_runtime_session, publish_render_group_changed, save_scene_store_snapshot,
    scenes,
};

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateZoneRequest {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateZoneRequest {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub color: Option<Option<String>>,
    pub brightness: Option<f32>,
    pub enabled: Option<bool>,
    pub make_primary: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignDevicesRequest {
    #[schema(value_type = Vec<Object>)]
    pub device_zones: Vec<DeviceZoneAssignment>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUnassignedBehaviorRequest {
    #[schema(value_type = String)]
    pub unassigned_behavior: UnassignedBehavior,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum DeviceZoneAssignment {
    Existing { id: String },
    New(DeviceZone),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ZoneListResponse {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<RenderGroup>,
    pub groups_revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ZoneResponse {
    #[schema(value_type = Object)]
    pub zone: RenderGroup,
    pub groups_revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ZoneMutationResponse {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<RenderGroup>,
    pub groups_revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UnassignedBehaviorResponse {
    #[schema(value_type = String)]
    pub unassigned_behavior: UnassignedBehavior,
    pub groups_revision: u64,
}

pub async fn list_zones(
    State(state): State<Arc<AppState>>,
    Path(scene_id_raw): Path<String>,
) -> Response {
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };
    let Some(scene) = manager.get(&scene_id) else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };
    zones_response(scene.groups.clone(), scene.groups_revision, StatusKind::Ok)
}

pub async fn create_zone(
    State(state): State<Arc<AppState>>,
    Path(scene_id_raw): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateZoneRequest>,
) -> Response {
    if body.name.trim().is_empty() {
        return ApiError::bad_request("zone name must not be empty");
    }
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };

    let (scene_id, zone, groups_revision) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if let Some(response) = check_groups_revision(&manager, scene_id, expected_revision) {
            return response;
        }
        let group_id = match manager.create_render_group(&scene_id, body.name, body.color) {
            Ok(group_id) => group_id,
            Err(error) => return zone_mutation_error(error),
        };
        let Some(scene) = manager.get(&scene_id) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        let Some(zone) = find_group_in_scene(scene, group_id) else {
            return ApiError::not_found(format!("Zone not found: {group_id}"));
        };
        (scene_id, zone.clone(), scene.groups_revision)
    };

    if let Err(response) =
        finalize_zone_mutation(&state, scene_id, &zone, RenderGroupChangeKind::Created).await
    {
        return response;
    }
    zone_response(zone, groups_revision, StatusKind::Created)
}

pub async fn get_zone(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return ApiError::bad_request("zone_id must be a valid UUID");
    };
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };
    let Some(scene) = manager.get(&scene_id) else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };
    let Some(zone) = find_group_in_scene(scene, zone_id) else {
        return ApiError::not_found(format!("Zone not found: {zone_id}"));
    };
    zone_response(zone.clone(), scene.groups_revision, StatusKind::Ok)
}

pub async fn update_zone(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateZoneRequest>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return ApiError::bad_request("zone_id must be a valid UUID");
    };
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let structural = body.make_primary == Some(true);
    let patch = body.into_patch();

    let (scene_id, zone, groups_revision) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if structural
            && let Some(response) = check_groups_revision(&manager, scene_id, expected_revision)
        {
            return response;
        }
        let zone = match manager.update_render_group_meta(&scene_id, zone_id, patch) {
            Ok(zone) => zone,
            Err(error) => return zone_mutation_error(error),
        };
        let groups_revision = manager
            .get(&scene_id)
            .map(|scene| scene.groups_revision)
            .unwrap_or_default();
        (scene_id, zone, groups_revision)
    };

    if let Err(response) =
        finalize_zone_mutation(&state, scene_id, &zone, RenderGroupChangeKind::Updated).await
    {
        return response;
    }
    zone_response(zone, groups_revision, StatusKind::Ok)
}

pub async fn delete_zone(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return ApiError::bad_request("zone_id must be a valid UUID");
    };
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };

    let (scene_id, zone, groups_revision) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if let Some(response) = check_groups_revision(&manager, scene_id, expected_revision) {
            return response;
        }
        let Some(zone) = manager
            .get(&scene_id)
            .and_then(|scene| find_group_in_scene(scene, zone_id))
            .cloned()
        else {
            return ApiError::not_found(format!("Zone not found: {zone_id}"));
        };
        if let Err(error) = manager.delete_render_group(&scene_id, zone_id) {
            return zone_mutation_error(error);
        }
        let groups_revision = manager
            .get(&scene_id)
            .map(|scene| scene.groups_revision)
            .unwrap_or_default();
        (scene_id, zone, groups_revision)
    };

    if let Err(response) =
        finalize_zone_mutation(&state, scene_id, &zone, RenderGroupChangeKind::Removed).await
    {
        return response;
    }
    attach_groups_revision_headers(
        ApiResponse::ok(serde_json::json!({
            "zone_id": zone_id,
            "deleted": true,
            "groups_revision": groups_revision,
        })),
        groups_revision,
    )
}

pub async fn assign_devices(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AssignDevicesRequest>,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return ApiError::bad_request("zone_id must be a valid UUID");
    };
    if body.device_zones.is_empty() {
        return ApiError::bad_request("device_zones must include at least one item");
    }
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };

    let (scene_id, zones, target_group, groups_revision) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if let Some(response) = check_groups_revision(&manager, scene_id, expected_revision) {
            return response;
        }
        let Some(scene) = manager.get(&scene_id) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if find_group_in_scene(scene, zone_id).is_none() {
            return ApiError::not_found(format!("Zone not found: {zone_id}"));
        }
        let device_zones = match resolve_device_zone_assignments(scene, body.device_zones) {
            Ok(device_zones) => device_zones,
            Err(device_zone_id) => {
                return ApiError::not_found(format!("Device zone not found: {device_zone_id}"));
            }
        };
        for device_zone in device_zones {
            if let Err(error) = manager.assign_device_zone(&scene_id, zone_id, device_zone) {
                return zone_mutation_error(error);
            }
        }
        let Some(scene) = manager.get(&scene_id) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        let Some(target_group) = find_group_in_scene(scene, zone_id) else {
            return ApiError::not_found(format!("Zone not found: {zone_id}"));
        };
        (
            scene_id,
            scene.groups.clone(),
            target_group.clone(),
            scene.groups_revision,
        )
    };

    if let Err(response) = finalize_zone_mutation(
        &state,
        scene_id,
        &target_group,
        RenderGroupChangeKind::Updated,
    )
    .await
    {
        return response;
    }
    zones_response(zones, groups_revision, StatusKind::Ok)
}

pub async fn unassign_device(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw, device_zone_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return ApiError::bad_request("zone_id must be a valid UUID");
    };
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };

    let (scene_id, zones, target_group, groups_revision) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if let Some(response) = check_groups_revision(&manager, scene_id, expected_revision) {
            return response;
        }
        let Some(scene) = manager.get(&scene_id) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        let Some(target_group) = find_group_in_scene(scene, zone_id) else {
            return ApiError::not_found(format!("Zone not found: {zone_id}"));
        };
        if !target_group
            .layout
            .zones
            .iter()
            .any(|zone| zone.id == device_zone_id)
        {
            return ApiError::not_found(format!("Device zone not found: {device_zone_id}"));
        }
        if let Err(error) = manager.unassign_device_zone(&scene_id, &device_zone_id) {
            return zone_mutation_error(error);
        }
        let Some(scene) = manager.get(&scene_id) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        let Some(target_group) = find_group_in_scene(scene, zone_id) else {
            return ApiError::not_found(format!("Zone not found: {zone_id}"));
        };
        (
            scene_id,
            scene.groups.clone(),
            target_group.clone(),
            scene.groups_revision,
        )
    };

    if let Err(response) = finalize_zone_mutation(
        &state,
        scene_id,
        &target_group,
        RenderGroupChangeKind::Updated,
    )
    .await
    {
        return response;
    }
    zones_response(zones, groups_revision, StatusKind::Ok)
}

pub async fn update_unassigned_behavior(
    State(state): State<Arc<AppState>>,
    Path(scene_id_raw): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateUnassignedBehaviorRequest>,
) -> Response {
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };

    let (scene_id, behavior, groups_revision) = {
        let mut manager = state.scene_manager.write().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        if let Some(response) = check_groups_revision(&manager, scene_id, expected_revision) {
            return response;
        }
        let behavior = match manager.set_unassigned_behavior(&scene_id, body.unassigned_behavior) {
            Ok(behavior) => behavior,
            Err(error) => return zone_mutation_error(error),
        };
        let groups_revision = manager
            .get(&scene_id)
            .map(|scene| scene.groups_revision)
            .unwrap_or_default();
        (scene_id, behavior, groups_revision)
    };

    if let Err(response) = finalize_scene_settings_mutation(&state, scene_id, groups_revision).await
    {
        return response;
    }
    unassigned_behavior_response(behavior, groups_revision)
}

impl UpdateZoneRequest {
    fn into_patch(self) -> RenderGroupMetaPatch {
        RenderGroupMetaPatch {
            name: self.name,
            description: self.description,
            color: self.color,
            brightness: self.brightness,
            enabled: self.enabled,
            make_primary: self.make_primary,
        }
    }
}

enum StatusKind {
    Ok,
    Created,
}

fn zones_response(groups: Vec<RenderGroup>, groups_revision: u64, status: StatusKind) -> Response {
    let body = ZoneListResponse {
        items: groups,
        groups_revision,
    };
    let response = match status {
        StatusKind::Ok => ApiResponse::ok(body),
        StatusKind::Created => ApiResponse::created(body),
    };
    attach_groups_revision_headers(response, groups_revision)
}

fn zone_response(zone: RenderGroup, groups_revision: u64, status: StatusKind) -> Response {
    let body = ZoneResponse {
        zone,
        groups_revision,
    };
    let response = match status {
        StatusKind::Ok => ApiResponse::ok(body),
        StatusKind::Created => ApiResponse::created(body),
    };
    attach_groups_revision_headers(response, groups_revision)
}

fn unassigned_behavior_response(behavior: UnassignedBehavior, groups_revision: u64) -> Response {
    attach_groups_revision_headers(
        ApiResponse::ok(UnassignedBehaviorResponse {
            unassigned_behavior: behavior,
            groups_revision,
        }),
        groups_revision,
    )
}

fn find_group_in_scene(
    scene: &hypercolor_types::scene::Scene,
    group_id: RenderGroupId,
) -> Option<&RenderGroup> {
    scene.groups.iter().find(|group| group.id == group_id)
}

fn resolve_device_zone_assignments(
    scene: &hypercolor_types::scene::Scene,
    assignments: Vec<DeviceZoneAssignment>,
) -> Result<Vec<DeviceZone>, String> {
    assignments
        .into_iter()
        .map(|assignment| match assignment {
            DeviceZoneAssignment::Existing { id } => scene
                .groups
                .iter()
                .flat_map(|group| group.layout.zones.iter())
                .find(|zone| zone.id == id)
                .cloned()
                .ok_or(id),
            DeviceZoneAssignment::New(device_zone) => Ok(device_zone),
        })
        .collect()
}

fn check_groups_revision(
    manager: &SceneManager,
    scene_id: SceneId,
    expected_revision: Option<u64>,
) -> Option<Response> {
    let expected = expected_revision?;
    let current = manager.get(&scene_id)?.groups_revision;
    (expected != current).then(|| groups_revision_mismatch_response(current))
}

async fn finalize_zone_mutation(
    state: &Arc<AppState>,
    scene_id: SceneId,
    group: &RenderGroup,
    kind: RenderGroupChangeKind,
) -> Result<(), Response> {
    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return Err(ApiError::internal(format!(
            "Failed to persist zones: {error}"
        )));
    }
    persist_runtime_session(state).await;
    publish_render_group_changed(state.as_ref(), scene_id, group, kind);
    Ok(())
}

async fn finalize_scene_settings_mutation(
    state: &Arc<AppState>,
    scene_id: SceneId,
    groups_revision: u64,
) -> Result<(), Response> {
    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return Err(ApiError::internal(format!(
            "Failed to persist scene settings: {error}"
        )));
    }
    persist_runtime_session(state).await;
    state
        .event_bus
        .publish(HypercolorEvent::SceneSettingsChanged {
            scene_id,
            groups_revision,
            kind: SceneSettingsChangeKind::UnassignedBehavior,
        });
    Ok(())
}

fn zone_mutation_error(error: ZoneMutationError) -> Response {
    match error {
        ZoneMutationError::SceneMissing => ApiError::not_found("Scene not found"),
        ZoneMutationError::GroupMissing => ApiError::not_found("Zone not found"),
        ZoneMutationError::DeviceZoneMissing => ApiError::not_found("Device zone not found"),
        ZoneMutationError::SnapshotLocked => {
            ApiError::conflict("Snapshot scene cannot be structurally edited")
        }
        ZoneMutationError::InvalidRole {
            role: RenderGroupRole::Primary,
        } => ApiError::conflict("Primary zone cannot be deleted through the custom zone endpoint"),
        ZoneMutationError::InvalidRole {
            role: RenderGroupRole::Display,
        } => ApiError::conflict("Display zone cannot be deleted through the custom zone endpoint"),
        ZoneMutationError::InvalidRole { .. } => {
            ApiError::conflict("Zone role does not support this mutation")
        }
    }
}

fn parse_zone_id(raw: &str) -> Result<RenderGroupId, uuid::Error> {
    raw.parse::<uuid::Uuid>().map(RenderGroupId)
}

fn parse_if_match_groups_revision(headers: &HeaderMap) -> Result<Option<u64>, &'static str> {
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
        .map_err(|_| "If-Match must be a non-negative integer groups_revision")
}

fn groups_revision_mismatch_response(current: u64) -> Response {
    let body = serde_json::json!({
        "error": "groups_revision mismatch",
        "current": current,
    });
    let mut response = (StatusCode::PRECONDITION_FAILED, Json(body)).into_response();
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{current}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn attach_groups_revision_headers(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}
