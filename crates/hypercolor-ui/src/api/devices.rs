//! Device-related API types and fetch functions.

use serde::{Deserialize, Serialize};

use super::client;

// ── Types ───────────────────────────────────────────────────────────────────

// Wire contracts are shared with the daemon (single definition in
// hypercolor-types) — drift is now a compile error, not a runtime parse
// failure. Pairing vocabulary likewise comes from hypercolor-types.
pub use hypercolor_types::api::devices::{
    ComponentBindingSummary, DeletePairingResponse, DeviceComponentsResponse,
    DeviceComponentsUpdateResponse, DeviceConnectionSummary, DeviceListResponse, DeviceSummary,
    IdentifyAttachmentRequest, IdentifyRequest, PairDeviceResponse, UpdateAttachmentsRequest,
    UpdateDeviceRequest, ZoneSummary, ZoneTopologySummary,
};
pub use hypercolor_types::api::settings::SetBrightnessRequest;
pub use hypercolor_types::attachment::ComponentBinding;
pub use hypercolor_types::pairing::{
    DeviceAuthState, DeviceAuthSummary, PairDeviceRequest, PairDeviceStatus, PairingDescriptor,
    PairingFieldDescriptor, PairingFlowKind,
};

/// Global brightness payload from `/api/v1/settings/brightness`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrightnessSettingsResponse {
    pub brightness: u8,
}

// ── Attachment Types ────────────────────────────────────────────────────────

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

/// The identify blink the UI asks for: two seconds in the given hex color.
fn identify_request(color: &str) -> IdentifyRequest {
    IdentifyRequest {
        duration_ms: Some(2000),
        color: Some(color.to_owned()),
    }
}

/// Identify a device by flashing its LEDs.
pub async fn identify_device(id: &str) -> Result<(), String> {
    let body = identify_request("FF06B5");
    client::post_json_discard(&format!("/api/v1/devices/{id}/identify"), &body)
        .await
        .map_err(Into::into)
}

/// Identify a single zone by flashing only its LEDs.
pub async fn identify_zone(device_id: &str, zone_id: &str) -> Result<(), String> {
    let body = identify_request("FF06B5");
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
    let body = IdentifyAttachmentRequest {
        base: identify_request("80FFEA"),
        binding_index,
        instance,
    };
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
) -> Result<DeviceComponentsUpdateResponse, String> {
    client::put_json(&format!("/api/v1/devices/{device_id}/attachments"), req)
        .await
        .map_err(Into::into)
}

/// Update the global output brightness.
pub async fn set_global_brightness(brightness: u8) -> Result<u8, String> {
    let body = SetBrightnessRequest { brightness };
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

/// `DELETE /api/v1/simulators/displays/{id}` — remove a simulated display
/// device along with its stored config and face assignments.
pub async fn delete_simulated_display(id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/simulators/displays/{id}"))
        .await
        .map_err(Into::into)
}

/// Fetch the current global brightness.
pub async fn fetch_global_brightness() -> Result<u8, String> {
    let resp: BrightnessSettingsResponse =
        client::fetch_json("/api/v1/settings/brightness").await?;
    Ok(resp.brightness)
}
