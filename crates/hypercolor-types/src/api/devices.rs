//! Device API contracts — `/api/v1/devices/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::common::Pagination;
use crate::attachment::ComponentBinding;
use crate::device::{DeviceOrigin, DriverPresentation};
use crate::pairing::DeviceAuthSummary;

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
    pub zones: Vec<ZoneSummary>,
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

/// One LED zone of a device (hardware topology, not scene render groups).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ZoneSummary {
    pub id: String,
    pub name: String,
    pub led_count: u32,
    pub topology: String,
    #[serde(default)]
    pub topology_hint: Option<ZoneTopologySummary>,
}

/// Structured topology hint for a device zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

/// Request body for `POST /api/v1/devices/{id}/identify`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IdentifyRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// Request body for `PUT /api/v1/devices/{id}/attachments`.
///
/// The binding list replaces the device's attachments wholesale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAttachmentsRequest {
    #[serde(default)]
    pub bindings: Vec<ComponentBinding>,
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
