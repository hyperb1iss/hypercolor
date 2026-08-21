use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Govee ──────────────────────────────────────────────────────────────────

/// Global Govee backend settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoveeConfig {
    /// IPs that are always probed during Govee LAN discovery.
    #[serde(default)]
    pub known_ips: Vec<IpAddr>,

    /// Device-level power-off on backend disconnect.
    #[serde(default)]
    pub power_off_on_disconnect: bool,

    /// Maximum whole-device LAN state command rate.
    #[serde(default = "defaults::govee_lan_state_fps")]
    pub lan_state_fps: u32,

    /// Maximum validated Razer/Desktop streaming frame rate.
    #[serde(default = "defaults::govee_razer_fps")]
    pub razer_fps: u32,
}

impl Default for GoveeConfig {
    fn default() -> Self {
        Self {
            known_ips: Vec::new(),
            power_off_on_disconnect: false,
            lan_state_fps: defaults::govee_lan_state_fps(),
            razer_fps: defaults::govee_razer_fps(),
        }
    }
}
