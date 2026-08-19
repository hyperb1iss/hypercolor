//! Device API contracts — `/api/v1/devices/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::common::Pagination;
use crate::attachment::{ComponentBinding, ComponentSlot, ComponentSuggestedZone};
use crate::device::{DeviceOrigin, DriverPresentation};
use crate::pairing::{DeviceAuthSummary, PairDeviceStatus};

/// Query parameters for `GET /api/v1/devices`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListDevicesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Free-text filter over device name and model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Comma-separated summary expansions. The only supported value is
    /// `attachments`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
}

/// Response for `GET /api/v1/devices`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceListResponse {
    pub items: Vec<DeviceSummary>,
    pub pagination: Pagination,
}

/// One device in the list/detail responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSummary {
    pub id: String,
    pub layout_device_id: String,
    pub name: String,
    pub origin: DeviceOrigin,
    pub presentation: DriverPresentation,
    pub status: String,
    pub brightness: u8,
    #[serde(default)]
    pub firmware_version: Option<String>,
    #[serde(default)]
    pub connection: DeviceConnectionSummary,
    pub total_leds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<DeviceAuthSummary>,
    #[serde(default)]
    pub segments: Vec<SegmentSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<DeviceComponentsResponse>,
}

/// Transport details for one device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

/// One LED segment of a device (hardware topology, not scene render zones).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SegmentSummary {
    pub id: String,
    pub name: String,
    pub led_count: u32,
    pub topology: String,
    #[serde(default)]
    pub topology_hint: Option<SegmentTopologySummary>,
}

/// Structured topology hint for a device segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SegmentTopologySummary {
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

/// Request body for `PATCH /api/v1/devices/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpdateDeviceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
}

/// Response for `DELETE /api/v1/devices/{id}`.
///
/// `id` echoes the resolved device id, which may differ from the name or
/// prefix the caller addressed the device by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteDeviceResponse {
    pub id: String,
    pub removed: bool,
}

/// Request body for `POST /api/v1/devices/{id}/identify`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IdentifyRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Response for `POST /api/v1/devices/{id}/identify`.
///
/// The blink runs in the background, so the response only acknowledges
/// that it started and echoes the parameters actually used. `color` is
/// `null` when the caller sent no color and the daemon used its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifyDeviceResponse {
    pub device_id: String,
    pub identifying: bool,
    pub duration_ms: u64,
    pub color: Option<String>,
}

/// Response for `POST /api/v1/devices/{id}/segments/{segment}/identify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifySegmentResponse {
    pub device_id: String,
    pub segment: String,
    pub segment_name: String,
    pub identifying: bool,
    pub duration_ms: u64,
    pub color: Option<String>,
}

/// Request body for
/// `POST /api/v1/devices/{id}/attachments/{component_id}/identify`.
///
/// Carries the base identify parameters plus the selectors that narrow
/// the blink to one attached component instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifyAttachmentRequest {
    #[serde(flatten)]
    pub base: IdentifyRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<u32>,
}

/// Response for
/// `POST /api/v1/devices/{id}/attachments/{component_id}/identify`.
///
/// `instance` is `null` when the request blinked every instance of the
/// binding rather than one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifyAttachmentResponse {
    pub device_id: String,
    pub slot_id: String,
    pub binding_index: usize,
    pub instance: Option<u32>,
    pub identifying: bool,
    pub duration_ms: u64,
    pub color: Option<String>,
}

/// Request body for `PUT /api/v1/devices/{id}/attachments`.
///
/// The binding list replaces the device's attachments wholesale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpdateAttachmentsRequest {
    #[serde(default)]
    pub bindings: Vec<ComponentBinding>,
    /// Validate and resolve the profile without applying any side effects.
    #[serde(default)]
    pub validate_only: bool,
}

/// Response for `GET /api/v1/devices/{id}/attachments`.
///
/// `slots` are the controller's physical attachment points, `bindings`
/// what is attached to them, and `suggested_zones` the layout zones the
/// attachments imply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceComponentsResponse {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub slots: Vec<ComponentSlot>,
    #[serde(default)]
    pub bindings: Vec<ComponentBindingSummary>,
    #[serde(default)]
    pub suggested_zones: Vec<ComponentSuggestedZone>,
}

/// Response for `PUT /api/v1/devices/{id}/attachments`.
///
/// Same body as the GET plus `needs_layout_update`, which reports that
/// the active layout targets this device and no longer matches the LED
/// ranges the new bindings describe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceComponentsUpdateResponse {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub slots: Vec<ComponentSlot>,
    #[serde(default)]
    pub bindings: Vec<ComponentBindingSummary>,
    #[serde(default)]
    pub suggested_zones: Vec<ComponentSuggestedZone>,
    pub needs_layout_update: bool,
}

/// One resolved attachment binding, with the template it instantiates and
/// the LED range it occupies on the controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

/// Response for `DELETE /api/v1/devices/{id}/attachments`.
///
/// `deleted` is false when the device had no stored profile to remove,
/// which is a success rather than a 404.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteAttachmentsResponse {
    pub device_id: String,
    pub deleted: bool,
}

/// Optional body for `POST /api/v1/devices/discover`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DiscoverRequest {
    /// Discovery targets to scan; omitted scans every enabled target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Block until the scan finishes instead of returning a scan id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// Query parameters for `GET /api/v1/logical-devices`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListLogicalDevicesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Filter to the logical devices carved out of one physical device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Request body for `POST /api/v1/devices/{id}/logical-devices`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLogicalDeviceRequest {
    pub name: String,
    pub led_start: u32,
    pub led_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Request body for `PUT /api/v1/logical-devices/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateLogicalDeviceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Response for `GET /api/v1/logical-devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalDeviceListResponse {
    pub items: Vec<LogicalDeviceSummary>,
    pub pagination: Pagination,
}

/// One logical device: a named LED range carved out of a physical one.
///
/// `origin` and `physical_status` describe the physical device behind the
/// range, and are `null` and `"unknown"` respectively when it is not
/// currently attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub origin: Option<DeviceOrigin>,
    pub physical_status: String,
}

/// Response for `DELETE /api/v1/logical-devices/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteLogicalDeviceResponse {
    pub id: String,
    pub deleted: bool,
}

/// Response for `GET /api/v1/devices/bindings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceBindingsResponse {
    /// Layout bindings that no attached device currently resolves.
    pub unresolved: Vec<UnresolvedBindingSummary>,
    /// Attached devices no layout binding references, offered for re-bind.
    pub candidates: Vec<RebindCandidateSummary>,
}

/// One layout binding with no attached device behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UnresolvedBindingSummary {
    /// The layout binding id the zones reference.
    pub layout_device_id: String,
    /// The layouts whose zones reference it.
    pub layout_ids: Vec<String>,
    /// Whether a recorded identity exists for this binding, which is what
    /// a durable re-bind needs to inherit.
    pub rebindable: bool,
}

/// One attached device offered as a re-bind target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RebindCandidateSummary {
    pub device_id: String,
    pub name: String,
    /// The layout binding id this device currently derives.
    pub layout_device_id: String,
    pub status: String,
    /// The device's portable key. Only claimed devices can inherit a
    /// binding durably; a claimless candidate re-binds by layout edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_key: Option<String>,
}

/// Request body for `POST /api/v1/devices/rebind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RebindDeviceRequest {
    /// The orphaned layout binding to inherit.
    pub layout_device_id: String,
    /// The attached, claimed device that should inherit it.
    pub device_id: String,
}

/// Response for `POST /api/v1/devices/rebind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RebindDeviceResponse {
    pub device_id: String,
    /// The layout binding id the device now resolves to.
    pub layout_device_id: String,
    /// The portable key that was re-pinned to the inherited identity.
    pub portable_key: String,
}

/// Response for `POST /api/v1/devices/{id}/pair`.
///
/// `device` carries the device's refreshed summary when pairing changed
/// its state enough to be worth re-rendering, and is omitted otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairDeviceResponse {
    pub status: PairDeviceStatus,
    pub message: String,
    /// Whether the device was connected and started rendering as part of
    /// the pairing.
    #[serde(default)]
    pub activated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceSummary>,
}

/// Response for `DELETE /api/v1/devices/{id}/pair`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeletePairingResponse {
    #[serde(default)]
    pub status: String,
    pub message: String,
    /// Whether forgetting the credentials also dropped a live connection.
    #[serde(default)]
    pub disconnected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceSummary>,
}
