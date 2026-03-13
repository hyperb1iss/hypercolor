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
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BlocksDeviceResponse {
    pub uid: u32,
    pub serial: String,
    pub block_type: String,
    pub name: String,
    pub battery_level: u8,
    pub battery_charging: bool,
    pub grid_width: u32,
    pub grid_height: u32,
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
