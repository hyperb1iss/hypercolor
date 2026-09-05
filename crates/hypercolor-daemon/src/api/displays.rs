//! Display-face and preview endpoints — `/api/v1/displays/*`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::displays::{
    DeleteDisplayFaceResponse, DisplayFaceResponse, DisplayFaceScope, DisplayFaceScopeQuery,
    DisplaySummary, SetDisplayFaceRequest, UpdateDisplayFaceCompositionRequest,
};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::display::{DisplayDescriptor, DisplayPixelFormat};
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::{DisplayFaceTarget, Zone};

use crate::api::devices;
use crate::api::envelope;
use crate::app_state::AppState;
use crate::display_frames::DisplayFrameSnapshot;
use crate::domain::display::{
    SetDefaultDisplayFace, display_face_layout, display_surface_info, normalize_display_face_target,
};
use crate::domain::{DomainError, ResourceKind};

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
        let target_fps = crate::display_output::capped_zone_direct_display_target_fps(
            tracked.info.capabilities.max_fps,
            face_fps_cap,
        );
        let Some(descriptor) = display_descriptor_for_device(&tracked.info, target_fps) else {
            continue;
        };
        displays.push(DisplaySummary {
            id: tracked.info.id.to_string(),
            name: tracked
                .user_settings
                .name
                .clone()
                .unwrap_or_else(|| tracked.info.name.clone()),
            vendor: tracked.info.vendor.clone(),
            family: tracked.info.family.to_string(),
            width: surface.width,
            height: surface.height,
            circular: surface.circular,
            descriptor,
        });
    }

    disambiguate_display_names(&state, &mut displays).await;
    displays.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    envelope::ok(displays)
}

/// A stack of identical panels ships identical names, so a name shared by
/// more than one display gets the USB port it hangs off, which is the one
/// fact that tells the units apart.
async fn disambiguate_display_names(state: &AppState, displays: &mut [DisplaySummary]) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for display in displays.iter() {
        *counts.entry(display.name.as_str()).or_default() += 1;
    }
    let shared: Vec<usize> = displays
        .iter()
        .enumerate()
        .filter(|(_, display)| counts.get(display.name.as_str()).copied().unwrap_or(0) > 1)
        .map(|(index, _)| index)
        .collect();
    for index in shared {
        let Ok(device_id) = displays[index].id.parse::<DeviceId>() else {
            continue;
        };
        let Some(metadata) = state.device_registry.metadata_for_id(&device_id).await else {
            continue;
        };
        if let Some(path) = metadata.get("usb_path") {
            displays[index].name = format!("{} (USB {path})", displays[index].name);
        }
    }
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

    let Some(frame) = state.domains.display.frames().read().await.frame(device_id) else {
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

    let layers = state.domains.display.face_layers(device_id).await;
    if layers.scene_assigned {
        return match current_display_face_assignment(&state, device_id).await {
            Ok(response) => envelope::ok(Some(response)),
            Err(error) => error.into_response(),
        };
    }
    if layers.default_assigned {
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

    let Some(effect) = state
        .domains
        .effects
        .resolve_for_mutation(&body.effect_id)
        .await
    else {
        return DomainError::not_found(ResourceKind::Effect, &body.effect_id).into_response();
    };

    let composition_explicit = body.blend_mode.is_some() || body.opacity.is_some();
    // Without an explicit composition the face blends over the live effect
    // instead of replacing it.
    let display_target = normalize_display_face_target(DisplayFaceTarget {
        blend_mode: body.blend_mode.unwrap_or(BlendMode::Alpha),
        device_id,
        opacity: body.opacity.unwrap_or(1.0),
    });

    if body.scope == DisplayFaceScope::Default {
        let written = match state
            .domains
            .display
            .set_default_face(SetDefaultDisplayFace {
                device_id,
                effect,
                controls: body.controls,
                target: display_target,
            })
            .await
        {
            Ok(written) => written,
            Err(error) => return error.into_response(),
        };

        return envelope::ok(DisplayFaceResponse {
            default_assigned: true,
            device_id: device_id.to_string(),
            effect: written.effect,
            zone: written.zone,
            live_scope: if written.scene_assigned {
                DisplayFaceScope::Scene
            } else {
                DisplayFaceScope::Default
            },
            scene_assigned: written.scene_assigned,
            scene_id: written.scene_id.to_string(),
        });
    }

    let default_assigned = state.domains.display.has_default_face(device_id).await;

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

    let layers = state.domains.display.face_layers(device_id).await;
    if !layers.scene_assigned && layers.default_assigned {
        if let Err(error) = state
            .domains
            .display
            .patch_default_composition(device_id, body.blend_mode, body.opacity)
            .await
        {
            return error.into_response();
        }
        return match current_default_face_assignment(state.as_ref(), device_id).await {
            Ok(response) => envelope::ok(response),
            Err(error) => error.into_response(),
        };
    }

    let (zone, effect) = match current_display_face_assignment(state.as_ref(), device_id).await {
        Ok(response) => (response.zone, response.effect),
        Err(error) => return error.into_response(),
    };

    let written = match crate::domain::display::patch_display_composition(
        &state.domains.scene,
        crate::domain::display::PatchDisplayComposition {
            zone_id: zone.id,
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
        default_assigned: state.domains.display.has_default_face(device_id).await,
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
        let cleared = match state.domains.display.clear_default_face(device_id).await {
            Ok(cleared) => cleared,
            Err(error) => return error.into_response(),
        };

        return envelope::ok(DeleteDisplayFaceResponse {
            device_id: device_id.to_string(),
            scene_id: None,
            scope: DisplayFaceScope::Default,
            deleted: cleared.removed,
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

    let requested_controls = body.values.into_iter().collect::<HashMap<_, _>>();

    let layers = state.domains.display.face_layers(device_id).await;
    if !layers.scene_assigned && layers.default_assigned {
        if let Err(error) = state
            .domains
            .display
            .merge_default_controls(device_id, &requested_controls)
            .await
        {
            return error.into_response();
        }
        return match current_default_face_assignment(state.as_ref(), device_id).await {
            Ok(response) => envelope::ok(response),
            Err(error) => error.into_response(),
        };
    }

    let zone = match current_display_face_assignment(state.as_ref(), device_id).await {
        Ok(response) => response.zone,
        Err(error) => return error.into_response(),
    };

    let written = match crate::domain::display::patch_display_face_controls(
        &state.domains.effects,
        crate::domain::display::PatchDisplayFaceControls {
            zone_id: zone.id,
            controls: requested_controls,
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
        default_assigned: state.domains.display.has_default_face(device_id).await,
        device_id: device_id.to_string(),
        effect: written.effect,
        zone: written.written.zone,
        live_scope: DisplayFaceScope::Scene,
        scene_assigned: true,
        scene_id: written.written.scene_id.to_string(),
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
    let (scene_id, zone) = {
        let scene_manager = state.scene_manager.snapshot().await;
        let Some(active_scene) = scene_manager.active_scene() else {
            return Err(DomainError::not_found(ResourceKind::Scene, "active"));
        };
        let Some(zone) = active_scene.display_zone_for(device_id).cloned() else {
            return Err(DomainError::not_found(
                ResourceKind::Zone,
                format!("display-face:{device_id}"),
            ));
        };
        (active_scene.id, zone)
    };

    let Some(effect_id) = zone.effect_ids().next() else {
        return Err(DomainError::not_found(
            ResourceKind::Effect,
            format!("zone:{}", zone.id),
        ));
    };
    let Some(effect) = state.domains.effects.metadata(effect_id).await else {
        return Err(DomainError::not_found(ResourceKind::Effect, effect_id));
    };

    let default_assigned = state.domains.display.has_default_face(device_id).await;

    Ok(DisplayFaceResponse {
        default_assigned,
        device_id: device_id.to_string(),
        effect,
        zone,
        live_scope: DisplayFaceScope::Scene,
        scene_assigned: true,
        scene_id: scene_id.to_string(),
    })
}

/// Current assignment for the *default* layer, materialized from the
/// preference store and the runtime overlay zone.
async fn current_default_face_assignment(
    state: &AppState,
    device_id: DeviceId,
) -> Result<DisplayFaceResponse, DomainError> {
    let Some(zone) = state
        .domains
        .display
        .apply_preference_overlay(device_id)
        .await
    else {
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

fn compact_display_face_assignment_zone(mut zone: Zone) -> Zone {
    if let Some(target) = zone.display_target.as_mut()
        && target.blend_mode == BlendMode::Replace
        && (target.opacity - 1.0).abs() <= f32::EPSILON
    {
        target.blend_mode = BlendMode::Alpha;
    }
    zone
}

/// Build the API-facing descriptor for a display device — the same shared
/// derivation that feeds the face-page injection.
pub(crate) fn display_descriptor_for_device(
    info: &DeviceInfo,
    target_fps: u32,
) -> Option<DisplayDescriptor> {
    let surface = info.display_surface()?;

    Some(DisplayDescriptor::derive(
        surface.width,
        surface.height,
        surface.circular,
        None,
        target_fps,
        DisplayPixelFormat::from(surface.format),
    ))
}
