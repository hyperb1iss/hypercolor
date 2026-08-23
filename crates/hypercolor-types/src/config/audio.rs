use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Audio ───────────────────────────────────────────────────────────────────

/// Audio capture and FFT analysis settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::audio_device")]
    pub device: String,

    #[serde(default = "defaults::fft_size")]
    pub fft_size: u32,

    #[serde(default = "defaults::smoothing")]
    pub smoothing: f32,

    #[serde(default = "defaults::noise_gate")]
    pub noise_gate: f32,

    #[serde(default = "defaults::beat_sensitivity")]
    pub beat_sensitivity: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            device: defaults::audio_device(),
            fft_size: defaults::fft_size(),
            smoothing: defaults::smoothing(),
            noise_gate: defaults::noise_gate(),
            beat_sensitivity: defaults::beat_sensitivity(),
        }
    }
}
