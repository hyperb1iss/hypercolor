use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Display ─────────────────────────────────────────────────────────────────

/// Bounds for [`DisplayConfig::face_fps_cap`].
pub const FACE_FPS_CAP_MIN: u32 = 15;
pub const FACE_FPS_CAP_MAX: u32 = 60;

/// Device display (LCD face) output settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    /// Upper bound for HTML face rendering on the zone-direct path.
    /// The device transport limit still wins below this cap.
    #[serde(default = "defaults::face_fps_cap")]
    pub face_fps_cap: u32,
}

impl DisplayConfig {
    /// The configured cap clamped to the supported range.
    #[must_use]
    pub fn effective_face_fps_cap(&self) -> u32 {
        self.face_fps_cap.clamp(FACE_FPS_CAP_MIN, FACE_FPS_CAP_MAX)
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            face_fps_cap: defaults::face_fps_cap(),
        }
    }
}
