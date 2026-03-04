//! Device identity, capabilities, and state types.
//!
//! These types form the shared vocabulary for device management across the
//! Hypercolor engine. They live in `hypercolor-types` so every crate can
//! reference them without pulling in I/O, async, or backend-specific logic.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── DeviceId ──────────────────────────────────────────────────────────────

/// Opaque, globally unique device identifier.
///
/// Wraps a `UUIDv7` so identifiers are time-ordered and safe to use as
/// database keys, map keys, and log correlation IDs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub Uuid);

impl DeviceId {
    /// Generate a fresh identifier (`UUIDv7` -- time-ordered).
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// The inner UUID value.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.0)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DeviceId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

// ── DeviceInfo ────────────────────────────────────────────────────────────

/// Complete description of a device's identity, capabilities, and topology.
///
/// Populated during discovery and enriched during connection. Serialized to
/// TOML for the device registry and transmitted to frontends via the event
/// bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable unique identifier.
    pub id: DeviceId,

    /// User-facing display name (e.g., "Main Case RGB").
    pub name: String,

    /// Hardware manufacturer or brand.
    pub vendor: String,

    /// Device family classification.
    pub family: DeviceFamily,

    /// Transport / connection type.
    pub connection_type: ConnectionType,

    /// Zones within this device, each with its own LED topology.
    pub zones: Vec<ZoneInfo>,

    /// Firmware version string, if known.
    pub firmware_version: Option<String>,

    /// Aggregate device capabilities.
    pub capabilities: DeviceCapabilities,
}

impl DeviceInfo {
    /// Total LED count across all zones.
    #[must_use]
    pub fn total_led_count(&self) -> u32 {
        self.zones.iter().map(|z| z.led_count).sum()
    }
}

// ── DeviceCapabilities ────────────────────────────────────────────────────

/// Aggregate capability flags for a device.
///
/// These describe what the hardware can do, not what software supports.
/// Backends populate this during discovery / connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    /// Total addressable LEDs (may differ from zone sum if hardware
    /// reserves slots).
    pub led_count: u32,

    /// Whether the device supports direct per-LED color writes.
    pub supports_direct: bool,

    /// Whether brightness can be controlled independently of color.
    pub supports_brightness: bool,

    /// Maximum sustainable frame rate (0 = unknown / unlimited).
    pub max_fps: u32,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            led_count: 0,
            supports_direct: true,
            supports_brightness: false,
            max_fps: 60,
        }
    }
}

// ── ZoneInfo ──────────────────────────────────────────────────────────────

/// A single zone within a device.
///
/// Each zone maps to a contiguous range of LEDs with a specific topology.
/// The spatial layout engine positions zones on the canvas independently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneInfo {
    /// Zone name (e.g., "Channel 1", "ATX Strimer", "Keyboard Backlight").
    pub name: String,

    /// Number of LEDs in this zone.
    pub led_count: u32,

    /// Physical arrangement of LEDs.
    pub topology: DeviceTopologyHint,

    /// Wire-level color format for this zone.
    pub color_format: DeviceColorFormat,
}

// ── DeviceTopologyHint ────────────────────────────────────────────────────

/// Hardware-level hint about the physical LED arrangement within a zone.
///
/// This is a simplified topology used during device discovery and registration.
/// For the richer spatial layout topology (with directional params, serpentine
/// wiring, concentric rings, etc.), see
/// [`spatial::LedTopology`](crate::spatial::LedTopology).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceTopologyHint {
    /// Linear strip of LEDs.
    Strip,

    /// 2D matrix (e.g., Strimer cable, LED panel).
    Matrix {
        /// Number of rows.
        rows: u32,
        /// Number of columns.
        cols: u32,
    },

    /// Circular ring (e.g., fan, Hue Iris).
    Ring {
        /// Number of LEDs around the ring.
        count: u32,
    },

    /// Single-LED point source (e.g., Hue bulb).
    Point,

    /// Arbitrary positions defined in the spatial layout.
    Custom,
}

// ── ConnectionType ────────────────────────────────────────────────────────

/// How the device connects to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionType {
    /// USB HID (`PrismRGB`, Nollie).
    Usb,

    /// Network protocols (WLED DDP, E1.31, `OpenRGB` SDK TCP, Hue HTTP).
    Network,

    /// Bluetooth Low Energy.
    Bluetooth,

    /// Out-of-process bridge (gRPC, Unix socket).
    Bridge,
}

// ── DeviceFamily ──────────────────────────────────────────────────────────

/// Device family classification for protocol selection and device database
/// lookup.
///
/// Known families are `Copy` and `Hash`-able. Truly unknown hardware uses
/// `Custom(String)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceFamily {
    /// Any device managed through the `OpenRGB` SDK.
    OpenRgb,

    /// WLED ESP8266/ESP32 controller.
    Wled,

    /// Philips Hue bridge + lights.
    Hue,

    /// Unknown or user-defined device family.
    Custom(String),
}

impl fmt::Display for DeviceFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenRgb => write!(f, "OpenRGB"),
            Self::Wled => write!(f, "WLED"),
            Self::Hue => write!(f, "Philips Hue"),
            Self::Custom(name) => write!(f, "{name}"),
        }
    }
}

// ── DeviceState ───────────────────────────────────────────────────────────

/// Device lifecycle state.
///
/// Transitions are enforced by `DeviceStateMachine` in `hypercolor-core`.
/// This enum is the serializable snapshot used by frontends and persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceState {
    /// Discovered but not yet connected.
    Known,

    /// Connection established, awaiting first successful frame push.
    Connected,

    /// Actively receiving and rendering frames.
    Active,

    /// Connection lost, attempting to reconnect.
    Reconnecting,

    /// Intentionally disabled by the user.
    Disabled,
}

impl DeviceState {
    /// Variant name for logging and error messages.
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Known => "Known",
            Self::Connected => "Connected",
            Self::Active => "Active",
            Self::Reconnecting => "Reconnecting",
            Self::Disabled => "Disabled",
        }
    }

    /// Whether the device is in a state where frames can be pushed.
    #[must_use]
    pub fn is_renderable(&self) -> bool {
        matches!(self, Self::Connected | Self::Active)
    }
}

impl fmt::Display for DeviceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.variant_name())
    }
}

// ── DeviceColorFormat ─────────────────────────────────────────────────────

/// Wire-level color format used by a device zone.
///
/// Backends apply the conversion in `push_frame` before writing bytes to
/// the transport.
///
/// This is the *device-side* format hint (2 variants). For the richer
/// canvas-level color format (including `RgbW16`), see
/// [`canvas::ColorFormat`](crate::canvas::ColorFormat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceColorFormat {
    /// Standard RGB byte order (WLED, Prism S, Prism Mini).
    Rgb,

    /// RGB + White channel (SK6812 RGBW strips via WLED).
    Rgbw,
}

impl fmt::Display for DeviceColorFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rgb => write!(f, "RGB"),
            Self::Rgbw => write!(f, "RGBW"),
        }
    }
}

// ── DeviceError ───────────────────────────────────────────────────────────

/// Errors from the device backend layer.
///
/// All variants are `Send + Sync` for use across async boundaries.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// Connection attempt failed.
    #[error("connection to {device} failed: {reason}")]
    ConnectionFailed {
        /// Device display name or identifier.
        device: String,
        /// Human-readable failure reason.
        reason: String,
    },

    /// Write / push operation failed.
    #[error("write error on {device}: {detail}")]
    WriteError {
        /// Device display name or identifier.
        device: String,
        /// Error detail.
        detail: String,
    },

    /// Operation timed out.
    #[error("timeout communicating with {device}: {operation}")]
    Timeout {
        /// Device display name or identifier.
        device: String,
        /// What was being attempted.
        operation: String,
    },

    /// Device not found during connection attempt.
    #[error("device not found: {device}")]
    NotFound {
        /// Device display name or identifier.
        device: String,
    },

    /// Protocol-level error (DDP, E1.31, `OpenRGB` SDK, Hue, etc.).
    #[error("protocol error for {device}: {detail}")]
    ProtocolError {
        /// Device display name or identifier.
        device: String,
        /// Protocol-specific detail.
        detail: String,
    },

    /// Device disconnected unexpectedly.
    #[error("device disconnected: {device}")]
    Disconnected {
        /// Device display name or identifier.
        device: String,
    },

    /// Connection handle is stale or unknown.
    #[error("invalid handle {handle_id} for backend {backend}")]
    InvalidHandle {
        /// Monotonic handle ID.
        handle_id: u64,
        /// Backend identifier that reported the invalid handle.
        backend: String,
    },

    /// Invalid lifecycle transition attempted by the state machine.
    #[error("invalid device transition for {device}: {from} -> {to}")]
    InvalidTransition {
        /// Device display name or identifier.
        device: String,
        /// Current state name.
        from: String,
        /// Requested next state name.
        to: String,
    },
}

impl DeviceError {
    /// Whether this error indicates the device is gone and reconnection
    /// should be attempted.
    #[must_use]
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::ConnectionFailed { .. }
                | Self::WriteError { .. }
                | Self::Timeout { .. }
                | Self::ProtocolError { .. }
                | Self::Disconnected { .. }
        )
    }
}

// ── DeviceIdentifier ──────────────────────────────────────────────────────

/// Transport-specific stable identity for a physical device.
///
/// Used as the deduplication key during discovery and as the primary key
/// in the persistent device registry. Two values are equal if they refer
/// to the same physical hardware.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceIdentifier {
    /// USB HID device identified by vendor/product IDs.
    UsbHid {
        /// USB Vendor ID.
        vendor_id: u16,
        /// USB Product ID.
        product_id: u16,
        /// USB serial number, if available.
        serial: Option<String>,
        /// USB topology path (fallback identity).
        usb_path: Option<String>,
    },

    /// Network device identified by MAC address.
    Network {
        /// MAC address (colon-separated hex).
        mac_address: String,
        /// Last known IP address.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_ip: Option<IpAddr>,
        /// mDNS hostname.
        #[serde(skip_serializing_if = "Option::is_none")]
        mdns_hostname: Option<String>,
    },

    /// Philips Hue bridge + individual light.
    HueBridge {
        /// Bridge identifier (stable forever).
        bridge_id: String,
        /// Individual light/group ID.
        light_id: String,
    },

    /// Device managed by `OpenRGB`.
    OpenRgb {
        /// Controller name as reported by `OpenRGB`.
        controller_name: String,
        /// Location string (bus type + address).
        location: String,
    },
}

impl DeviceIdentifier {
    /// Short human-readable string for logging and display.
    #[must_use]
    pub fn display_short(&self) -> String {
        match self {
            Self::UsbHid {
                vendor_id,
                product_id,
                serial,
                ..
            } => match serial {
                Some(s) => format!("USB {vendor_id:04X}:{product_id:04X} [{s}]"),
                None => format!("USB {vendor_id:04X}:{product_id:04X}"),
            },
            Self::Network {
                mac_address,
                mdns_hostname,
                ..
            } => match mdns_hostname {
                Some(h) => format!("{h} ({mac_address})"),
                None => mac_address.clone(),
            },
            Self::HueBridge {
                bridge_id,
                light_id,
                ..
            } => {
                let prefix_len = 8.min(bridge_id.len());
                format!("Hue {}:{light_id}", &bridge_id[..prefix_len])
            }
            Self::OpenRgb {
                controller_name, ..
            } => controller_name.clone(),
        }
    }

    /// Compute a stable fingerprint for deduplication.
    #[must_use]
    pub fn fingerprint(&self) -> DeviceFingerprint {
        match self {
            Self::UsbHid {
                vendor_id,
                product_id,
                serial,
                usb_path,
            } => {
                let key = serial
                    .as_deref()
                    .or(usb_path.as_deref())
                    .unwrap_or("unknown");
                DeviceFingerprint(format!("usb:{vendor_id:04x}:{product_id:04x}:{key}"))
            }
            Self::Network { mac_address, .. } => {
                DeviceFingerprint(format!("net:{}", mac_address.to_lowercase()))
            }
            Self::HueBridge {
                bridge_id,
                light_id,
                ..
            } => DeviceFingerprint(format!("hue:{bridge_id}:{light_id}")),
            Self::OpenRgb {
                controller_name,
                location,
                ..
            } => DeviceFingerprint(format!("orgb:{controller_name}:{location}")),
        }
    }
}

impl fmt::Display for DeviceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_short())
    }
}

// ── DeviceHandle ───────────────────────────────────────────────────────────

/// Opaque handle to a connected device.
///
/// Handles are monotonically increasing within a daemon session and prevent
/// stale connection reuse after reconnects.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHandle {
    id: u64,
    device_id: DeviceIdentifier,
    backend_id: String,
}

/// Global handle ID counter for `DeviceHandle::new`.
static NEXT_DEVICE_HANDLE_ID: AtomicU64 = AtomicU64::new(1);

impl DeviceHandle {
    /// Create a fresh handle for a connected device.
    #[must_use]
    pub fn new(device_id: DeviceIdentifier, backend_id: impl Into<String>) -> Self {
        Self {
            id: NEXT_DEVICE_HANDLE_ID.fetch_add(1, Ordering::Relaxed),
            device_id,
            backend_id: backend_id.into(),
        }
    }

    /// Monotonic connection ID.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Device identifier associated with this connection.
    #[must_use]
    pub fn device_id(&self) -> &DeviceIdentifier {
        &self.device_id
    }

    /// Backend identifier that issued the handle.
    #[must_use]
    pub fn backend_id(&self) -> &str {
        &self.backend_id
    }
}

impl fmt::Display for DeviceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.backend_id, self.id)
    }
}

/// Stable hash key for device deduplication.
///
/// Two devices with the same fingerprint are considered the same physical
/// hardware.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFingerprint(pub String);

impl fmt::Display for DeviceFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
