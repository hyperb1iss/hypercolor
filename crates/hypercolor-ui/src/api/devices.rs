//! Device-related API types and fetch functions.

use serde::{Deserialize, Serialize};

use super::client;

// ── Types ───────────────────────────────────────────────────────────────────

// Wire contracts are shared with the daemon (single definition in
// hypercolor-types) — drift is now a compile error, not a runtime parse
// failure. Pairing vocabulary likewise comes from hypercolor-types.
pub use hypercolor_types::api::devices::{
    DeviceConnectionSummary, DeviceListResponse, DeviceSummary, UpdateDeviceRequest, ZoneSummary,
    ZoneTopologySummary,
};
pub use hypercolor_types::pairing::{
    DeviceAuthState, DeviceAuthSummary, PairDeviceRequest, PairDeviceStatus, PairingDescriptor,
    PairingFieldDescriptor, PairingFlowKind,
};

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

/// Global brightness payload from `/api/v1/settings/brightness`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrightnessSettingsResponse {
    pub brightness: u8,
}

// ── Attachment Types ────────────────────────────────────────────────────────

/// Attachment binding summary from `GET /api/v1/devices/:id/attachments`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComponentBindingSummary {
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
pub struct DeviceComponentsResponse {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub slots: Vec<hypercolor_types::attachment::ComponentSlot>,
    #[serde(default)]
    pub bindings: Vec<ComponentBindingSummary>,
    #[serde(default)]
    pub suggested_zones: Vec<hypercolor_types::attachment::ComponentSuggestedZone>,
}

/// Template summary from `GET /api/v1/attachments/templates`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: hypercolor_types::attachment::ComponentCategory,
    #[serde(default)]
    pub origin: Option<hypercolor_types::attachment::ComponentOrigin>,
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
    pub bindings: Vec<ComponentBindingRequest>,
}

/// A single binding entry sent to the update endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentBindingRequest {
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
pub async fn create_attachment_template(
    template: &hypercolor_types::attachment::ComponentTemplate,
) -> Result<TemplateSummary, String> {
    client::post_json("/api/v1/attachments/templates", template)
        .await
        .map_err(Into::into)
}

/// Fetch attachment bindings and import-ready zones for a physical device.
pub async fn fetch_device_attachments(device_id: &str) -> Result<DeviceComponentsResponse, String> {
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
) -> Result<DeviceComponentsResponse, String> {
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
pub async fn pair_device(id: &str, req: &PairDeviceRequest) -> Result<PairDeviceResponse, String> {
    client::post_json(&format!("/api/v1/devices/{id}/pair"), req)
        .await
        .map_err(Into::into)
}

/// Remove stored credentials for a device.
pub async fn unpair_device(id: &str) -> Result<DeletePairingResponse, String> {
    client::delete_json(&format!("/api/v1/devices/{id}/pair"))
        .await
        .map_err(Into::into)
}

/// Fetch the current global brightness.
pub async fn fetch_global_brightness() -> Result<u8, String> {
    let resp: BrightnessSettingsResponse =
        client::fetch_json("/api/v1/settings/brightness").await?;
    Ok(resp.brightness)
}
