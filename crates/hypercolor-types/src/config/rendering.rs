use serde::{Deserialize, Serialize};

use super::{ServoGpuImportMode, defaults};

// ─── Rendering ───────────────────────────────────────────────────────────────

/// Rendering-path feature switches and import policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingConfig {
    pub servo_gpu_import: ServoGpuImportConfig,
}

/// Linux Servo GPU import policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServoGpuImportConfig {
    #[serde(default = "defaults::servo_gpu_import_mode")]
    pub mode: ServoGpuImportMode,
}
