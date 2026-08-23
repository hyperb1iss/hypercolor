use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Media ──────────────────────────────────────────────────────────────────

/// User media decoder policy and resource caps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaConfig {
    #[serde(default = "defaults::max_video_producers")]
    pub max_video_producers: u8,

    #[serde(default = "defaults::max_livestream_producers")]
    pub max_livestream_producers: u8,

    #[serde(default)]
    pub stream_private_network_allowlist: Vec<String>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_video_producers: defaults::max_video_producers(),
            max_livestream_producers: defaults::max_livestream_producers(),
            stream_private_network_allowlist: Vec::new(),
        }
    }
}
