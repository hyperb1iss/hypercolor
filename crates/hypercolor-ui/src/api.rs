//! REST API client — thin wrappers around the daemon's HTTP endpoints.
#![allow(dead_code)] // API surface is pre-built for upcoming features

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use hypercolor_types::effect::{ControlDefinition, ControlValue, PresetTemplate};

// ── API Response Types ──────────────────────────────────────────────────────

/// Mirrors the daemon's envelope: `{ "data": T, "meta": { ... } }`.
#[derive(Debug, Deserialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
}

/// Effect list item from `GET /api/v1/effects`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    #[serde(default)]
    pub audio_reactive: bool,
}

/// Paginated effect list response.
#[derive(Debug, Deserialize)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
}

/// Active effect response from `GET /api/v1/effects/active`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ActiveEffectResponse {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub control_values: HashMap<String, ControlValue>,
    #[serde(default)]
    pub active_preset_id: Option<String>,
}

/// Detailed effect payload from `GET /api/v1/effects/:id`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EffectDetailResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    pub audio_reactive: bool,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub presets: Vec<PresetTemplate>,
    #[serde(default)]
    pub active_control_values: Option<HashMap<String, ControlValue>>,
}

/// System status from `GET /api/v1/status`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SystemStatus {
    pub running: bool,
    pub version: String,
    #[serde(default)]
    pub config_path: String,
    pub uptime_seconds: u64,
    pub device_count: usize,
    pub effect_count: usize,
    pub active_effect: Option<String>,
    pub global_brightness: u8,
}

// ── Device Types ────────────────────────────────────────────────────────────

/// Device zone summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZoneSummary {
    pub id: String,
    pub name: String,
    pub led_count: usize,
    pub topology: String,
    #[serde(default)]
    pub topology_hint: Option<ZoneTopologySummary>,
}

/// Structured topology hint from `GET /api/v1/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Device summary from `GET /api/v1/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceSummary {
    pub id: String,
    pub layout_device_id: String,
    pub name: String,
    pub backend: String,
    pub status: String,
    pub brightness: u8,
    #[serde(default)]
    pub firmware_version: Option<String>,
    #[serde(default)]
    pub network_ip: Option<String>,
    #[serde(default)]
    pub network_hostname: Option<String>,
    #[serde(default)]
    pub connection_label: Option<String>,
    pub total_leds: usize,
    #[serde(default)]
    pub zones: Vec<ZoneSummary>,
}

/// Paginated device list response.
#[derive(Debug, Deserialize)]
pub struct DeviceListResponse {
    pub items: Vec<DeviceSummary>,
}

/// Attachment binding summary from `GET /api/v1/devices/:id/attachments`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentBindingSummary {
    pub slot_id: String,
    pub template_id: String,
    pub template_name: String,
    #[serde(default)]
    pub name: Option<String>,
    pub enabled: bool,
    pub instances: u32,
    pub led_offset: u32,
    pub effective_led_count: u32,
}

/// Device attachment profile summary from `GET /api/v1/devices/:id/attachments`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceAttachmentsResponse {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub slots: Vec<hypercolor_types::attachment::AttachmentSlot>,
    #[serde(default)]
    pub bindings: Vec<AttachmentBindingSummary>,
    #[serde(default)]
    pub suggested_zones: Vec<hypercolor_types::attachment::AttachmentSuggestedZone>,
}

// ── Logical Device Types ────────────────────────────────────────────────────

/// Logical device summary from device segmentation APIs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Paginated logical device list response.
#[derive(Debug, Deserialize)]
pub struct LogicalDeviceListResponse {
    pub items: Vec<LogicalDeviceSummary>,
}

/// Request body for creating a logical device segment.
#[derive(Debug, Serialize)]
pub struct CreateLogicalDeviceRequest {
    pub name: String,
    pub led_start: u32,
    pub led_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Request body for updating a device.
#[derive(Debug, Serialize)]
pub struct UpdateDeviceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
}

/// Global brightness payload from `/api/v1/settings/brightness`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrightnessSettingsResponse {
    pub brightness: u8,
}

// ── Layout Types ────────────────────────────────────────────────────────────

/// Layout summary from `GET /api/v1/layouts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
    #[serde(default)]
    pub group_count: usize,
    #[serde(default)]
    pub is_active: bool,
}

/// Paginated layout list response.
#[derive(Debug, Deserialize)]
pub struct LayoutListResponse {
    pub items: Vec<LayoutSummary>,
}

/// Request body for creating a layout.
#[derive(Debug, Serialize)]
pub struct CreateLayoutRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_height: Option<u32>,
}

/// Request body for updating a layout.
#[derive(Debug, Serialize)]
pub struct UpdateLayoutApiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<Vec<hypercolor_types::spatial::DeviceZone>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<hypercolor_types::spatial::ZoneGroup>>,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all registered effects.
pub async fn fetch_effects() -> Result<Vec<EffectSummary>, String> {
    let resp = Request::get("/api/v1/effects")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<EffectListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Fetch the currently active effect, if any.
pub async fn fetch_active_effect() -> Result<Option<ActiveEffectResponse>, String> {
    let resp = Request::get("/api/v1/effects/active")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() == 404 {
        return Ok(None);
    }
    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<ActiveEffectResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(Some(envelope.data))
}

/// Fetch detailed metadata for one effect.
pub async fn fetch_effect_detail(id: &str) -> Result<EffectDetailResponse, String> {
    let url = format!("/api/v1/effects/{id}");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<EffectDetailResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Fetch the bundled (effect-defined) presets for an effect.
pub async fn fetch_bundled_presets(id: &str) -> Result<Vec<PresetTemplate>, String> {
    let detail = fetch_effect_detail(id).await?;
    Ok(detail.presets)
}

/// Apply an effect by ID or name.
pub async fn apply_effect(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/effects/{id}/apply");
    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Stop the currently active effect.
pub async fn stop_effect() -> Result<(), String> {
    let resp = Request::post("/api/v1/effects/stop")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Fetch system status.
pub async fn fetch_status() -> Result<SystemStatus, String> {
    let resp = Request::get("/api/v1/status")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<SystemStatus> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Fetch the current global brightness.
pub async fn fetch_global_brightness() -> Result<u8, String> {
    let resp = Request::get("/api/v1/settings/brightness")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<BrightnessSettingsResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.brightness)
}

// ── Device Fetch Functions ──────────────────────────────────────────────────

/// Fetch all tracked devices.
pub async fn fetch_devices() -> Result<Vec<DeviceSummary>, String> {
    let resp = Request::get("/api/v1/devices")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<DeviceListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Trigger device discovery scan.
pub async fn discover_devices() -> Result<(), String> {
    let resp = Request::post("/api/v1/devices/discover")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 && resp.status() != 202 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Layout Fetch Functions ─────────────────────────────────────────────────

/// Fetch all spatial layouts.
pub async fn fetch_layouts() -> Result<Vec<LayoutSummary>, String> {
    let resp = Request::get("/api/v1/layouts")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LayoutListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Fetch a single device by ID.
pub async fn fetch_device(id: &str) -> Result<DeviceSummary, String> {
    let url = format!("/api/v1/devices/{id}");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<DeviceSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update a device (name, enabled, brightness).
pub async fn update_device(id: &str, req: &UpdateDeviceRequest) -> Result<DeviceSummary, String> {
    let url = format!("/api/v1/devices/{id}");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<DeviceSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update the global output brightness.
pub async fn set_global_brightness(brightness: u8) -> Result<u8, String> {
    let body = serde_json::json!({ "brightness": brightness });
    let resp = Request::put("/api/v1/settings/brightness")
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<BrightnessSettingsResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.brightness)
}

/// Identify a device by flashing its LEDs.
pub async fn identify_device(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/devices/{id}/identify");
    let body = serde_json::json!({
        "duration_ms": 2000,
        "color": "FF06B5",
    });

    let resp = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Logical Device Fetch Functions ────────────────────────────────────────

/// Fetch logical devices for a physical device.
pub async fn fetch_logical_devices(device_id: &str) -> Result<Vec<LogicalDeviceSummary>, String> {
    let url = format!("/api/v1/devices/{device_id}/logical-devices");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LogicalDeviceListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Fetch attachment bindings and import-ready zones for a physical device.
pub async fn fetch_device_attachments(
    device_id: &str,
) -> Result<DeviceAttachmentsResponse, String> {
    let url = format!("/api/v1/devices/{device_id}/attachments");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<DeviceAttachmentsResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

// ── Attachment Template Types ──────────────────────────────────────────────

/// Template summary from `GET /api/v1/attachments/templates`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: hypercolor_types::attachment::AttachmentCategory,
    pub led_count: u32,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Paginated template list response.
#[derive(Debug, Deserialize)]
pub struct TemplateListResponse {
    pub items: Vec<TemplateSummary>,
}

/// Request body for `PUT /api/v1/devices/:id/attachments`.
#[derive(Debug, Serialize)]
pub struct UpdateAttachmentsRequest {
    pub bindings: Vec<AttachmentBindingRequest>,
}

/// A single binding entry sent to the update endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct AttachmentBindingRequest {
    pub slot_id: String,
    pub template_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    #[serde(default = "default_instances")]
    pub instances: u32,
    #[serde(default)]
    pub led_offset: u32,
}

fn bool_true() -> bool {
    true
}

fn default_instances() -> u32 {
    1
}

/// Fetch attachment templates, optionally filtered by category.
pub async fn fetch_attachment_templates(
    category: Option<&str>,
) -> Result<Vec<TemplateSummary>, String> {
    let mut url = "/api/v1/attachments/templates?limit=200".to_string();
    if let Some(cat) = category {
        url.push_str(&format!("&category={cat}"));
    }

    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<TemplateListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Update attachment bindings for a device.
pub async fn update_device_attachments(
    device_id: &str,
    req: &UpdateAttachmentsRequest,
) -> Result<DeviceAttachmentsResponse, String> {
    let url = format!("/api/v1/devices/{device_id}/attachments");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<DeviceAttachmentsResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Delete all attachment bindings for a device.
pub async fn delete_device_attachments(device_id: &str) -> Result<(), String> {
    let url = format!("/api/v1/devices/{device_id}/attachments");

    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    Ok(())
}

/// Create a logical device segment on a physical device.
pub async fn create_logical_device(
    device_id: &str,
    req: &CreateLogicalDeviceRequest,
) -> Result<LogicalDeviceSummary, String> {
    let url = format!("/api/v1/devices/{device_id}/logical-devices");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 && resp.status() != 201 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LogicalDeviceSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Delete a logical device segment.
pub async fn delete_logical_device(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/logical-devices/{id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Layout Detail + Mutation Functions ────────────────────────────────────

/// Fetch a single layout with full zone data.
pub async fn fetch_layout(id: &str) -> Result<hypercolor_types::spatial::SpatialLayout, String> {
    let url = format!("/api/v1/layouts/{id}");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<hypercolor_types::spatial::SpatialLayout> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Fetch the currently active layout.
pub async fn fetch_active_layout() -> Result<hypercolor_types::spatial::SpatialLayout, String> {
    let resp = Request::get("/api/v1/layouts/active")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<hypercolor_types::spatial::SpatialLayout> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Create a new layout.
pub async fn create_layout(req: &CreateLayoutRequest) -> Result<LayoutSummary, String> {
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post("/api/v1/layouts")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 && resp.status() != 201 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LayoutSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update a layout (metadata + optionally zones).
pub async fn update_layout(
    id: &str,
    req: &UpdateLayoutApiRequest,
) -> Result<LayoutSummary, String> {
    let url = format!("/api/v1/layouts/{id}");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LayoutSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Apply a layout to the spatial engine.
pub async fn apply_layout(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/layouts/{id}/apply");
    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Push a layout to the spatial engine for live preview (no persistence).
pub async fn preview_layout(
    layout: &hypercolor_types::spatial::SpatialLayout,
) -> Result<(), String> {
    let body = serde_json::to_string(layout).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put("/api/v1/layouts/active/preview")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Delete a layout.
pub async fn delete_layout(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/layouts/{id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        let msg = resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or_else(|| format!("HTTP {}", resp.status()));
        return Err(msg);
    }
    Ok(())
}

// ── Preset Types ────────────────────────────────────────────────────────

/// Preset summary from `GET /api/v1/library/presets`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresetSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: String,
    #[serde(default)]
    pub controls: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

/// Paginated preset list response.
#[derive(Debug, Deserialize)]
pub struct PresetListResponse {
    pub items: Vec<PresetSummary>,
}

/// Request body for creating a preset.
#[derive(Debug, Serialize)]
pub struct CreatePresetRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub effect: String,
    pub controls: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

// ── Preset Fetch Functions ──────────────────────────────────────────────

/// Fetch all saved presets.
pub async fn fetch_presets() -> Result<Vec<PresetSummary>, String> {
    let resp = Request::get("/api/v1/library/presets")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<PresetListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Create a new preset from current control values.
pub async fn create_preset(req: &CreatePresetRequest) -> Result<PresetSummary, String> {
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post("/api/v1/library/presets")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 201 && resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<PresetSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Apply a saved preset by ID.
pub async fn apply_preset(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/library/presets/{id}/apply");
    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Update an existing preset (name, controls, etc.).
pub async fn update_preset(id: &str, req: &CreatePresetRequest) -> Result<PresetSummary, String> {
    let url = format!("/api/v1/library/presets/{id}");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<PresetSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Delete a preset by ID.
pub async fn delete_preset(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/library/presets/{id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Favorite Types ──────────────────────────────────────────────────────

/// Favorite entry from `GET /api/v1/library/favorites`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FavoriteSummary {
    pub effect_id: String,
    pub effect_name: String,
    pub added_at_ms: u64,
}

/// Paginated favorites list response.
#[derive(Debug, Deserialize)]
pub struct FavoriteListResponse {
    pub items: Vec<FavoriteSummary>,
}

// ── Favorite Fetch Functions ───────────────────────────────────────────

/// Fetch all favorited effect IDs.
pub async fn fetch_favorites() -> Result<Vec<FavoriteSummary>, String> {
    let resp = Request::get("/api/v1/library/favorites")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<FavoriteListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Add an effect to favorites.
pub async fn add_favorite(effect_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "effect": effect_id });

    let resp = Request::post("/api/v1/library/favorites")
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Remove an effect from favorites.
pub async fn remove_favorite(effect_id: &str) -> Result<(), String> {
    let url = format!("/api/v1/library/favorites/{effect_id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Reset all controls on the active effect to their defaults.
pub async fn reset_controls() -> Result<(), String> {
    let resp = Request::post("/api/v1/effects/current/reset")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Config API ─────────────────────────────────────────────────────────────

/// Fetch the full daemon config.
pub async fn fetch_config() -> Result<hypercolor_types::config::HypercolorConfig, String> {
    let resp = Request::get("/api/v1/config")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<hypercolor_types::config::HypercolorConfig> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Set a single config key. Value is JSON-stringified per daemon contract.
pub async fn set_config_value(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let live = key == "audio" || key.starts_with("audio.");
    let body = serde_json::json!({
        "key": key,
        "value": serde_json::to_string(value).unwrap_or_default(),
        "live": live,
    });

    let resp = Request::post("/api/v1/config/set")
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Reset a config key or section to defaults.
pub async fn reset_config_key(key: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "key": key,
        "live": key == "audio" || key.starts_with("audio."),
    });

    let resp = Request::post("/api/v1/config/reset")
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Audio device info from `GET /api/v1/audio/devices`.
#[derive(Debug, Clone, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AudioDevicesData {
    pub devices: Vec<AudioDeviceInfo>,
    pub current: String,
}

/// Enumerate available audio devices.
pub async fn fetch_audio_devices() -> Result<AudioDevicesData, String> {
    let resp = Request::get("/api/v1/audio/devices")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Ok(AudioDevicesData {
            devices: vec![AudioDeviceInfo {
                id: "default".to_string(),
                name: "Default".to_string(),
                description: "System default".to_string(),
            }],
            current: "default".to_string(),
        });
    }

    let envelope: ApiEnvelope<AudioDevicesData> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update effect control parameters.
pub async fn update_controls(controls: &serde_json::Value) -> Result<(), String> {
    let url = "/api/v1/effects/current/controls";
    let body = serde_json::json!({ "controls": controls });

    let resp = Request::patch(url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}
