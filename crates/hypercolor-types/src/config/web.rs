use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Web UI ──────────────────────────────────────────────────────────────────

/// Embedded web UI and WebSocket preview server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default)]
    pub open_browser: bool,

    #[serde(default)]
    pub cors_origins: Vec<String>,

    #[serde(default = "defaults::websocket_fps")]
    pub websocket_fps: u32,

    #[serde(default = "defaults::interactive_preview_resource_bytes")]
    pub interactive_preview_resource_bytes: u64,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            open_browser: false,
            cors_origins: Vec::new(),
            websocket_fps: defaults::websocket_fps(),
            interactive_preview_resource_bytes: defaults::interactive_preview_resource_bytes(),
        }
    }
}
