//! Driver configuration: manually configured pixel controllers.
//!
//! Pixel controllers do not describe themselves, so every device is an
//! entry in `drivers.pixel.devices`. The schema mirrors what controller
//! setup screens ask for: host, protocol, universe or offset, channel
//! order, and pixel counts per output.

use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::color::ColorOrder;

/// Default E1.31 source name shown by receivers.
pub const DEFAULT_SOURCE_NAME: &str = "Hypercolor";

/// Most devices one driver config may list.
pub const MAX_DEVICES: usize = 256;

/// Longest device id, in bytes.
pub const MAX_DEVICE_ID_LEN: usize = 64;

/// Longest display name, in characters.
pub const MAX_NAME_LEN: usize = 128;

/// Highest configurable per-device frame rate cap.
pub const MAX_FPS_LIMIT: u32 = 1000;

/// Streaming protocol spoken by one pixel controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum PixelProtocol {
    /// ANSI E1.31 Streaming ACN (sACN), unicast.
    E131,
    /// Art-Net 4 ArtDmx, unicast.
    ArtNet,
    /// Distributed Display Protocol.
    Ddp,
}

impl PixelProtocol {
    /// Every supported protocol, in display order.
    pub const ALL: [Self; 3] = [Self::E131, Self::ArtNet, Self::Ddp];

    /// Stable lowercase identifier used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::E131 => "e131",
            Self::ArtNet => "artnet",
            Self::Ddp => "ddp",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::E131 => "E1.31 (sACN)",
            Self::ArtNet => "Art-Net",
            Self::Ddp => "DDP",
        }
    }

    /// Standard UDP port for the protocol.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::E131 => hypercolor_pixel_protocols::e131::E131_PORT,
            Self::ArtNet => hypercolor_pixel_protocols::artnet::ARTNET_PORT,
            Self::Ddp => hypercolor_pixel_protocols::ddp::DDP_PORT,
        }
    }

    /// Whether the protocol carries DMX512 universes.
    #[must_use]
    pub const fn is_dmx(self) -> bool {
        matches!(self, Self::E131 | Self::ArtNet)
    }

    /// Parse a protocol name, accepting common spellings in any case.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "e131" | "e1.31" | "sacn" => Some(Self::E131),
            "artnet" | "art-net" | "art_net" => Some(Self::ArtNet),
            "ddp" => Some(Self::Ddp),
            _ => None,
        }
    }
}

impl fmt::Display for PixelProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for PixelProtocol {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
            .ok_or_else(|| format!("unknown protocol '{value}' (expected e131, artnet, or ddp)"))
    }
}

impl From<PixelProtocol> for String {
    fn from(value: PixelProtocol) -> Self {
        value.as_str().to_owned()
    }
}

/// Pixel controller driver configuration (`[drivers.pixel]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PixelConfig {
    /// Manually configured controllers.
    #[serde(default)]
    pub devices: Vec<PixelDeviceConfig>,

    /// Default E1.31 source name for devices that do not set one.
    #[serde(default = "default_source_name")]
    pub source_name: String,

    /// Default E1.31 priority (0-200) for devices that do not set one.
    #[serde(default = "default_priority")]
    pub priority: u8,
}

impl Default for PixelConfig {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            source_name: default_source_name(),
            priority: default_priority(),
        }
    }
}

impl PixelConfig {
    /// Device config with driver-level E1.31 defaults filled in.
    #[must_use]
    pub fn effective_device(&self, device: &PixelDeviceConfig) -> PixelDeviceConfig {
        let mut effective = device.clone();
        if device.protocol == PixelProtocol::E131 {
            effective.priority.get_or_insert(self.priority);
            effective
                .source_name
                .get_or_insert_with(|| self.source_name.clone());
        }
        effective
    }
}

fn default_source_name() -> String {
    DEFAULT_SOURCE_NAME.to_owned()
}

const fn default_priority() -> u8 {
    hypercolor_pixel_protocols::e131::E131_DEFAULT_PRIORITY
}

/// One manually configured pixel controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelDeviceConfig {
    /// Stable identifier. Device identity derives from it, so renaming
    /// the display name or moving the host keeps layouts intact.
    pub id: String,

    /// Display name; defaults to the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Controller IP address. Unicast only.
    pub host: IpAddr,

    /// Streaming protocol.
    pub protocol: PixelProtocol,

    /// UDP port; defaults to the protocol's standard port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Channel order on the wire.
    #[serde(default)]
    pub color_order: ColorOrder,

    /// Channels per pixel; defaults to the color order's channel count.
    /// Extra channels are sent as zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels_per_pixel: Option<u16>,

    /// Pixel count for a single-output device. Use `segments` instead to
    /// split the device into several outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_count: Option<u32>,

    /// Outputs in channel order. Mutually exclusive with `pixel_count`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<PixelSegmentConfig>,

    /// First universe (E1.31: 1-63999, Art-Net Port-Address: 0-32767).
    /// Defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_universe: Option<u16>,

    /// First DMX channel (1-based) inside `start_universe`. Defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_channel: Option<u16>,

    /// Channels used per universe before wrapping to the next one.
    /// Defaults to the largest count that holds whole pixels (510 for RGB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels_per_universe: Option<u16>,

    /// DDP byte offset of the first channel. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ddp_offset: Option<u32>,

    /// E1.31 priority (0-200); defaults to the driver-level priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,

    /// E1.31 source name; defaults to the driver-level source name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,

    /// E1.31 synchronization universe. When set, every frame ends with a
    /// universe synchronization packet so outputs latch together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_universe: Option<u16>,

    /// E1.31 sender CID; defaults to a UUID derived from the device id so
    /// it stays stable across restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<Uuid>,

    /// Output frame rate cap. Unset or 0 follows the render loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<u32>,
}

/// One output of a pixel controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelSegmentConfig {
    /// Display name; defaults to "Output N".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Pixels on this output.
    pub pixel_count: u32,

    /// Jump to this universe instead of continuing after the previous
    /// output (E1.31 and Art-Net).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_universe: Option<u16>,

    /// First DMX channel (1-based) inside `start_universe`. Requires
    /// `start_universe`; defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_channel: Option<u16>,

    /// Jump to this DDP byte offset instead of continuing after the
    /// previous output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ddp_offset: Option<u32>,
}

impl PixelDeviceConfig {
    /// Display name, falling back to the id.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    /// Destination UDP port.
    #[must_use]
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.protocol.default_port())
    }
}

/// Validation failure for the pixel driver config.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PixelConfigError {
    /// The config lists more devices than the driver accepts.
    #[error("{count} pixel devices configured; the limit is {MAX_DEVICES}")]
    TooManyDevices {
        /// Configured device count.
        count: usize,
    },

    /// A driver-level E1.31 setting is out of range.
    #[error("{0}")]
    Driver(String),

    /// One device entry is invalid.
    #[error("pixel device '{id}': {message}")]
    Device {
        /// Device id as written in config.
        id: String,
        /// What is wrong.
        message: String,
    },

    /// Two devices use the same id.
    #[error("pixel device id '{id}' is used more than once")]
    DuplicateId {
        /// Duplicated id.
        id: String,
    },

    /// Two devices send the same universe to the same receiver.
    #[error(
        "pixel devices '{first}' and '{second}' both send {protocol} universe {universe} to \
         {endpoint}; one sender owns a universe, so make them segments of one device"
    )]
    UniverseConflict {
        /// First device id.
        first: String,
        /// Second device id.
        second: String,
        /// Protocol label.
        protocol: &'static str,
        /// Shared universe.
        universe: u16,
        /// Receiver `host:port`.
        endpoint: String,
    },

    /// Two DDP devices write overlapping bytes of the same receiver.
    #[error("pixel devices '{first}' and '{second}' overlap DDP bytes {start}-{end} at {endpoint}")]
    DdpOverlap {
        /// First device id.
        first: String,
        /// Second device id.
        second: String,
        /// First overlapping byte offset.
        start: u64,
        /// Last overlapping byte offset.
        end: u64,
        /// Receiver `host:port`.
        endpoint: String,
    },
}

impl PixelConfigError {
    pub(crate) fn device(id: &str, message: impl Into<String>) -> Self {
        Self::Device {
            id: id.to_owned(),
            message: message.into(),
        }
    }
}

/// Whether `id` is a valid device id: 1-64 ASCII letters, digits, `-`, `_`.
#[must_use]
pub fn is_valid_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_DEVICE_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Whether `ip` may receive unicast pixel data.
///
/// Rejects unspecified, multicast, and IPv4 limited-broadcast addresses.
/// The driver never sends multicast or broadcast traffic.
#[must_use]
pub fn is_unicast_target(ip: IpAddr) -> bool {
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    match ip {
        IpAddr::V4(ipv4) => !ipv4.is_broadcast(),
        IpAddr::V6(_) => true,
    }
}
