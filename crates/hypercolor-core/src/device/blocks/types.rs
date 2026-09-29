//! ROLI Blocks types — device models and API message structs.

use serde::{Deserialize, Serialize};

/// ROLI Block hardware variants, identified by serial number prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoliBlockType {
    Lightpad,
    LightpadM,
    LumiKeys,
    Seaboard,
    Live,
    Loop,
    Touch,
    Developer,
    Unknown,
}

impl RoliBlockType {
    /// Human-readable device name.
    #[must_use]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Lightpad => "Lightpad Block",
            Self::LightpadM => "Lightpad Block M",
            Self::LumiKeys => "LUMI Keys",
            Self::Seaboard => "Seaboard Block",
            Self::Live => "Live Block",
            Self::Loop => "Loop Block",
            Self::Touch => "Touch Block",
            Self::Developer => "Developer Control Block",
            Self::Unknown => "ROLI Block",
        }
    }

    /// Parse from blocksd's `block_type` JSON field.
    #[must_use]
    pub fn from_api(s: &str) -> Self {
        match s {
            "lightpad" => Self::Lightpad,
            "lightpad_m" => Self::LightpadM,
            "lumi_keys" => Self::LumiKeys,
            "seaboard" => Self::Seaboard,
            "live" => Self::Live,
            "loop" => Self::Loop,
            "touch" => Self::Touch,
            "developer" => Self::Developer,
            _ => Self::Unknown,
        }
    }
}

// ── API message types ────────────────────────────────────────────────────

/// Device info as reported by blocksd's discover response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[allow(dead_code)]
pub struct BlocksDeviceResponse {
    pub uid: u64,
    pub serial: String,
    pub block_type: String,
    pub name: String,
    pub battery_level: u8,
    pub battery_charging: bool,
    pub grid_width: u32,
    pub grid_height: u32,
    #[serde(default)]
    pub key_count: u32,
    pub firmware_version: Option<String>,
}

/// Discover response from blocksd.
#[derive(Debug, Deserialize)]
pub struct DiscoverResponse {
    pub devices: Vec<BlocksDeviceResponse>,
}

/// Pong response from blocksd.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PongResponse {
    pub version: String,
    pub uptime_seconds: u64,
    pub device_count: u32,
}

/// Frame protocol selected from the daemon's advertised lighting capability.
#[derive(Debug, Clone, Copy)]
pub(super) enum BlocksSurface {
    Grid,
    Keys,
}

impl BlocksSurface {
    pub(super) fn from_device(dev: &BlocksDeviceResponse) -> Option<Self> {
        match (
            RoliBlockType::from_api(&dev.block_type),
            dev.grid_width,
            dev.grid_height,
            dev.key_count,
        ) {
            (RoliBlockType::Lightpad | RoliBlockType::LightpadM, 15, 15, 0) => Some(Self::Grid),
            (RoliBlockType::LumiKeys, 0, 0, 24) => Some(Self::Keys),
            _ => None,
        }
    }

    pub(super) fn metadata_value(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Keys => "keys",
        }
    }

    pub(super) fn from_metadata(value: &str) -> Option<Self> {
        match value {
            "grid" => Some(Self::Grid),
            "keys" => Some(Self::Keys),
            _ => None,
        }
    }
}

// ── Event stream ─────────────────────────────────────────────────────────

/// One server-pushed message on a blocksd event subscription.
///
/// Unknown message types are tolerated so newer daemons can add events
/// without breaking older readers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum BlocksEvent {
    Subscribed {
        events: Vec<String>,
    },
    Touch(BlocksTouch),
    Button(BlocksButton),
    /// The payload is ignored: devices reach this stream through adoption.
    DeviceAdded {},
    DeviceRemoved {
        uid: u64,
    },
    /// Sent before blocksd closes a subscriber that fell behind.
    Error {
        message: String,
    },
    #[serde(other)]
    Other,
}

/// Contact lifecycle reported by blocksd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BlocksTouchAction {
    Start,
    Move,
    End,
}

/// One touch sample. Positions and pressure are normalized to `[0, 1]`;
/// velocities are signed and normalized to `[-1, 1)`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct BlocksTouch {
    pub uid: u64,
    pub action: BlocksTouchAction,
    pub index: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    #[serde(default)]
    pub vx: f32,
    #[serde(default)]
    pub vy: f32,
    #[serde(default)]
    pub vz: f32,
    /// Device clock in milliseconds; older blocksd releases omit it.
    #[serde(default)]
    pub timestamp: Option<u64>,
}

/// Button press or release reported by blocksd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BlocksButtonAction {
    Press,
    Release,
}

/// One control button edge. Older blocksd releases omit button identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct BlocksButton {
    pub uid: u64,
    pub action: BlocksButtonAction,
    #[serde(default)]
    pub button_id: Option<u16>,
    #[serde(default)]
    pub button: Option<String>,
    #[serde(default)]
    pub timestamp: Option<u64>,
}
