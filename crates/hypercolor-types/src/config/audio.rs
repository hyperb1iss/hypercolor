use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Audio ───────────────────────────────────────────────────────────────────

/// Default `[audio] beat_sensitivity`, shared with
/// [`AudioPipelineConfig::default`](crate::audio::AudioPipelineConfig).
///
/// The value multiplies the beat detector's onset thresholds, so higher
/// means fewer, stronger beats. Below 1.0 the bass onset detector fires on
/// any held bass note, so beat-reactive effects strobe on sustain instead
/// of kicks; 1.2 is where that stops while kicks still land.
pub const DEFAULT_BEAT_SENSITIVITY: f32 = 1.2;

/// Audio capture and FFT analysis settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Beat onset threshold multiplier. Despite the name, higher values make
    /// detection less sensitive: only sharper hits fire a beat.
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
