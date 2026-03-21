//! Device-related API types and fetch functions.

use std::collections::HashMap;

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use super::ApiEnvelope;

// ── Types ───────────────────────────────────────────────────────────────────

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

// ── Pairing / Auth Types ────────────────────────────────────────────────────

/// Device authentication state from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAuthState {
    /// Device does not require credentials.
    Open,
    /// Device requires credentials and none are stored.
    Required,
    /// Credentials are stored and can be used for connect attempts.
    Configured,
    /// Credentials are stored but known to be invalid or stale.
    Error,
}

/// The kind of pairing flow to present.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingFlowKind {
    /// User must perform a physical action, then press the action button.
    PhysicalAction,
    /// UI must render input fields and submit entered credentials.
    CredentialsForm,
}

/// Describes a single input field for credential-based pairing flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingFieldDescriptor {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub optional: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
}

/// Backend-provided descriptor that tells the UI exactly how to render a pairing flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingDescriptor {
    pub kind: PairingFlowKind,
    pub title: String,
    pub instructions: Vec<String>,
    pub action_label: String,
    #[serde(default)]
    pub fields: Vec<PairingFieldDescriptor>,
}

/// Auth/pairing summary attached to each device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceAuthSummary {
    pub state: DeviceAuthState,
    pub can_pair: bool,
    #[serde(default)]
    pub descriptor: Option<PairingDescriptor>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Generic pair request sent to `POST /api/v1/devices/:id/pair`.
#[derive(Debug, Serialize)]
pub struct PairDeviceRequest {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub values: HashMap<String, String>,
    pub activate_after_pair: bool,
}

/// Status returned by the generic pair endpoint.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairDeviceStatus {
    Paired,
    ActionRequired,
    AlreadyPaired,
    InvalidInput,
}

/// Response from `POST /api/v1/devices/:id/pair`.
#[derive(Debug, Clone, Deserialize)]
pub struct PairDeviceResponse {
    pub status: PairDeviceStatus,
    pub message: String,
    pub activated: bool,
    #[serde(default)]
    pub device: Option<DeviceSummary>,
}

/// Response from `DELETE /api/v1/devices/:id/pair`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeletePairingResponse {
    pub status: String,
    pub message: String,
    pub disconnected: bool,
    #[serde(default)]
    pub device: Option<DeviceSummary>,
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
    pub auth: Option<DeviceAuthSummary>,
    #[serde(default)]
    pub zones: Vec<ZoneSummary>,
}

/// Paginated device list response.
#[derive(Debug, Deserialize)]
pub struct DeviceListResponse {
    pub items: Vec<DeviceSummary>,
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

// ── Attachment Types ────────────────────────────────────────────────────────

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

/// Template summary from `GET /api/v1/attachments/templates`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: hypercolor_types::attachment::AttachmentCategory,
    #[serde(default)]
    pub origin: Option<hypercolor_types::attachment::AttachmentOrigin>,
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

// ── Fetch Functions ─────────────────────────────────────────────────────────

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

/// Identify a single zone by flashing only its LEDs.
pub async fn identify_zone(device_id: &str, zone_id: &str) -> Result<(), String> {
    let url = format!("/api/v1/devices/{device_id}/zones/{zone_id}/identify");
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

/// Identify a specific attachment component by flashing its LED range.
pub async fn identify_attachment(
    device_id: &str,
    slot_id: &str,
    binding_index: Option<usize>,
) -> Result<(), String> {
    let url = format!("/api/v1/devices/{device_id}/attachments/{slot_id}/identify");
    let mut body = serde_json::json!({
        "duration_ms": 2000,
        "color": "80FFEA",
    });
    if let Some(idx) = binding_index {
        body["binding_index"] = serde_json::json!(idx);
    }

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

/// Create a user-authored attachment template (custom strip, matrix, etc.).
pub async fn create_attachment_template(
    template: &hypercolor_types::attachment::AttachmentTemplate,
) -> Result<TemplateSummary, String> {
    let body = serde_json::to_string(template).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post("/api/v1/attachments/templates")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 && resp.status() != 201 {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {text}", resp.status()));
    }

    let envelope: ApiEnvelope<TemplateSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update a user-authored attachment template (change LED count, dimensions, etc.).
pub async fn update_attachment_template(
    template: &hypercolor_types::attachment::AttachmentTemplate,
) -> Result<TemplateSummary, String> {
    let url = format!("/api/v1/attachments/templates/{}", template.id);
    let body = serde_json::to_string(template).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {text}", resp.status()));
    }

    let envelope: ApiEnvelope<TemplateSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Fetch a single attachment template by ID.
pub async fn fetch_attachment_template(
    id: &str,
) -> Result<hypercolor_types::attachment::AttachmentTemplate, String> {
    let url = format!("/api/v1/attachments/templates/{id}");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    // The detail response includes topology — parse just the fields we need
    let envelope: ApiEnvelope<serde_json::Value> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    serde_json::from_value(envelope.data).map_err(|e| format!("Parse template: {e}"))
}

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

// ── Pairing Functions ───────────────────────────────────────────────────────

/// Pair a device using the generic pairing surface.
pub async fn pair_device(id: &str, req: &PairDeviceRequest) -> Result<PairDeviceResponse, String> {
    let url = format!("/api/v1/devices/{id}/pair");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {text}", resp.status()));
    }

    let envelope: ApiEnvelope<PairDeviceResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Remove stored credentials for a device.
pub async fn unpair_device(id: &str) -> Result<DeletePairingResponse, String> {
    let url = format!("/api/v1/devices/{id}/pair");

    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {text}", resp.status()));
    }

    let envelope: ApiEnvelope<DeletePairingResponse> =
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
