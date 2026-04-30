//! Device-related API types and fetch functions.

use std::collections::HashMap;

use gloo_net::http::Request;
use hypercolor_types::device::{DeviceOrigin, DriverPresentation};
use serde::{Deserialize, Serialize};

use super::client;

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
    #[serde(skip_serializing_if = "HashMap::is_empty")]
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
}

/// Response from `DELETE /api/v1/devices/:id/pair`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeletePairingResponse {
    pub message: String,
}

/// Device summary from `GET /api/v1/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceSummary {
    pub id: String,
    pub layout_device_id: String,
    pub name: String,
    pub backend: String,
    pub origin: DeviceOrigin,
    pub presentation: DriverPresentation,
    pub status: String,
    pub brightness: u8,
    #[serde(default)]
    pub firmware_version: Option<String>,
    #[serde(default)]
    pub connection: DeviceConnectionSummary,
    pub total_leds: usize,
    #[serde(default)]
    pub auth: Option<DeviceAuthSummary>,
    #[serde(default)]
    pub zones: Vec<ZoneSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceConnectionSummary {
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
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
    pub enabled: bool,
    pub instances: u32,
    pub led_offset: u32,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all tracked devices.
pub async fn fetch_devices() -> Result<Vec<DeviceSummary>, String> {
    let list: DeviceListResponse = client::fetch_json("/api/v1/devices").await?;
    Ok(list.items)
}

/// Trigger device discovery scan.
pub async fn discover_devices() -> Result<(), String> {
    client::post_empty("/api/v1/devices/discover")
        .await
        .map_err(Into::into)
}

/// Update a device (name, enabled, brightness).
pub async fn update_device(id: &str, req: &UpdateDeviceRequest) -> Result<DeviceSummary, String> {
    client::put_json(&format!("/api/v1/devices/{id}"), req)
        .await
        .map_err(Into::into)
}

/// Identify a device by flashing its LEDs.
pub async fn identify_device(id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "duration_ms": 2000, "color": "FF06B5" });
    client::post_json_discard(&format!("/api/v1/devices/{id}/identify"), &body)
        .await
        .map_err(Into::into)
}

/// Identify a single zone by flashing only its LEDs.
pub async fn identify_zone(device_id: &str, zone_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "duration_ms": 2000, "color": "FF06B5" });
    client::post_json_discard(
        &format!("/api/v1/devices/{device_id}/zones/{zone_id}/identify"),
        &body,
    )
    .await
    .map_err(Into::into)
}

/// Identify a specific attachment component by flashing its LED range.
pub async fn identify_attachment(
    device_id: &str,
    slot_id: &str,
    binding_index: Option<usize>,
    instance: Option<u32>,
) -> Result<(), String> {
    let mut body = serde_json::json!({ "duration_ms": 2000, "color": "80FFEA" });
    if let Some(idx) = binding_index {
        body["binding_index"] = serde_json::json!(idx);
    }
    if let Some(instance) = instance {
        body["instance"] = serde_json::json!(instance);
    }
    client::post_json_discard(
        &format!("/api/v1/devices/{device_id}/attachments/{slot_id}/identify"),
        &body,
    )
    .await
    .map_err(Into::into)
}

/// Create a user-authored attachment template (custom strip, matrix, etc.).
/// Uses raw request because the daemon returns detailed error text on failure.
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
    if !(200..300).contains(&resp.status()) {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {text}", resp.status()));
    }
    resp.json::<super::ApiEnvelope<TemplateSummary>>()
        .await
        .map(|e| e.data)
        .map_err(|e| format!("Parse error: {e}"))
}

/// Fetch attachment bindings and import-ready zones for a physical device.
pub async fn fetch_device_attachments(
    device_id: &str,
) -> Result<DeviceAttachmentsResponse, String> {
    client::fetch_json(&format!("/api/v1/devices/{device_id}/attachments"))
        .await
        .map_err(Into::into)
}

/// Fetch attachment templates, optionally filtered by category.
pub async fn fetch_attachment_templates(
    category: Option<&str>,
) -> Result<Vec<TemplateSummary>, String> {
    let mut url = "/api/v1/attachments/templates?limit=200".to_string();
    if let Some(cat) = category {
        url.push_str(&format!("&category={cat}"));
    }
    let list: TemplateListResponse = client::fetch_json(&url).await?;
    Ok(list.items)
}

/// Update attachment bindings for a device.
pub async fn update_device_attachments(
    device_id: &str,
    req: &UpdateAttachmentsRequest,
) -> Result<DeviceAttachmentsResponse, String> {
    client::put_json(&format!("/api/v1/devices/{device_id}/attachments"), req)
        .await
        .map_err(Into::into)
}

/// Update the global output brightness.
pub async fn set_global_brightness(brightness: u8) -> Result<u8, String> {
    let body = serde_json::json!({ "brightness": brightness });
    let resp: BrightnessSettingsResponse =
        client::put_json("/api/v1/settings/brightness", &body).await?;
    Ok(resp.brightness)
}

// ── Pairing Functions ───────────────────────────────────────────────────────

/// Pair a device using the generic pairing surface.
/// Uses raw request because the daemon returns detailed error text on failure.
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
    resp.json::<super::ApiEnvelope<PairDeviceResponse>>()
        .await
        .map(|e| e.data)
        .map_err(|e| format!("Parse error: {e}"))
}

/// Remove stored credentials for a device.
/// Uses raw request because the daemon returns detailed error text on failure.
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
    resp.json::<super::ApiEnvelope<DeletePairingResponse>>()
        .await
        .map(|e| e.data)
        .map_err(|e| format!("Parse error: {e}"))
}

/// Fetch the current global brightness.
pub async fn fetch_global_brightness() -> Result<u8, String> {
    let resp: BrightnessSettingsResponse =
        client::fetch_json("/api/v1/settings/brightness").await?;
    Ok(resp.brightness)
}
