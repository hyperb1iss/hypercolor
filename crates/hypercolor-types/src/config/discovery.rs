use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Discovery ───────────────────────────────────────────────────────────────

/// Network device discovery: mDNS, WLED, Hue, and blocksd.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct DiscoveryConfig {
    /// Run startup, hotplug, and periodic background discovery.
    ///
    /// Manual discovery requests remain available when this is disabled.
    #[serde(default = "defaults::bool_true")]
    pub background_enabled: bool,

    #[serde(default = "defaults::bool_true")]
    pub mdns_enabled: bool,

    #[serde(default = "defaults::scan_interval")]
    pub scan_interval_secs: u64,

    /// Enable ROLI Blocks discovery via blocksd bridge.
    #[serde(default = "defaults::bool_true")]
    pub blocks_scan: bool,

    /// Custom socket path for blocksd (empty = auto-detect).
    #[serde(default)]
    pub blocks_socket_path: Option<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            background_enabled: defaults::bool_true(),
            mdns_enabled: defaults::bool_true(),
            scan_interval_secs: defaults::scan_interval(),
            blocks_scan: defaults::bool_true(),
            blocks_socket_path: None,
        }
    }
}
