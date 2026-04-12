//! Display overlay endpoints and runtime diagnostics — `/api/v1/displays/*`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint};
use hypercolor_types::effect::{EffectMetadata, EffectSource};
use hypercolor_types::overlay::{
    DisplayOverlayConfig, OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySlotId,
    OverlaySource,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::api::AppState;
use crate::api::devices;
use crate::api::envelope::{ApiError, ApiResponse, iso8601_system_time};
use crate::display_frames::DisplayFrameSnapshot;
use crate::display_overlays::{OverlaySlotRuntime, OverlaySlotStatus};

#[derive(Debug, Clone, Serialize)]
pub struct DisplaySummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub family: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub overlay_count: usize,
    pub enabled_overlay_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DisplaySurfaceInfo {
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

struct OwnedDisplayJpeg(Arc<Vec<u8>>);

impl AsRef<[u8]> for OwnedDisplayJpeg {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().as_slice()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateOverlaySlotRequest {
    pub name: String,
    pub source: OverlaySource,
    pub position: OverlayPosition,
    #[serde(default)]
    pub blend_mode: OverlayBlendMode,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateOverlaySlotRequest {
    pub name: Option<String>,
    pub source: Option<OverlaySource>,
    pub position: Option<OverlayPosition>,
    pub blend_mode: Option<OverlayBlendMode>,
    pub opacity: Option<f32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ReorderOverlaySlotsRequest {
    pub slot_ids: Vec<OverlaySlotId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayRuntimeResponse {
    pub last_rendered_at: Option<String>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub status: crate::display_overlays::OverlaySlotStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlaySlotResponse {
    pub slot: OverlaySlot,
    pub runtime: OverlayRuntimeResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayCompatibilityWarning {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    pub device_key: String,
    pub slot_id: String,
    pub slot_name: String,
}

pub async fn list_displays(State(state): State<Arc<AppState>>) -> Response {
    let tracked_devices = state.device_registry.list().await;
    let mut displays = Vec::new();

    for tracked in tracked_devices {
        let Some(surface) = display_surface_info(&tracked.info) else {
            continue;
        };
        let config = current_overlay_config(state.as_ref(), tracked.info.id).await;
        displays.push(DisplaySummary {
            id: tracked.info.id.to_string(),
            name: tracked.info.name.clone(),
            vendor: tracked.info.vendor.clone(),
            family: tracked.info.family.to_string(),
            width: surface.width,
            height: surface.height,
            circular: surface.circular,
            overlay_count: config.overlays.len(),
            enabled_overlay_count: config.overlays.iter().filter(|slot| slot.enabled).count(),
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

pub async fn list_overlays(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    ApiResponse::ok(current_overlay_config(state.as_ref(), device_id).await)
}

pub async fn get_overlay(
    State(state): State<Arc<AppState>>,
    Path((device, slot_id)): Path<(String, String)>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let slot_id = match slot_id.parse::<OverlaySlotId>() {
        Ok(slot_id) => slot_id,
        Err(_) => return ApiError::validation(format!("Invalid overlay slot id: {slot_id}")),
    };
    let config = current_overlay_config(state.as_ref(), device_id).await;
    match config.overlays.into_iter().find(|slot| slot.id == slot_id) {
        Some(slot) => ApiResponse::ok(OverlaySlotResponse {
            runtime: current_overlay_runtime(&state, device_id, &slot).await,
            slot,
        }),
        None => ApiError::not_found(format!("Overlay not found: {slot_id}")),
    }
}

pub async fn replace_overlays(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<DisplayOverlayConfig>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let config = body.normalized();
    if let Err(error) = validate_overlay_config(state.as_ref(), &config).await {
        return ApiError::conflict(error);
    }
    if let Err(error) = persist_overlay_config(state.as_ref(), device_id, &config).await {
        return ApiError::internal(format!("Failed to persist display overlays: {error}"));
    }
    ApiResponse::ok(config)
}

pub async fn add_overlay(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<CreateOverlaySlotRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut config = current_overlay_config(state.as_ref(), device_id).await;
    let slot = OverlaySlot {
        id: OverlaySlotId::generate(),
        name: body.name,
        source: body.source,
        position: body.position,
        blend_mode: body.blend_mode,
        opacity: body.opacity,
        enabled: body.enabled,
    }
    .normalized();
    config.overlays.push(slot.clone());
    if let Err(error) = validate_overlay_config(state.as_ref(), &config).await {
        return ApiError::conflict(error);
    }

    if let Err(error) = persist_overlay_config(state.as_ref(), device_id, &config).await {
        return ApiError::internal(format!("Failed to persist display overlays: {error}"));
    }
    ApiResponse::created(slot)
}

pub async fn patch_overlay(
    State(state): State<Arc<AppState>>,
    Path((device, slot_id)): Path<(String, String)>,
    Json(body): Json<UpdateOverlaySlotRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let slot_id = match slot_id.parse::<OverlaySlotId>() {
        Ok(slot_id) => slot_id,
        Err(_) => return ApiError::validation(format!("Invalid overlay slot id: {slot_id}")),
    };

    let mut config = current_overlay_config(state.as_ref(), device_id).await;
    let Some(slot_index) = find_slot_index(&config, slot_id) else {
        return ApiError::not_found(format!("Overlay not found: {slot_id}"));
    };

    let slot = &mut config.overlays[slot_index];
    if let Some(name) = body.name {
        slot.name = name;
    }
    if let Some(source) = body.source {
        slot.source = source;
    }
    if let Some(position) = body.position {
        slot.position = position;
    }
    if let Some(blend_mode) = body.blend_mode {
        slot.blend_mode = blend_mode;
    }
    if let Some(opacity) = body.opacity {
        slot.opacity = opacity;
    }
    if let Some(enabled) = body.enabled {
        slot.enabled = enabled;
    }
    let slot = slot.clone().normalized();
    config.overlays[slot_index] = slot.clone();
    if let Err(error) = validate_overlay_config(state.as_ref(), &config).await {
        return ApiError::conflict(error);
    }

    if let Err(error) = persist_overlay_config(state.as_ref(), device_id, &config).await {
        return ApiError::internal(format!("Failed to persist display overlays: {error}"));
    }
    ApiResponse::ok(slot)
}

pub async fn delete_overlay(
    State(state): State<Arc<AppState>>,
    Path((device, slot_id)): Path<(String, String)>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let slot_id = match slot_id.parse::<OverlaySlotId>() {
        Ok(slot_id) => slot_id,
        Err(_) => return ApiError::validation(format!("Invalid overlay slot id: {slot_id}")),
    };

    let mut config = current_overlay_config(state.as_ref(), device_id).await;
    let previous_len = config.overlays.len();
    config.overlays.retain(|slot| slot.id != slot_id);
    if config.overlays.len() == previous_len {
        return ApiError::not_found(format!("Overlay not found: {slot_id}"));
    }

    if let Err(error) = persist_overlay_config(state.as_ref(), device_id, &config).await {
        return ApiError::internal(format!("Failed to persist display overlays: {error}"));
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn reorder_overlays(
    State(state): State<Arc<AppState>>,
    Path(device): Path<String>,
    Json(body): Json<ReorderOverlaySlotsRequest>,
) -> Response {
    let device_id = match resolve_display_device_id_or_response(&state, &device).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let config = current_overlay_config(state.as_ref(), device_id).await;
    if has_duplicate_slot_ids(&body.slot_ids) {
        return ApiError::conflict("slot_ids must not contain duplicates");
    }
    if body.slot_ids.len() != config.overlays.len() {
        return ApiError::conflict("slot_ids must include every configured overlay exactly once");
    }

    let mut reordered = Vec::with_capacity(config.overlays.len());
    for slot_id in &body.slot_ids {
        let Some(slot) = config.overlays.iter().find(|slot| &slot.id == slot_id) else {
            return ApiError::conflict("slot_ids must match the configured overlay set");
        };
        reordered.push(slot.clone());
    }

    let config = DisplayOverlayConfig {
        overlays: reordered,
    }
    .normalized();
    if let Err(error) = persist_overlay_config(state.as_ref(), device_id, &config).await {
        return ApiError::internal(format!("Failed to persist display overlays: {error}"));
    }
    ApiResponse::ok(config)
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
            "Device does not support display overlays: {}",
            tracked.info.name
        )));
    }
    Ok(device_id)
}

pub(crate) async fn current_overlay_config(
    state: &AppState,
    device_id: DeviceId,
) -> DisplayOverlayConfig {
    let live = state.display_overlays.get(device_id).await;
    if !live.is_empty() {
        return live.as_ref().clone();
    }

    let key = devices::device_settings_key(state, device_id).await;
    let persisted = state
        .device_settings
        .read()
        .await
        .display_overlays_for_key(&key)
        .unwrap_or_default()
        .normalized();
    if !persisted.is_empty() {
        state
            .display_overlays
            .set(device_id, persisted.clone())
            .await;
    }
    persisted
}

pub(crate) async fn validate_overlay_config(
    state: &AppState,
    config: &DisplayOverlayConfig,
) -> Result<(), String> {
    if has_duplicate_overlay_ids(config) {
        return Err("overlay ids must be unique".to_owned());
    }
    if contains_enabled_html_overlay(config) {
        let effect_engine = state.effect_engine.lock().await;
        if effect_engine.is_running()
            && let Some(metadata) = effect_engine.active_metadata()
            && effect_source_is_html(&metadata.source)
        {
            return Err(format!(
                "HTML overlays cannot be enabled while HTML effect '{}' is active; Servo multi-session rendering is still pending",
                metadata.name
            ));
        }
    }
    Ok(())
}

async fn current_overlay_runtime(
    state: &Arc<AppState>,
    device_id: DeviceId,
    slot: &OverlaySlot,
) -> OverlayRuntimeResponse {
    OverlayRuntimeResponse::from(overlay_runtime_for_slot(state.as_ref(), device_id, slot).await)
}

pub(crate) async fn overlay_runtime_for_slot(
    state: &AppState,
    device_id: DeviceId,
    slot: &OverlaySlot,
) -> OverlaySlotRuntime {
    let runtime = state
        .display_overlay_runtime
        .get(device_id)
        .await
        .slot(slot.id)
        .cloned()
        .unwrap_or_else(|| OverlaySlotRuntime::from_slot(slot));
    apply_html_gate_runtime_status(state, slot, runtime).await
}

pub(crate) async fn persist_overlay_config(
    state: &AppState,
    device_id: DeviceId,
    config: &DisplayOverlayConfig,
) -> Result<(), String> {
    let key = devices::device_settings_key(state, device_id).await;
    {
        let mut store = state.device_settings.write().await;
        store.set_display_overlays(&key, (!config.is_empty()).then(|| config.clone()));
        store.save().map_err(|error| error.to_string())?;
    }

    if config.is_empty() {
        state.display_overlays.clear(device_id).await;
    } else {
        state.display_overlays.set(device_id, config.clone()).await;
    }
    Ok(())
}

pub async fn auto_disable_html_overlays_for_effect(
    state: &AppState,
    effect: &EffectMetadata,
) -> Vec<OverlayCompatibilityWarning> {
    if !effect_source_is_html(&effect.source) {
        return Vec::new();
    }

    let mut connected_displays = Vec::new();
    let mut connected_keys = HashSet::new();
    for tracked in state.device_registry.list().await {
        if display_surface_info(&tracked.info).is_none() {
            continue;
        }
        let key = devices::device_settings_key(state, tracked.info.id).await;
        connected_keys.insert(key.clone());
        connected_displays.push((tracked.info.id, tracked.info.name.clone(), key));
    }
    connected_displays
        .sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.2.cmp(&right.2)));

    let mut warnings = Vec::new();
    for (device_id, device_name, key) in connected_displays {
        let config = current_overlay_config(state, device_id).await;
        let Some((updated_config, disabled_slots)) = disable_enabled_html_overlays(&config) else {
            continue;
        };

        warnings.extend(disabled_slots.into_iter().map(|slot| {
            overlay_warning(
                effect.name.as_str(),
                &key,
                Some(device_id),
                Some(device_name.as_str()),
                &slot,
            )
        }));

        if let Err(error) = persist_overlay_config(state, device_id, &updated_config).await {
            if updated_config.is_empty() {
                state.display_overlays.clear(device_id).await;
            } else {
                state.display_overlays.set(device_id, updated_config).await;
            }
            warn!(
                effect = %effect.name,
                device_id = %device_id,
                device_name = %device_name,
                %error,
                "Failed to persist auto-disabled HTML overlays for connected display"
            );
        }
    }

    let mut disconnected_entries = {
        let store = state.device_settings.read().await;
        store.display_overlay_entries()
    };
    disconnected_entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut disconnected_updates = Vec::new();
    for (key, device_name, config) in disconnected_entries {
        if connected_keys.contains(&key) {
            continue;
        }
        let Some((updated_config, disabled_slots)) = disable_enabled_html_overlays(&config) else {
            continue;
        };

        warnings.extend(disabled_slots.into_iter().map(|slot| {
            overlay_warning(
                effect.name.as_str(),
                &key,
                None,
                device_name.as_deref(),
                &slot,
            )
        }));
        disconnected_updates.push((key, updated_config));
    }

    if !disconnected_updates.is_empty() {
        let mut store = state.device_settings.write().await;
        for (key, updated_config) in &disconnected_updates {
            store.set_display_overlays(
                key,
                (!updated_config.is_empty()).then(|| updated_config.clone()),
            );
        }
        if let Err(error) = store.save() {
            warn!(
                effect = %effect.name,
                %error,
                "Failed to persist auto-disabled HTML overlays for disconnected displays"
            );
        }
    }

    warnings
}

fn find_slot_index(config: &DisplayOverlayConfig, slot_id: OverlaySlotId) -> Option<usize> {
    config.overlays.iter().position(|slot| slot.id == slot_id)
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

fn contains_enabled_html_overlay(config: &DisplayOverlayConfig) -> bool {
    config
        .overlays
        .iter()
        .any(|slot| slot.enabled && matches!(slot.source, OverlaySource::Html(_)))
}

fn disable_enabled_html_overlays(
    config: &DisplayOverlayConfig,
) -> Option<(DisplayOverlayConfig, Vec<OverlaySlot>)> {
    let mut updated = config.clone();
    let mut disabled_slots = Vec::new();
    for slot in &mut updated.overlays {
        if slot.enabled && matches!(slot.source, OverlaySource::Html(_)) {
            disabled_slots.push(slot.clone());
            slot.enabled = false;
        }
    }

    if disabled_slots.is_empty() {
        None
    } else {
        Some((updated.normalized(), disabled_slots))
    }
}

async fn apply_html_gate_runtime_status(
    state: &AppState,
    slot: &OverlaySlot,
    mut runtime: OverlaySlotRuntime,
) -> OverlaySlotRuntime {
    if slot.enabled || !matches!(slot.source, OverlaySource::Html(_)) {
        return runtime;
    }

    let effect_engine = state.effect_engine.lock().await;
    if effect_engine.is_running()
        && let Some(metadata) = effect_engine.active_metadata()
        && effect_source_is_html(&metadata.source)
    {
        runtime.status = OverlaySlotStatus::HtmlGated;
    }
    runtime
}

fn overlay_warning(
    effect_name: &str,
    device_key: &str,
    device_id: Option<DeviceId>,
    device_name: Option<&str>,
    slot: &OverlaySlot,
) -> OverlayCompatibilityWarning {
    let display_label = device_name.unwrap_or(device_key);
    OverlayCompatibilityWarning {
        code: "html_overlay_disabled",
        message: format!(
            "Disabled HTML overlay '{}' on '{}' because HTML effect '{}' is active; Servo multi-session rendering is still pending",
            slot.name, display_label, effect_name
        ),
        device_id: device_id.map(|id| id.to_string()),
        device_name: device_name.map(ToOwned::to_owned),
        device_key: device_key.to_owned(),
        slot_id: slot.id.to_string(),
        slot_name: slot.name.clone(),
    }
}

fn effect_source_is_html(source: &EffectSource) -> bool {
    matches!(source, EffectSource::Html { .. })
}

fn has_duplicate_overlay_ids(config: &DisplayOverlayConfig) -> bool {
    let mut seen = std::collections::HashSet::with_capacity(config.overlays.len());
    config.overlays.iter().any(|slot| !seen.insert(slot.id))
}

fn has_duplicate_slot_ids(slot_ids: &[OverlaySlotId]) -> bool {
    let mut seen = std::collections::HashSet::with_capacity(slot_ids.len());
    slot_ids.iter().any(|slot_id| !seen.insert(*slot_id))
}

fn default_enabled() -> bool {
    true
}

fn default_opacity() -> f32 {
    1.0
}

impl From<OverlaySlotRuntime> for OverlayRuntimeResponse {
    fn from(runtime: OverlaySlotRuntime) -> Self {
        Self {
            last_rendered_at: runtime.last_rendered_at.map(format_system_time),
            consecutive_failures: runtime.consecutive_failures,
            last_error: runtime.last_error,
            status: runtime.status,
        }
    }
}

fn format_system_time(time: SystemTime) -> String {
    iso8601_system_time(time)
}
