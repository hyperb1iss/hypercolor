//! Device endpoints — `/api/v1/devices/*`.
//!
//! Core CRUD, identify flows, and shared helpers live here. Attachment,
//! pairing, discovery, and logical-device endpoints are split into sibling
//! submodules.

mod attachments;
mod discovery;
mod logical;
mod pairing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use utoipa::ToSchema;

use hypercolor_core::device::{BackendIo, BackendManager, DeviceLifecycleManager};
use hypercolor_driver_api::DeviceAuthSummary;
use hypercolor_types::attachment::{AttachmentBinding, AttachmentSlot};
use hypercolor_types::device::{
    ConnectionType, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    DeviceUserSettings, DriverPresentation,
};
use hypercolor_types::event::HypercolorEvent;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::device_metrics::DeviceMetricsSnapshot;
use crate::discovery as core_discovery;

pub use attachments::{
    AttachmentBindingSummary, AttachmentPreviewResponse, AttachmentPreviewZone,
    DeviceAttachmentsResponse, DeviceAttachmentsUpdateResponse, UpdateAttachmentsRequest,
    delete_attachments, get_attachments, preview_attachments, update_attachments,
};
pub use discovery::{DiscoverRequest, discover_devices};
pub use logical::{
    CreateLogicalDeviceRequest, ListLogicalDevicesQuery, LogicalDeviceListResponse,
    LogicalDeviceSummary, UpdateLogicalDeviceRequest, create_logical_device, delete_logical_device,
    get_logical_device, list_device_logical_devices, list_logical_devices, update_logical_device,
};
pub use pairing::{
    GenericPairDeviceRequest, GenericPairDeviceResponse, delete_pairing, pair_device,
};

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDeviceRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub brightness: Option<u8>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct IdentifyRequest {
    pub duration_ms: Option<u64>,
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IdentifyAttachmentRequest {
    #[serde(flatten)]
    pub base: IdentifyRequest,
    pub binding_index: Option<usize>,
    pub instance: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeviceListResponse {
    pub items: Vec<DeviceSummary>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeviceSummary {
    pub id: String,
    pub layout_device_id: String,
    pub name: String,
    pub backend: String,
    pub origin: DeviceOrigin,
    pub presentation: DriverPresentation,
    pub status: String,
    pub brightness: u8,
    pub firmware_version: Option<String>,
    pub network_ip: Option<String>,
    pub network_hostname: Option<String>,
    pub connection_label: Option<String>,
    pub total_leds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<DeviceAuthSummary>,
    pub zones: Vec<ZoneSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ZoneSummary {
    pub id: String,
    pub name: String,
    pub led_count: u32,
    pub topology: String,
    pub topology_hint: ZoneTopologySummary,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ZoneTopologySummary {
    Strip,
    Matrix {
        rows: u32,
        cols: u32,
    },
    Ring {
        count: u32,
    },
    Point,
    Display {
        width: u32,
        height: u32,
        circular: bool,
    },
    Custom,
}

#[derive(Debug, Serialize, ToSchema)]
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
    pub driver: Option<String>,
    pub q: Option<String>,
}

const IDENTIFY_FLASH_INTERVAL_MS: u64 = 250;
const DEFAULT_IDENTIFY_COLOR_RGB: [u8; 3] = [255, 255, 255];

#[derive(Debug)]
enum ResolveDeviceError {
    AmbiguousName(String),
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/devices` — List all tracked devices.
#[utoipa::path(
    get,
    path = "/api/v1/devices",
    params(
        ("offset" = Option<usize>, Query, description = "Number of devices to skip"),
        ("limit" = Option<usize>, Query, description = "Maximum number of devices to return"),
        ("status" = Option<String>, Query, description = "Filter by device status"),
        ("backend" = Option<String>, Query, description = "Filter by output backend route"),
        ("driver" = Option<String>, Query, description = "Filter by owning driver module"),
        ("q" = Option<String>, Query, description = "Case-insensitive name/vendor search")
    ),
    responses(
        (
            status = 200,
            description = "Tracked devices",
            body = crate::api::envelope::ApiResponse<DeviceListResponse>
        ),
        (
            status = 422,
            description = "Query validation failed",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "devices"
)]
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
    let driver_filter = query
        .driver
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
                tracked.info.output_backend_id().to_ascii_lowercase() == *expected
            })
        })
        .filter(|tracked| {
            driver_filter
                .as_deref()
                .is_none_or(|expected| tracked.info.driver_id().to_ascii_lowercase() == *expected)
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
        let layout_device_id = ensure_default_logical_entry(&state, &tracked.info).await;
        let metadata = state
            .device_registry
            .metadata_for_id(&tracked.info.id)
            .await;
        items.push(
            summarize_device_for_response(
                &state,
                &tracked.info,
                &tracked.state,
                tracked.user_settings.brightness,
                layout_device_id,
                metadata.as_ref(),
            )
            .await,
        );
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

/// `GET /api/v1/devices/metrics` — List current per-device output telemetry.
pub async fn list_device_metrics(State(state): State<Arc<AppState>>) -> Response {
    let snapshot = state.device_metrics.load_full();
    ApiResponse::ok(DeviceMetricsSnapshot {
        taken_at_ms: snapshot.taken_at_ms,
        items: snapshot.items.clone(),
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
#[utoipa::path(
    get,
    path = "/api/v1/devices/{id}",
    params(("id" = String, Path, description = "Device id or display name")),
    responses(
        (
            status = 200,
            description = "Device detail",
            body = crate::api::envelope::ApiResponse<DeviceSummary>
        ),
        (
            status = 404,
            description = "Device was not found",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "devices"
)]
pub async fn get_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let layout_device_id = ensure_default_logical_entry(&state, &tracked.info).await;
    let metadata = state
        .device_registry
        .metadata_for_id(&tracked.info.id)
        .await;

    ApiResponse::ok(
        summarize_device_for_response(
            &state,
            &tracked.info,
            &tracked.state,
            tracked.user_settings.brightness,
            layout_device_id,
            metadata.as_ref(),
        )
        .await,
    )
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

    if body.name.is_none() && body.enabled.is_none() && body.brightness.is_none() {
        return ApiError::validation(
            "At least one field must be provided: name, enabled, or brightness",
        );
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
    let normalized_brightness = match body.brightness {
        Some(brightness) if brightness <= 100 => Some(percent_to_brightness(brightness)),
        Some(_) => return ApiError::validation("Device brightness must be between 0 and 100"),
        None => None,
    };

    let enabled_handled_by_lifecycle = if let Some(enabled) = body.enabled {
        let runtime = super::discovery_runtime(&state);
        match core_discovery::apply_user_enabled_state(&runtime, device_id, enabled).await {
            Ok(core_discovery::UserEnabledStateResult::Applied) => true,
            Ok(core_discovery::UserEnabledStateResult::MissingLifecycle) => false,
            Err(error) => {
                return ApiError::internal(format!(
                    "Failed to update device state for {id}: {error}"
                ));
            }
        }
    } else {
        false
    };

    let Some(mut updated) = state
        .device_registry
        .update_user_settings(
            &device_id,
            normalized_name,
            body.enabled,
            normalized_brightness,
        )
        .await
    else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    if !enabled_handled_by_lifecycle && let Some(enabled) = body.enabled {
        let fallback_state = if enabled {
            DeviceState::Known
        } else {
            DeviceState::Disabled
        };
        let _ = state
            .device_registry
            .set_state(&device_id, fallback_state)
            .await;
        if let Some(tracked) = state.device_registry.get(&device_id).await {
            updated = tracked;
        }
    }

    if let Err(error) = persist_device_settings_for(&state, device_id, &updated.user_settings).await
    {
        return ApiError::internal(format!("Failed to persist device settings: {error}"));
    }
    sync_device_output_brightness(&state, device_id, &updated.user_settings).await;
    publish_device_settings_changed(&state, device_id, &updated.user_settings);

    let layout_device_id = ensure_default_logical_entry(&state, &updated.info).await;
    let metadata = state
        .device_registry
        .metadata_for_id(&updated.info.id)
        .await;

    ApiResponse::ok(
        summarize_device_for_response(
            &state,
            &updated.info,
            &updated.state,
            updated.user_settings.brightness,
            layout_device_id,
            metadata.as_ref(),
        )
        .await,
    )
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
    crate::api::prune_scene_display_groups_for_device(&state, device_id).await;

    ApiResponse::ok(serde_json::json!({
        "id": device_id.to_string(),
        "removed": true,
    }))
}

/// `POST /api/v1/devices/:id/identify` — Flash identification pattern.
#[expect(
    clippy::too_many_lines,
    reason = "identify setup validates request state, acquires direct backend access, and launches the flash task in one API entrypoint"
)]
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
    let identify_brightness = ((*state.power_state.borrow()).effective_brightness()
        * tracked.user_settings.brightness)
        .clamp(0.0, 1.0);
    let identify_color = scale_rgb(identify_rgb, identify_brightness);
    let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if led_count == 0 {
        return ApiError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
        ));
    }

    let backend_id = resolved_backend_id(&tracked.info);
    let network_metadata = state.device_registry.metadata_for_id(&device_id).await;
    let network_ip = network_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("ip").cloned());
    let network_hostname = network_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("hostname").cloned());
    let on_frame = vec![identify_color; led_count];
    let (manager, direct_backend, disconnect_after_identify) = match prepare_identify_backend(
        &state,
        device_id,
        &tracked.info,
        tracked.state,
        &backend_id,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    debug!(
        backend_id = %backend_id,
        device_id = %device_id,
        led_count,
        color = ?identify_rgb,
        effective_brightness = identify_brightness,
        network_ip = ?network_ip,
        network_hostname = ?network_hostname,
        disconnect_after_identify,
        "identify enabling direct control and issuing initial on-frame"
    );

    if let Err(error) = direct_backend.write_colors(device_id, &on_frame).await {
        let mut manager = manager.lock().await;
        manager.end_direct_control(&backend_id, device_id);
        if disconnect_after_identify {
            let _ = direct_backend.disconnect(device_id).await;
        }
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

    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        backend = %backend_id,
        led_count,
        duration_ms,
        color = ?identify_rgb,
        effective_brightness = identify_brightness,
        network_ip = ?network_ip,
        network_hostname = ?network_hostname,
        "Identify flash started"
    );
    tokio::spawn(run_identify_flash(
        manager,
        direct_backend,
        backend_id,
        device_id,
        on_frame,
        Duration::from_millis(duration_ms),
        disconnect_after_identify,
    ));

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "identifying": true,
        "duration_ms": duration_ms,
        "color": color,
    }))
}

/// `POST /api/v1/devices/:id/zones/:zone_id/identify` — Flash a single zone.
#[allow(
    clippy::too_many_lines,
    reason = "the handler intentionally keeps validation, direct-control orchestration, and response shaping together"
)]
pub async fn identify_zone(
    State(state): State<Arc<AppState>>,
    Path((id, zone_id)): Path<(String, String)>,
    body: Option<Json<IdentifyRequest>>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let zone_index = match resolve_zone_index(&tracked.info, &zone_id) {
        Ok(index) => index,
        Err(response) => return response,
    };

    let total_leds = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if total_leds == 0 {
        return ApiError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
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
    let identify_brightness = ((*state.power_state.borrow()).effective_brightness()
        * tracked.user_settings.brightness)
        .clamp(0.0, 1.0);
    let identify_color = scale_rgb(identify_rgb, identify_brightness);

    let on_frame = build_zone_identify_frame(&tracked.info, zone_index, identify_color);

    let backend_id = resolved_backend_id(&tracked.info);
    let (manager, direct_backend, disconnect_after_identify) = match prepare_identify_backend(
        &state,
        device_id,
        &tracked.info,
        tracked.state,
        &backend_id,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };

    if let Err(error) = direct_backend.write_colors(device_id, &on_frame).await {
        let mut manager = manager.lock().await;
        manager.end_direct_control(&backend_id, device_id);
        if disconnect_after_identify {
            let _ = direct_backend.disconnect(device_id).await;
        }
        warn!(
            backend_id = %backend_id,
            device_id = %device_id,
            zone = %tracked.info.zones[zone_index].name,
            error = %error,
            "zone identify initial write failed"
        );
        return ApiError::internal(format!(
            "Failed to start zone identify for {}: {error}",
            tracked.info.name
        ));
    }

    let zone_name = tracked.info.zones[zone_index].name.clone();
    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        zone = %zone_name,
        zone_index,
        backend = %backend_id,
        duration_ms,
        color = ?identify_rgb,
        "Zone identify flash started"
    );
    tokio::spawn(run_identify_flash(
        manager,
        direct_backend,
        backend_id,
        device_id,
        on_frame,
        Duration::from_millis(duration_ms),
        disconnect_after_identify,
    ));

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "zone_id": zone_id,
        "zone_name": zone_name,
        "identifying": true,
        "duration_ms": duration_ms,
        "color": color,
    }))
}

/// `POST /api/v1/devices/:id/attachments/:slot_id/identify` — Flash a single
/// attachment component within a slot.
#[allow(
    clippy::too_many_lines,
    reason = "the handler intentionally keeps validation, direct-control orchestration, and response shaping together"
)]
pub async fn identify_attachment(
    State(state): State<Arc<AppState>>,
    Path((id, slot_id)): Path<(String, String)>,
    body: Option<Json<IdentifyAttachmentRequest>>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let total_leds = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if total_leds == 0 {
        return ApiError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
        ));
    }

    let duration_ms = body
        .as_ref()
        .and_then(|b| b.base.duration_ms)
        .unwrap_or(3000);
    if duration_ms == 0 || duration_ms > 120_000 {
        return ApiError::validation("duration_ms must be between 1 and 120000");
    }
    let color = match body.as_ref().and_then(|b| b.base.color.as_deref()) {
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
    let identify_brightness = ((*state.power_state.borrow()).effective_brightness()
        * tracked.user_settings.brightness)
        .clamp(0.0, 1.0);
    let identify_color = scale_rgb(identify_rgb, identify_brightness);

    let binding_index = body.as_ref().and_then(|b| b.binding_index).unwrap_or(0);
    let instance = body.as_ref().and_then(|b| b.instance);

    let on_frame = {
        let profiles = state.attachment_profiles.read().await;
        let registry = state.attachment_registry.read().await;
        match build_attachment_identify_frame(
            &profiles,
            &registry,
            AttachmentIdentifyTarget {
                binding_index,
                device_id,
                instance,
                slot_id: &slot_id,
            },
            total_leds,
            identify_color,
        ) {
            Ok(frame) => frame,
            Err(msg) => return ApiError::not_found(msg),
        }
    };

    let backend_id = resolved_backend_id(&tracked.info);
    let (manager, direct_backend, disconnect_after_identify) = match prepare_identify_backend(
        &state,
        device_id,
        &tracked.info,
        tracked.state,
        &backend_id,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };

    if let Err(error) = direct_backend.write_colors(device_id, &on_frame).await {
        let mut manager = manager.lock().await;
        manager.end_direct_control(&backend_id, device_id);
        if disconnect_after_identify {
            let _ = direct_backend.disconnect(device_id).await;
        }
        warn!(
            backend_id = %backend_id,
            device_id = %device_id,
            slot_id = %slot_id,
            error = %error,
            "attachment identify initial write failed"
        );
        return ApiError::internal(format!(
            "Failed to start attachment identify for {}: {error}",
            tracked.info.name
        ));
    }

    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        slot_id = %slot_id,
        binding_index,
        instance,
        backend = %backend_id,
        duration_ms,
        color = ?identify_rgb,
        "Attachment identify flash started"
    );
    tokio::spawn(run_identify_flash(
        manager,
        direct_backend,
        backend_id,
        device_id,
        on_frame,
        Duration::from_millis(duration_ms),
        disconnect_after_identify,
    ));

    ApiResponse::ok(serde_json::json!({
        "device_id": device_id.to_string(),
        "slot_id": slot_id,
        "binding_index": binding_index,
        "instance": instance,
        "identifying": true,
        "duration_ms": duration_ms,
        "color": color,
    }))
}

// ── Shared helpers ───────────────────────────────────────────────────────

pub(super) async fn ensure_default_logical_entry(
    state: &AppState,
    device_info: &DeviceInfo,
) -> String {
    let fallback_layout_id = resolved_layout_device_id(state, device_info).await;

    let mut store = state.logical_devices.write().await;
    let default = crate::logical_devices::ensure_default_logical_device(
        &mut store,
        device_info.id,
        &fallback_layout_id,
        &device_info.name,
        device_info.total_led_count(),
    );
    default.id
}

pub(super) async fn summarize_device_for_response(
    state: &AppState,
    info: &DeviceInfo,
    device_state: &DeviceState,
    brightness: f32,
    layout_device_id: String,
    metadata: Option<&HashMap<String, String>>,
) -> DeviceSummary {
    DeviceSummary {
        id: info.id.to_string(),
        layout_device_id,
        name: info.name.clone(),
        backend: info.output_backend_id().to_owned(),
        origin: info.origin.clone(),
        presentation: crate::network::device_presentation(state.driver_registry.as_ref(), info),
        status: device_state.variant_name().to_lowercase(),
        brightness: brightness_percent(brightness),
        firmware_version: info.firmware_version.clone(),
        network_ip: metadata.and_then(|values| values.get("ip").cloned()),
        network_hostname: metadata.and_then(|values| values.get("hostname").cloned()),
        connection_label: device_connection_label(metadata),
        total_leds: info.total_led_count(),
        auth: pairing::build_device_auth_summary(state, info, device_state, metadata).await,
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

pub(super) async fn refreshed_device_summary(
    state: &AppState,
    device_id: DeviceId,
) -> Result<Option<DeviceSummary>, Response> {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return Ok(None);
    };
    let layout_device_id = ensure_default_logical_entry(state, &tracked.info).await;
    let metadata = state.device_registry.metadata_for_id(&device_id).await;

    Ok(Some(
        summarize_device_for_response(
            state,
            &tracked.info,
            &tracked.state,
            tracked.user_settings.brightness,
            layout_device_id,
            metadata.as_ref(),
        )
        .await,
    ))
}

fn device_connection_label(metadata: Option<&HashMap<String, String>>) -> Option<String> {
    metadata.and_then(|values| {
        values
            .get("serial")
            .cloned()
            .or_else(|| values.get("usb_path").map(|path| format!("USB {path}")))
    })
}

fn percent_to_brightness(percent: u8) -> f32 {
    (f32::from(percent) / 100.0).clamp(0.0, 1.0)
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to 0-100 percent before narrowing to a byte"
)]
fn brightness_percent(brightness: f32) -> u8 {
    (brightness.clamp(0.0, 1.0) * 100.0).round() as u8
}

fn scale_rgb(color: [u8; 3], brightness: f32) -> [u8; 3] {
    let factor = brightness_factor(brightness);
    [
        scale_channel(color[0], factor),
        scale_channel(color[1], factor),
        scale_channel(color[2], factor),
    ]
}

fn brightness_factor(brightness: f32) -> u16 {
    let target = f64::from(brightness.clamp(0.0, 1.0)) * f64::from(u8::MAX);
    (0_u16..=u16::from(u8::MAX))
        .min_by(|left, right| {
            let left_delta = (f64::from(*left) - target).abs();
            let right_delta = (f64::from(*right) - target).abs();
            left_delta
                .partial_cmp(&right_delta)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("brightness factor search range should be non-empty")
}

fn scale_channel(value: u8, factor: u16) -> u8 {
    let scaled = (u16::from(value) * factor) / u16::from(u8::MAX);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

async fn resolved_layout_device_id(state: &AppState, device_info: &DeviceInfo) -> String {
    if let Some(layout_device_id) = {
        let lifecycle = state.lifecycle_manager.lock().await;
        lifecycle
            .layout_device_id_for(device_info.id)
            .map(ToOwned::to_owned)
    } {
        return layout_device_id;
    }

    let fingerprint = state
        .device_registry
        .fingerprint_for_id(&device_info.id)
        .await;
    DeviceLifecycleManager::canonical_layout_device_id(device_info, fingerprint.as_ref())
}

pub(super) async fn device_settings_key(state: &AppState, device_id: DeviceId) -> String {
    state
        .device_registry
        .fingerprint_for_id(&device_id)
        .await
        .map_or_else(
            || device_id.to_string(),
            |fingerprint| fingerprint.to_string(),
        )
}

pub(crate) async fn persist_device_settings_for(
    state: &AppState,
    device_id: DeviceId,
    settings: &DeviceUserSettings,
) -> Result<(), String> {
    let key = device_settings_key(state, device_id).await;
    let mut store = state.device_settings.write().await;
    store.set_device_settings(
        &key,
        crate::device_settings::StoredDeviceSettings {
            name: settings.name.clone(),
            disabled: !settings.enabled,
            brightness: settings.brightness,
        },
    );
    store.save().map_err(|error| error.to_string())
}

pub(crate) async fn sync_device_output_brightness(
    state: &AppState,
    device_id: DeviceId,
    settings: &DeviceUserSettings,
) {
    let mut manager = state.backend_manager.lock().await;
    manager.set_device_output_brightness(device_id, settings.brightness);
}

pub(crate) fn publish_device_settings_changed(
    state: &AppState,
    device_id: DeviceId,
    settings: &DeviceUserSettings,
) {
    let mut changes = HashMap::new();
    changes.insert(
        "name".to_owned(),
        settings
            .name
            .as_ref()
            .map_or(serde_json::Value::Null, |name| {
                serde_json::Value::String(name.clone())
            }),
    );
    changes.insert(
        "enabled".to_owned(),
        serde_json::Value::Bool(settings.enabled),
    );
    changes.insert(
        "brightness".to_owned(),
        serde_json::Value::from(brightness_percent(settings.brightness)),
    );
    state
        .event_bus
        .publish(HypercolorEvent::DeviceStateChanged {
            device_id: device_id.to_string(),
            changes,
        });
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
        DeviceTopologyHint::Display {
            width,
            height,
            circular,
        } => ZoneTopologySummary::Display {
            width: *width,
            height: *height,
            circular: *circular,
        },
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

pub(super) async fn resolve_device_id_or_response(
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

pub(super) fn resolved_backend_id(info: &DeviceInfo) -> String {
    info.output_backend_id().to_owned()
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
    direct_backend: BackendIo,
    backend_id: String,
    device_id: DeviceId,
    on_frame: Vec<[u8; 3]>,
    duration: Duration,
    disconnect_after_identify: bool,
) {
    if on_frame.is_empty() {
        return;
    }

    let off_frame = vec![[0, 0, 0]; on_frame.len()];
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
        let result = direct_backend.write_colors(device_id, frame).await;

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
        let clear_result = direct_backend.write_colors(device_id, &off_frame).await;
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

    if disconnect_after_identify {
        if let Err(error) = direct_backend.disconnect(device_id).await {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                error = %error,
                "identify temporary disconnect failed"
            );
        } else {
            debug!(
                backend_id = %backend_id,
                device_id = %device_id,
                "identify released temporary backend connection"
            );
        }
    }

    if identify_failed {
        return;
    }

    tracing::info!(
        device_id = %device_id,
        backend = %backend_id,
        "Identify flash completed"
    );
}

async fn prepare_identify_backend(
    state: &Arc<AppState>,
    device_id: DeviceId,
    info: &DeviceInfo,
    device_state: DeviceState,
    backend_id: &str,
) -> Result<(Arc<tokio::sync::Mutex<BackendManager>>, BackendIo, bool), Response> {
    let supports_temporary_identify = matches!(info.connection_type, ConnectionType::Network)
        && device_state != DeviceState::Disabled;
    if !device_state.is_renderable() && !supports_temporary_identify {
        return Err(ApiError::conflict(format!(
            "Device is not connected: {} (state={device_state})",
            info.name
        )));
    }

    let manager = Arc::clone(&state.backend_manager);
    let direct_backend = {
        let manager = manager.lock().await;
        let Some(direct_backend) = manager.backend_io(backend_id) else {
            return Err(ApiError::internal(format!(
                "Failed to start identify flash for {}: backend '{backend_id}' is not registered",
                info.name
            )));
        };
        direct_backend
    };

    let disconnect_after_identify = if device_state.is_renderable() {
        false
    } else if supports_temporary_identify {
        if let Err(error) = direct_backend.connect_with_refresh(device_id).await {
            return Err(ApiError::conflict(format!(
                "Device is not connected and temporary identify failed for {}: {error}",
                info.name
            )));
        }
        if let Ok(Some(refreshed_info)) = direct_backend.connected_device_info(device_id).await {
            let _ = state
                .device_registry
                .update_info(&device_id, refreshed_info)
                .await;
        }
        true
    } else {
        return Err(ApiError::conflict(format!(
            "Device is not connected: {} (state={device_state})",
            info.name
        )));
    };

    {
        let mut manager = manager.lock().await;
        manager.begin_direct_control(backend_id, device_id);
    }

    Ok((manager, direct_backend, disconnect_after_identify))
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

// ── Identify helpers ─────────────────────────────────────────────────────

/// Resolve a zone specifier (`"zone_0"`, `"0"`, or zone name) to an index.
#[allow(
    clippy::result_large_err,
    reason = "API helpers return ready-made HTTP responses for ergonomic handler control flow"
)]
fn resolve_zone_index(info: &DeviceInfo, zone_id: &str) -> Result<usize, Response> {
    // Try "zone_N" format
    if let Some(stripped) = zone_id.strip_prefix("zone_")
        && let Ok(index) = stripped.parse::<usize>()
        && index < info.zones.len()
    {
        return Ok(index);
    }

    // Try bare numeric index
    if let Ok(index) = zone_id.parse::<usize>()
        && index < info.zones.len()
    {
        return Ok(index);
    }

    // Try name match (case-insensitive)
    let needle = zone_id.to_ascii_lowercase();
    for (i, zone) in info.zones.iter().enumerate() {
        if zone.name.to_ascii_lowercase() == needle {
            return Ok(i);
        }
    }

    Err(ApiError::not_found(format!(
        "Zone not found: {zone_id} (device has {} zone(s): {})",
        info.zones.len(),
        info.zones
            .iter()
            .enumerate()
            .map(|(i, z)| format!("zone_{i}={}", z.name))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Build a full-device LED frame with only one zone lit.
fn build_zone_identify_frame(info: &DeviceInfo, zone_index: usize, color: [u8; 3]) -> Vec<[u8; 3]> {
    let total_leds = usize::try_from(info.total_led_count()).unwrap_or_default();
    let mut frame = vec![[0_u8; 3]; total_leds];

    let mut offset = 0_usize;
    for (i, zone) in info.zones.iter().enumerate() {
        let count = usize::try_from(zone.led_count).unwrap_or_default();
        if i == zone_index {
            for led in &mut frame[offset..offset + count] {
                *led = color;
            }
        }
        offset += count;
    }

    frame
}

/// Build a full-device LED frame with only a single attachment component lit.
#[derive(Clone, Copy)]
struct AttachmentIdentifyTarget<'a> {
    device_id: DeviceId,
    slot_id: &'a str,
    binding_index: usize,
    instance: Option<u32>,
}

fn build_attachment_identify_frame(
    profiles: &crate::attachment_profiles::AttachmentProfileStore,
    registry: &hypercolor_core::attachment::AttachmentRegistry,
    target: AttachmentIdentifyTarget<'_>,
    total_leds: usize,
    color: [u8; 3],
) -> Result<Vec<[u8; 3]>, String> {
    let AttachmentIdentifyTarget {
        device_id,
        slot_id,
        binding_index,
        instance,
    } = target;
    let device_key = device_id.to_string();
    let profile = profiles
        .get(&device_key)
        .ok_or_else(|| format!("No attachment profile for device {device_id}"))?;

    let slot = profile
        .slots
        .iter()
        .find(|s| s.id == slot_id)
        .ok_or_else(|| {
            format!(
                "Slot '{slot_id}' not found (available: {})",
                profile
                    .slots
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    let slot_bindings: Vec<(usize, &AttachmentBinding)> = profile
        .bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.slot_id == slot_id && binding.enabled)
        .collect();

    if slot_bindings.is_empty() {
        return Err(format!("No enabled bindings in slot '{slot_id}'"));
    }
    let (start, led_count) = if let Some(instance_index) = instance {
        resolve_attachment_instance_range(
            registry,
            slot_bindings.as_slice(),
            slot,
            binding_index,
            instance_index,
        )?
    } else {
        resolve_attachment_component_range(registry, slot_bindings.as_slice(), slot, binding_index)?
    };
    let end = (start + led_count).min(total_leds);

    let mut frame = vec![[0_u8; 3]; total_leds];
    for led in &mut frame[start..end] {
        *led = color;
    }

    Ok(frame)
}

fn resolve_attachment_instance_range(
    registry: &hypercolor_core::attachment::AttachmentRegistry,
    slot_bindings: &[(usize, &AttachmentBinding)],
    slot: &AttachmentSlot,
    binding_index: usize,
    instance_index: u32,
) -> Result<(usize, usize), String> {
    let available = slot_bindings
        .iter()
        .map(|(index, _)| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let (_, binding) = slot_bindings
        .iter()
        .find(|(index, _)| *index == binding_index)
        .ok_or_else(|| {
            format!(
                "Binding index {binding_index} not found in slot '{slot_id}' (available: {available})",
                slot_id = slot.id
            )
        })?;

    let template = registry
        .get(&binding.template_id)
        .ok_or_else(|| format!("Attachment template '{}' not found", binding.template_id))?;
    let total_instances = binding.instances.max(1);
    if instance_index >= total_instances {
        return Err(format!(
            "Instance {instance_index} out of range for binding {binding_index} in slot '{slot_id}' (instances: {total_instances})",
            slot_id = slot.id
        ));
    }

    let slot_start = usize::try_from(slot.led_start).unwrap_or_default();
    let binding_offset = usize::try_from(binding.led_offset).unwrap_or_default();
    let instance_stride = usize::try_from(template.led_count()).unwrap_or_default();
    let instance_offset = usize::try_from(instance_index).unwrap_or_default();

    Ok((
        slot_start + binding_offset + instance_offset.saturating_mul(instance_stride),
        instance_stride,
    ))
}

fn resolve_attachment_component_range(
    registry: &hypercolor_core::attachment::AttachmentRegistry,
    slot_bindings: &[(usize, &AttachmentBinding)],
    slot: &AttachmentSlot,
    component_index: usize,
) -> Result<(usize, usize), String> {
    let mut sorted = slot_bindings
        .iter()
        .map(|(binding_index, binding)| {
            let template = registry.get(&binding.template_id).ok_or_else(|| {
                format!("Attachment template '{}' not found", binding.template_id)
            })?;
            Ok((*binding_index, *binding, template))
        })
        .collect::<Result<Vec<_>, String>>()?;
    sorted.sort_by(|left, right| {
        left.1
            .led_offset
            .cmp(&right.1.led_offset)
            .then_with(|| left.2.name.cmp(&right.2.name))
            .then_with(|| left.2.id.cmp(&right.2.id))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut remaining = component_index;
    for (_, binding, template) in sorted {
        let instances = usize::try_from(binding.instances.max(1)).unwrap_or(usize::MAX);
        let instance_stride = usize::try_from(template.led_count()).unwrap_or_default();
        if remaining < instances {
            let slot_start = usize::try_from(slot.led_start).unwrap_or_default();
            let binding_offset = usize::try_from(binding.led_offset).unwrap_or_default();
            return Ok((
                slot_start + binding_offset + remaining.saturating_mul(instance_stride),
                instance_stride,
            ));
        }
        remaining = remaining.saturating_sub(instances);
    }

    let available = slot_bindings
        .iter()
        .map(|(_, binding)| usize::try_from(binding.instances.max(1)).unwrap_or(usize::MAX))
        .fold(0_usize, usize::saturating_add);
    Err(format!(
        "Component index {component_index} out of range for slot '{slot_id}' (available components: {available})",
        slot_id = slot.id
    ))
}
