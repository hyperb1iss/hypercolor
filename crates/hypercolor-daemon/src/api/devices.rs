//! Device endpoints — `/api/v1/devices/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

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

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/devices` — List all tracked devices.
pub async fn list_devices(State(state): State<Arc<AppState>>) -> Response {
    let devices = state.device_registry.list().await;

    let items: Vec<DeviceSummary> = devices
        .iter()
        .map(|tracked| {
            let info = &tracked.info;
            DeviceSummary {
                id: info.id.to_string(),
                name: info.name.clone(),
                backend: format!("{}", info.family),
                status: tracked.state.variant_name().to_lowercase(),
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
        })
        .collect();

    let total = items.len();
    ApiResponse::ok(DeviceListResponse {
        items,
        pagination: Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/devices/:id` — Get a single device.
pub async fn get_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(device_id) = id.parse() else {
        return ApiError::bad_request(format!("Invalid device ID: {id}"));
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let info = &tracked.info;
    ApiResponse::ok(DeviceSummary {
        id: info.id.to_string(),
        name: info.name.clone(),
        backend: format!("{}", info.family),
        status: tracked.state.variant_name().to_lowercase(),
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
    })
}

/// `PUT /api/v1/devices/:id` — Update a device's metadata.
pub async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDeviceRequest>,
) -> Response {
    let Ok(device_id) = id.parse() else {
        return ApiError::bad_request(format!("Invalid device ID: {id}"));
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    // In a full implementation, the name/enabled fields would be persisted.
    // For now, return the device with any name override applied.
    let info = &tracked.info;
    let name = body.name.unwrap_or_else(|| info.name.clone());
    let _ = body.enabled; // Acknowledged but not persisted yet.

    ApiResponse::ok(DeviceSummary {
        id: info.id.to_string(),
        name,
        backend: format!("{}", info.family),
        status: tracked.state.variant_name().to_lowercase(),
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
    })
}

/// `DELETE /api/v1/devices/:id` — Remove a device from tracking.
pub async fn delete_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(device_id) = id.parse() else {
        return ApiError::bad_request(format!("Invalid device ID: {id}"));
    };

    if state.device_registry.remove(&device_id).await.is_none() {
        return ApiError::not_found(format!("Device not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": id,
        "removed": true,
    }))
}

/// `POST /api/v1/devices/discover` — Trigger device discovery.
pub async fn discover_devices(
    State(state): State<Arc<AppState>>,
    body: Option<Json<DiscoverRequest>>,
) -> Response {
    let backends = body
        .as_ref()
        .and_then(|b| b.backends.clone())
        .unwrap_or_else(|| vec!["wled".to_owned(), "openrgb".to_owned()]);
    let timeout_ms = body.as_ref().and_then(|b| b.timeout_ms).unwrap_or(10_000);

    let scan_id = format!("scan_{}", uuid::Uuid::now_v7());

    // Publish discovery started event.
    state.event_bus.publish(
        hypercolor_core::types::event::HypercolorEvent::DeviceDiscoveryStarted {
            backends: backends.clone(),
        },
    );

    ApiResponse::accepted(serde_json::json!({
        "scan_id": scan_id,
        "status": "scanning",
        "backends": backends,
        "timeout_ms": timeout_ms,
    }))
}

/// `POST /api/v1/devices/:id/identify` — Flash identification pattern.
pub async fn identify_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<IdentifyRequest>>,
) -> Response {
    let Ok(device_id) = id.parse() else {
        return ApiError::bad_request(format!("Invalid device ID: {id}"));
    };

    let Some(_tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let duration_ms = body.as_ref().and_then(|b| b.duration_ms).unwrap_or(3000);

    ApiResponse::ok(serde_json::json!({
        "device_id": id,
        "identifying": true,
        "duration_ms": duration_ms,
    }))
}
