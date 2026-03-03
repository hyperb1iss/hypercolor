//! Audio analysis data types — spectrum, beats, features.
//!
//! [`AudioData`] is the single source of truth for all audio-reactive rendering.
//! Computed once per DSP frame on the audio thread, consumed by both the Servo
//! (Lightscript) and wgpu (native shader) paths.

use serde::{Deserialize, Serialize};

// ─── Constants ────────────────────────────────────────────────────────

/// Number of logarithmically-spaced frequency bins in the spectrum (20 Hz – 20 kHz).
pub const SPECTRUM_BINS: usize = 200;

/// Number of mel-spaced frequency bands.
pub const MEL_BANDS: usize = 24;

/// Number of pitch-class bins in the chromagram (C through B).
pub const CHROMA_BINS: usize = 12;

/// Bass range: bins 0–39 (20–250 Hz).
const BASS_END: usize = 40;

/// Mid range: bins 40–129 (250–4000 Hz).
const MID_END: usize = 130;

// ─── AudioData ────────────────────────────────────────────────────────

/// Complete audio analysis snapshot, produced once per DSP frame.
///
/// All level/energy fields are normalized to 0.0–1.0 unless otherwise noted.
/// When no audio is playing, use [`AudioData::silence`] for a zero-filled default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioData {
    /// 200 logarithmically-spaced frequency bins (normalized 0.0–1.0).
    pub spectrum: Vec<f32>,

    /// 24 mel-spaced frequency bands.
    pub mel_bands: Vec<f32>,

    /// 12 pitch-class energy bins \[C, C#, D, … B\] (max bin = 1.0).
    pub chromagram: Vec<f32>,

    /// True on the frame a beat onset is detected.
    pub beat_detected: bool,

    /// Confidence in the current tempo estimate (0.0–1.0).
    pub beat_confidence: f32,

    /// Estimated tempo in beats per minute.
    pub bpm: f32,

    /// Overall RMS audio level (0.0–1.0).
    pub rms_level: f32,

    /// Peak sample magnitude in the current frame (0.0–1.0).
    pub peak_level: f32,

    /// Spectral centroid — brightness (0.0–1.0).
    pub spectral_centroid: f32,

    /// Spectral flux — rate of spectral change between frames (0.0–1.0).
    pub spectral_flux: f32,

    /// True when a transient onset (not necessarily beat-aligned) is detected.
    pub onset_detected: bool,
}

impl AudioData {
    /// Returns a zero-filled silence state suitable for "no audio" frames.
    #[must_use]
    pub fn silence() -> Self {
        Self {
            spectrum: vec![0.0; SPECTRUM_BINS],
            mel_bands: vec![0.0; MEL_BANDS],
            chromagram: vec![0.0; CHROMA_BINS],
            beat_detected: false,
            beat_confidence: 0.0,
            bpm: 0.0,
            rms_level: 0.0,
            peak_level: 0.0,
            spectral_centroid: 0.0,
            spectral_flux: 0.0,
            onset_detected: false,
        }
    }

    /// Average energy of the bass range (bins 0–39, 20–250 Hz).
    ///
    /// Returns 0.0 if the spectrum is empty or shorter than the bass range.
    #[must_use]
    pub fn bass(&self) -> f32 {
        band_average(&self.spectrum, 0, BASS_END)
    }

    /// Average energy of the mid range (bins 40–129, 250–4000 Hz).
    ///
    /// Returns 0.0 if the spectrum is shorter than the mid range.
    #[must_use]
    pub fn mid(&self) -> f32 {
        band_average(&self.spectrum, BASS_END, MID_END)
    }

    /// Average energy of the treble range (bins 130–199, 4000–20 000 Hz).
    ///
    /// Returns 0.0 if the spectrum is shorter than the treble range.
    #[must_use]
    pub fn treble(&self) -> f32 {
        band_average(&self.spectrum, MID_END, self.spectrum.len())
    }
}

/// Computes the arithmetic mean of `spectrum[start..end]`, returning 0.0 for empty slices.
fn band_average(spectrum: &[f32], start: usize, end: usize) -> f32 {
    let end = end.min(spectrum.len());
    let start = start.min(end);
    let slice = &spectrum[start..end];
    if slice.is_empty() {
        return 0.0;
    }
    // SPECTRUM_BINS is 200 — well within f32 mantissa range (2^23).
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let count = slice.len() as f32;
    slice.iter().sum::<f32>() / count
}

// ─── AudioSourceType ──────────────────────────────────────────────────

/// Describes how the audio capture source is selected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioSourceType {
    /// Auto-detect system audio output monitor (loopback).
    #[default]
    SystemMonitor,
    /// A specific PulseAudio/PipeWire source or WASAPI device by name.
    Named(String),
    /// Default microphone input device.
    Microphone,
    /// No audio input — effects receive silence.
    None,
}

// ─── AudioPipelineConfig ──────────────────────────────────────────────

/// DSP pipeline configuration for the audio analysis engine.
///
/// Controls FFT size, smoothing, gain, noise floor, and beat sensitivity.
/// This is the *pipeline-tuning* config. For the TOML user-facing `[audio]`
/// section (enable/disable, device selection), see
/// [`config::AudioConfig`](crate::config::AudioConfig).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioPipelineConfig {
    /// Audio capture source selection.
    pub source: AudioSourceType,

    /// Primary FFT window size (256, 512, 1024, 2048, or 4096).
    pub fft_size: usize,

    /// Asymmetric EMA smoothing factor for falling edges (0.0–1.0).
    pub smoothing: f32,

    /// Input gain multiplier (0.1–5.0, 1.0 = unity).
    pub gain: f32,

    /// Noise gate threshold in dB (signals below this are treated as silence).
    pub noise_floor: f32,

    /// Beat onset sensitivity multiplier (lower = more sensitive).
    pub beat_sensitivity: f32,
}

impl Default for AudioPipelineConfig {
    fn default() -> Self {
        Self {
            source: AudioSourceType::default(),
            fft_size: 1024,
            smoothing: 0.15,
            gain: 1.0,
            noise_floor: -60.0,
            beat_sensitivity: 1.5,
        }
    }
}
