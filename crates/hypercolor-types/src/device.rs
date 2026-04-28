//! Device identity, capabilities, and state types.
//!
//! These types form the shared vocabulary for device management across the
//! Hypercolor engine. They live in `hypercolor-types` so every crate can
//! reference them without pulling in I/O, async, or backend-specific logic.

use std::borrow::Cow;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ── DeviceId ──────────────────────────────────────────────────────────────

/// Opaque, globally unique device identifier.
///
/// Wraps a `UUIDv7` so identifiers are time-ordered and safe to use as
/// database keys, map keys, and log correlation IDs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
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

    /// Optional device model identifier for compatibility matching.
    #[serde(default)]
    pub model: Option<String>,

    /// Transport / connection type.
    pub connection_type: ConnectionType,

    /// Driver ownership and output routing metadata.
    pub origin: DeviceOrigin,

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

    /// Driver module that owns this device's semantics.
    #[must_use]
    pub fn driver_id(&self) -> &str {
        &self.origin.driver_id
    }

    /// Output backend responsible for writing frames to this device.
    #[must_use]
    pub fn output_backend_id(&self) -> &str {
        &self.origin.backend_id
    }
}

// ── DeviceCapabilities ────────────────────────────────────────────────────

/// Aggregate capability flags for a device.
///
/// These describe what the hardware can do, not what software supports.
/// Backends populate this during discovery / connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFeatures {
    /// Supports tactile/free-spin scroll wheel toggle.
    pub scroll_mode: bool,

    /// Supports Smart Reel auto-switching.
    pub scroll_smart_reel: bool,

    /// Supports scroll acceleration toggle.
    pub scroll_acceleration: bool,
}

/// Generic scroll wheel operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollMode {
    /// Ratcheted tactile scrolling.
    Tactile = 0x00,

    /// Free-spin scrolling.
    FreeSpin = 0x01,
}

impl From<ScrollMode> for u8 {
    fn from(value: ScrollMode) -> Self {
        match value {
            ScrollMode::Tactile => 0x00,
            ScrollMode::FreeSpin => 0x01,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    /// Total addressable LEDs (may differ from zone sum if hardware
    /// reserves slots).
    pub led_count: u32,

    /// Whether the device supports direct per-LED color writes.
    pub supports_direct: bool,

    /// Whether brightness can be controlled independently of color.
    pub supports_brightness: bool,

    /// Whether the device exposes a pixel display surface.
    pub has_display: bool,

    /// Display resolution in pixels, when applicable.
    pub display_resolution: Option<(u32, u32)>,

    /// Maximum sustainable frame rate (0 = unknown / unlimited).
    pub max_fps: u32,

    /// Native color model expected by the device/backend.
    #[serde(default)]
    pub color_space: DeviceColorSpace,

    /// Optional non-lighting device features.
    #[serde(default)]
    pub features: DeviceFeatures,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            led_count: 0,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }
}

// ── DeviceUserSettings ────────────────────────────────────────────────────

/// User-controlled device settings that should survive discovery refreshes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceUserSettings {
    /// Optional user-facing name override.
    pub name: Option<String>,

    /// Whether the device should participate in rendering.
    pub enabled: bool,

    /// User-selected device brightness scalar.
    pub brightness: f32,
}

impl Default for DeviceUserSettings {
    fn default() -> Self {
        Self {
            name: None,
            enabled: true,
            brightness: 1.0,
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

    /// Pixel display (LCD, OLED, etc.).
    Display {
        /// Display width in pixels.
        width: u32,
        /// Display height in pixels.
        height: u32,
        /// Whether the panel is circular.
        circular: bool,
    },

    /// Arbitrary positions defined in the spatial layout.
    Custom,
}

// ── ConnectionType ────────────────────────────────────────────────────────

/// How the device connects to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionType {
    /// USB HID (`PrismRGB`, Nollie).
    Usb,

    /// Local I2C/SMBus device node (`/dev/i2c-*`).
    SmBus,

    /// Network protocols (WLED DDP, E1.31, Hue HTTP).
    Network,

    /// Bluetooth Low Energy.
    Bluetooth,

    /// Out-of-process bridge (gRPC, Unix socket).
    Bridge,
}

// ── Driver Metadata ──────────────────────────────────────────────────────

/// High-level module category used for driver registry introspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriverModuleKind {
    /// Driver owns network discovery, pairing, and output.
    Network,

    /// Driver contributes hardware protocol descriptors to a shared transport.
    Hal,

    /// Driver communicates through an out-of-process bridge service.
    Bridge,

    /// Driver is provided by the host process.
    Host,

    /// Driver exposes virtual or synthetic devices.
    Virtual,
}

/// API-facing transport category for a driver module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriverTransportKind {
    /// IP network transport.
    Network,

    /// USB HID, bulk, serial-over-USB, or vendor USB transport.
    Usb,

    /// Local I2C/SMBus transport.
    Smbus,

    /// MIDI transport.
    Midi,

    /// Host serial transport.
    Serial,

    /// Out-of-process bridge transport.
    Bridge,

    /// In-process or synthetic transport.
    Virtual,

    /// Driver-defined transport category.
    Custom(String),
}

impl From<ConnectionType> for DriverTransportKind {
    fn from(value: ConnectionType) -> Self {
        match value {
            ConnectionType::Usb => Self::Usb,
            ConnectionType::SmBus => Self::Smbus,
            ConnectionType::Network => Self::Network,
            ConnectionType::Bluetooth => Self::Custom("bluetooth".to_owned()),
            ConnectionType::Bridge => Self::Bridge,
        }
    }
}

/// Capability flags exposed by a driver module.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DriverCapabilitySet {
    /// Exposes driver-scoped configuration.
    pub config: bool,

    /// Discovers devices.
    pub discovery: bool,

    /// Supports pairing or authorization flows.
    pub pairing: bool,

    /// Builds an output backend.
    pub output_backend: bool,

    /// Contributes protocols to a shared backend.
    pub protocol_catalog: bool,

    /// Keeps runtime cache state.
    pub runtime_cache: bool,

    /// Stores credentials or authorization material.
    pub credentials: bool,

    /// Provides presentation metadata.
    pub presentation: bool,

    /// Exposes typed dynamic control surfaces.
    #[serde(default)]
    pub controls: bool,
}

impl DriverCapabilitySet {
    /// Return an empty capability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            config: false,
            discovery: false,
            pairing: false,
            output_backend: false,
            protocol_catalog: false,
            runtime_cache: false,
            credentials: false,
            presentation: false,
            controls: false,
        }
    }
}

/// Presentation hint for devices owned by a driver module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClassHint {
    /// Keyboard-like device.
    Keyboard,

    /// Mouse-like device.
    Mouse,

    /// Hub or bridge device.
    Hub,

    /// LED controller.
    Controller,

    /// Light or luminaire.
    Light,

    /// Pixel display surface.
    Display,

    /// Audio-reactive or audio-adjacent device.
    Audio,

    /// Unclassified device.
    Other,
}

/// API and UI presentation metadata for a driver module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DriverPresentation {
    /// Human-readable driver label.
    pub label: String,

    /// Compact label for dense UI surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_label: Option<String>,

    /// Primary RGB accent color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_rgb: Option<[u8; 3]>,

    /// Secondary RGB accent color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_rgb: Option<[u8; 3]>,

    /// Stable icon identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Default device class for devices produced by this driver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_device_class: Option<DeviceClassHint>,
}

/// Stable module descriptor for native and future Wasm driver registries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DriverModuleDescriptor {
    /// Stable driver identifier.
    pub id: String,

    /// Human-readable driver name.
    pub display_name: String,

    /// Optional vendor or organization name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_name: Option<String>,

    /// High-level module category.
    pub module_kind: DriverModuleKind,

    /// Transport categories used by this driver.
    pub transports: Vec<DriverTransportKind>,

    /// Driver capabilities.
    pub capabilities: DriverCapabilitySet,

    /// Version of the driver-facing API schema.
    pub api_schema_version: u32,

    /// Version of this driver's config schema.
    pub config_version: u32,

    /// Whether this driver should be enabled by default.
    pub default_enabled: bool,
}

/// Protocol descriptor contributed by a driver module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DriverProtocolDescriptor {
    /// Driver module that owns this protocol.
    pub driver_id: String,

    /// Stable protocol implementation identifier.
    pub protocol_id: String,

    /// Human-readable protocol or device label.
    pub display_name: String,

    /// USB vendor ID when the protocol maps to a concrete USB device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u16>,

    /// USB product ID when the protocol maps to a concrete USB device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<u16>,

    /// Stable device family identifier.
    pub family_id: String,

    /// Optional model identifier exposed by a driver-specific catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,

    /// Transport category used by this protocol.
    pub transport: DriverTransportKind,

    /// Output backend ID that should route devices using this protocol.
    pub route_backend_id: String,

    /// Optional presentation override for devices using this protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<DriverPresentation>,
}

/// Origin metadata that separates device ownership from output routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceOrigin {
    /// Driver module that owns discovery, semantics, and presentation.
    pub driver_id: String,

    /// Output backend responsible for writing frames.
    pub backend_id: String,

    /// Transport category used by this device.
    pub transport: DriverTransportKind,

    /// Optional protocol implementation selected by the driver/backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_id: Option<String>,
}

impl DeviceOrigin {
    /// Create origin metadata from a high-level connection type.
    #[must_use]
    pub fn native(
        driver_id: impl Into<String>,
        backend_id: impl Into<String>,
        connection_type: ConnectionType,
    ) -> Self {
        Self::new(
            driver_id,
            backend_id,
            DriverTransportKind::from(connection_type),
        )
    }

    /// Create origin metadata without a protocol selection.
    #[must_use]
    pub fn new(
        driver_id: impl Into<String>,
        backend_id: impl Into<String>,
        transport: DriverTransportKind,
    ) -> Self {
        Self {
            driver_id: driver_id.into(),
            backend_id: backend_id.into(),
            transport,
            protocol_id: None,
        }
    }

    /// Attach a protocol identifier to this origin.
    #[must_use]
    pub fn with_protocol_id(mut self, protocol_id: impl Into<String>) -> Self {
        self.protocol_id = Some(protocol_id.into());
        self
    }
}

// ── DeviceFamily ──────────────────────────────────────────────────────────

/// Device family classification for presentation and driver-owned metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceFamily {
    id: Cow<'static, str>,
    name: Cow<'static, str>,
}

impl DeviceFamily {
    /// Build a compile-time family descriptor for built-in driver catalogs.
    #[must_use]
    pub const fn new_static(id: &'static str, name: &'static str) -> Self {
        Self {
            id: Cow::Borrowed(id),
            name: Cow::Borrowed(name),
        }
    }

    /// Build a driver-defined family descriptor.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: Cow::Owned(sanitize_family_id(id.into().as_str())),
            name: Cow::Owned(name.into()),
        }
    }

    /// Build a descriptor from only a display name.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: Cow::Owned(sanitize_family_id(&name)),
            name: Cow::Owned(name),
        }
    }

    /// Human-readable family or vendor name for display purposes.
    #[must_use]
    pub fn vendor_name(&self) -> &str {
        self.name.as_ref()
    }

    /// Stable machine-readable identifier (lowercase, ASCII-safe).
    #[must_use]
    pub fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.id.as_ref())
    }
}

impl fmt::Display for DeviceFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.vendor_name())
    }
}

fn sanitize_family_id(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
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

    /// Green-Red-Blue channel order.
    Grb,

    /// Red-Blue-Green channel order.
    Rbg,

    /// JPEG-compressed pixel data.
    Jpeg,
}

impl fmt::Display for DeviceColorFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rgb => write!(f, "RGB"),
            Self::Rgbw => write!(f, "RGBW"),
            Self::Grb => write!(f, "GRB"),
            Self::Rbg => write!(f, "RBG"),
            Self::Jpeg => write!(f, "JPEG"),
        }
    }
}

/// Pixel payload format for display-capable devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFrameFormat {
    /// JPEG-compressed image bytes.
    Jpeg,

    /// Raw RGB byte triplets, row-major.
    Rgb,
}

impl DisplayFrameFormat {
    /// Convert a display zone color format into a payload format.
    #[must_use]
    pub const fn from_device_color_format(format: DeviceColorFormat) -> Self {
        match format {
            DeviceColorFormat::Rgb => Self::Rgb,
            DeviceColorFormat::Rgbw
            | DeviceColorFormat::Grb
            | DeviceColorFormat::Rbg
            | DeviceColorFormat::Jpeg => Self::Jpeg,
        }
    }
}

impl fmt::Display for DisplayFrameFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jpeg => write!(f, "JPEG"),
            Self::Rgb => write!(f, "RGB"),
        }
    }
}

/// Borrowed display frame payload.
#[derive(Debug, Clone, Copy)]
pub struct DisplayFramePayload<'a> {
    /// Pixel payload format.
    pub format: DisplayFrameFormat,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Pixel or compressed image bytes.
    pub data: &'a [u8],
}

/// Owned display frame payload.
#[derive(Debug, Clone)]
pub struct OwnedDisplayFramePayload {
    /// Pixel payload format.
    pub format: DisplayFrameFormat,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Pixel or compressed image bytes.
    pub data: Arc<Vec<u8>>,
}

impl OwnedDisplayFramePayload {
    /// Create an owned JPEG display payload.
    #[must_use]
    pub fn jpeg(width: u32, height: u32, data: Arc<Vec<u8>>) -> Self {
        Self {
            format: DisplayFrameFormat::Jpeg,
            width,
            height,
            data,
        }
    }

    /// Return a borrowed view of the payload.
    #[must_use]
    pub fn as_borrowed(&self) -> DisplayFramePayload<'_> {
        DisplayFramePayload {
            format: self.format,
            width: self.width,
            height: self.height,
            data: self.data.as_slice(),
        }
    }
}

/// Native device/backend color model used for transport conversion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceColorSpace {
    /// Standard sRGB/RGB byte triplets.
    #[default]
    Rgb,

    /// CIE 1931 xy chromaticity with separate brightness.
    CieXy,
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

    /// Protocol-level error (DDP, E1.31, Hue, etc.).
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

    /// Local `SMBus` slave on one host I2C bus.
    SmBus {
        /// Linux device path for the parent bus (for example, `/dev/i2c-9`).
        bus_path: String,
        /// 7-bit `SMBus` address.
        address: u16,
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

    /// Device managed by an external bridge service.
    Bridge {
        /// Bridge service identifier (for example, `openlinkhub`).
        service: String,
        /// Device ID inside the bridge service.
        device_serial: String,
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
            Self::SmBus { bus_path, address } => {
                format!("SMBus {bus_path} [0x{address:02X}]")
            }
            Self::Network {
                mac_address,
                mdns_hostname,
                ..
            } => match mdns_hostname {
                Some(h) => format!("{h} ({mac_address})"),
                None => mac_address.clone(),
            },
            Self::Bridge {
                service,
                device_serial,
            } => format!("{service}:{device_serial}"),
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
            Self::SmBus { bus_path, address } => {
                DeviceFingerprint(format!("smbus:{bus_path}:{address:02x}"))
            }
            Self::Network { mac_address, .. } => {
                DeviceFingerprint(format!("net:{}", mac_address.to_lowercase()))
            }
            Self::Bridge {
                service,
                device_serial,
                ..
            } => DeviceFingerprint(format!("bridge:{service}:{device_serial}")),
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

impl DeviceFingerprint {
    /// Derive a deterministic [`DeviceId`] from this fingerprint.
    ///
    /// This keeps scanner and backend-side discovery aligned on a stable ID
    /// without requiring shared runtime state.
    #[must_use]
    pub fn stable_device_id(&self) -> DeviceId {
        // Deterministic 128-bit hash encoded as UUIDv8 (custom payload).
        const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
        const FNV_OFFSET_A: u64 = 0xCBF2_9CE4_8422_2325;
        const FNV_OFFSET_B: u64 = 0x8422_2325_CBF2_9CE4;

        let mut hash_a = FNV_OFFSET_A;
        let mut hash_b = FNV_OFFSET_B;
        for byte in self.0.as_bytes() {
            let value = u64::from(*byte);

            hash_a ^= value;
            hash_a = hash_a.wrapping_mul(FNV_PRIME);

            hash_b ^= value.rotate_left(1);
            hash_b = hash_b.wrapping_mul(FNV_PRIME);
        }

        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&hash_a.to_be_bytes());
        bytes[8..].copy_from_slice(&hash_b.to_be_bytes());

        // RFC 9562: set UUIDv8 marker and RFC4122 variant bits.
        bytes[6] = (bytes[6] & 0x0F) | 0x80;
        bytes[8] = (bytes[8] & 0x3F) | 0x80;

        DeviceId::from_uuid(Uuid::from_bytes(bytes))
    }
}

impl fmt::Display for DeviceFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
