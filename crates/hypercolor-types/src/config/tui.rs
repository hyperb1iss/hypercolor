use serde::{Deserialize, Serialize};

use super::defaults;

// ─── TUI ─────────────────────────────────────────────────────────────────────

/// Terminal UI preferences: theme, frame rate, keybindings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "defaults::tui_theme")]
    pub theme: String,

    #[serde(default = "defaults::preview_fps")]
    pub preview_fps: u32,

    #[serde(default = "defaults::keybindings")]
    pub keybindings: String,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: defaults::tui_theme(),
            preview_fps: defaults::preview_fps(),
            keybindings: defaults::keybindings(),
        }
    }
}
