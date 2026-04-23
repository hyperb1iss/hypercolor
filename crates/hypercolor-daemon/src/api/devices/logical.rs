//! Logical device group management — `/api/v1/logical-devices`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_core::device::{BackendManager, SegmentRange};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::discovery;
use crate::logical_devices::{self, LogicalDevice, LogicalDeviceKind};

use super::{
    Pagination, ensure_default_logical_entry, resolve_device_id_or_response, resolved_backend_id,
};

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

#[derive(Debug, Clone)]
pub(super) struct PhysicalSnapshot {
    pub(super) name: String,
    pub(super) backend: String,
    pub(super) status: String,
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
        ensure_default_logical_entry(&state, &tracked.info).await;
    }

    let physical_filter = match query.physical_device {
        Some(raw) => match resolve_device_id_or_response(&state, raw.trim()).await {
            Ok(id) => Some(id),
            Err(response) => return response,
        },
        None => None,
    };

    let mut physical_entries = Vec::with_capacity(physical_devices.len());
    for tracked in &physical_devices {
        physical_entries.push((
            tracked.info.id,
            PhysicalSnapshot {
                name: tracked.info.name.clone(),
                backend: resolved_backend_id(&state, tracked.info.id, &tracked.info.family).await,
                status: tracked.state.variant_name().to_ascii_lowercase(),
            },
        ));
    }
    let physical_index = build_physical_index(physical_entries);
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

    ensure_default_logical_entry(&state, &tracked.info).await;

    let logical_entries = {
        let store = state.logical_devices.read().await;
        logical_devices::list_for_physical(&store, physical_id)
    };
    let physical_index = build_physical_index(std::iter::once((
        tracked.info.id,
        PhysicalSnapshot {
            name: tracked.info.name.clone(),
            backend: resolved_backend_id(&state, tracked.info.id, &tracked.info.family).await,
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

    let physical_layout_id = ensure_default_logical_entry(&state, &tracked.info).await;
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

    let runtime = crate::api::discovery_runtime(&state);
    let connected_only = HashSet::from([physical_id]);
    discovery::sync_active_layout_connectivity(&runtime, Some(&connected_only)).await;
    sync_live_logical_mappings_for_device(&state, physical_id).await;

    let physical_index = build_physical_index(std::iter::once((
        tracked.info.id,
        PhysicalSnapshot {
            name: tracked.info.name.clone(),
            backend: resolved_backend_id(&state, tracked.info.id, &tracked.info.family).await,
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
    let mut physical_entries = Vec::with_capacity(physical_devices.len());
    for tracked in &physical_devices {
        physical_entries.push((
            tracked.info.id,
            PhysicalSnapshot {
                name: tracked.info.name.clone(),
                backend: resolved_backend_id(&state, tracked.info.id, &tracked.info.family).await,
                status: tracked.state.variant_name().to_ascii_lowercase(),
            },
        ));
    }
    let physical_index = build_physical_index(physical_entries);
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

    let runtime = crate::api::discovery_runtime(&state);
    let connected_only = HashSet::from([existing.physical_device_id]);
    discovery::sync_active_layout_connectivity(&runtime, Some(&connected_only)).await;
    sync_live_logical_mappings_for_device(&state, existing.physical_device_id).await;

    let physical_index = build_physical_index(std::iter::once((
        tracked.info.id,
        PhysicalSnapshot {
            name: tracked.info.name.clone(),
            backend: resolved_backend_id(&state, tracked.info.id, &tracked.info.family).await,
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

    let runtime = crate::api::discovery_runtime(&state);
    let connected_only = HashSet::from([existing.physical_device_id]);
    discovery::sync_active_layout_connectivity(&runtime, Some(&connected_only)).await;
    sync_live_logical_mappings_for_device(&state, existing.physical_device_id).await;

    ApiResponse::ok(serde_json::json!({
        "id": id,
        "deleted": true,
    }))
}

pub(super) fn build_physical_index<I>(entries: I) -> HashMap<DeviceId, PhysicalSnapshot>
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

pub(super) async fn sync_live_logical_mappings_for_device(state: &AppState, physical_id: DeviceId) {
    let Some(tracked) = state.device_registry.get(&physical_id).await else {
        return;
    };

    let fallback_layout_id = ensure_default_logical_entry(state, &tracked.info).await;

    let backend_id = resolved_backend_id(state, physical_id, &tracked.info.family).await;
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
        map_device_with_zone_segments(
            &mut manager,
            fallback_layout_id.clone(),
            backend_id.clone(),
            physical_id,
            Some(SegmentRange::new(
                0,
                usize::try_from(tracked.info.total_led_count()).unwrap_or_default(),
            )),
            &tracked.info,
        );
        return;
    }

    for entry in logical_entries {
        let start = usize::try_from(entry.led_start).unwrap_or_default();
        let length = usize::try_from(entry.led_count).unwrap_or_default();
        map_device_with_zone_segments(
            &mut manager,
            entry.id,
            backend_id.clone(),
            physical_id,
            Some(SegmentRange::new(start, length)),
            &tracked.info,
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

fn map_device_with_zone_segments(
    manager: &mut BackendManager,
    layout_device_id: impl Into<String>,
    backend_id: impl Into<String>,
    physical_id: DeviceId,
    segment: Option<SegmentRange>,
    device_info: &DeviceInfo,
) {
    let layout_device_id = layout_device_id.into();
    manager.map_device_with_segment(layout_device_id.clone(), backend_id, physical_id, segment);
    let _ = manager.set_device_zone_segments(&layout_device_id, device_info);
}
