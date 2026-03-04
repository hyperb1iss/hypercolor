//! Device endpoints — `/api/v1/devices/*`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::discovery;

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UpdateDeviceRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverRequest {
    pub backends: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    pub wait: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct IdentifyRequest {
    pub duration_ms: Option<u64>,
    pub color: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeviceListResponse {
    pub items: Vec<DeviceSummary>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub status: String,
    pub firmware_version: Option<String>,
    pub total_leds: u32,
    pub zones: Vec<ZoneSummary>,
}

#[derive(Debug, Serialize)]
pub struct ZoneSummary {
    pub id: String,
    pub name: String,
    pub led_count: u32,
    pub topology: String,
}

#[derive(Debug, Serialize)]
pub struct Pagination {
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListDevicesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub status: Option<String>,
    pub backend: Option<String>,
    pub q: Option<String>,
}

#[derive(Debug)]
enum ResolveDeviceError {
    AmbiguousName(String),
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/devices` — List all tracked devices.
pub async fn list_devices(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListDevicesQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return ApiError::validation("limit must be between 1 and 200");
    }
    let offset = query.offset.unwrap_or(0);

    let devices = state.device_registry.list().await;
    let status_filter = match parse_status_filter(query.status.as_deref()) {
        Ok(filter) => filter,
        Err(error) => return ApiError::validation(error),
    };
    let backend_filter = query
        .backend
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let query_filter = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);

    let mut items: Vec<DeviceSummary> = devices
        .iter()
        .filter(|tracked| {
            status_filter
                .as_deref()
                .is_none_or(|expected| tracked.state.variant_name().eq_ignore_ascii_case(expected))
        })
        .filter(|tracked| {
            backend_filter.as_deref().is_none_or(|expected| {
                format!("{}", tracked.info.family).to_ascii_lowercase() == *expected
            })
        })
        .filter(|tracked| {
            query_filter.as_deref().is_none_or(|needle| {
                let name = tracked.info.name.to_ascii_lowercase();
                let vendor = tracked.info.vendor.to_ascii_lowercase();
                name.contains(needle) || vendor.contains(needle)
            })
        })
        .map(|tracked| summarize_device(&tracked.info, &tracked.state))
        .collect();
    items.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    let total = items.len();
    let paged_items: Vec<DeviceSummary> = items.into_iter().skip(offset).take(limit).collect();
    let has_more = offset.saturating_add(limit) < total;
    ApiResponse::ok(DeviceListResponse {
        items: paged_items,
        pagination: Pagination {
            offset,
            limit,
            total,
            has_more,
        },
    })
}

/// `GET /api/v1/devices/debug/queues` — Inspect backend output queue diagnostics.
pub async fn debug_output_queues(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.backend_manager.lock().await;
    ApiResponse::ok(manager.debug_snapshot())
}

/// `GET /api/v1/devices/debug/routing` — Inspect layout/backend routing diagnostics.
pub async fn debug_device_routing(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.backend_manager.lock().await;
    ApiResponse::ok(manager.routing_snapshot())
}

/// `GET /api/v1/devices/:id` — Get a single device.
pub async fn get_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    ApiResponse::ok(summarize_device(&tracked.info, &tracked.state))
}

/// `PUT /api/v1/devices/:id` — Update a device's metadata.
pub async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDeviceRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    if body.name.is_none() && body.enabled.is_none() {
        return ApiError::validation("At least one field must be provided: name or enabled");
    }

    let normalized_name = match body.name {
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return ApiError::validation("Device name must not be empty");
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

    let Some(updated) = state
        .device_registry
        .update_user_settings(&device_id, normalized_name, body.enabled)
        .await
    else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    ApiResponse::ok(summarize_device(&updated.info, &updated.state))
}

/// `DELETE /api/v1/devices/:id` — Remove a device from tracking.
pub async fn delete_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    if state.device_registry.remove(&device_id).await.is_none() {
        return ApiError::not_found(format!("Device not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": device_id.to_string(),
        "removed": true,
    }))
}

/// `POST /api/v1/devices/discover` — Trigger device discovery.
pub async fn discover_devices(
    State(state): State<Arc<AppState>>,
    body: Option<Json<DiscoverRequest>>,
) -> Response {
    let config = state.config_manager.as_ref().map_or_else(
        || Arc::new(HypercolorConfig::default()),
        |manager| Arc::clone(&manager.get()),
    );
    let requested_backends = body.as_ref().and_then(|request| request.backends.as_ref());
    let resolved_backends =
        match discovery::resolve_backends(requested_backends.map(Vec::as_slice), config.as_ref()) {
            Ok(backends) => backends,
            Err(error) => return ApiError::validation(error),
        };
    let timeout = discovery::normalize_timeout_ms(body.as_ref().and_then(|b| b.timeout_ms));
    let wait_for_completion = body.as_ref().and_then(|b| b.wait).unwrap_or(false);

    if state
        .discovery_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ApiError::conflict("A discovery scan is already in progress");
    }

    let scan_id = format!("scan_{}", uuid::Uuid::now_v7());
    let backend_names = discovery::backend_names(&resolved_backends);
    if wait_for_completion {
        let runtime = discovery::DiscoveryRuntime {
            device_registry: state.device_registry.clone(),
            backend_manager: Arc::clone(&state.backend_manager),
            lifecycle_manager: Arc::clone(&state.lifecycle_manager),
            reconnect_tasks: Arc::clone(&state.reconnect_tasks),
            event_bus: Arc::clone(&state.event_bus),
            in_progress: Arc::clone(&state.discovery_in_progress),
        };
        let result = discovery::execute_discovery_scan(
            runtime,
            config,
            resolved_backends,
            timeout,
        )
        .await;

        return ApiResponse::ok(serde_json::json!({
            "scan_id": scan_id,
            "status": "completed",
            "result": result,
        }));
    }

    let state_for_task = Arc::clone(&state);
    tokio::spawn(async move {
        let runtime = discovery::DiscoveryRuntime {
            device_registry: state_for_task.device_registry.clone(),
            backend_manager: Arc::clone(&state_for_task.backend_manager),
            lifecycle_manager: Arc::clone(&state_for_task.lifecycle_manager),
            reconnect_tasks: Arc::clone(&state_for_task.reconnect_tasks),
            event_bus: Arc::clone(&state_for_task.event_bus),
            in_progress: Arc::clone(&state_for_task.discovery_in_progress),
        };
        let _ = discovery::execute_discovery_scan(
            runtime,
            config,
            resolved_backends,
            timeout,
        )
        .await;
    });

    ApiResponse::accepted(serde_json::json!({
        "scan_id": scan_id,
        "status": "scanning",
        "backends": backend_names,
        "timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    }))
}

/// `POST /api/v1/devices/:id/identify` — Flash identification pattern.
pub async fn identify_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<IdentifyRequest>>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(_tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let duration_ms = body.as_ref().and_then(|b| b.duration_ms).unwrap_or(3000);
    if duration_ms == 0 || duration_ms > 120_000 {
        return ApiError::validation("duration_ms must be between 1 and 120000");
    }
    let color = match body.as_ref().and_then(|b| b.color.as_deref()) {
        Some(color) => match parse_hex_color(color) {
            Some(normalized) => Some(normalized),
            None => return ApiError::validation("color must be a 6-digit hex value (RRGGBB)"),
        },
        None => None,
    };

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "identifying": true,
        "duration_ms": duration_ms,
        "color": color,
    }))
}

fn summarize_device(info: &DeviceInfo, state: &DeviceState) -> DeviceSummary {
    DeviceSummary {
        id: info.id.to_string(),
        name: info.name.clone(),
        backend: format!("{}", info.family),
        status: state.variant_name().to_lowercase(),
        firmware_version: info.firmware_version.clone(),
        total_leds: info.total_led_count(),
        zones: info
            .zones
            .iter()
            .enumerate()
            .map(|(i, z)| ZoneSummary {
                id: format!("zone_{i}"),
                name: z.name.clone(),
                led_count: z.led_count,
                topology: format!("{:?}", z.topology).to_lowercase(),
            })
            .collect(),
    }
}

async fn resolve_device_id(
    state: &AppState,
    id_or_name: &str,
) -> Result<Option<DeviceId>, ResolveDeviceError> {
    if let Ok(id) = id_or_name.parse::<DeviceId>() {
        return Ok(Some(id));
    }

    let devices = state.device_registry.list().await;
    let matches: Vec<DeviceId> = devices
        .iter()
        .filter(|d| d.info.name.eq_ignore_ascii_case(id_or_name))
        .map(|d| d.info.id)
        .collect();

    if matches.len() > 1 {
        return Err(ResolveDeviceError::AmbiguousName(id_or_name.to_owned()));
    }
    Ok(matches.first().copied())
}

async fn resolve_device_id_or_response(
    state: &AppState,
    id_or_name: &str,
) -> Result<DeviceId, Response> {
    match resolve_device_id(state, id_or_name).await {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(ApiError::not_found(format!(
            "Device not found: {id_or_name}"
        ))),
        Err(ResolveDeviceError::AmbiguousName(name)) => Err(ApiError::conflict(format!(
            "Device name is ambiguous: {name}"
        ))),
    }
}

fn parse_status_filter(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }

    match normalized.as_str() {
        "known" | "connected" | "active" | "reconnecting" | "disabled" => Ok(Some(normalized)),
        _ => Err(format!(
            "Invalid status filter '{raw}'. Expected one of: known, connected, active, reconnecting, disabled"
        )),
    }
}

fn parse_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let color = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if color.len() != 6 || !color.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", color.to_ascii_uppercase()))
}
