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
use hypercolor_types::effect::{EffectCategory, EffectSource};
use hypercolor_types::event::ZoneChangeKind;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, Zone};
use hypercolor_types::spatial::SpatialLayout;
use tracing::warn;

use crate::api::devices;
use crate::api::envelope;
use crate::api::publish_render_group_changed;
use crate::app_state::AppState;
use crate::display_frames::DisplayFrameSnapshot;
pub(crate) use crate::domain::display::{
    DisplaySurfaceInfo, display_face_layout, display_surface_info,
};
use crate::domain::{DomainError, ResourceKind};

pub use hypercolor_types::api::displays::{
    DeleteDisplayFaceResponse, DisplayFaceResponse, DisplayFaceScope, DisplayFaceScopeQuery,
    DisplaySummary, SetDisplayFaceRequest, UpdateDisplayFaceCompositionRequest,
};
use hypercolor_types::api::scene::PatchControlsRequest;

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
    envelope::ok(displays)
}

/// `GET /api/v1/displays/{id}/frame` — latest composited frame for a display.
///
/// Honors `If-None-Match` (ETag derived from the monotonic frame counter) and
/// `If-Modified-Since` (derived from the capture timestamp) so polling clients
/// can re-fetch cheaply during idle periods. Returns `404` when the display has
/// not yet produced a frame.
pub async fn get_display_frame(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    headers: HeaderMap,
) -> Response {
    let device_id = match resolve_display_device_id_or_error(&state, &device).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(frame) = state.display_frames.read().await.frame(device_id) else {
        return DomainError::not_found(ResourceKind::DisplayFrame, device_id).into_response();
    };

    let etag = format_display_frame_etag(device_id, frame.frame_number);
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

    display_frame_response(&etag, &last_modified, &frame)
}

/// `GET /api/v1/displays/{id}/face` — current face assignment for a display.
///
/// Reports the live layer (active scene's zone wins over the stored
/// default) plus which layers carry an assignment.
pub async fn get_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
) -> Response {
    let device_id = match resolve_display_device_id_or_error(&state, &device).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let (scene_assigned, default_assigned) = display_face_layer_state(&state, device_id).await;
    if scene_assigned {
        return match current_display_face_assignment(&state, device_id).await {
            Ok(response) => envelope::ok(Some(response)),
            Err(error) => error.into_response(),
        };
    }
    if default_assigned {
        return match current_default_face_assignment(&state, device_id).await {
            Ok(response) => envelope::ok(Some(response)),
            Err(error) => error.into_response(),
        };
    }

    envelope::ok(None::<DisplayFaceResponse>)
}

/// `PUT /api/v1/displays/{id}/face` — assign or update a face in the active scene.
pub async fn set_display_face(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<SetDisplayFaceRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_error(&state, &device).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &device).into_response();
    };
    let Some(surface) = display_surface_info(&tracked.info) else {
        return DomainError::validation(format!(
            "Device does not support display faces: {}",
            tracked.info.name
        ))
        .into_response();
    };

    let effect = {
        let Some(effect) = state
            .domains
            .effects
            .resolve_for_mutation(&body.effect_id)
            .await
        else {
            return DomainError::not_found(ResourceKind::Effect, &body.effect_id).into_response();
        };
        if effect.category != EffectCategory::Display {
            return DomainError::validation(format!(
                "Effect '{}' is not a display face",
                effect.name
            ))
            .into_response();
        }
        if !effect_source_is_html(&effect.source) {
            return DomainError::validation(format!(
                "Effect '{}' is not an HTML display face",
                effect.name
            ))
            .into_response();
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
        let _admission = match state.domains.effects.admit(&effect).await {
            Ok(admission) => admission,
            Err(error) => return error.into_response(),
        };
        let preference = crate::display_preferences::DisplayPreference {
            blend_mode: display_target.blend_mode,
            controls: body.controls,
            effect_id: effect.id,
            opacity: display_target.opacity,
        };
        {
            let mut store = state.display_preferences.write().await;
            if let Err(error) = store.set(device_id, preference) {
                return DomainError::Internal(anyhow::anyhow!(
                    "Failed to prepare display preference persistence: {error}"
                ))
                .into_response();
            }
        }
        let Some(zone) = apply_display_preference_overlay_admitted(state.as_ref(), device_id).await
        else {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to install the default face overlay"
            ))
            .into_response();
        };

        let (scene_assigned, _) = display_face_layer_state(&state, device_id).await;
        let scene_id = {
            let scene_manager = state.scene_manager.snapshot().await;
            scene_manager
                .active_scene()
                .map(|scene| scene.id)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT)
        };
        if !scene_assigned {
            publish_render_group_changed(state.as_ref(), scene_id, &zone, ZoneChangeKind::Updated);
        }

        return envelope::ok(DisplayFaceResponse {
            default_assigned: true,
            device_id: device_id.to_string(),
            effect: effect.into_metadata(),
            zone,
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

    let written = match crate::domain::display::set_display_face(
        &state.domains.effects,
        crate::domain::display::SetDisplayFace {
            device_id,
            device_name: tracked.info.name.clone(),
            effect: effect.clone(),
            controls: body.controls,
            layout: display_face_layout(device_id, tracked.info.name.as_str(), surface),
            target: display_target,
        },
    )
    .await
    {
        Ok(written) => written,
        Err(error) => return error.into_response(),
    };

    envelope::ok(DisplayFaceResponse {
        default_assigned,
        device_id: device_id.to_string(),
        effect: effect.into_metadata(),
        zone: if composition_explicit {
            written.zone
        } else {
            compact_display_face_assignment_zone(written.zone)
        },
        live_scope: DisplayFaceScope::Scene,
        scene_assigned: true,
        scene_id: written.scene_id.to_string(),
    })
}

/// `PATCH /api/v1/displays/{id}/face/composition` — update how the assigned
/// face composes with the effect layer beneath it.
pub async fn patch_display_face_composition(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<UpdateDisplayFaceCompositionRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_error(&state, &device).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    if body.blend_mode.is_none() && body.opacity.is_none() {
        return DomainError::validation("composition payload must include blend_mode or opacity")
            .into_response();
    }

    let (scene_assigned, default_assigned) = display_face_layer_state(&state, device_id).await;
    if !scene_assigned && default_assigned {
        {
            let mut store = state.display_preferences.write().await;
            let Some(preference) = store.get(device_id).cloned() else {
                return DomainError::not_found(
                    ResourceKind::Zone,
                    format!("default-face:{device_id}"),
                )
                .into_response();
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
            if let Err(error) = store.set(device_id, updated) {
                return DomainError::Internal(anyhow::anyhow!(
                    "Failed to prepare display preference persistence: {error}"
                ))
                .into_response();
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
                    &response.zone,
                    ZoneChangeKind::Updated,
                );
                envelope::ok(response)
            }
            Err(error) => error.into_response(),
        };
    }

    let (group, effect) = match current_display_face_assignment(state.as_ref(), device_id).await {
        Ok(response) => (response.zone, response.effect),
        Err(error) => return error.into_response(),
    };

    let written = match crate::domain::display::patch_display_composition(
        &state.domains.scene,
        crate::domain::display::PatchDisplayComposition {
            zone_id: group.id,
            blend_mode: body.blend_mode,
            opacity: body.opacity,
        },
    )
    .await
    {
        Ok(Some(written)) => written,
        Ok(None) => {
            return DomainError::not_found(ResourceKind::Zone, format!("display-face:{device_id}"))
                .into_response();
        }
        Err(error) => return error.into_response(),
    };

    envelope::ok(DisplayFaceResponse {
        default_assigned: {
            let store = state.display_preferences.read().await;
            store.get(device_id).is_some()
        },
        device_id: device_id.to_string(),
        effect,
        zone: written.zone,
        live_scope: DisplayFaceScope::Scene,
        scene_assigned: true,
        scene_id: written.scene_id.to_string(),
    })
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
    let device_id = match resolve_display_device_id_or_error(&state, &device).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    if query.scope == DisplayFaceScope::Default {
        let removed = {
            let mut store = state.display_preferences.write().await;
            match store.remove(device_id) {
                Ok(removed) => removed.is_some(),
                Err(error) => {
                    return DomainError::Internal(anyhow::anyhow!(
                        "Failed to prepare display preference persistence: {error}"
                    ))
                    .into_response();
                }
            }
        };
        let scene_assigned = {
            let scene_manager = state.scene_manager.snapshot().await;
            scene_manager
                .active_scene()
                .and_then(|scene| scene.display_zone_for(device_id))
                .is_some_and(display_zone_has_face_assignment)
        };
        match crate::domain::display::remove_default_display_overlay(
            &state.domains.scene,
            device_id,
        )
        .await
        {
            Ok(cleared) => {
                if removed
                    && !scene_assigned
                    && let Some(mut zone) = cleared
                {
                    zone.layers.clear();
                    let scene_id = {
                        let scene_manager = state.scene_manager.snapshot().await;
                        scene_manager
                            .active_scene()
                            .map_or(hypercolor_types::scene::SceneId::DEFAULT, |scene| scene.id)
                    };
                    publish_render_group_changed(
                        state.as_ref(),
                        scene_id,
                        &zone,
                        ZoneChangeKind::Updated,
                    );
                }
            }
            Err(error) => return error.into_response(),
        }

        return envelope::ok(DeleteDisplayFaceResponse {
            device_id: device_id.to_string(),
            scene_id: None,
            scope: DisplayFaceScope::Default,
            deleted: removed,
        });
    }
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &device).into_response();
    };
    let Some(surface) = display_surface_info(&tracked.info) else {
        return DomainError::validation(format!(
            "Device does not support display faces: {}",
            tracked.info.name
        ))
        .into_response();
    };

    let cleared = match crate::domain::display::clear_display_face(
        &state.domains.scene,
        crate::domain::display::ClearDisplayFace {
            device_id,
            device_name: tracked.info.name.clone(),
            layout: display_face_layout(device_id, tracked.info.name.as_str(), surface),
        },
    )
    .await
    {
        Ok(cleared) => cleared,
        Err(error) => return error.into_response(),
    };

    envelope::ok(DeleteDisplayFaceResponse {
        device_id: device_id.to_string(),
        scene_id: Some(cleared.scene_id.to_string()),
        scope: DisplayFaceScope::Scene,
        deleted: true,
    })
}

/// `PATCH /api/v1/displays/{id}/face/controls` — merge control overrides
/// into the zone without replacing the face assignment itself.
///
/// Returns the full `DisplayFaceResponse` so callers can reconcile their
/// optimistic local state with the authoritative values the daemon
/// persisted (defaults are resolved server-side and colors are normalized).
pub async fn patch_display_face_controls(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<PatchControlsRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_error(&state, &device).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    if !body.clear_bindings.is_empty() {
        return DomainError::validation_field(
            "clear_bindings",
            "display faces do not support input bindings",
        )
        .into_response();
    }
    if body.values.is_empty() {
        return DomainError::validation_field(
            "values",
            "values payload must include at least one key",
        )
        .into_response();
    }

    let mut requested_controls = std::collections::HashMap::with_capacity(body.values.len());
    for (name, value) in body.values {
        let value = match value.to_effect_wire() {
            Ok(value) => value,
            Err(error) => {
                return DomainError::validation_field(format!("values.{name}"), error.to_string())
                    .into_response();
            }
        };
        requested_controls.insert(name, value);
    }

    let (scene_assigned, default_assigned) = display_face_layer_state(&state, device_id).await;
    if !scene_assigned && default_assigned {
        let effect = match current_default_face_assignment(state.as_ref(), device_id).await {
            Ok(response) => response.effect,
            Err(error) => return error.into_response(),
        };
        let (normalized_controls, rejected) =
            crate::domain::effect::normalize_control_values(&effect, &requested_controls);
        if !rejected.is_empty() {
            return DomainError::validation_details(
                "Invalid display face control values",
                serde_json::json!({ "rejected": rejected }),
            )
            .into_response();
        }
        {
            let mut store = state.display_preferences.write().await;
            let Some(preference) = store.get(device_id).cloned() else {
                return DomainError::not_found(
                    ResourceKind::Zone,
                    format!("default-face:{device_id}"),
                )
                .into_response();
            };
            let mut updated = preference;
            updated.controls.extend(normalized_controls);
            if let Err(error) = store.set(device_id, updated) {
                return DomainError::Internal(anyhow::anyhow!(
                    "Failed to prepare display preference persistence: {error}"
                ))
                .into_response();
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
                    &response.zone,
                    ZoneChangeKind::ControlsPatched,
                );
                envelope::ok(response)
            }
            Err(error) => error.into_response(),
        };
    }

    let (group, effect) = match current_display_face_assignment(state.as_ref(), device_id).await {
        Ok(response) => (response.zone, response.effect),
        Err(error) => return error.into_response(),
    };
    let (normalized_controls, rejected) =
        crate::domain::effect::normalize_control_values(&effect, &requested_controls);
    if !rejected.is_empty() {
        return DomainError::validation_details(
            "Invalid display face control values",
            serde_json::json!({ "rejected": rejected }),
        )
        .into_response();
    }

    let written = match crate::domain::display::patch_display_face_controls(
        &state.domains.scene,
        crate::domain::display::PatchDisplayFaceControls {
            zone_id: group.id,
            controls: normalized_controls,
        },
    )
    .await
    {
        Ok(Some(written)) => written,
        Ok(None) => {
            return DomainError::not_found(ResourceKind::Zone, format!("display-face:{device_id}"))
                .into_response();
        }
        Err(error) => return error.into_response(),
    };

    envelope::ok(DisplayFaceResponse {
        default_assigned: {
            let store = state.display_preferences.read().await;
            store.get(device_id).is_some()
        },
        device_id: device_id.to_string(),
        effect,
        zone: written.zone,
        live_scope: DisplayFaceScope::Scene,
        scene_assigned: true,
        scene_id: written.scene_id.to_string(),
    })
}

fn display_frame_response(
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

fn format_display_frame_etag(device_id: DeviceId, frame_number: u64) -> String {
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

async fn resolve_display_device_id_or_error(
    state: &Arc<AppState>,
    id_or_name: &str,
) -> Result<DeviceId, DomainError> {
    let device_id = devices::resolve_device_id_or_error(state, id_or_name).await?;
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return Err(DomainError::not_found(ResourceKind::Device, id_or_name));
    };
    if display_surface_info(&tracked.info).is_none() {
        return Err(DomainError::validation(format!(
            "Device does not support display faces: {}",
            tracked.info.name
        )));
    }
    Ok(device_id)
}

async fn current_display_face_assignment(
    state: &AppState,
    device_id: DeviceId,
) -> Result<DisplayFaceResponse, DomainError> {
    let (scene_id, group) = {
        let scene_manager = state.scene_manager.snapshot().await;
        let Some(active_scene) = scene_manager.active_scene() else {
            return Err(DomainError::not_found(ResourceKind::Scene, "active"));
        };
        let Some(group) = active_scene.display_zone_for(device_id).cloned() else {
            return Err(DomainError::not_found(
                ResourceKind::Zone,
                format!("display-face:{device_id}"),
            ));
        };
        (active_scene.id, group)
    };

    let Some(effect_id) = group.effect_ids().next() else {
        return Err(DomainError::not_found(
            ResourceKind::Effect,
            format!("zone:{}", group.id),
        ));
    };
    let Some(effect) = state.domains.effects.metadata(effect_id).await else {
        return Err(DomainError::not_found(ResourceKind::Effect, effect_id));
    };

    let default_assigned = {
        let store = state.display_preferences.read().await;
        store.get(device_id).is_some()
    };

    Ok(DisplayFaceResponse {
        default_assigned,
        device_id: device_id.to_string(),
        effect,
        zone: group,
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
        layers: vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            effect_id,
            preference.controls.clone(),
            std::collections::HashMap::new(),
            None,
        )],
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
    let _effect_admission = state.domains.effects.admit_current().await;
    apply_display_preference_overlay_admitted(state, device_id).await
}

pub(crate) async fn apply_display_preference_overlay_admitted(
    state: &AppState,
    device_id: DeviceId,
) -> Option<Zone> {
    let preference = {
        let store = state.display_preferences.read().await;
        store.get(device_id).cloned()
    };
    let Some(preference) = preference else {
        return retract_default_display_overlay(state, device_id).await;
    };

    let tracked = state.device_registry.get(&device_id).await?;
    let surface = display_surface_info(&tracked.info)?;
    let effect_resolves = state
        .domains
        .effects
        .metadata(preference.effect_id)
        .await
        .is_some();
    if !effect_resolves {
        warn!(
            %device_id,
            effect_id = %preference.effect_id,
            "Default display face effect is not installed; skipping overlay"
        );
        return retract_default_display_overlay(state, device_id).await;
    }

    let zone = build_default_display_zone(
        device_id,
        tracked.info.name.as_str(),
        preference.effect_id,
        &preference,
        display_face_layout(device_id, tracked.info.name.as_str(), surface),
    );
    match crate::domain::display::set_default_display_overlay(&state.domains.scene, device_id, zone)
        .await
    {
        Ok(installed) => installed,
        Err(error) => {
            warn!(%error, %device_id, "Failed to install the default face overlay");
            None
        }
    }
}

/// Drop a display's runtime default overlay, reporting `None` either way
/// so the caller reads it as "no overlay is installed".
async fn retract_default_display_overlay(state: &AppState, device_id: DeviceId) -> Option<Zone> {
    if let Err(error) =
        crate::domain::display::remove_default_display_overlay(&state.domains.scene, device_id)
            .await
    {
        warn!(%error, %device_id, "Failed to retract the default face overlay");
    }
    None
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
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.display_zone_for(device_id))
            .is_some_and(display_zone_has_face_assignment)
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
) -> Result<DisplayFaceResponse, DomainError> {
    let Some(zone) = apply_display_preference_overlay(state, device_id).await else {
        return Err(DomainError::not_found(
            ResourceKind::Zone,
            format!("default-face:{device_id}"),
        ));
    };
    let Some(effect_id) = zone.effect_ids().next() else {
        return Err(DomainError::Internal(anyhow::anyhow!(
            "Default face zone has no effect"
        )));
    };
    let Some(effect) = state.domains.effects.metadata(effect_id).await else {
        return Err(DomainError::not_found(ResourceKind::Effect, effect_id));
    };
    let scene_id = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager.active_scene().map_or_else(
            || hypercolor_types::scene::SceneId::DEFAULT.to_string(),
            |scene| scene.id.to_string(),
        )
    };

    Ok(DisplayFaceResponse {
        default_assigned: true,
        device_id: device_id.to_string(),
        effect,
        zone,
        live_scope: DisplayFaceScope::Default,
        scene_assigned: false,
        scene_id,
    })
}

fn compact_display_face_assignment_zone(mut group: Zone) -> Zone {
    if let Some(target) = group.display_target.as_mut()
        && target.blend_mode == DisplayFaceBlendMode::Replace
        && (target.opacity - 1.0).abs() <= f32::EPSILON
    {
        target.blend_mode = DisplayFaceBlendMode::Alpha;
    }
    group
}

fn display_zone_has_face_assignment(group: &Zone) -> bool {
    group.effect_ids().next().is_some()
}

pub(crate) async fn sync_connected_display_surfaces(state: &AppState) {
    let displays = state
        .domains
        .devices
        .connected_display_surface_layouts()
        .await;
    if let Err(error) =
        crate::domain::display::hydrate_existing_display_surfaces(&state.domains.scene, displays)
            .await
    {
        warn!(%error, "Failed to hydrate connected display surfaces");
    }
}

/// Build the API-facing descriptor for a display device — the same shared
/// derivation that feeds the face-page injection.
pub(crate) fn display_descriptor_for_device(
    info: &DeviceInfo,
    target_fps: u32,
) -> Option<DisplayDescriptor> {
    let surface = display_surface_info(info)?;
    let pixel_format = info
        .segments
        .iter()
        .find_map(|segment| match segment.topology {
            DeviceTopologyHint::Display { .. } => Some(
                DisplayFrameFormat::from_device_color_format(segment.color_format),
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
