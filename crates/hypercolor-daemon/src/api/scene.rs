//! `/api/v1/scene` — the live scene tree (Spec 78 §1).
//!
//! One resource, always present: the active scene with its zones, their
//! member device segments, and their layer stacks. Every mutation here
//! routes through `SceneMutation`/`commit_scene`, so this module is a
//! transport adapter and nothing more — it parses, calls one service,
//! and renders the canonical envelope.
//!
//! Concurrency is one token. `GET /scene` serves the commit generation
//! as `ETag`; structural mutations honor an optional `If-Match` against
//! it and answer 412 with the current revision. Control-value writes
//! take no token at all: the layer-identity lifecycle (§1.4) already
//! fences staleness, because a replaced layer answers 404 rather than
//! absorbing a patch meant for the effect it replaced.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use hypercolor_types::api::envelope::ListResponse;
use hypercolor_types::api::scene::{
    AssignMembersRequest, ClearSceneRequest, CreateLayerRequest, CreateZoneRequest,
    PatchControlsRequest, PatchZoneRequest, ReorderLayersRequest, ReplaceLayerRequest,
    ScenePatchRequest, ZoneLayoutRequest, ZoneMemberId,
};
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{ZoneId, ZoneRole};

use crate::api::AppState;
use crate::api::envelope;
use crate::domain::scene_tree::{
    AssignMembers, ClearScene, PatchLayerControls, PatchScene, ReplaceLayer, TreeWritten,
    ZoneWritten,
};
use crate::domain::{DomainError, MutationContext, ResourceKind, scene_tree};

use axum::Json;

// ── Scene document ───────────────────────────────────────────────────────

/// `GET /api/v1/scene` — the full live document.
pub async fn get_scene(State(state): State<Arc<AppState>>) -> Response {
    match scene_tree::read_document(&state.scene_tree).await {
        Ok(document) => {
            let revision = document.revision;
            with_revision(envelope::ok(document), revision)
        }
        Err(error) => error.into_response(),
    }
}

/// `PATCH /api/v1/scene` — scene-level fields.
pub async fn patch_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ScenePatchRequest>,
) -> Response {
    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };
    tree_response(
        scene_tree::patch_scene(
            &state.scene_tree,
            PatchScene {
                name: body.name,
                unassigned_behavior: body.unassigned_behavior,
                expected_revision: expected,
            },
        )
        .await,
    )
}

/// `POST /api/v1/scene/deactivate` — return to the default scene.
pub async fn deactivate_scene(State(state): State<Arc<AppState>>) -> Response {
    if let Err(error) =
        crate::domain::scene::deactivate_scene(state.as_ref(), MutationContext::api()).await
    {
        return error.into_response();
    }
    get_scene(State(state)).await
}

/// `POST /api/v1/scene/clear` — empty one zone's stack, or every one.
pub async fn clear_scene(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<ClearSceneRequest>>,
) -> Response {
    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };
    let zone = body.and_then(|Json(body)| body.zone);
    tree_response(
        scene_tree::clear_scene(
            &state.scene_tree,
            ClearScene {
                zone,
                expected_revision: expected,
            },
        )
        .await,
    )
}

// ── Zones ────────────────────────────────────────────────────────────────

/// `POST /api/v1/scene/zones` — create a zone.
pub async fn create_zone(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateZoneRequest>,
) -> Response {
    // Primary and Display zones are minted by the engine — a primary
    // when a scene is created, a display when a face is assigned — so
    // the only role this route can honor is the custom one it makes.
    if !matches!(body.role, None | Some(ZoneRole::Custom)) {
        return DomainError::validation_field(
            "role",
            "only custom zones are created through this route; \
             a primary zone is seeded with the scene and a display zone \
             is created by assigning a face",
        )
        .into_response();
    }

    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };

    let canvas = {
        let spatial = state.spatial_engine.snapshot();
        let layout = spatial.layout();
        (layout.canvas_width, layout.canvas_height)
    };

    let created = crate::domain::zone::create_zone(
        &state.scene,
        crate::domain::zone::CreateZone {
            name: body.name,
            color: body.color,
            fallback_canvas: canvas,
            expected_revision: expected,
        },
    )
    .await;

    match created {
        Ok(written) => {
            let revision = written.commit.revision();
            with_revision(
                (
                    StatusCode::CREATED,
                    envelope::ok(scene_tree::zone_resource(&written.zone)),
                )
                    .into_response(),
                revision,
            )
        }
        Err(error) => error.into_response(),
    }
}

/// `GET /api/v1/scene/zones/{zone}` — one zone resource.
pub async fn get_zone(State(state): State<Arc<AppState>>, Path(zone): Path<String>) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    match scene_tree::read_zone(&state.scene_tree, zone_id).await {
        Ok((resource, revision)) => with_revision(envelope::ok(resource), revision),
        Err(error) => error.into_response(),
    }
}

/// `PATCH /api/v1/scene/zones/{zone}` — name and structural fields.
pub async fn patch_zone(
    State(state): State<Arc<AppState>>,
    Path(zone): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PatchZoneRequest>,
) -> Response {
    let (zone_id, expected) = match zone_request(&zone, &headers) {
        Ok(parts) => parts,
        Err(error) => return error.into_response(),
    };

    zone_written_response(
        crate::domain::zone::update_zone(
            &state.scene,
            crate::domain::zone::UpdateZone {
                zone_id,
                patch: hypercolor_core::scene::ZoneMetaPatch {
                    name: body.name,
                    description: None,
                    color: body.color,
                    brightness: body.brightness,
                    enabled: body.enabled,
                    make_primary: None,
                },
                expected_revision: expected,
            },
        )
        .await,
    )
}

/// `DELETE /api/v1/scene/zones/{zone}` — remove a zone.
pub async fn delete_zone(
    State(state): State<Arc<AppState>>,
    Path(zone): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (zone_id, expected) = match zone_request(&zone, &headers) {
        Ok(parts) => parts,
        Err(error) => return error.into_response(),
    };

    if let Err(error) = crate::domain::zone::delete_zone(
        &state.scene,
        crate::domain::zone::DeleteZone {
            zone_id,
            expected_revision: expected,
        },
    )
    .await
    {
        return error.into_response();
    }
    get_scene(State(state)).await
}

/// `PUT /api/v1/scene/zones/{zone}/layout` — reposition the members.
pub async fn put_zone_layout(
    State(state): State<Arc<AppState>>,
    Path(zone): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ZoneLayoutRequest>,
) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };

    written_response(
        scene_tree::set_zone_layout(&state.scene_tree, zone_id, body.placements, expected).await,
    )
}

/// `POST /api/v1/scene/zones/{zone}/members` — assign device segments.
pub async fn assign_members(
    State(state): State<Arc<AppState>>,
    Path(zone): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AssignMembersRequest>,
) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };

    written_response(
        scene_tree::assign_members(
            &state.scene_tree,
            AssignMembers {
                zone_id,
                request: body,
                expected_revision: expected,
            },
        )
        .await,
    )
}

/// `DELETE /api/v1/scene/zones/{zone}/members/{member}` — unassign one.
pub async fn unassign_member(
    State(state): State<Arc<AppState>>,
    Path((zone, member)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };

    written_response(
        scene_tree::unassign_member(&state.scene_tree, zone_id, &ZoneMemberId(member), expected)
            .await,
    )
}

// ── Layers ───────────────────────────────────────────────────────────────

/// `GET /api/v1/scene/zones/{zone}/layers` — the zone's stack.
pub async fn list_layers(State(state): State<Arc<AppState>>, Path(zone): Path<String>) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    match scene_tree::read_zone(&state.scene_tree, zone_id).await {
        Ok((resource, revision)) => {
            let total = resource.layers.len() as u64;
            with_revision(
                envelope::ok(ListResponse {
                    items: resource.layers,
                    total,
                    page: None,
                }),
                revision,
            )
        }
        Err(error) => error.into_response(),
    }
}

/// `POST /api/v1/scene/zones/{zone}/layers` — append a layer.
pub async fn create_layer(
    State(state): State<Arc<AppState>>,
    Path(zone): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateLayerRequest>,
) -> Response {
    let (zone_id, expected) = match zone_request(&zone, &headers) {
        Ok(parts) => parts,
        Err(error) => return error.into_response(),
    };
    let layer = match build_layer(state.as_ref(), body).await {
        Ok(layer) => layer,
        Err(error) => return error.into_response(),
    };
    let inserted =
        crate::domain::layer::insert_layer(&state.scene, zone_id, layer, None, expected).await;

    match inserted {
        Ok(Ok(written)) => {
            let revision = written.commit.revision();
            with_revision(
                (
                    StatusCode::CREATED,
                    envelope::ok(scene_tree::zone_resource(written.zone())),
                )
                    .into_response(),
                revision,
            )
        }
        Ok(Err(error)) => scene_tree::layer_error(error, zone_id, None).into_response(),
        Err(error) => error.into_response(),
    }
}

/// `PATCH /api/v1/scene/zones/{zone}/layers/order` — reorder the stack.
pub async fn reorder_layers(
    State(state): State<Arc<AppState>>,
    Path(zone): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReorderLayersRequest>,
) -> Response {
    let (zone_id, expected) = match zone_request(&zone, &headers) {
        Ok(parts) => parts,
        Err(error) => return error.into_response(),
    };

    layer_stack_response(
        crate::domain::layer::reorder_layers(&state.scene, zone_id, body.order, expected).await,
        zone_id,
    )
}

/// `PUT /api/v1/scene/zones/{zone}/layers/{layer}` — whole-layer replace.
///
/// Replacement is creation: the response carries a layer id the caller
/// has never seen, and the id it addressed is gone (Spec 78 §1.4).
pub async fn replace_layer(
    State(state): State<Arc<AppState>>,
    Path((zone, layer)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<ReplaceLayerRequest>,
) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    let layer_id = match parse_layer_id(&layer) {
        Ok(layer_id) => layer_id,
        Err(error) => return error.into_response(),
    };
    let expected = match parse_if_match(&headers) {
        Ok(expected) => expected,
        Err(error) => return error.into_response(),
    };
    let replacement = match build_layer(state.as_ref(), body).await {
        Ok(layer) => layer,
        Err(error) => return error.into_response(),
    };
    written_response(
        scene_tree::replace_layer(
            &state.scene_tree,
            ReplaceLayer {
                zone_id,
                layer_id,
                layer: replacement,
                expected_revision: expected,
            },
        )
        .await,
    )
}

/// `DELETE /api/v1/scene/zones/{zone}/layers/{layer}` — drop a layer.
pub async fn delete_layer(
    State(state): State<Arc<AppState>>,
    Path((zone, layer)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (zone_id, expected) = match zone_request(&zone, &headers) {
        Ok(parts) => parts,
        Err(error) => return error.into_response(),
    };
    let layer_id = match parse_layer_id(&layer) {
        Ok(layer_id) => layer_id,
        Err(error) => return error.into_response(),
    };

    layer_stack_response(
        crate::domain::layer::remove_layer(&state.scene, zone_id, layer_id, expected).await,
        zone_id,
    )
}

/// `PATCH /api/v1/scene/zones/{zone}/layers/{layer}/controls` — the one
/// control-patch shape, unguarded by contract (Spec 78 §1.6).
pub async fn patch_layer_controls(
    State(state): State<Arc<AppState>>,
    Path((zone, layer)): Path<(String, String)>,
    Json(body): Json<PatchControlsRequest>,
) -> Response {
    let zone_id = match parse_zone_id(&zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return error.into_response(),
    };
    let layer_id = match parse_layer_id(&layer) {
        Ok(layer_id) => layer_id,
        Err(error) => return error.into_response(),
    };

    let mut values = HashMap::with_capacity(body.values.len());
    for (name, value) in body.values {
        let projected = match value.to_effect_wire() {
            Ok(value) => value,
            Err(error) => {
                return DomainError::validation_field(format!("values.{name}"), error.to_string())
                    .into_response();
            }
        };
        values.insert(name, projected);
    }
    written_response(
        scene_tree::patch_layer_controls(
            &state.scene_tree,
            PatchLayerControls {
                zone_id,
                layer_id,
                values,
                clear_bindings: body.clear_bindings,
                expected_revision: None,
            },
            MutationContext::api(),
        )
        .await,
    )
}

// ── Adapter plumbing ─────────────────────────────────────────────────────

fn tree_response(result: Result<TreeWritten, DomainError>) -> Response {
    match result {
        Ok(written) => {
            let revision = written.document.revision;
            with_revision(envelope::ok(written.document), revision)
        }
        Err(error) => error.into_response(),
    }
}

fn written_response(result: Result<ZoneWritten, DomainError>) -> Response {
    match result {
        Ok(written) => with_revision(envelope::ok(written.zone), written.revision),
        Err(error) => error.into_response(),
    }
}

fn zone_written_response(
    result: Result<crate::domain::zone::ZoneWritten, DomainError>,
) -> Response {
    match result {
        Ok(written) => with_revision(
            envelope::ok(scene_tree::zone_resource(&written.zone)),
            written.commit.revision(),
        ),
        Err(error) => error.into_response(),
    }
}

fn layer_stack_response(
    result: Result<crate::domain::layer::LayerResult, DomainError>,
    zone_id: ZoneId,
) -> Response {
    match result {
        Ok(Ok(written)) => {
            let revision = written.commit.revision();
            with_revision(
                envelope::ok(scene_tree::zone_resource(written.zone())),
                revision,
            )
        }
        Ok(Err(error)) => scene_tree::layer_error(error, zone_id, None).into_response(),
        Err(error) => error.into_response(),
    }
}

/// Attach the one wire version so every answer tells the caller what to
/// send back on its next guarded write.
pub(crate) fn with_revision(response: Response, revision: u64) -> Response {
    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(&format!("\"{revision}\"")) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// Parse the optional structural precondition.
///
/// `*` means "any current state", which is what a caller sends when it
/// wants the write to land regardless, so it reads as no precondition.
pub(crate) fn parse_if_match(headers: &HeaderMap) -> Result<Option<u64>, DomainError> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| DomainError::malformed("If-Match must be ASCII"))?
        .trim();
    if raw == "*" {
        return Ok(None);
    }
    raw.trim_matches('"')
        .parse::<u64>()
        .map(Some)
        .map_err(|_| DomainError::malformed("If-Match must be the scene revision"))
}

/// The two transport details every zone-scoped mutation parses before
/// the domain resolves the active scene from its commit candidate.
fn zone_request(zone: &str, headers: &HeaderMap) -> Result<(ZoneId, Option<u64>), DomainError> {
    let zone_id = parse_zone_id(zone)?;
    let expected = parse_if_match(headers)?;
    Ok((zone_id, expected))
}

fn parse_zone_id(raw: &str) -> Result<ZoneId, DomainError> {
    raw.parse::<uuid::Uuid>()
        .map(ZoneId)
        .map_err(|_| DomainError::not_found(ResourceKind::Zone, raw))
}

fn parse_layer_id(raw: &str) -> Result<SceneLayerId, DomainError> {
    raw.parse::<uuid::Uuid>()
        .map(SceneLayerId::from_uuid)
        .map_err(|_| DomainError::not_found(ResourceKind::Layer, raw))
}

/// Turn a creation request into a validated layer carrying a freshly
/// minted id (Spec 78 §1.4).
#[allow(
    clippy::unused_async,
    reason = "the layer factory stays async so effect-registry normalization can land without churning every call site"
)]
async fn build_layer(
    state: &AppState,
    request: CreateLayerRequest,
) -> Result<SceneLayer, DomainError> {
    let _ = state;
    let layer = SceneLayer {
        id: SceneLayerId::new(),
        name: request.name,
        source: request.source,
        blend: request.blend.unwrap_or_default(),
        opacity: request.opacity.unwrap_or(1.0),
        transform: request.transform.unwrap_or_default(),
        adjust: request.adjust.unwrap_or_default(),
        bindings: request.bindings.unwrap_or_default(),
        enabled: request.enabled.unwrap_or(true),
    }
    .normalized();
    layer.validate().map_err(|errors| {
        DomainError::validation_details(
            "Layer payload is invalid",
            serde_json::json!({ "errors": errors }),
        )
    })?;
    Ok(layer)
}
