//! Device API contracts — `/api/v1/devices/*`.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::attachment::{ComponentBinding, ComponentSlot, ComponentSuggestedZone};
use crate::device::{DeviceOrigin, DriverPresentation};
use crate::event::DeviceRef;
use crate::pairing::{DeviceAuthSummary, PairDeviceStatus};
use crate::scene::DisplayRotation;

/// Query parameters for `GET /api/v1/devices`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema, utoipa::IntoParams))]
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
pub type DeviceListResponse = ListResponse<DeviceSummary>;

/// One device in the list/detail responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeviceSummary {
    pub id: String,
    pub layout_device_id: String,
    pub name: String,
    pub origin: DeviceOrigin,
    pub presentation: DriverPresentation,
    pub status: String,
    pub brightness: u8,
    /// How the panel is mounted; present only for display-capable devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_rotation: Option<DisplayRotation>,
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
    /// Bridge route facts; present only when `origin.transport` is `bridge`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<BridgeDeviceSummary>,
}

/// How an out-of-process bridge (OpenRGB) reaches one device.
///
/// Filled from the bridge driver's discovery metadata. `output_enabled`
/// is the effective value: a route the daemon's conflict guard has
/// output-disabled reports `false` here with the guard's reason, even
/// when the bridge itself still advertises the controller as writable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BridgeDeviceSummary {
    /// Bridge server endpoint (`host:port`).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Controller index on that server.
    #[serde(default)]
    pub controller_index: Option<u32>,
    /// How confident the bridge is that this route maps to one physical
    /// device across restarts (`stable`, `heuristic`, ...).
    #[serde(default)]
    pub identity_confidence: Option<String>,
    /// The bridge-side detector that produced the controller.
    #[serde(default)]
    pub detector_class: Option<String>,
    /// Whether frames may be written through this route.
    pub output_enabled: bool,
    /// Why output is disabled, when it is.
    #[serde(default)]
    pub disabled_reason: Option<String>,
    /// Negotiated bridge protocol version.
    #[serde(default)]
    pub protocol_version: Option<u32>,
    /// The bridge's stable route fingerprint
    /// (`bridge:openrgb:<endpoint>:serial:<SERIAL>` or `...:location:<LOCATION>`),
    /// the key `drivers.openrgb.zone_sizes` entries use.
    #[serde(default)]
    pub fingerprint: Option<String>,
}

/// Transport details for one device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SegmentSummary {
    pub id: String,
    pub name: String,
    pub led_count: u32,
    pub topology: String,
    #[serde(default)]
    pub topology_hint: Option<SegmentTopologySummary>,
}

/// Structured topology hint for a device segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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

/// Request body for `PUT /api/v1/devices/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UpdateDeviceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    /// How the panel is mounted; accepted only for display-capable devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_rotation: Option<DisplayRotation>,
}

/// Response for `DELETE /api/v1/devices/{id}`.
///
/// `id` echoes the resolved device id, which may differ from the name or
/// prefix the caller addressed the device by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeleteDeviceResponse {
    pub id: String,
    pub removed: bool,
}

/// Forget saved controller content by its stable layout binding, even offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ForgetDeviceRequest {
    pub layout_device_id: String,
}

/// Request body for `POST /api/v1/devices/{id}/identify`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct IdentifyDeviceResponse {
    pub device_id: String,
    pub identifying: bool,
    pub duration_ms: u64,
    pub color: Option<String>,
}

/// Response for `POST /api/v1/devices/{id}/segments/{segment}/identify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct IdentifySegmentResponse {
    pub device_id: String,
    pub segment: String,
    pub segment_name: String,
    pub identifying: bool,
    pub duration_ms: u64,
    pub color: Option<String>,
}

/// Request body for
/// `POST /api/v1/devices/{id}/attachments/{slot}/identify`.
///
/// Carries the base identify parameters plus the selectors that narrow
/// the blink to one attached component instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct IdentifyAttachmentRequest {
    #[serde(flatten)]
    pub base: IdentifyRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<u32>,
}

/// Response for
/// `POST /api/v1/devices/{id}/attachments/{slot}/identify`.
///
/// `instance` is `null` when the request blinked every instance of the
/// binding rather than one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeleteAttachmentsResponse {
    pub device_id: String,
    pub deleted: bool,
}

/// Optional body for `POST /api/v1/devices/discover`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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

/// Per-scanner diagnostics from a completed discovery scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiscoveryScannerResult {
    pub scanner: String,
    pub duration_ms: u64,
    pub discovered: usize,
    pub status: String,
    pub error: Option<String>,
}

/// Detailed result from a completed discovery scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiscoveryScanResult {
    pub targets: Vec<String>,
    pub timeout_ms: u64,
    pub new_devices: Vec<DeviceRef>,
    pub reappeared_devices: Vec<DeviceRef>,
    pub vanished_devices: Vec<String>,
    pub total_known: usize,
    pub duration_ms: u64,
    pub scanners: Vec<DiscoveryScannerResult>,
}

/// Response from `POST /api/v1/devices/discover`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DiscoverResponse {
    /// Immediate acknowledgement for an asynchronous discovery scan.
    #[cfg_attr(feature = "schema", schema(title = "DiscoveryScanningResponse"))]
    Scanning {
        scan_id: String,
        targets: Vec<String>,
        timeout_ms: u64,
    },
    /// Completed response for a synchronous discovery scan.
    #[cfg_attr(feature = "schema", schema(title = "DiscoveryCompletedResponse"))]
    Completed {
        scan_id: String,
        result: DiscoveryScanResult,
    },
}

/// Response for `POST /api/v1/devices/{id}/pair`.
///
/// `device` carries the device's refreshed summary when pairing changed
/// its state enough to be worth re-rendering, and is omitted otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PairDeviceResponse {
    #[cfg_attr(feature = "schema", schema(value_type = String))]
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
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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

/// Response for `GET /api/v1/devices/unclaimed`.
pub type UnclaimedDeviceListResponse = ListResponse<UnclaimedDevice>;

/// A USB device the host can see that no enabled native driver claims.
///
/// `claimable_by` names the native driver whose protocol database matches
/// the device when that driver is disabled by config; `None` means no
/// native protocol exists for the vendor/product pair at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UnclaimedDevice {
    pub vendor_id: u16,
    pub product_id: u16,
    #[serde(default)]
    pub manufacturer: Option<String>,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub serial: Option<String>,
    /// Host bus path (`<bus>-<port chain>`), when the platform reports one.
    #[serde(default)]
    pub bus_path: Option<String>,
    /// USB interface class codes of the active configuration, sorted and
    /// deduplicated; empty where the platform does not expose them.
    #[serde(default)]
    pub interface_classes: Vec<u8>,
    #[serde(default)]
    pub claimable_by: Option<String>,
}

/// Response for `GET /api/v1/devices/coverage`.
pub type DeviceCoverageListResponse = ListResponse<DeviceCoverageRow>;

/// Which key joined the sources of one coverage row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CoverageIdentityKind {
    /// Device serial number, compared trimmed and case-insensitively.
    Serial,
    /// SMBus bus plus slave address.
    Smbus,
    /// USB bus path.
    UsbPath,
    /// No shared key; the row is one source's own device id.
    Device,
}

/// The physical-device identity a coverage row was joined on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CoverageIdentity {
    pub kind: CoverageIdentityKind,
    /// The normalized key value.
    pub value: String,
    /// Best available human label for the hardware.
    pub label: String,
}

/// The native side of a coverage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CoverageNativeDevice {
    pub device_id: String,
    pub driver_id: String,
    /// Lifecycle state name in lowercase (`connected`, `known`, ...).
    pub state: String,
}

/// The bridge side of a coverage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CoverageBridgeDevice {
    pub device_id: String,
    /// Lifecycle state name in lowercase.
    pub state: String,
    pub output_enabled: bool,
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

/// Which stack currently owns the hardware in one coverage row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CoverageActive {
    /// A native driver renders the device.
    Native,
    /// The bridge renders the device.
    Bridge,
    /// Nobody renders it.
    None,
    /// Native renders it while a bridge route is still output-enabled.
    Conflict,
}

/// One physical device across the native registry, bridge routes, and the
/// unclaimed inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeviceCoverageRow {
    pub identity: CoverageIdentity,
    #[serde(default)]
    pub native: Option<CoverageNativeDevice>,
    #[serde(default)]
    pub bridge: Option<CoverageBridgeDevice>,
    /// Whether the unclaimed USB inventory also lists this hardware.
    pub unclaimed: bool,
    pub active: CoverageActive,
}
