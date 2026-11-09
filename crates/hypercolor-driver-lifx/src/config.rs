//! LIFX driver configuration (Spec 88 §11.3).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::color::{DEFAULT_WHITE_KELVIN, MAX_CONFIG_KELVIN, MIN_CONFIG_KELVIN};

/// Default per-device message budget: the only figure LIFX has published,
/// "Maximum recommended message transmit rate to a device: 20 per second"
/// (legacy LIFX protocol documentation, Spec 88 §9.1).
pub const DEFAULT_MAX_MESSAGES_PER_SECOND: u16 = 20;
/// Highest budget a user may opt into.
pub const MAX_MESSAGES_PER_SECOND_LIMIT: u16 = 200;
/// Longest per-frame transition accepted.
pub const MAX_TRANSITION_MS: u16 = 10_000;

/// LIFX backend and discovery configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifxConfig {
    /// Addresses probed directly on port 56700, for networks that filter
    /// broadcast.
    #[serde(default)]
    pub device_ips: Vec<IpAddr>,

    /// Send discovery as UDP broadcast on the local networks.
    #[serde(default = "default_broadcast_discovery")]
    pub broadcast_discovery: bool,

    /// Per-device message budget the output path stays within.
    #[serde(default = "default_max_messages_per_second")]
    pub max_messages_per_second: u16,

    /// Transition carried by every color message, in milliseconds.
    #[serde(default)]
    pub transition_ms: u16,

    /// Kelvin for desaturated output, clamped to each product's range.
    #[serde(default = "default_white_kelvin")]
    pub white_kelvin: u16,
}

impl Default for LifxConfig {
    fn default() -> Self {
        Self {
            device_ips: Vec::new(),
            broadcast_discovery: default_broadcast_discovery(),
            max_messages_per_second: default_max_messages_per_second(),
            transition_ms: 0,
            white_kelvin: default_white_kelvin(),
        }
    }
}

impl LifxConfig {
    /// Check value ranges. Device IPs are validated by the module.
    ///
    /// # Errors
    ///
    /// Returns a description of the first out-of-range field.
    pub fn validate_ranges(&self) -> Result<(), String> {
        if !(1..=MAX_MESSAGES_PER_SECOND_LIMIT).contains(&self.max_messages_per_second) {
            return Err(format!(
                "max_messages_per_second must be between 1 and {MAX_MESSAGES_PER_SECOND_LIMIT}, got {}",
                self.max_messages_per_second
            ));
        }
        if self.transition_ms > MAX_TRANSITION_MS {
            return Err(format!(
                "transition_ms must be at most {MAX_TRANSITION_MS}, got {}",
                self.transition_ms
            ));
        }
        if !(MIN_CONFIG_KELVIN..=MAX_CONFIG_KELVIN).contains(&self.white_kelvin) {
            return Err(format!(
                "white_kelvin must be between {MIN_CONFIG_KELVIN} and {MAX_CONFIG_KELVIN}, got {}",
                self.white_kelvin
            ));
        }
        Ok(())
    }
}

const fn default_broadcast_discovery() -> bool {
    true
}

const fn default_max_messages_per_second() -> u16 {
    DEFAULT_MAX_MESSAGES_PER_SECOND
}

const fn default_white_kelvin() -> u16 {
    DEFAULT_WHITE_KELVIN
}
