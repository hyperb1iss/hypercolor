//! Zone lifecycle endpoints for `/api/v1/scenes/{id}/zones/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

use hypercolor_core::scene::{OutputPlacement, ZoneMetaPatch};
use hypercolor_types::scene::{SceneId, UnassignedBehavior, Zone, ZoneId};
use hypercolor_types::spatial::{Output, SpatialLayout};

use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::layouts::{validate_layout_sampling_radii, validate_output_sampling_radii};
use crate::api::{AppState, scenes};
use crate::domain::zone;
use crate::domain::{DomainError, MutationContext, ResourceKind, legacy};

// Wire contracts live in hypercolor-types::api::zones — shared with the
// web UI and the TUI. OutputAssignment's untagged variant ORDER is part
// of the wire contract; see the shared definition.
pub use hypercolor_types::api::zones::{
    AssignDevicesRequest, CreateZoneRequest, OutputAssignment, UnassignedBehaviorResponse,
    UpdateUnassignedBehaviorRequest, UpdateZoneRequest, ZoneListResponse, ZoneMutationResponse,
    ZoneResponse,
};

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
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };

    let fallback_canvas = {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        (layout.canvas_width, layout.canvas_height)
    };

    match zone::create_zone(
        state.as_ref(),
        zone::CreateZone {
            scene_id,
            name: body.name,
            color: body.color,
            fallback_canvas,
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zone_response(written.zone, written.groups_revision, StatusKind::Created),
        // A blank name has always been a 400 here, not the canonical
        // 422 the domain validation error renders.
        Err(DomainError::Validation {
            field: Some(field),
            message,
        }) if field == "name" => ApiError::bad_request(message),
        Err(error) => zone_error_response(error, &scene_id_raw),
    }
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
    let Some(zone) = scene.groups.iter().find(|zone| zone.id == zone_id) else {
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
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };

    match zone::update_zone(
        state.as_ref(),
        zone::UpdateZone {
            scene_id,
            zone_id,
            patch: zone_update_patch(body),
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zone_response(written.zone, written.groups_revision, StatusKind::Ok),
        Err(error) => zone_error_response(error, &scene_id_raw),
    }
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
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };

    let removed = match zone::delete_zone(
        state.as_ref(),
        zone::DeleteZone {
            scene_id,
            zone_id,
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(removed) => removed,
        Err(error) => return zone_error_response(error, &scene_id_raw),
    };

    attach_groups_revision_headers(
        ApiResponse::ok(serde_json::json!({
            "zone_id": zone_id,
            "deleted": true,
            "groups_revision": removed.groups_revision,
        })),
        removed.groups_revision,
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
    for assignment in &body.device_zones {
        if let OutputAssignment::New(output) = assignment
            && let Err(error) = validate_output_sampling_radii(output)
        {
            return ApiError::validation(error);
        }
    }
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let placement = if body.preserve_placement {
        OutputPlacement::Preserve
    } else {
        OutputPlacement::AutoGrid
    };

    // Existing-output references name outputs anywhere in the scene, so
    // they resolve against the scene the caller addressed before the
    // mutation opens.
    let (scene_id, outputs) = {
        let manager = state.scene_manager.read().await;
        let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        let Some(scene) = manager.get(&scene_id) else {
            return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
        };
        match resolve_device_zone_assignments(scene, body.device_zones) {
            Ok(outputs) => (scene_id, outputs),
            Err(output_id) => {
                return ApiError::not_found(format!("Device zone not found: {output_id}"));
            }
        }
    };

    match zone::assign_outputs(
        state.as_ref(),
        zone::AssignOutputs {
            scene_id,
            zone_id,
            outputs,
            placement,
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zones_response(written.zones, written.groups_revision, StatusKind::Ok),
        Err(error) => zone_error_response(error, &scene_id_raw),
    }
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
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };

    match zone::unassign_output(
        state.as_ref(),
        zone::UnassignOutput {
            scene_id,
            zone_id,
            output_id: device_zone_id,
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zones_response(written.zones, written.groups_revision, StatusKind::Ok),
        Err(error) => zone_error_response(error, &scene_id_raw),
    }
}

/// `PUT /api/v1/scenes/{id}/zones/{zone_id}/layout` — placement-only
/// update of a zone's spatial layout. The body is a [`SpatialLayout`]; it
/// may reposition the outputs the zone already owns and retune the canvas,
/// but adds and drops route through the device endpoints (§5.1).
pub async fn update_zone_layout(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
    Json(layout): Json<SpatialLayout>,
) -> Response {
    if let Err(error) = validate_layout_sampling_radii(&layout) {
        return ApiError::validation(error);
    }
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return ApiError::bad_request("zone_id must be a valid UUID");
    };
    let expected_revision = match parse_if_match_groups_revision(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };

    let written = match zone::set_zone_layout(
        state.as_ref(),
        zone::SetZoneLayout {
            scene_id,
            zone_id,
            layout,
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => written,
        Err(error) => return zone_error_response(error, &scene_id_raw),
    };

    state.zone_layout_previews.clear(scene_id, zone_id).await;
    zone_response(written.zone, written.groups_revision, StatusKind::Ok)
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
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return ApiError::not_found(format!("Scene not found: {scene_id_raw}"));
    };

    match zone::set_unassigned_behavior(
        state.as_ref(),
        zone::SetUnassignedBehavior {
            scene_id,
            behavior: body.unassigned_behavior,
            expected_revision,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => unassigned_behavior_response(written.behavior, written.groups_revision),
        Err(error) => zone_error_response(error, &scene_id_raw),
    }
}

async fn resolve_scene(state: &AppState, scene_id_raw: &str) -> Option<SceneId> {
    let manager = state.scene_manager.read().await;
    scenes::resolve_scene_id(&manager, scene_id_raw)
}

fn zone_update_patch(request: UpdateZoneRequest) -> ZoneMetaPatch {
    ZoneMetaPatch {
        name: request.name,
        description: request.description,
        color: request.color,
        brightness: request.brightness,
        enabled: request.enabled,
        make_primary: request.make_primary,
    }
}

enum StatusKind {
    Ok,
    Created,
}

fn zones_response(groups: Vec<Zone>, groups_revision: u64, status: StatusKind) -> Response {
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

fn zone_response(zone: Zone, groups_revision: u64, status: StatusKind) -> Response {
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

fn resolve_device_zone_assignments(
    scene: &hypercolor_types::scene::Scene,
    assignments: Vec<OutputAssignment>,
) -> Result<Vec<Output>, String> {
    assignments
        .into_iter()
        .map(|assignment| match assignment {
            OutputAssignment::Existing { id } => scene
                .groups
                .iter()
                .flat_map(|group| group.layout.zones.iter())
                .find(|zone| zone.id == id)
                .cloned()
                .ok_or(id),
            OutputAssignment::New(device_zone) => Ok(*device_zone),
        })
        .collect()
}

/// The zone family's frozen v1 error prose.
///
/// The `groups_revision` precondition renders as the shape this family
/// has always sent — a bare `{ error, current }` with the revision as
/// `ETag` — rather than the canonical 412 envelope, and the not-found
/// arms name the resource the way these paths always have.
fn zone_error_response(error: DomainError, scene_label: &str) -> Response {
    match error {
        DomainError::NotFound {
            kind: ResourceKind::Scene,
            ..
        } => ApiError::not_found(format!("Scene not found: {scene_label}")),
        DomainError::NotFound {
            kind: ResourceKind::Zone,
            id,
        } => ApiError::not_found(format!("Zone not found: {id}")),
        DomainError::NotFound {
            kind: ResourceKind::Device,
            id,
        } => ApiError::not_found(format!("Device zone not found: {id}")),
        DomainError::PreconditionFailed {
            resource: ResourceKind::Zone,
            current,
            ..
        } => legacy::revision_mismatch_response("groups_revision", current),
        other => legacy::scene_family_error_response(other),
    }
}

fn parse_zone_id(raw: &str) -> Result<ZoneId, uuid::Error> {
    raw.parse::<uuid::Uuid>().map(ZoneId)
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

fn attach_groups_revision_headers(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}
