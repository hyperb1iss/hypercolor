//! Device endpoints — `/api/v1/devices/*`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use hypercolor_core::device::{BackendManager, SegmentRange};
use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::{
    AttachmentBinding, AttachmentSlot, AttachmentSuggestedZone, AttachmentTemplate,
    DeviceAttachmentProfile,
};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    DeviceFamily, DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint,
};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::discovery;
use crate::logical_devices::{self, LogicalDevice, LogicalDeviceKind};

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
    pub layout_device_id: String,
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
    pub topology_hint: ZoneTopologySummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ZoneTopologySummary {
    Strip,
    Matrix { rows: u32, cols: u32 },
    Ring { count: u32 },
    Point,
    Custom,
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

#[derive(Debug, Deserialize, Default)]
pub struct ListLogicalDevicesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub physical_device: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLogicalDeviceRequest {
    pub name: String,
    pub led_start: u32,
    pub led_count: u32,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLogicalDeviceRequest {
    pub name: Option<String>,
    pub led_start: Option<u32>,
    pub led_count: Option<u32>,
    pub enabled: Option<bool>,
}

const IDENTIFY_FLASH_INTERVAL_MS: u64 = 250;
const DEFAULT_IDENTIFY_COLOR_RGB: [u8; 3] = [255, 255, 255];

#[derive(Debug, Serialize)]
pub struct LogicalDeviceListResponse {
    pub items: Vec<LogicalDeviceSummary>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogicalDeviceSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub led_start: u32,
    pub led_count: u32,
    pub led_end: u32,
    pub physical_device_id: String,
    pub physical_device_name: String,
    pub backend: String,
    pub physical_status: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateAttachmentsRequest {
    #[serde(default)]
    pub bindings: Vec<AttachmentBinding>,
}

#[derive(Debug, Serialize)]
pub struct DeviceAttachmentsResponse {
    pub device_id: String,
    pub device_name: String,
    pub slots: Vec<AttachmentSlot>,
    pub bindings: Vec<AttachmentBindingSummary>,
    pub suggested_zones: Vec<AttachmentSuggestedZone>,
}

#[derive(Debug, Serialize)]
pub struct DeviceAttachmentsUpdateResponse {
    pub device_id: String,
    pub device_name: String,
    pub slots: Vec<AttachmentSlot>,
    pub bindings: Vec<AttachmentBindingSummary>,
    pub suggested_zones: Vec<AttachmentSuggestedZone>,
    pub needs_layout_update: bool,
}

#[derive(Debug, Serialize)]
pub struct AttachmentBindingSummary {
    pub slot_id: String,
    pub template_id: String,
    pub template_name: String,
    pub name: Option<String>,
    pub enabled: bool,
    pub instances: u32,
    pub led_offset: u32,
    pub effective_led_count: u32,
}

#[derive(Debug, Serialize)]
pub struct AttachmentPreviewResponse {
    pub device_id: String,
    pub device_name: String,
    pub zones: Vec<AttachmentPreviewZone>,
}

#[derive(Debug, Serialize)]
pub struct AttachmentPreviewZone {
    pub slot_id: String,
    pub binding_index: usize,
    pub instance: u32,
    pub template_id: String,
    pub template_name: String,
    pub name: String,
    pub led_start: u32,
    pub led_count: u32,
    pub topology: LedTopology,
    pub led_positions: Vec<NormalizedPosition>,
}

#[derive(Debug, Clone)]
struct ResolvedAttachmentBinding {
    index: usize,
    binding: AttachmentBinding,
    slot: AttachmentSlot,
    template: AttachmentTemplate,
    effective_led_count: u32,
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

    let filtered_devices: Vec<_> = devices
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
        .collect();
    let mut items: Vec<DeviceSummary> = Vec::with_capacity(filtered_devices.len());
    for tracked in filtered_devices {
        let layout_device_id = ensure_default_logical_entry(
            &state,
            tracked.info.id,
            &tracked.info.name,
            tracked.info.total_led_count(),
        )
        .await;
        items.push(summarize_device(
            &tracked.info,
            &tracked.state,
            layout_device_id,
        ));
    }
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

    let layout_device_id = ensure_default_logical_entry(
        &state,
        tracked.info.id,
        &tracked.info.name,
        tracked.info.total_led_count(),
    )
    .await;

    ApiResponse::ok(summarize_device(
        &tracked.info,
        &tracked.state,
        layout_device_id,
    ))
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

    let layout_device_id = ensure_default_logical_entry(
        &state,
        updated.info.id,
        &updated.info.name,
        updated.info.total_led_count(),
    )
    .await;

    ApiResponse::ok(summarize_device(
        &updated.info,
        &updated.state,
        layout_device_id,
    ))
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

/// `GET /api/v1/devices/:id/attachments` — Get a device attachment profile.
pub async fn get_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let profile = {
        let profiles = state.attachment_profiles.read().await;
        profiles.get_or_default(&tracked.info)
    };
    let registry = state.attachment_registry.read().await;

    ApiResponse::ok(summarize_attachment_profile(
        &tracked.info,
        profile,
        &registry,
    ))
}

/// `PUT /api/v1/devices/:id/attachments` — Save a device attachment profile.
pub async fn update_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAttachmentsRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };
    let slots = tracked.info.default_attachment_profile().slots;
    let resolved = {
        let registry = state.attachment_registry.read().await;
        match validate_attachment_bindings(&tracked.info, &slots, &body.bindings, &registry) {
            Ok(bindings) => bindings,
            Err(response) => return response,
        }
    };

    let suggested_zones = suggested_attachment_zones(&resolved);
    let profile = DeviceAttachmentProfile {
        schema_version: 1,
        slots: slots.clone(),
        bindings: resolved.iter().map(|item| item.binding.clone()).collect(),
        suggested_zones: suggested_zones.clone(),
    };
    let device_key = tracked.info.id.to_string();
    {
        let mut profiles = state.attachment_profiles.write().await;
        profiles.update(&device_key, profile);
        if let Err(error) = profiles.save() {
            return ApiError::internal(format!(
                "Failed to persist attachment profile: {error}"
            ));
        }
    }

    let layout_device_id = ensure_default_logical_entry(
        &state,
        tracked.info.id,
        &tracked.info.name,
        tracked.info.total_led_count(),
    )
    .await;
    let needs_layout_update =
        active_layout_targets_device(&state, tracked.info.id, &layout_device_id).await;

    ApiResponse::ok(DeviceAttachmentsUpdateResponse {
        device_id: tracked.info.id.to_string(),
        device_name: tracked.info.name.clone(),
        slots,
        bindings: summarize_resolved_bindings(&resolved),
        suggested_zones,
        needs_layout_update,
    })
}

/// `POST /api/v1/devices/:id/attachments/preview` — Preview attachment zones.
pub async fn preview_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAttachmentsRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };
    let slots = tracked.info.default_attachment_profile().slots;
    let resolved = {
        let registry = state.attachment_registry.read().await;
        match validate_attachment_bindings(&tracked.info, &slots, &body.bindings, &registry) {
            Ok(bindings) => bindings,
            Err(response) => return response,
        }
    };

    ApiResponse::ok(AttachmentPreviewResponse {
        device_id: tracked.info.id.to_string(),
        device_name: tracked.info.name.clone(),
        zones: preview_attachment_zones(&resolved),
    })
}

/// `DELETE /api/v1/devices/:id/attachments` — Remove a stored attachment profile.
pub async fn delete_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let deleted = {
        let mut profiles = state.attachment_profiles.write().await;
        let deleted = profiles.remove(&tracked.info.id.to_string()).is_some();
        if deleted && let Err(error) = profiles.save() {
            return ApiError::internal(format!(
                "Failed to persist attachment profile deletion: {error}"
            ));
        }
        deleted
    };

    ApiResponse::ok(serde_json::json!({
        "device_id": tracked.info.id.to_string(),
        "deleted": deleted,
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
            logical_devices: Arc::clone(&state.logical_devices),
            in_progress: Arc::clone(&state.discovery_in_progress),
        };
        let result =
            discovery::execute_discovery_scan(runtime, config, resolved_backends, timeout).await;

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
            logical_devices: Arc::clone(&state_for_task.logical_devices),
            in_progress: Arc::clone(&state_for_task.discovery_in_progress),
        };
        let _ =
            discovery::execute_discovery_scan(runtime, config, resolved_backends, timeout).await;
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

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };
    if !tracked.state.is_renderable() {
        return ApiError::conflict(format!(
            "Device is not connected: {} (state={})",
            tracked.info.name, tracked.state
        ));
    }

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
    let identify_rgb = color
        .as_deref()
        .and_then(parse_hex_rgb)
        .unwrap_or(DEFAULT_IDENTIFY_COLOR_RGB);
    let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if led_count == 0 {
        return ApiError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
        ));
    }

    let backend_id = backend_id_for_family(&tracked.info.family);
    let manager = Arc::clone(&state.backend_manager);
    let on_frame = vec![identify_rgb; led_count];
    {
        let mut manager = manager.lock().await;
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            led_count,
            color = ?identify_rgb,
            "identify enabling direct control and issuing initial on-frame"
        );
        manager.begin_direct_control(&backend_id, device_id);
        if let Err(error) = manager
            .write_device_colors(&backend_id, device_id, &on_frame)
            .await
        {
            manager.end_direct_control(&backend_id, device_id);
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                error = %error,
                "identify initial write failed"
            );
            return ApiError::internal(format!(
                "Failed to start identify flash for {}: {error}",
                tracked.info.name
            ));
        }
    }

    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        backend = %backend_id,
        led_count,
        duration_ms,
        color = ?identify_rgb,
        "Identify flash started"
    );
    tokio::spawn(run_identify_flash(
        manager,
        backend_id,
        device_id,
        led_count,
        Duration::from_millis(duration_ms),
        identify_rgb,
    ));

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "identifying": true,
        "duration_ms": duration_ms,
        "color": color,
    }))
}

/// `GET /api/v1/logical-devices` — List all logical devices.
pub async fn list_logical_devices(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListLogicalDevicesQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return ApiError::validation("limit must be between 1 and 200");
    }
    let offset = query.offset.unwrap_or(0);

    let physical_devices = state.device_registry.list().await;
    for tracked in &physical_devices {
        ensure_default_logical_entry(
            &state,
            tracked.info.id,
            &tracked.info.name,
            tracked.info.total_led_count(),
        )
        .await;
    }

    let physical_filter = match query.physical_device {
        Some(raw) => match resolve_device_id_or_response(&state, raw.trim()).await {
            Ok(id) => Some(id),
            Err(response) => return response,
        },
        None => None,
    };

    let physical_index = build_physical_index(physical_devices.iter().map(|tracked| {
        (
            tracked.info.id,
            PhysicalSnapshot {
                name: tracked.info.name.clone(),
                backend: backend_id_for_family(&tracked.info.family),
                status: tracked.state.variant_name().to_ascii_lowercase(),
            },
        )
    }));
    let mut items: Vec<LogicalDeviceSummary> = {
        let store = state.logical_devices.read().await;
        store
            .values()
            .filter(|entry| {
                physical_filter.is_none_or(|physical_id| entry.physical_device_id == physical_id)
            })
            .filter(|entry| query.enabled.is_none_or(|enabled| entry.enabled == enabled))
            .map(|entry| summarize_logical_device(entry, &physical_index))
            .collect()
    };

    items.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    let total = items.len();
    let paged_items: Vec<LogicalDeviceSummary> =
        items.into_iter().skip(offset).take(limit).collect();
    let has_more = offset.saturating_add(limit) < total;

    ApiResponse::ok(LogicalDeviceListResponse {
        items: paged_items,
        pagination: Pagination {
            offset,
            limit,
            total,
            has_more,
        },
    })
}

/// `GET /api/v1/devices/:id/logical-devices` — List logical devices for one physical device.
pub async fn list_device_logical_devices(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let physical_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&physical_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    ensure_default_logical_entry(
        &state,
        physical_id,
        &tracked.info.name,
        tracked.info.total_led_count(),
    )
    .await;

    let logical_entries = {
        let store = state.logical_devices.read().await;
        logical_devices::list_for_physical(&store, physical_id)
    };
    let physical_index = build_physical_index(std::iter::once((
        tracked.info.id,
        PhysicalSnapshot {
            name: tracked.info.name.clone(),
            backend: backend_id_for_family(&tracked.info.family),
            status: tracked.state.variant_name().to_ascii_lowercase(),
        },
    )));

    let items: Vec<LogicalDeviceSummary> = logical_entries
        .iter()
        .map(|entry| summarize_logical_device(entry, &physical_index))
        .collect();

    ApiResponse::ok(LogicalDeviceListResponse {
        pagination: Pagination {
            offset: 0,
            limit: items.len(),
            total: items.len(),
            has_more: false,
        },
        items,
    })
}

/// `POST /api/v1/devices/:id/logical-devices` — Create a logical segment for a physical device.
pub async fn create_logical_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<CreateLogicalDeviceRequest>,
) -> Response {
    let physical_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&physical_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };
    let normalized_name = match normalize_logical_name(&body.name) {
        Ok(name) => name,
        Err(error) => return ApiError::validation(error),
    };

    let physical_layout_id = ensure_default_logical_entry(
        &state,
        physical_id,
        &tracked.info.name,
        tracked.info.total_led_count(),
    )
    .await;
    let physical_led_count = tracked.info.total_led_count();

    let created = {
        let mut store = state.logical_devices.write().await;
        let logical_id =
            logical_devices::allocate_segment_id(&store, &physical_layout_id, &normalized_name);
        let entry = LogicalDevice {
            id: logical_id.clone(),
            physical_device_id: physical_id,
            name: normalized_name,
            led_start: body.led_start,
            led_count: body.led_count,
            enabled: body.enabled.unwrap_or(true),
            kind: LogicalDeviceKind::Segment,
        };
        if let Err(error) =
            logical_devices::validate_entry(&store, &entry, physical_led_count, None)
        {
            return ApiError::validation(error);
        }

        store.insert(logical_id.clone(), entry);
        logical_devices::reconcile_default_enabled(&mut store, physical_id);
        store
            .get(&logical_id)
            .expect("created logical device must exist")
            .clone()
    };

    if let Err(error) = persist_logical_segments(&state).await {
        return ApiError::internal(format!("Failed to persist logical devices: {error}"));
    }

    sync_live_logical_mappings_for_device(&state, physical_id).await;

    let physical_index = build_physical_index(std::iter::once((
        tracked.info.id,
        PhysicalSnapshot {
            name: tracked.info.name.clone(),
            backend: backend_id_for_family(&tracked.info.family),
            status: tracked.state.variant_name().to_ascii_lowercase(),
        },
    )));
    ApiResponse::created(summarize_logical_device(&created, &physical_index))
}

/// `GET /api/v1/logical-devices/:id` — Get one logical device.
pub async fn get_logical_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let entry = {
        let store = state.logical_devices.read().await;
        store.get(&id).cloned()
    };
    let Some(entry) = entry else {
        return ApiError::not_found(format!("Logical device not found: {id}"));
    };

    let physical_devices = state.device_registry.list().await;
    let physical_index = build_physical_index(physical_devices.iter().map(|tracked| {
        (
            tracked.info.id,
            PhysicalSnapshot {
                name: tracked.info.name.clone(),
                backend: backend_id_for_family(&tracked.info.family),
                status: tracked.state.variant_name().to_ascii_lowercase(),
            },
        )
    }));
    ApiResponse::ok(summarize_logical_device(&entry, &physical_index))
}

/// `PUT /api/v1/logical-devices/:id` — Update one logical device.
pub async fn update_logical_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLogicalDeviceRequest>,
) -> Response {
    if body.name.is_none()
        && body.led_start.is_none()
        && body.led_count.is_none()
        && body.enabled.is_none()
    {
        return ApiError::validation(
            "At least one field must be provided: name, led_start, led_count, or enabled",
        );
    }

    let existing = {
        let store = state.logical_devices.read().await;
        store.get(&id).cloned()
    };
    let Some(existing) = existing else {
        return ApiError::not_found(format!("Logical device not found: {id}"));
    };

    if existing.kind == LogicalDeviceKind::Default
        && (body.led_start.is_some() || body.led_count.is_some())
    {
        return ApiError::validation("Default logical devices always span the full physical range");
    }

    let Some(tracked) = state
        .device_registry
        .get(&existing.physical_device_id)
        .await
    else {
        return ApiError::not_found(format!(
            "Physical device not found for logical device: {}",
            existing.id
        ));
    };
    let physical_led_count = tracked.info.total_led_count();

    let updated = {
        let mut candidate = existing.clone();
        if let Some(name) = body.name {
            candidate.name = match normalize_logical_name(&name) {
                Ok(value) => value,
                Err(error) => return ApiError::validation(error),
            };
        }
        if let Some(led_start) = body.led_start {
            candidate.led_start = led_start;
        }
        if let Some(led_count) = body.led_count {
            candidate.led_count = led_count;
        }
        if let Some(enabled) = body.enabled {
            candidate.enabled = enabled;
        }

        let mut store = state.logical_devices.write().await;
        if let Err(error) =
            logical_devices::validate_entry(&store, &candidate, physical_led_count, Some(&id))
        {
            return ApiError::validation(error);
        }
        store.insert(id.clone(), candidate);
        logical_devices::reconcile_default_enabled(&mut store, existing.physical_device_id);
        store
            .get(&id)
            .expect("updated logical device must exist")
            .clone()
    };

    if let Err(error) = persist_logical_segments(&state).await {
        return ApiError::internal(format!("Failed to persist logical devices: {error}"));
    }

    sync_live_logical_mappings_for_device(&state, existing.physical_device_id).await;

    let physical_index = build_physical_index(std::iter::once((
        tracked.info.id,
        PhysicalSnapshot {
            name: tracked.info.name.clone(),
            backend: backend_id_for_family(&tracked.info.family),
            status: tracked.state.variant_name().to_ascii_lowercase(),
        },
    )));
    ApiResponse::ok(summarize_logical_device(&updated, &physical_index))
}

/// `DELETE /api/v1/logical-devices/:id` — Delete one logical device.
pub async fn delete_logical_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let existing = {
        let store = state.logical_devices.read().await;
        store.get(&id).cloned()
    };
    let Some(existing) = existing else {
        return ApiError::not_found(format!("Logical device not found: {id}"));
    };

    if existing.kind == LogicalDeviceKind::Default {
        return ApiError::conflict("Cannot delete the default logical device");
    }

    {
        let mut store = state.logical_devices.write().await;
        store.remove(&id);
        logical_devices::reconcile_default_enabled(&mut store, existing.physical_device_id);
    }

    if let Err(error) = persist_logical_segments(&state).await {
        return ApiError::internal(format!("Failed to persist logical devices: {error}"));
    }

    sync_live_logical_mappings_for_device(&state, existing.physical_device_id).await;

    ApiResponse::ok(serde_json::json!({
        "id": id,
        "deleted": true,
    }))
}

#[derive(Debug, Clone)]
struct PhysicalSnapshot {
    name: String,
    backend: String,
    status: String,
}

fn build_physical_index<I>(entries: I) -> HashMap<DeviceId, PhysicalSnapshot>
where
    I: IntoIterator<Item = (DeviceId, PhysicalSnapshot)>,
{
    entries.into_iter().collect()
}

fn summarize_logical_device(
    entry: &LogicalDevice,
    physical_index: &HashMap<DeviceId, PhysicalSnapshot>,
) -> LogicalDeviceSummary {
    let physical = physical_index
        .get(&entry.physical_device_id)
        .cloned()
        .unwrap_or(PhysicalSnapshot {
            name: "Unknown Device".to_owned(),
            backend: "unknown".to_owned(),
            status: "unknown".to_owned(),
        });

    LogicalDeviceSummary {
        id: entry.id.clone(),
        name: entry.name.clone(),
        kind: match entry.kind {
            LogicalDeviceKind::Default => "default",
            LogicalDeviceKind::Segment => "segment",
        }
        .to_owned(),
        enabled: entry.enabled,
        led_start: entry.led_start,
        led_count: entry.led_count,
        led_end: entry.led_end_exclusive(),
        physical_device_id: entry.physical_device_id.to_string(),
        physical_device_name: physical.name,
        backend: physical.backend,
        physical_status: physical.status,
    }
}

async fn ensure_default_logical_entry(
    state: &AppState,
    physical_id: DeviceId,
    physical_name: &str,
    physical_led_count: u32,
) -> String {
    let fallback_layout_id = {
        let lifecycle = state.lifecycle_manager.lock().await;
        lifecycle
            .layout_device_id_for(physical_id)
            .map_or_else(|| format!("device:{physical_id}"), ToOwned::to_owned)
    };

    let mut store = state.logical_devices.write().await;
    let default = logical_devices::ensure_default_logical_device(
        &mut store,
        physical_id,
        &fallback_layout_id,
        physical_name,
        physical_led_count,
    );
    default.id
}

async fn sync_live_logical_mappings_for_device(state: &AppState, physical_id: DeviceId) {
    let Some(tracked) = state.device_registry.get(&physical_id).await else {
        return;
    };

    let fallback_layout_id = ensure_default_logical_entry(
        state,
        physical_id,
        &tracked.info.name,
        tracked.info.total_led_count(),
    )
    .await;

    let backend_id = backend_id_for_family(&tracked.info.family);
    let logical_entries = {
        let store = state.logical_devices.read().await;
        logical_devices::list_for_physical(&store, physical_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .collect::<Vec<_>>()
    };

    let mut manager = state.backend_manager.lock().await;
    let _ = manager.remove_device_mappings_for_physical(&backend_id, physical_id);

    if !tracked.state.is_renderable() {
        return;
    }

    if logical_entries.is_empty() {
        manager.map_device_with_segment(
            fallback_layout_id.clone(),
            backend_id.clone(),
            physical_id,
            Some(SegmentRange::new(
                0,
                usize::try_from(tracked.info.total_led_count()).unwrap_or_default(),
            )),
        );
        map_physical_device_alias(
            &mut manager,
            backend_id,
            physical_id,
            &fallback_layout_id,
            SegmentRange::new(
                0,
                usize::try_from(tracked.info.total_led_count()).unwrap_or_default(),
            ),
        );
        return;
    }

    let mut default_enabled = false;
    for entry in logical_entries {
        let start = usize::try_from(entry.led_start).unwrap_or_default();
        let length = usize::try_from(entry.led_count).unwrap_or_default();
        if entry.id == fallback_layout_id {
            default_enabled = true;
        }
        manager.map_device_with_segment(
            entry.id,
            backend_id.clone(),
            physical_id,
            Some(SegmentRange::new(start, length)),
        );
    }

    if default_enabled {
        map_physical_device_alias(
            &mut manager,
            backend_id,
            physical_id,
            &fallback_layout_id,
            SegmentRange::new(
                0,
                usize::try_from(tracked.info.total_led_count()).unwrap_or_default(),
            ),
        );
    }
}

fn normalize_logical_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Logical device name must not be empty".to_owned());
    }
    Ok(trimmed.to_owned())
}

async fn persist_logical_segments(state: &AppState) -> Result<(), String> {
    let snapshot = {
        let store = state.logical_devices.read().await;
        store.clone()
    };
    logical_devices::save_segments(&state.logical_devices_path, &snapshot)
        .map_err(|error| format!("{} ({})", error, state.logical_devices_path.display()))
}

fn summarize_device(
    info: &DeviceInfo,
    state: &DeviceState,
    layout_device_id: String,
) -> DeviceSummary {
    DeviceSummary {
        id: info.id.to_string(),
        layout_device_id,
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
                topology_hint: summarize_zone_topology(&z.topology),
            })
            .collect(),
    }
}

fn summarize_attachment_profile(
    device: &DeviceInfo,
    profile: DeviceAttachmentProfile,
    registry: &hypercolor_core::attachment::AttachmentRegistry,
) -> DeviceAttachmentsResponse {
    let suggested_zones = if profile.suggested_zones.is_empty() {
        resolve_profile_bindings(device, &profile, registry)
            .map_or_else(Vec::new, |resolved| suggested_attachment_zones(&resolved))
    } else {
        profile.suggested_zones.clone()
    };
    let bindings = profile
        .bindings
        .iter()
        .map(|binding| summarize_attachment_binding(binding, registry.get(&binding.template_id)))
        .collect();

    DeviceAttachmentsResponse {
        device_id: device.id.to_string(),
        device_name: device.name.clone(),
        slots: profile.slots,
        bindings,
        suggested_zones,
    }
}

fn summarize_attachment_binding(
    binding: &AttachmentBinding,
    template: Option<&AttachmentTemplate>,
) -> AttachmentBindingSummary {
    AttachmentBindingSummary {
        slot_id: binding.slot_id.clone(),
        template_id: binding.template_id.clone(),
        template_name: template.map_or_else(
            || binding.template_id.clone(),
            |template| template.name.clone(),
        ),
        name: binding.name.clone(),
        enabled: binding.enabled,
        instances: binding.instances,
        led_offset: binding.led_offset,
        effective_led_count: template.map_or(0, |template| binding.effective_led_count(template)),
    }
}

fn summarize_resolved_bindings(
    bindings: &[ResolvedAttachmentBinding],
) -> Vec<AttachmentBindingSummary> {
    bindings
        .iter()
        .map(|binding| AttachmentBindingSummary {
            slot_id: binding.binding.slot_id.clone(),
            template_id: binding.binding.template_id.clone(),
            template_name: binding.template.name.clone(),
            name: binding.binding.name.clone(),
            enabled: binding.binding.enabled,
            instances: binding.binding.instances,
            led_offset: binding.binding.led_offset,
            effective_led_count: binding.effective_led_count,
        })
        .collect()
}

fn preview_attachment_zones(bindings: &[ResolvedAttachmentBinding]) -> Vec<AttachmentPreviewZone> {
    let mut zones = Vec::new();

    for binding in bindings {
        let led_positions = generate_positions(&binding.template.topology);
        let template_led_count = binding.template.led_count();
        for instance in 0..binding.binding.instances {
            let led_start = binding
                .slot
                .led_start
                .saturating_add(binding.binding.led_offset)
                .saturating_add(instance.saturating_mul(template_led_count));
            zones.push(AttachmentPreviewZone {
                slot_id: binding.binding.slot_id.clone(),
                binding_index: binding.index,
                instance,
                template_id: binding.binding.template_id.clone(),
                template_name: binding.template.name.clone(),
                name: preview_attachment_zone_name(binding, instance),
                led_start,
                led_count: template_led_count,
                topology: binding.template.topology.clone(),
                led_positions: led_positions.clone(),
            });
        }
    }

    zones
}

fn suggested_attachment_zones(bindings: &[ResolvedAttachmentBinding]) -> Vec<AttachmentSuggestedZone> {
    let mut zones = Vec::new();

    for binding in bindings {
        let template_led_count = binding.template.led_count();
        for instance in 0..binding.binding.instances {
            let led_start = binding
                .slot
                .led_start
                .saturating_add(binding.binding.led_offset)
                .saturating_add(instance.saturating_mul(template_led_count));
            zones.push(AttachmentSuggestedZone {
                slot_id: binding.binding.slot_id.clone(),
                template_id: binding.binding.template_id.clone(),
                template_name: binding.template.name.clone(),
                name: preview_attachment_zone_name(binding, instance),
                instance,
                led_start,
                led_count: template_led_count,
                category: binding.template.category.clone(),
                default_size: binding.template.default_size,
                topology: binding.template.topology.clone(),
                led_mapping: binding.template.led_mapping.clone(),
            });
        }
    }

    zones
}

fn preview_attachment_zone_name(binding: &ResolvedAttachmentBinding, instance: u32) -> String {
    match binding.binding.name.as_deref() {
        Some(name) if binding.binding.instances > 1 => {
            format!("{name} - {} {}", binding.template.name, instance + 1)
        }
        Some(name) => name.to_owned(),
        None if binding.binding.instances > 1 => {
            format!("{} {}", binding.template.name, instance + 1)
        }
        None => binding.template.name.clone(),
    }
}

fn resolve_profile_bindings(
    device: &DeviceInfo,
    profile: &DeviceAttachmentProfile,
    registry: &hypercolor_core::attachment::AttachmentRegistry,
) -> Option<Vec<ResolvedAttachmentBinding>> {
    validate_attachment_bindings(device, &profile.slots, &profile.bindings, registry).ok()
}

#[expect(
    clippy::result_large_err,
    reason = "private handler helper returns a concrete HTTP response on validation failure"
)]
fn validate_attachment_bindings(
    device: &DeviceInfo,
    slots: &[AttachmentSlot],
    bindings: &[AttachmentBinding],
    registry: &hypercolor_core::attachment::AttachmentRegistry,
) -> Result<Vec<ResolvedAttachmentBinding>, Response> {
    let slot_index = slots
        .iter()
        .map(|slot| (slot.id.as_str(), slot))
        .collect::<HashMap<_, _>>();
    let family = attachment_family_id(&device.family);
    let mut resolved = Vec::with_capacity(bindings.len());

    for (index, binding) in bindings.iter().enumerate() {
        let slot_id = binding.slot_id.trim();
        if slot_id.is_empty() {
            return Err(ApiError::validation(format!(
                "binding {index} has an empty slot_id"
            )));
        }

        let template_id = binding.template_id.trim();
        if template_id.is_empty() {
            return Err(ApiError::validation(format!(
                "binding {index} has an empty template_id"
            )));
        }

        if binding.instances == 0 {
            return Err(ApiError::validation(format!(
                "binding {index} must set instances to at least 1"
            )));
        }

        let Some(slot) = slot_index.get(slot_id).copied() else {
            return Err(ApiError::validation(format!(
                "binding {index} targets unknown slot '{slot_id}'"
            )));
        };
        let Some(template) = registry.get(template_id) else {
            return Err(ApiError::validation(format!(
                "binding {index} references unknown template '{template_id}'"
            )));
        };

        if !slot.supports_template(template) {
            return Err(ApiError::validation(format!(
                "template '{template_id}' is not allowed for slot '{slot_id}'"
            )));
        }
        if !template.supports_slot(&family, device.model.as_deref(), slot_id) {
            return Err(ApiError::validation(format!(
                "template '{template_id}' is not compatible with {} slot '{slot_id}'",
                device.name
            )));
        }

        let effective_led_count = binding.effective_led_count(template);
        let Some(binding_end) = binding.led_offset.checked_add(effective_led_count) else {
            return Err(ApiError::validation(format!(
                "binding {index} exceeds slot '{slot_id}' LED range"
            )));
        };
        if binding_end > slot.led_count {
            return Err(ApiError::validation(format!(
                "binding {index} exceeds slot '{slot_id}' capacity: {binding_end} > {}",
                slot.led_count
            )));
        }

        resolved.push(ResolvedAttachmentBinding {
            index,
            binding: AttachmentBinding {
                slot_id: slot_id.to_owned(),
                template_id: template_id.to_owned(),
                name: normalize_attachment_binding_name(binding.name.as_deref()),
                enabled: binding.enabled,
                instances: binding.instances,
                led_offset: binding.led_offset,
            },
            slot: slot.clone(),
            template: template.clone(),
            effective_led_count,
        });
    }

    validate_attachment_overlaps(&resolved)?;
    Ok(resolved)
}

#[expect(
    clippy::result_large_err,
    reason = "private handler helper returns a concrete HTTP response on validation failure"
)]
fn validate_attachment_overlaps(bindings: &[ResolvedAttachmentBinding]) -> Result<(), Response> {
    let mut enabled = bindings
        .iter()
        .filter(|binding| binding.binding.enabled)
        .collect::<Vec<_>>();
    enabled.sort_by(|left, right| {
        left.binding
            .slot_id
            .cmp(&right.binding.slot_id)
            .then_with(|| left.binding.led_offset.cmp(&right.binding.led_offset))
            .then_with(|| left.index.cmp(&right.index))
    });

    for pair in enabled.windows(2) {
        let [current, next] = pair else {
            continue;
        };
        if current.binding.slot_id != next.binding.slot_id {
            continue;
        }

        let current_end = current
            .binding
            .led_offset
            .saturating_add(current.effective_led_count);
        if next.binding.led_offset < current_end {
            return Err(ApiError::validation(format!(
                "bindings {} and {} overlap within slot '{}'",
                current.index, next.index, current.binding.slot_id
            )));
        }
    }

    Ok(())
}

fn normalize_attachment_binding_name(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn active_layout_targets_device(
    state: &AppState,
    physical_id: DeviceId,
    default_layout_id: &str,
) -> bool {
    let mut logical_ids = {
        let store = state.logical_devices.read().await;
        logical_devices::list_for_physical(&store, physical_id)
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>()
    };
    if !logical_ids.iter().any(|id| id == default_layout_id) {
        logical_ids.push(default_layout_id.to_owned());
    }

    let spatial = state.spatial_engine.read().await;
    spatial
        .layout()
        .zones
        .iter()
        .any(|zone| logical_ids.iter().any(|candidate| candidate == &zone.device_id))
}

fn map_physical_device_alias(
    manager: &mut BackendManager,
    backend_id: String,
    physical_id: DeviceId,
    layout_device_id: &str,
    segment: SegmentRange,
) {
    let physical_alias = physical_id.to_string();
    if physical_alias == layout_device_id {
        return;
    }

    manager.map_device_with_segment(physical_alias, backend_id, physical_id, Some(segment));
}

fn summarize_zone_topology(topology: &DeviceTopologyHint) -> ZoneTopologySummary {
    match topology {
        DeviceTopologyHint::Strip => ZoneTopologySummary::Strip,
        DeviceTopologyHint::Matrix { rows, cols } => ZoneTopologySummary::Matrix {
            rows: *rows,
            cols: *cols,
        },
        DeviceTopologyHint::Ring { count } => ZoneTopologySummary::Ring { count: *count },
        DeviceTopologyHint::Point => ZoneTopologySummary::Point,
        DeviceTopologyHint::Custom => ZoneTopologySummary::Custom,
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

fn backend_id_for_family(family: &DeviceFamily) -> String {
    match family {
        DeviceFamily::OpenRgb => "openrgb".to_owned(),
        DeviceFamily::Wled => "wled".to_owned(),
        DeviceFamily::Hue => "hue".to_owned(),
        DeviceFamily::Razer | DeviceFamily::LianLi | DeviceFamily::PrismRgb => "usb".to_owned(),
        DeviceFamily::Corsair => "corsair-bridge".to_owned(),
        DeviceFamily::Custom(name) => name.to_ascii_lowercase(),
    }
}

fn attachment_family_id(family: &DeviceFamily) -> String {
    match family {
        DeviceFamily::OpenRgb => "openrgb".to_owned(),
        DeviceFamily::Wled => "wled".to_owned(),
        DeviceFamily::Hue => "hue".to_owned(),
        DeviceFamily::Razer => "razer".to_owned(),
        DeviceFamily::Corsair => "corsair".to_owned(),
        DeviceFamily::LianLi => "lianli".to_owned(),
        DeviceFamily::PrismRgb => "prismrgb".to_owned(),
        DeviceFamily::Custom(name) => name
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || *character == '_' || *character == '-')
            .collect::<String>()
            .to_ascii_lowercase(),
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

async fn run_identify_flash(
    manager: Arc<tokio::sync::Mutex<BackendManager>>,
    backend_id: String,
    device_id: DeviceId,
    led_count: usize,
    duration: Duration,
    color: [u8; 3],
) {
    if led_count == 0 {
        return;
    }

    let on_frame = vec![color; led_count];
    let off_frame = vec![[0, 0, 0]; led_count];
    let started_at = Instant::now();
    let mut show_on = false;
    let mut identify_failed = false;
    let mut phase_index = 0_u32;

    loop {
        if started_at.elapsed() >= duration {
            break;
        }

        tokio::time::sleep(Duration::from_millis(IDENTIFY_FLASH_INTERVAL_MS)).await;

        let frame = if show_on { &on_frame } else { &off_frame };
        let phase = if show_on { "on" } else { "off" };
        phase_index = phase_index.saturating_add(1);
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            phase_index,
            phase,
            elapsed_ms = started_at.elapsed().as_millis(),
            frame_leds = frame.len(),
            "identify issuing flash phase"
        );
        let result = {
            let mut manager = manager.lock().await;
            manager
                .write_device_colors(&backend_id, device_id, frame)
                .await
        };

        if let Err(error) = result {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                error = %error,
                "identify write failed"
            );
            identify_failed = true;
            break;
        }

        show_on = !show_on;
    }

    if !identify_failed {
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            elapsed_ms = started_at.elapsed().as_millis(),
            "identify issuing final clear frame"
        );
        let clear_result = {
            let mut manager = manager.lock().await;
            manager
                .write_device_colors(&backend_id, device_id, &off_frame)
                .await
        };
        if let Err(error) = clear_result {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                error = %error,
                "identify clear write failed"
            );
        }
    }

    {
        let mut manager = manager.lock().await;
        manager.end_direct_control(&backend_id, device_id);
    }
    debug!(
        backend_id = %backend_id,
        device_id = %device_id,
        elapsed_ms = started_at.elapsed().as_millis(),
        identify_failed,
        "identify released direct control"
    );

    if identify_failed {
        return;
    }

    tracing::info!(
        device_id = %device_id,
        backend = %backend_id,
        "Identify flash completed"
    );
}

fn parse_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let color = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if color.len() != 6 || !color.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", color.to_ascii_uppercase()))
}

fn parse_hex_rgb(raw: &str) -> Option<[u8; 3]> {
    let color = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    if color.len() != 6 {
        return None;
    }

    let red = u8::from_str_radix(&color[0..2], 16).ok()?;
    let green = u8::from_str_radix(&color[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&color[4..6], 16).ok()?;
    Some([red, green, blue])
}
