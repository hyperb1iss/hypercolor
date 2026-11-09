//! Yeelight driver configuration (Spec 94 §8.3).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Default music-mode frame rate: Hyperion's Yeelight default of one
/// update every 40 ms, the most deployed data point (Spec 94 §6.2).
/// Yeelight documents no music-mode limit.
pub const DEFAULT_MAX_FPS: u32 = 25;
/// Highest music-mode frame rate a user may configure.
pub const MAX_FPS_LIMIT: u32 = 60;

/// Yeelight backend and discovery configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YeelightConfig {
    /// Addresses searched directly, for networks that filter multicast.
    #[serde(default)]
    pub device_ips: Vec<IpAddr>,

    /// Send the search to the SSDP multicast group on port 1982.
    #[serde(default = "default_multicast_discovery")]
    pub multicast_discovery: bool,

    /// Stream through music mode when the device supports it. Without it
    /// the device's documented quota allows one command per second.
    #[serde(default = "default_music_mode")]
    pub music_mode: bool,

    /// Music-mode frames per second sent to each light.
    #[serde(default = "default_max_fps")]
    pub max_fps: u32,
}

impl Default for YeelightConfig {
    fn default() -> Self {
        Self {
            device_ips: Vec::new(),
            multicast_discovery: default_multicast_discovery(),
            music_mode: default_music_mode(),
            max_fps: default_max_fps(),
        }
    }
}

impl YeelightConfig {
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

const fn default_multicast_discovery() -> bool {
    true
}

const fn default_music_mode() -> bool {
    true
}

const fn default_max_fps() -> u32 {
    DEFAULT_MAX_FPS
}
