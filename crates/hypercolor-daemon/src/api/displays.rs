//! Display-face and preview endpoints — `/api/v1/displays/*`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint};
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata, EffectSource};
use hypercolor_types::event::RenderGroupChangeKind;
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, RenderGroup};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::devices;
use crate::api::effects::resolve_effect_metadata;
use crate::api::envelope::ApiError;
use crate::api::envelope::ApiResponse;
use crate::api::{active_scene_id_for_runtime_mutation, publish_render_group_changed};
use crate::display_frames::DisplayFrameSnapshot;

#[derive(Debug, Clone, Serialize)]
pub struct DisplaySummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub family: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DisplaySurfaceInfo {
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetDisplayFaceRequest {
    pub effect_id: String,
    #[serde(default)]
    pub controls: std::collections::HashMap<String, ControlValue>,
    #[serde(default)]
    pub blend_mode: Option<DisplayFaceBlendMode>,
    #[serde(default)]
    pub opacity: Option<f32>,
}

/// Request body for `PATCH /api/v1/displays/{id}/face/controls`.
///
/// The payload carries only the overrides the caller wants to change;
/// existing control values on the render group are preserved unless their
/// key appears in this map. `controls` is typed as raw JSON (rather than
/// `HashMap<String, ControlValue>`) so callers can send natural shapes
/// like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
/// mirrors the effects controls patch endpoint.
#[derive(Debug, Deserialize)]
pub struct UpdateDisplayFaceControlsRequest {
    #[serde(default)]
    pub controls: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDisplayFaceCompositionRequest {
    #[serde(default)]
    pub blend_mode: Option<DisplayFaceBlendMode>,
    #[serde(default)]
    pub opacity: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplayFaceResponse {
    pub device_id: String,
    pub scene_id: String,
    pub effect: EffectMetadata,
    pub group: RenderGroup,
}

struct OwnedDisplayJpeg(Arc<Vec<u8>>);

impl AsRef<[u8]> for OwnedDisplayJpeg {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().as_slice()
    }
}

pub async fn list_displays(State(state): State<Arc<AppState>>) -> Response {
    let tracked_devices = state.device_registry.list().await;
    let mut displays = Vec::new();

    for tracked in tracked_devices {
        let Some(surface) = display_surface_info(&tracked.info) else {
            continue;
        };
        displays.push(DisplaySummary {
            id: tracked.info.id.to_string(),
            name: tracked.info.name.clone(),
            vendor: tracked.info.vendor.clone(),
            family: tracked.info.family.to_string(),
            width: surface.width,
            height: surface.height,
            circular: surface.circular,
        });
    }

    displays.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    ApiResponse::ok(displays)
}

/// `GET /api/v1/displays/{id}/preview.jpg` — latest composited frame for a display.
///
/// Honors `If-None-Match` (ETag derived from the monotonic frame counter) and
/// `If-Modified-Since` (derived from the capture timestamp) so polling clients
/// can re-fetch cheaply during idle periods. Returns `404` when the display has
/// not yet produced a frame.
pub async fn get_display_preview(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    headers: HeaderMap,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(frame) = state.display_frames.read().await.frame(device_id) else {
        return ApiError::not_found(format!("Display preview frame not available: {device_id}"));
    };

    let etag = format_display_preview_etag(device_id, frame.frame_number);
    let last_modified = http_date(frame.captured_at);

    if client_cache_is_current(&headers, &etag, frame.captured_at) {
        let mut not_modified = StatusCode::NOT_MODIFIED.into_response();
        let response_headers = not_modified.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response_headers.insert(header::ETAG, value);
        }
        if let Ok(value) = HeaderValue::from_str(&last_modified) {
            response_headers.insert(header::LAST_MODIFIED, value);
        }
        response_headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=0, must-revalidate"),
        );
        return not_modified;
    }

    display_preview_response(&etag, &last_modified, &frame)
}

/// `GET /api/v1/displays/{id}/face` — current face assignment for a display.
pub async fn get_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let has_face_assignment = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .is_some()
    };
    if !has_face_assignment {
        return ApiResponse::ok(None::<DisplayFaceResponse>);
    }

    match current_display_face_assignment(&state, device_id).await {
        Ok(response) => ApiResponse::ok(Some(response)),
        Err(response) => response,
    }
}

/// `PUT /api/v1/displays/{id}/face` — assign or update a face in the active scene.
pub async fn set_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<SetDisplayFaceRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {device}"));
    };
    let Some(surface) = display_surface_info(&tracked.info) else {
        return ApiError::validation(format!(
            "Device does not support display faces: {}",
            tracked.info.name
        ));
    };

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect_id) else {
            return ApiError::not_found(format!("Effect not found: {}", body.effect_id));
        };
        if effect.category != EffectCategory::Display {
            return ApiError::validation(format!("Effect '{}' is not a display face", effect.name));
        }
        if !effect_source_is_html(&effect.source) {
            return ApiError::validation(format!(
                "Effect '{}' is not an HTML display face",
                effect.name
            ));
        }
        effect
    };

    let mut display_target = DisplayFaceTarget {
        blend_mode: body.blend_mode.unwrap_or(DisplayFaceBlendMode::Replace),
        device_id,
        opacity: body.opacity.unwrap_or(1.0),
    }
    .normalized();
    if !display_target.clone().blends_with_effect() {
        display_target.opacity = 1.0;
    }

    let (scene_id, response, change_kind) = {
        let mut scene_manager = state.scene_manager.write().await;
        let active_scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("assigning a display face"),
        };
        let change_kind = if scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .is_some()
        {
            RenderGroupChangeKind::Updated
        } else {
            RenderGroupChangeKind::Created
        };
        let group = match scene_manager.upsert_display_group(
            device_id,
            tracked.info.name.as_str(),
            &effect,
            body.controls,
            display_face_layout(device_id, tracked.info.name.as_str(), surface),
        ) {
            Ok(group) => group.clone(),
            Err(error) => {
                return ApiError::internal(format!("Failed to update active scene: {error}"));
            }
        };
        let Some(group) = scene_manager.patch_display_group_target(
            group.id,
            Some(display_target.blend_mode),
            Some(display_target.opacity),
        ) else {
            return ApiError::internal("Failed to update display face composition");
        };

        (
            active_scene_id,
            DisplayFaceResponse {
                device_id: device_id.to_string(),
                scene_id: active_scene_id.to_string(),
                effect,
                group: group.clone(),
            },
            change_kind,
        )
    };

    publish_render_group_changed(state.as_ref(), scene_id, &response.group, change_kind);
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(response)
}

/// `PATCH /api/v1/displays/{id}/face/composition` — update how the assigned
/// face composes with the effect layer beneath it.
pub async fn patch_display_face_composition(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<UpdateDisplayFaceCompositionRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    if body.blend_mode.is_none() && body.opacity.is_none() {
        return ApiError::bad_request("composition payload must include blend_mode or opacity");
    }

    let (scene_id, response) = {
        let (active_scene_id, group, effect) =
            match current_display_face_assignment(state.as_ref(), device_id).await {
                Ok(response) => {
                    let scene_id = response.scene_id.clone();
                    (scene_id, response.group, response.effect)
                }
                Err(response) => return response,
            };
        {
            let mut scene_manager = state.scene_manager.write().await;
            if let Err(error) = active_scene_id_for_runtime_mutation(&scene_manager) {
                return error.api_response("updating display face composition");
            }
            if scene_manager
                .patch_display_group_target(group.id, body.blend_mode, body.opacity)
                .is_none()
            {
                return ApiError::not_found(format!(
                    "No display face is assigned to device {device_id}"
                ));
            }
        }
        let refreshed_group = match current_display_face_assignment(state.as_ref(), device_id).await
        {
            Ok(response) => response.group,
            Err(response) => return response,
        };

        (
            active_scene_id
                .parse::<uuid::Uuid>()
                .map(hypercolor_types::scene::SceneId)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT),
            DisplayFaceResponse {
                device_id: device_id.to_string(),
                scene_id: active_scene_id,
                effect,
                group: refreshed_group,
            },
        )
    };

    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &response.group,
        RenderGroupChangeKind::Updated,
    );
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(response)
}

/// `DELETE /api/v1/displays/{id}/face` — remove the active-scene face assignment.
pub async fn delete_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let (scene_id, removed_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let active_scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("removing a display face"),
        };
        let removed_group = scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .cloned();
        if let Err(error) = scene_manager.remove_display_group(device_id) {
            return ApiError::internal(format!("Failed to update active scene: {error}"));
        }
        (active_scene_id, removed_group)
    };

    if let Some(removed_group) = removed_group.as_ref() {
        publish_render_group_changed(
            state.as_ref(),
            scene_id,
            removed_group,
            RenderGroupChangeKind::Removed,
        );
    }
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "scene_id": scene_id.to_string(),
        "deleted": true,
    }))
}

/// `PATCH /api/v1/displays/{id}/face/controls` — merge control overrides
/// into the render group without replacing the face assignment itself.
///
/// Returns the full `DisplayFaceResponse` so callers can reconcile their
/// optimistic local state with the authoritative values the daemon
/// persisted (defaults are resolved server-side, colors are normalized,
/// etc.). Individual raw JSON values are converted via the shared
/// `json_to_control_value` helper — unsupported shapes are reported in
/// the `rejected` array instead of silently dropped.
pub async fn patch_display_face_controls(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<UpdateDisplayFaceControlsRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let controls_object = body
        .controls
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    if controls_object.is_empty() {
        return ApiError::bad_request("controls payload must include at least one key");
    }

    let mut rejected: Vec<String> = Vec::new();
    let (scene_id, response, effect_name) = {
        let (active_scene_id, group, effect) =
            match current_display_face_assignment(state.as_ref(), device_id).await {
                Ok(response) => {
                    let scene_id = response.scene_id.clone();
                    (scene_id, response.group, response.effect)
                }
                Err(response) => return response,
            };
        let (normalized_controls, invalid) =
            crate::api::effects::normalize_control_payload(&effect, &controls_object);
        rejected.extend(invalid);
        {
            let mut scene_manager = state.scene_manager.write().await;
            if let Err(error) = active_scene_id_for_runtime_mutation(&scene_manager) {
                return error.api_response("updating display face controls");
            }
            if scene_manager
                .patch_group_controls(group.id, normalized_controls)
                .is_none()
            {
                return ApiError::not_found(format!(
                    "No display face is assigned to device {device_id}"
                ));
            }
        }
        let effect_name = effect.name.clone();
        let refreshed_group = match current_display_face_assignment(state.as_ref(), device_id).await
        {
            Ok(response) => response.group,
            Err(response) => return response,
        };

        (
            active_scene_id
                .parse::<uuid::Uuid>()
                .map(hypercolor_types::scene::SceneId)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT),
            DisplayFaceResponse {
                device_id: device_id.to_string(),
                scene_id: active_scene_id,
                effect,
                group: refreshed_group,
            },
            effect_name,
        )
    };

    if !rejected.is_empty() {
        tracing::warn!(
            face = %effect_name,
            rejected_controls = ?rejected,
            "Rejected one or more display face control updates"
        );
    }

    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &response.group,
        RenderGroupChangeKind::ControlsPatched,
    );
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(response)
}

fn display_preview_response(
    etag: &str,
    last_modified: &str,
    frame: &DisplayFrameSnapshot,
) -> Response {
    let jpeg_body = Bytes::from_owner(OwnedDisplayJpeg(Arc::clone(&frame.jpeg_data)));
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"))],
        jpeg_body,
    )
        .into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    if let Ok(value) = HeaderValue::from_str(last_modified) {
        headers.insert(header::LAST_MODIFIED, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0, must-revalidate"),
    );
    headers.insert("X-Display-Frame-Number", frame.frame_number.into());
    if let Ok(value) = HeaderValue::from_str(&frame.width.to_string()) {
        headers.insert("X-Display-Width", value);
    }
    if let Ok(value) = HeaderValue::from_str(&frame.height.to_string()) {
        headers.insert("X-Display-Height", value);
    }
    headers.insert(
        "X-Display-Circular",
        HeaderValue::from_static(if frame.circular { "1" } else { "0" }),
    );
    response
}

fn format_display_preview_etag(device_id: DeviceId, frame_number: u64) -> String {
    format!("\"{device_id}-{frame_number}\"")
}

fn client_cache_is_current(headers: &HeaderMap, etag: &str, captured_at: SystemTime) -> bool {
    // RFC 7232 §6: when `If-None-Match` is present, a recipient MUST NOT
    // perform `If-Modified-Since`. We honor that here — if the client sent
    // `If-None-Match` we only care whether the etag matches; we never fall
    // back to the timestamp test. This matters because display frames can
    // advance multiple times within the same HTTP-date second.
    if let Some(value) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        return value
            .split(',')
            .map(str::trim)
            .any(|candidate| candidate == etag);
    }
    if let Some(value) = headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        && let Some(since) = parse_http_date(value)
        && let Ok(captured_secs) = captured_at.duration_since(UNIX_EPOCH)
        && let Ok(since_secs) = since.duration_since(UNIX_EPOCH)
    {
        return captured_secs.as_secs() <= since_secs.as_secs();
    }
    false
}

fn http_date(time: SystemTime) -> String {
    httpdate::fmt_http_date(time)
}

fn parse_http_date(value: &str) -> Option<SystemTime> {
    httpdate::parse_http_date(value).ok().map(|time| {
        // httpdate rounds to whole seconds, so we round-trip through Duration
        // to keep arithmetic deterministic in tests.
        let _ = Duration::from_secs(0);
        time
    })
}

async fn resolve_display_device_id_or_response(
    state: &Arc<AppState>,
    id_or_name: &str,
) -> Result<DeviceId, Response> {
    let device_id = devices::resolve_device_id_or_response(state, id_or_name).await?;
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return Err(ApiError::not_found(format!(
            "Device not found: {id_or_name}"
        )));
    };
    if display_surface_info(&tracked.info).is_none() {
        return Err(ApiError::validation(format!(
            "Device does not support display faces: {}",
            tracked.info.name
        )));
    }
    Ok(device_id)
}

async fn current_display_face_assignment(
    state: &AppState,
    device_id: DeviceId,
) -> Result<DisplayFaceResponse, Response> {
    let (scene_id, group) = {
        let scene_manager = state.scene_manager.read().await;
        let Some(active_scene) = scene_manager.active_scene() else {
            return Err(ApiError::not_found(
                "No active scene has a display face assignment".to_owned(),
            ));
        };
        let Some(group) = active_scene.display_group_for(device_id).cloned() else {
            return Err(ApiError::not_found(format!(
                "No display face is assigned to device {device_id}"
            )));
        };
        (active_scene.id, group)
    };

    let Some(effect_id) = group.effect_id else {
        return Err(ApiError::not_found(format!(
            "Display face group {} has no assigned effect",
            group.id
        )));
    };
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(entry) = registry.get(&effect_id) else {
            return Err(ApiError::not_found(format!(
                "Assigned display face effect not found: {effect_id}"
            )));
        };
        entry.metadata.clone()
    };

    Ok(DisplayFaceResponse {
        device_id: device_id.to_string(),
        scene_id: scene_id.to_string(),
        effect,
        group,
    })
}

pub(crate) fn display_face_layout(
    device_id: DeviceId,
    device_name: &str,
    surface: DisplaySurfaceInfo,
) -> SpatialLayout {
    SpatialLayout {
        id: format!("display-face:{device_id}"),
        name: format!("{device_name} Display Face"),
        description: Some(format!("Native-resolution face canvas for {device_name}")),
        canvas_width: surface.width,
        canvas_height: surface.height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

pub(crate) fn display_surface_info(info: &DeviceInfo) -> Option<DisplaySurfaceInfo> {
    for zone in &info.zones {
        if let DeviceTopologyHint::Display {
            width,
            height,
            circular,
        } = &zone.topology
        {
            return Some(DisplaySurfaceInfo {
                width: *width,
                height: *height,
                circular: *circular,
            });
        }
    }

    info.capabilities
        .display_resolution
        .filter(|_| info.capabilities.has_display)
        .map(|(width, height)| DisplaySurfaceInfo {
            width,
            height,
            circular: false,
        })
}

fn effect_source_is_html(source: &EffectSource) -> bool {
    matches!(source, EffectSource::Html { .. })
}
