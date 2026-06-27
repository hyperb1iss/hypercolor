//! Display-face and preview endpoints — `/api/v1/displays/*`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint, DisplayFrameFormat};
use hypercolor_types::display::{DisplayDescriptor, DisplayPixelFormat};
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata, EffectSource};
use hypercolor_types::event::ZoneChangeKind;
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, Zone};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use serde::{Deserialize, Serialize};
use tracing::warn;

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
    /// Full surface description (shape, safe area, fps, pixel format) —
    /// the same descriptor injected into face pages.
    pub descriptor: DisplayDescriptor,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DisplaySurfaceInfo {
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

/// Which assignment layer a face operation targets (spec 69 §3.6).
///
/// `default` persists across scenes (the display's own face); `scene`
/// writes into the active scene's display zone, which always wins while
/// that scene is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFaceScope {
    #[default]
    Default,
    Scene,
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
    #[serde(default)]
    pub scope: DisplayFaceScope,
}

/// Query parameters for `DELETE /api/v1/displays/{id}/face`.
#[derive(Debug, Default, Deserialize)]
pub struct DisplayFaceScopeQuery {
    #[serde(default)]
    pub scope: DisplayFaceScope,
}

/// Request body for `PATCH /api/v1/displays/{id}/face/controls`.
///
/// The payload carries only the overrides the caller wants to change;
/// existing control values on the zone are preserved unless their
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
    pub group: Zone,
    /// Which layer the returned assignment lives on.
    pub live_scope: DisplayFaceScope,
    /// Whether the active scene has its own face assignment for this display.
    pub scene_assigned: bool,
    /// Whether a persisted default face exists for this display.
    pub default_assigned: bool,
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

    let face_fps_cap = state
        .config_manager
        .as_ref()
        .map_or(crate::display_output::DISPLAY_FACE_DEFAULT_FPS, |manager| {
            manager.get().display.effective_face_fps_cap()
        });
    for tracked in tracked_devices {
        let Some(surface) = display_surface_info(&tracked.info) else {
            continue;
        };
        let target_fps = crate::display_output::capped_group_direct_display_target_fps(
            tracked.info.capabilities.max_fps,
            face_fps_cap,
        );
        let Some(descriptor) = display_descriptor_for_device(&tracked.info, target_fps) else {
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
            descriptor,
        });
    }

    displays.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    ApiResponse::ok(displays)
}

pub(crate) async fn sync_active_display_surfaces(state: &Arc<AppState>) -> bool {
    // Keep default-face overlays aligned with the preference store whenever
    // surfaces are reconciled (scene activation, display listing, reconnect).
    sync_display_preference_overlays(state).await;

    let mut displays = state
        .device_registry
        .list()
        .await
        .into_iter()
        .filter_map(|tracked| {
            let surface = display_surface_info(&tracked.info)?;
            let layout = display_face_layout(tracked.info.id, tracked.info.name.as_str(), surface);
            Some((tracked.info.id, tracked.info.name, layout))
        })
        .collect::<Vec<_>>();
    displays.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then(left.0.to_string().cmp(&right.0.to_string()))
    });

    if displays.is_empty() {
        return false;
    }

    let mut scene_manager = state.scene_manager.write().await;
    let Some(active_scene) = scene_manager.active_scene() else {
        return false;
    };
    if active_scene.blocks_runtime_mutation() {
        return false;
    }

    let mut changed = false;
    for (device_id, device_name, layout) in displays {
        let before_revision = scene_manager
            .active_scene()
            .map_or(0, |scene| scene.groups_revision);
        if let Err(error) =
            scene_manager.ensure_display_group_surface(device_id, device_name.as_str(), layout)
        {
            warn!(%error, %device_id, "Failed to sync display screen surface");
            continue;
        }
        let after_revision = scene_manager
            .active_scene()
            .map_or(before_revision, |scene| scene.groups_revision);
        changed |= after_revision != before_revision;
    }

    changed
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
///
/// Reports the live layer (active scene's zone wins over the stored
/// default) plus which layers carry an assignment.
pub async fn get_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let (scene_assigned, default_assigned) = display_face_layer_state(&state, device_id).await;
    if scene_assigned {
        return match current_display_face_assignment(&state, device_id).await {
            Ok(response) => ApiResponse::ok(Some(response)),
            Err(response) => response,
        };
    }
    if default_assigned {
        return match current_default_face_assignment(&state, device_id).await {
            Ok(response) => ApiResponse::ok(Some(response)),
            Err(response) => response,
        };
    }

    ApiResponse::ok(None::<DisplayFaceResponse>)
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

    let composition_explicit = body.blend_mode.is_some() || body.opacity.is_some();
    let mut display_target = if composition_explicit {
        DisplayFaceTarget {
            blend_mode: body.blend_mode.unwrap_or(DisplayFaceBlendMode::Alpha),
            device_id,
            opacity: body.opacity.unwrap_or(1.0),
        }
    } else {
        // No explicit composition: default to a blended overlay so the face
        // layers over the live effect instead of replacing it.
        DisplayFaceTarget {
            blend_mode: DisplayFaceBlendMode::Alpha,
            device_id,
            opacity: 1.0,
        }
    }
    .normalized();
    if !display_target.clone().blends_with_effect() {
        display_target.opacity = 1.0;
    }

    if body.scope == DisplayFaceScope::Default {
        let preference = crate::display_preferences::DisplayPreference {
            blend_mode: display_target.blend_mode,
            controls: body.controls,
            effect_id: effect.id,
            opacity: display_target.opacity,
        };
        {
            let mut store = state.display_preferences.write().await;
            store.set(device_id, preference);
            if let Err(error) = store.save() {
                warn!(%error, "Failed to persist display preferences");
            }
        }
        let Some(zone) = apply_display_preference_overlay(state.as_ref(), device_id).await else {
            return ApiError::internal("Failed to install the default face overlay");
        };

        let (scene_assigned, _) = display_face_layer_state(&state, device_id).await;
        let scene_id = {
            let scene_manager = state.scene_manager.read().await;
            scene_manager
                .active_scene()
                .map(|scene| scene.id)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT)
        };
        if !scene_assigned {
            publish_render_group_changed(state.as_ref(), scene_id, &zone, ZoneChangeKind::Updated);
        }

        return ApiResponse::ok(DisplayFaceResponse {
            default_assigned: true,
            device_id: device_id.to_string(),
            effect,
            group: zone,
            live_scope: if scene_assigned {
                DisplayFaceScope::Scene
            } else {
                DisplayFaceScope::Default
            },
            scene_assigned,
            scene_id: scene_id.to_string(),
        });
    }

    let default_assigned = {
        let store = state.display_preferences.read().await;
        store.get(device_id).is_some()
    };
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
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
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

        let response_group = if composition_explicit {
            group.clone()
        } else {
            compact_display_face_assignment_group(group.clone())
        };

        (
            active_scene_id,
            DisplayFaceResponse {
                default_assigned,
                device_id: device_id.to_string(),
                effect,
                group: response_group,
                live_scope: DisplayFaceScope::Scene,
                scene_assigned: true,
                scene_id: active_scene_id.to_string(),
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

    let (scene_assigned, default_assigned) = display_face_layer_state(&state, device_id).await;
    if !scene_assigned && default_assigned {
        {
            let mut store = state.display_preferences.write().await;
            let Some(preference) = store.get(device_id).cloned() else {
                return ApiError::not_found(format!(
                    "No display face is assigned to device {device_id}"
                ));
            };
            let mut updated = preference;
            let mut target = DisplayFaceTarget {
                blend_mode: body.blend_mode.unwrap_or(updated.blend_mode),
                device_id,
                opacity: body.opacity.unwrap_or(updated.opacity),
            }
            .normalized();
            if !target.clone().blends_with_effect() {
                target.opacity = 1.0;
            }
            updated.blend_mode = target.blend_mode;
            updated.opacity = target.opacity;
            store.set(device_id, updated);
            if let Err(error) = store.save() {
                warn!(%error, "Failed to persist display preferences");
            }
        }
        return match current_default_face_assignment(state.as_ref(), device_id).await {
            Ok(response) => {
                let scene_id = response
                    .scene_id
                    .parse::<uuid::Uuid>()
                    .map(hypercolor_types::scene::SceneId)
                    .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT);
                publish_render_group_changed(
                    state.as_ref(),
                    scene_id,
                    &response.group,
                    ZoneChangeKind::Updated,
                );
                ApiResponse::ok(response)
            }
            Err(response) => response,
        };
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
                default_assigned: {
                    let store = state.display_preferences.read().await;
                    store.get(device_id).is_some()
                },
                device_id: device_id.to_string(),
                effect,
                group: refreshed_group,
                live_scope: DisplayFaceScope::Scene,
                scene_assigned: true,
                scene_id: active_scene_id,
            },
        )
    };

    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &response.group,
        ZoneChangeKind::Updated,
    );
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(response)
}

/// `DELETE /api/v1/displays/{id}/face` — remove a face assignment.
///
/// `?scope=default` (the default) clears the persisted default face;
/// `?scope=scene` clears the active scene's assignment. Clearing the
/// default while a scene override is active changes nothing visibly
/// until the next scene switch.
pub async fn delete_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DisplayFaceScopeQuery>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    if query.scope == DisplayFaceScope::Default {
        let removed = {
            let mut store = state.display_preferences.write().await;
            let removed = store.remove(device_id).is_some();
            if removed && let Err(error) = store.save() {
                warn!(%error, "Failed to persist display preferences");
            }
            removed
        };
        let (was_live, scene_id, cleared_zone) = {
            let mut scene_manager = state.scene_manager.write().await;
            let scene_assigned = scene_manager
                .active_scene()
                .and_then(|scene| scene.display_group_for(device_id))
                .is_some_and(display_group_has_face_assignment);
            let cleared = scene_manager.default_display_group_for(device_id).cloned();
            scene_manager.remove_default_display_group(device_id);
            let scene_id = scene_manager
                .active_scene()
                .map(|scene| scene.id)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT);
            (!scene_assigned, scene_id, cleared)
        };
        if was_live && let Some(mut zone) = cleared_zone {
            zone.effect_id = None;
            zone.layers.clear();
            publish_render_group_changed(state.as_ref(), scene_id, &zone, ZoneChangeKind::Updated);
        }

        return ApiResponse::ok(serde_json::json!({
            "device_id": device_id.to_string(),
            "scope": DisplayFaceScope::Default,
            "deleted": removed,
        }));
    }
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {device}"));
    };
    let Some(surface) = display_surface_info(&tracked.info) else {
        return ApiError::validation(format!(
            "Device does not support display faces: {}",
            tracked.info.name
        ));
    };

    let (scene_id, previous_group, cleared_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let active_scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("removing a display face"),
        };
        let previous_group = scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .cloned();
        let layout = display_face_layout(device_id, tracked.info.name.as_str(), surface);
        let cleared_group = match scene_manager.clear_display_group_assignment(
            device_id,
            tracked.info.name.as_str(),
            layout,
        ) {
            Ok(group) => group.clone(),
            Err(error) => {
                return ApiError::internal(format!("Failed to update active scene: {error}"));
            }
        };
        (active_scene_id, previous_group, cleared_group)
    };

    if previous_group.is_some() {
        publish_render_group_changed(
            state.as_ref(),
            scene_id,
            &cleared_group,
            ZoneChangeKind::Updated,
        );
    } else {
        publish_render_group_changed(
            state.as_ref(),
            scene_id,
            &cleared_group,
            ZoneChangeKind::Created,
        );
    }
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "scene_id": scene_id.to_string(),
        "scope": DisplayFaceScope::Scene,
        "deleted": true,
    }))
}

/// `PATCH /api/v1/displays/{id}/face/controls` — merge control overrides
/// into the zone without replacing the face assignment itself.
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

    let (scene_assigned, default_assigned) = display_face_layer_state(&state, device_id).await;
    if !scene_assigned && default_assigned {
        let effect = match current_default_face_assignment(state.as_ref(), device_id).await {
            Ok(response) => response.effect,
            Err(response) => return response,
        };
        let (normalized_controls, rejected) =
            crate::api::effects::normalize_control_payload(&effect, &controls_object);
        if !rejected.is_empty() {
            warn!(
                face = %effect.name,
                rejected_controls = ?rejected,
                "Rejected one or more default face control updates"
            );
        }
        {
            let mut store = state.display_preferences.write().await;
            let Some(preference) = store.get(device_id).cloned() else {
                return ApiError::not_found(format!(
                    "No display face is assigned to device {device_id}"
                ));
            };
            let mut updated = preference;
            updated.controls.extend(normalized_controls);
            store.set(device_id, updated);
            if let Err(error) = store.save() {
                warn!(%error, "Failed to persist display preferences");
            }
        }
        return match current_default_face_assignment(state.as_ref(), device_id).await {
            Ok(response) => {
                let scene_id = response
                    .scene_id
                    .parse::<uuid::Uuid>()
                    .map(hypercolor_types::scene::SceneId)
                    .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT);
                publish_render_group_changed(
                    state.as_ref(),
                    scene_id,
                    &response.group,
                    ZoneChangeKind::ControlsPatched,
                );
                ApiResponse::ok(response)
            }
            Err(response) => response,
        };
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
                default_assigned: {
                    let store = state.display_preferences.read().await;
                    store.get(device_id).is_some()
                },
                device_id: device_id.to_string(),
                effect,
                group: refreshed_group,
                live_scope: DisplayFaceScope::Scene,
                scene_assigned: true,
                scene_id: active_scene_id,
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
        ZoneChangeKind::ControlsPatched,
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
    httpdate::parse_http_date(value).ok()
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

    let default_assigned = {
        let store = state.display_preferences.read().await;
        store.get(device_id).is_some()
    };

    Ok(DisplayFaceResponse {
        default_assigned,
        device_id: device_id.to_string(),
        effect,
        group,
        live_scope: DisplayFaceScope::Scene,
        scene_assigned: true,
        scene_id: scene_id.to_string(),
    })
}

/// Build the runtime-only default zone a preference materializes into.
fn build_default_display_zone(
    device_id: DeviceId,
    device_name: &str,
    effect_id: hypercolor_types::effect::EffectId,
    preference: &crate::display_preferences::DisplayPreference,
    layout: SpatialLayout,
) -> Zone {
    Zone {
        id: hypercolor_types::scene::ZoneId::new(),
        name: format!("{device_name} Face"),
        description: Some(format!("Default face for {device_name}")),
        effect_id: Some(effect_id),
        controls: preference.controls.clone(),
        control_bindings: std::collections::HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(
            DisplayFaceTarget {
                blend_mode: preference.blend_mode,
                device_id,
                opacity: preference.opacity,
            }
            .normalized(),
        ),
        role: hypercolor_types::scene::ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    }
}

/// Install (or refresh) the runtime default zone for one display from its
/// stored preference. Removes the overlay when the preference is gone or
/// its effect no longer resolves. Returns the installed zone, if any.
pub(crate) async fn apply_display_preference_overlay(
    state: &AppState,
    device_id: DeviceId,
) -> Option<Zone> {
    let preference = {
        let store = state.display_preferences.read().await;
        store.get(device_id).cloned()
    };
    let Some(preference) = preference else {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager.remove_default_display_group(device_id);
        return None;
    };

    let tracked = state.device_registry.get(&device_id).await?;
    let surface = display_surface_info(&tracked.info)?;
    let effect_resolves = {
        let registry = state.effect_registry.read().await;
        registry.get(&preference.effect_id).is_some()
    };
    if !effect_resolves {
        warn!(
            %device_id,
            effect_id = %preference.effect_id,
            "Default display face effect is not installed; skipping overlay"
        );
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager.remove_default_display_group(device_id);
        return None;
    }

    let zone = build_default_display_zone(
        device_id,
        tracked.info.name.as_str(),
        preference.effect_id,
        &preference,
        display_face_layout(device_id, tracked.info.name.as_str(), surface),
    );
    let mut scene_manager = state.scene_manager.write().await;
    scene_manager.set_default_display_group(zone);
    scene_manager.default_display_group_for(device_id).cloned()
}

/// Reconcile every connected display's default-face overlay with the
/// preference store. Runs alongside surface sync (scene activation and
/// display listing) so defaults follow devices as they appear.
pub(crate) async fn sync_display_preference_overlays(state: &Arc<AppState>) {
    let device_ids = {
        let store = state.display_preferences.read().await;
        store
            .iter()
            .map(|(device_id, _)| device_id)
            .collect::<Vec<_>>()
    };
    for device_id in device_ids {
        apply_display_preference_overlay(state.as_ref(), device_id).await;
    }
}

/// Resolve both assignment layers for a display.
async fn display_face_layer_state(state: &AppState, device_id: DeviceId) -> (bool, bool) {
    let scene_assigned = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .is_some_and(display_group_has_face_assignment)
    };
    let default_assigned = {
        let store = state.display_preferences.read().await;
        store.get(device_id).is_some()
    };
    (scene_assigned, default_assigned)
}

/// Current assignment for the *default* layer, materialized from the
/// preference store and the runtime overlay zone.
async fn current_default_face_assignment(
    state: &AppState,
    device_id: DeviceId,
) -> Result<DisplayFaceResponse, Response> {
    let Some(zone) = apply_display_preference_overlay(state, device_id).await else {
        return Err(ApiError::not_found(format!(
            "No default face is stored for device {device_id}"
        )));
    };
    let Some(effect_id) = zone.effect_id else {
        return Err(ApiError::internal("Default face zone has no effect"));
    };
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(entry) = registry.get(&effect_id) else {
            return Err(ApiError::not_found(format!(
                "Default display face effect not found: {effect_id}"
            )));
        };
        entry.metadata.clone()
    };
    let scene_id = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager.active_scene().map_or_else(
            || hypercolor_types::scene::SceneId::DEFAULT.to_string(),
            |scene| scene.id.to_string(),
        )
    };

    Ok(DisplayFaceResponse {
        default_assigned: true,
        device_id: device_id.to_string(),
        effect,
        group: zone,
        live_scope: DisplayFaceScope::Default,
        scene_assigned: false,
        scene_id,
    })
}

fn compact_display_face_assignment_group(mut group: Zone) -> Zone {
    if let Some(target) = group.display_target.as_mut()
        && target.blend_mode == DisplayFaceBlendMode::Replace
        && (target.opacity - 1.0).abs() <= f32::EPSILON
    {
        target.blend_mode = DisplayFaceBlendMode::Alpha;
    }
    group
}

fn display_group_has_face_assignment(group: &Zone) -> bool {
    group.effect_id.is_some()
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

/// Build the API-facing descriptor for a display device — the same shared
/// derivation that feeds the face-page injection.
pub(crate) fn display_descriptor_for_device(
    info: &DeviceInfo,
    target_fps: u32,
) -> Option<DisplayDescriptor> {
    let surface = display_surface_info(info)?;
    let pixel_format = info
        .zones
        .iter()
        .find_map(|zone| match zone.topology {
            DeviceTopologyHint::Display { .. } => Some(
                DisplayFrameFormat::from_device_color_format(zone.color_format),
            ),
            _ => None,
        })
        .map_or(DisplayPixelFormat::Yuv420, |format| match format {
            DisplayFrameFormat::Rgb => DisplayPixelFormat::Rgb,
            DisplayFrameFormat::Jpeg => DisplayPixelFormat::Yuv420,
        });

    Some(DisplayDescriptor::derive(
        surface.width,
        surface.height,
        surface.circular,
        None,
        target_fps,
        pixel_format,
    ))
}

fn effect_source_is_html(source: &EffectSource) -> bool {
    matches!(source, EffectSource::Html { .. })
}
