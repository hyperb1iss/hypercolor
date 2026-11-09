//! Twinkly driver configuration (Spec 92 §10.3).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Default real-time frame rate: the host's default output cadence. No
/// Twinkly source documents a real-time ceiling (Spec 92 §8.1), so the
/// driver does not invent one below the engine's own rate.
pub const DEFAULT_MAX_FPS: u32 = 60;
/// Highest frame rate a user may configure.
pub const MAX_FPS_LIMIT: u32 = 120;

/// Twinkly backend and discovery configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwinklyConfig {
    /// Addresses probed directly, for networks that filter broadcast.
    #[serde(default)]
    pub device_ips: Vec<IpAddr>,

    /// Send the discovery datagram as a UDP broadcast on the local networks.
    #[serde(default = "default_broadcast_discovery")]
    pub broadcast_discovery: bool,

    /// Real-time frames per second sent to each device.
    #[serde(default = "default_max_fps")]
    pub max_fps: u32,
}

impl Default for TwinklyConfig {
    fn default() -> Self {
        Self {
            device_ips: Vec::new(),
            broadcast_discovery: default_broadcast_discovery(),
            max_fps: default_max_fps(),
        }
    }
}

impl TwinklyConfig {
    /// Check value ranges. Device IPs are validated by the module.
    ///
    /// # Errors
    ///
    /// Returns a description of the first out-of-range field.
    pub fn validate_ranges(&self) -> Result<(), String> {
        if !(1..=MAX_FPS_LIMIT).contains(&self.max_fps) {
            return Err(format!(
                "max_fps must be between 1 and {MAX_FPS_LIMIT}, got {}",
                self.max_fps
            ));
        }
        Ok(())
    }
}

const fn default_broadcast_discovery() -> bool {
    true
}

const fn default_max_fps() -> u32 {
    DEFAULT_MAX_FPS
}
