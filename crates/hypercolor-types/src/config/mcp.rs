use serde::{Deserialize, Serialize};

use super::defaults;

// ─── MCP ─────────────────────────────────────────────────────────────────────

/// Model Context Protocol server settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default = "defaults::bool_false")]
    pub enabled: bool,

    #[serde(default = "defaults::mcp_base_path")]
    pub base_path: String,

    #[serde(default = "defaults::bool_true")]
    pub stateful_mode: bool,

    #[serde(default = "defaults::bool_false")]
    pub json_response: bool,

    #[serde(default = "defaults::sse_keep_alive_secs")]
    pub sse_keep_alive_secs: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_false(),
            base_path: defaults::mcp_base_path(),
            stateful_mode: defaults::bool_true(),
            json_response: defaults::bool_false(),
            sse_keep_alive_secs: defaults::sse_keep_alive_secs(),
        }
    }
}
