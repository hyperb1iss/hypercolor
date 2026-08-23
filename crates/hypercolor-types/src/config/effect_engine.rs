use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Effect Engine ───────────────────────────────────────────────────────────

/// Renderer selection, hot-reload, and effect directory config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectEngineConfig {
    #[serde(default = "defaults::auto_string")]
    pub preferred_renderer: String,

    #[serde(default = "defaults::bool_true")]
    pub servo_enabled: bool,

    #[serde(default = "defaults::auto_string")]
    pub wgpu_backend: String,

    #[serde(default = "defaults::compositor_acceleration_mode")]
    pub compositor_acceleration_mode: RenderAccelerationMode,

    #[serde(default)]
    pub effect_error_fallback: EffectErrorFallbackPolicy,

    #[serde(default)]
    pub extra_effect_dirs: Vec<PathBuf>,

    #[serde(default = "defaults::bool_true")]
    pub watch_effects: bool,

    #[serde(default = "defaults::bool_true")]
    pub watch_config: bool,
}

impl Default for EffectEngineConfig {
    fn default() -> Self {
        Self {
            preferred_renderer: defaults::auto_string(),
            servo_enabled: defaults::bool_true(),
            wgpu_backend: defaults::auto_string(),
            compositor_acceleration_mode: defaults::compositor_acceleration_mode(),
            effect_error_fallback: EffectErrorFallbackPolicy::default(),
            extra_effect_dirs: Vec::new(),
            watch_effects: defaults::bool_true(),
            watch_config: defaults::bool_true(),
        }
    }
}

/// Preferred scene compositor acceleration path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderAccelerationMode {
    /// Always use the CPU path.
    Cpu,
    /// Prefer GPU acceleration when available, otherwise fall back safely.
    Auto,
    /// Require the GPU acceleration lane.
    Gpu,
}

/// Servo framebuffer import policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServoGpuImportMode {
    /// Never attempt Servo GPU framebuffer import.
    Off,
    /// Attempt import when startup capabilities indicate it can work.
    #[default]
    Auto,
    /// Require import and report frame errors instead of silent CPU fallback.
    On,
}

/// Daemon response when a live effect emits an
/// [`crate::event::HypercolorEvent::EffectError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectErrorFallbackPolicy {
    /// Leave the failing assignment in place and surface the error only.
    #[default]
    None,
    /// Clear every active render-zone assignment using the failing effect.
    ClearZones,
}

impl EffectErrorFallbackPolicy {
    #[must_use]
    pub const fn event_label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ClearZones => Some("clear_zones"),
        }
    }
}
