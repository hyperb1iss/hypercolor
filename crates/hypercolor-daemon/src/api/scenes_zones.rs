//! Zone lifecycle endpoints for `/api/v1/scenes/{id}/zones/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};

use hypercolor_core::scene::{OutputPlacement, ZoneMetaPatch};
use hypercolor_types::scene::{SceneId, UnassignedBehavior, Zone, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

use crate::api::envelope::ApiResponse;
use crate::api::layouts::{validate_layout_sampling_radii, validate_output_sampling_radii};
use crate::api::{AppState, scenes};
use crate::domain::zone;
use crate::domain::{DomainError, MutationContext, ResourceKind};

// Wire contracts live in hypercolor-types::api::zones — shared with the
// web UI and the TUI. OutputAssignment's untagged variant ORDER is part
// of the wire contract; see the shared definition.
pub use hypercolor_types::api::zones::{
    AssignDevicesRequest, CreateZoneRequest, DeleteZoneResponse, OutputAssignment,
    UnassignedBehaviorResponse, UpdateUnassignedBehaviorRequest, UpdateZoneRequest,
    ZoneListResponse, ZoneMutationResponse, ZoneResponse,
};

pub async fn list_zones(
    State(state): State<Arc<AppState>>,
    Path(scene_id_raw): Path<String>,
) -> Response {
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = scenes::resolve_scene_id(&manager, &scene_id_raw) else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };
    let Some(scene) = manager.get(&scene_id) else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
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
        return DomainError::validation_field("name", "zone name must not be empty")
            .into_response();
    }
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    let fallback_canvas = {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        (layout.canvas_width, layout.canvas_height)
    };

    match zone::create_zone(
        state.as_ref(),
        zone::CreateZone {
            scene: scene_id.into(),
            name: body.name,
            color: body.color,
            fallback_canvas,
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zone_response(written.zone, written.groups_revision, StatusKind::Created),
        Err(error) => error.into_response(),
    }
}

pub async fn get_zone(
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
    let Some(scene) = manager.get(&scene_id) else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };
    let Some(zone) = scene.groups.iter().find(|zone| zone.id == zone_id) else {
        return DomainError::not_found(ResourceKind::Zone, zone_id).into_response();
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
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match zone::update_zone(
        state.as_ref(),
        zone::UpdateZone {
            scene: scene_id.into(),
            zone_id,
            patch: zone_update_patch(body),
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zone_response(written.zone, written.groups_revision, StatusKind::Ok),
        Err(error) => error.into_response(),
    }
}

pub async fn delete_zone(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    let removed = match zone::delete_zone(
        state.as_ref(),
        zone::DeleteZone {
            scene: scene_id.into(),
            zone_id,
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(removed) => removed,
        Err(error) => return error.into_response(),
    };

    attach_zones_revision_headers(
        ApiResponse::ok(DeleteZoneResponse {
            zone_id,
            deleted: true,
            zones_revision: removed.groups_revision,
        }),
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
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    if body.device_zones.is_empty() {
        return DomainError::validation_field(
            "device_zones",
            "device_zones must include at least one item",
        )
        .into_response();
    }
    for assignment in &body.device_zones {
        if let OutputAssignment::New(output) = assignment
            && let Err(error) = validate_output_sampling_radii(output)
        {
            return DomainError::validation(error).into_response();
        }
    }
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let placement = if body.preserve_placement {
        OutputPlacement::Preserve
    } else {
        OutputPlacement::AutoGrid
    };

    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match zone::assign_outputs(
        state.as_ref(),
        zone::AssignOutputs {
            scene_id,
            zone_id,
            assignments: body.device_zones,
            placement,
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zones_response(written.zones, written.groups_revision, StatusKind::Ok),
        Err(error) => error.into_response(),
    }
}

pub async fn unassign_device(
    State(state): State<Arc<AppState>>,
    Path((scene_id_raw, zone_id_raw, device_zone_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match zone::unassign_output(
        state.as_ref(),
        zone::UnassignOutput {
            scene_id,
            zone_id,
            output_id: device_zone_id,
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => zones_response(written.zones, written.groups_revision, StatusKind::Ok),
        Err(error) => error.into_response(),
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
        return DomainError::validation(error).into_response();
    }
    let Ok(zone_id) = parse_zone_id(&zone_id_raw) else {
        return DomainError::malformed("zone_id must be a valid UUID").into_response();
    };
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    let written = match zone::set_zone_layout(
        state.as_ref(),
        zone::SetZoneLayout {
            scene_id,
            zone_id,
            layout,
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => written,
        Err(error) => return error.into_response(),
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
    let expected_revision = match parse_if_match_zones_revision(&headers) {
        Ok(version) => version,
        Err(message) => return DomainError::malformed(message).into_response(),
    };
    let Some(scene_id) = resolve_scene(&state, &scene_id_raw).await else {
        return DomainError::not_found(ResourceKind::Scene, &scene_id_raw).into_response();
    };

    match zone::set_unassigned_behavior(
        state.as_ref(),
        zone::SetUnassignedBehavior {
            scene_id,
            behavior: body.unassigned_behavior,
            expected_revision,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(written) => unassigned_behavior_response(written.behavior, written.groups_revision),
        Err(error) => error.into_response(),
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

fn zones_response(zones: Vec<Zone>, zones_revision: u64, status: StatusKind) -> Response {
    let body = ZoneListResponse {
        items: zones,
        zones_revision,
    };
    let response = match status {
        StatusKind::Ok => ApiResponse::ok(body),
        StatusKind::Created => ApiResponse::created(body),
    };
    attach_zones_revision_headers(response, zones_revision)
}

fn zone_response(zone: Zone, zones_revision: u64, status: StatusKind) -> Response {
    let body = ZoneResponse {
        zone,
        zones_revision,
    };
    let response = match status {
        StatusKind::Ok => ApiResponse::ok(body),
        StatusKind::Created => ApiResponse::created(body),
    };
    attach_zones_revision_headers(response, zones_revision)
}

fn unassigned_behavior_response(behavior: UnassignedBehavior, zones_revision: u64) -> Response {
    attach_zones_revision_headers(
        ApiResponse::ok(UnassignedBehaviorResponse {
            unassigned_behavior: behavior,
            zones_revision,
        }),
        zones_revision,
    )
}

fn parse_zone_id(raw: &str) -> Result<ZoneId, uuid::Error> {
    raw.parse::<uuid::Uuid>().map(ZoneId)
}

fn parse_if_match_zones_revision(headers: &HeaderMap) -> Result<Option<u64>, &'static str> {
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
        .map_err(|_| "If-Match must be a non-negative integer zones_revision")
}

fn attach_zones_revision_headers(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}
