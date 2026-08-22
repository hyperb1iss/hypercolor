use serde::{Deserialize, Serialize};

use super::defaults;

// ─── D-Bus ───────────────────────────────────────────────────────────────────

/// D-Bus integration settings (Linux desktop integration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbusConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::bus_name")]
    pub bus_name: String,
}

impl Default for DbusConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            bus_name: defaults::bus_name(),
        }
    }
}
