//! Device-related API types and fetch functions.

use super::{ApiResult, client};

// ── Types ───────────────────────────────────────────────────────────────────

// Wire contracts are shared with the daemon (single definition in
// hypercolor-types) — drift is now a compile error, not a runtime parse
// failure. Pairing vocabulary likewise comes from hypercolor-types.
pub use hypercolor_types::api::devices::{
    ComponentBindingSummary, DeletePairingResponse, DeviceComponentsResponse,
    DeviceComponentsUpdateResponse, DeviceConnectionSummary, DeviceListResponse, DeviceSummary,
    IdentifyAttachmentRequest, IdentifyRequest, PairDeviceResponse, SegmentSummary,
    SegmentTopologySummary, UpdateAttachmentsRequest, UpdateDeviceRequest,
};
pub use hypercolor_types::attachment::ComponentBinding;
pub use hypercolor_types::pairing::{
    DeviceAuthState, DeviceAuthSummary, PairDeviceRequest, PairDeviceStatus, PairingDescriptor,
    PairingFieldDescriptor, PairingFlowKind,
};

// ── Attachment Types ────────────────────────────────────────────────────────

// The attachment-template catalog contracts are shared with the daemon
// (hypercolor-types::api::attachments).
pub use hypercolor_types::api::attachments::{
    TemplateDetail, TemplateListResponse, TemplateSummary,
};

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all tracked devices.
pub async fn fetch_devices() -> ApiResult<Vec<DeviceSummary>> {
    client::fetch_all_pages("/api/v1/devices?include=attachments").await
}

/// Trigger device discovery scan.
pub async fn discover_devices() -> ApiResult<()> {
    client::post_empty("/api/v1/devices/discover").await
}

/// Update a device (name, enabled, brightness).
pub async fn update_device(id: &str, req: &UpdateDeviceRequest) -> ApiResult<DeviceSummary> {
    client::put_json(&format!("/api/v1/devices/{id}"), req).await
}

/// The identify blink the UI asks for: two seconds in the given hex color.
fn identify_request(color: &str) -> IdentifyRequest {
    IdentifyRequest {
        duration_ms: Some(2000),
        color: Some(color.to_owned()),
    }
}

/// Identify a device by flashing its LEDs.
pub async fn identify_device(id: &str) -> ApiResult<()> {
    let body = identify_request("FF06B5");
    client::post_json_discard(&format!("/api/v1/devices/{id}/identify"), &body).await
}

/// Identify a single hardware segment by flashing only its LEDs.
pub async fn identify_segment(device_id: &str, segment: &str) -> ApiResult<()> {
    let body = identify_request("FF06B5");
    client::post_json_discard(
        &format!("/api/v1/devices/{device_id}/segments/{segment}/identify"),
        &body,
    )
    .await
}

/// Identify a specific attachment component by flashing its LED range.
pub async fn identify_attachment(
    device_id: &str,
    slot_id: &str,
    binding_index: Option<usize>,
    instance: Option<u32>,
) -> ApiResult<()> {
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
}

/// Create a user-authored attachment template (custom strip, matrix, etc.).
pub async fn create_attachment_template(
    template: &hypercolor_types::attachment::ComponentTemplate,
) -> ApiResult<TemplateDetail> {
    client::post_json("/api/v1/attachments/templates", template).await
}

/// Fetch attachment bindings and import-ready zones for a physical device.
pub async fn fetch_device_attachments(device_id: &str) -> ApiResult<DeviceComponentsResponse> {
    client::fetch_json(&format!("/api/v1/devices/{device_id}/attachments")).await
}

/// Fetch attachment templates, optionally filtered by category.
pub async fn fetch_attachment_templates(category: Option<&str>) -> ApiResult<Vec<TemplateSummary>> {
    let mut url = "/api/v1/attachments/templates".to_string();
    if let Some(cat) = category {
        url.push_str(&format!("?category={cat}"));
    }
    client::fetch_all_pages(&url).await
}

/// Update attachment bindings for a device.
pub async fn update_device_attachments(
    device_id: &str,
    req: &UpdateAttachmentsRequest,
) -> ApiResult<DeviceComponentsUpdateResponse> {
    client::put_json(&format!("/api/v1/devices/{device_id}/attachments"), req).await
}

// ── Pairing Functions ───────────────────────────────────────────────────────

/// Pair a device using the generic pairing surface.
pub async fn pair_device(id: &str, req: &PairDeviceRequest) -> ApiResult<PairDeviceResponse> {
    client::post_json(&format!("/api/v1/devices/{id}/pair"), req).await
}

/// Remove stored credentials for a device.
pub async fn unpair_device(id: &str) -> ApiResult<DeletePairingResponse> {
    client::delete_json(&format!("/api/v1/devices/{id}/pair")).await
}

/// `DELETE /api/v1/simulators/displays/{id}` — remove a simulated display
/// device along with its stored config and face assignments.
pub async fn delete_simulated_display(id: &str) -> ApiResult<()> {
    client::delete_empty(&format!("/api/v1/simulators/displays/{id}")).await
}
