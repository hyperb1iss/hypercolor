use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::defaults;

// ─── Driver Registry ────────────────────────────────────────────────────────

/// Stable config map for all driver-owned settings.
pub type DriverConfigs = BTreeMap<String, DriverConfigEntry>;

/// Host-owned wrapper around one driver's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DriverConfigEntry {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(flatten)]
    pub settings: BTreeMap<String, serde_json::Value>,
}

impl DriverConfigEntry {
    #[must_use]
    pub fn enabled(settings: BTreeMap<String, serde_json::Value>) -> Self {
        Self {
            enabled: true,
            settings,
        }
    }

    #[must_use]
    pub fn disabled(settings: BTreeMap<String, serde_json::Value>) -> Self {
        Self {
            enabled: false,
            settings,
        }
    }
}

impl Default for DriverConfigEntry {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            settings: BTreeMap::new(),
        }
    }
}

#[must_use]
pub fn default_driver_configs() -> DriverConfigs {
    DriverConfigs::new()
}
