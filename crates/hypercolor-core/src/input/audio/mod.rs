//! Audio input pipeline — FFT analysis, beat detection, feature extraction.
//!
//! This module implements [`InputSource`] for real-time audio analysis, feeding
//! the effect engine with [`AudioData`] snapshots every frame.
//!
//! # Architecture
//!
//! The pipeline is split into pure-computation submodules (no OS dependencies)
//! and an optional hardware integration layer:
//!
//! - [`fft`] — ring buffer, Hann window, r2c FFT, log-frequency resampling
//! - [`features`] — mel filterbank, chromagram, band energy, RMS, smoothing
//! - [`beat`] — spectral flux onset, adaptive threshold, BPM estimation
//!
//! The [`AudioInput`] struct wraps these into a complete [`InputSource`] that
//! can be fed from either a live audio device (via cpal, behind feature gate)
//! or synthetic sample buffers for testing.

pub mod beat;
pub mod features;
pub mod fft;

use crate::input::traits::{InputData, InputSource};
use crate::types::audio::{AudioData, AudioPipelineConfig};

use beat::{BeatDetector, BeatFrame};
use features::{
    ArraySmoother, MelFilterbank, Smoother, band_energies, compute_chromagram, compute_peak,
    compute_rms, spectral_centroid,
};
use fft::{FftPipeline, RingBuffer};

use crate::types::audio::{CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};

// ── AudioAnalyzer ────────────────────────────────────────────────────────

/// Pure audio analysis pipeline — no hardware, no threads, no OS calls.
///
/// Feed it `f32` samples via [`push_samples`](AudioAnalyzer::push_samples),
/// then call [`analyze`](AudioAnalyzer::analyze) to get a complete
/// [`AudioData`] snapshot. This is the core of the audio input module,
/// usable in tests with synthetic data.
pub struct AudioAnalyzer {
    config: AudioPipelineConfig,
    ring: RingBuffer,
    fft: FftPipeline,
    mel: MelFilterbank,
    beat: BeatDetector,

    // Smoothers.
    spectrum_smoother: ArraySmoother,
    mel_smoother: ArraySmoother,
    chroma_smoother: ArraySmoother,
    rms_smoother: Smoother,
    centroid_smoother: Smoother,
    flux_smoother: Smoother,

    /// Accumulated time in seconds (for beat detection timestamps).
    elapsed: f64,

    /// Time-domain buffer for RMS/peak computation (reused each frame).
    time_buf: Vec<f32>,
}

impl AudioAnalyzer {
    /// Create a new analyzer from the given audio config.
    #[must_use]
    pub fn new(config: &AudioPipelineConfig) -> Self {
        let fft_size = config.fft_size;
        // Ring buffer holds 4x the FFT size for comfortable overlap.
        let ring_capacity = fft_size * 4;

        Self {
            config: config.clone(),
            ring: RingBuffer::new(ring_capacity),
            fft: FftPipeline::new(fft_size, 48_000),
            mel: MelFilterbank::new(fft_size, 48_000, MEL_BANDS),
            beat: BeatDetector::new(config.beat_sensitivity),

            spectrum_smoother: ArraySmoother::new(SPECTRUM_BINS, 0.6, 0.15),
            mel_smoother: ArraySmoother::new(MEL_BANDS, 0.6, 0.15),
            chroma_smoother: ArraySmoother::new(CHROMA_BINS, 0.2, 0.05),
            rms_smoother: Smoother::new(0.4, 0.10),
            centroid_smoother: Smoother::new(0.3, 0.08),
            flux_smoother: Smoother::new(0.8, 0.3),

            elapsed: 0.0,
            time_buf: vec![0.0; fft_size],
        }
    }

    /// Push raw audio samples into the ring buffer.
    ///
    /// Samples should be mono, f32, in [-1.0, 1.0]. Gain is applied here.
    pub fn push_samples(&mut self, samples: &[f32]) {
        let gain = self.config.gain;
        if (gain - 1.0).abs() < f32::EPSILON {
            self.ring.push_slice(samples);
        } else {
            // Apply gain. We collect to a temp vec to avoid mutating the input.
            let gained: Vec<f32> = samples.iter().map(|&s| s * gain).collect();
            self.ring.push_slice(&gained);
        }
    }

    /// Run the full analysis pipeline and return an [`AudioData`] snapshot.
    ///
    /// `dt` is the frame delta in seconds (typically ~16ms at 60fps).
    /// Returns `None` if there aren't enough samples yet.
    ///
    /// # Errors
    ///
    /// Returns an error if FFT processing fails.
    pub fn analyze(&mut self, dt: f32) -> anyhow::Result<Option<AudioData>> {
        let fft_size = self.fft.fft_size();

        if self.ring.len() < fft_size {
            return Ok(None);
        }

        self.elapsed += f64::from(dt);

        // Read the latest window from the ring buffer.
        self.ring.read_last(&mut self.time_buf);

        // RMS and peak from the time-domain signal.
        let raw_rms = compute_rms(&self.time_buf);
        let peak = compute_peak(&self.time_buf);

        // Noise gate: if below threshold, decay smoothers and beat state
        // but return silence. This ensures beat pulses decay rather than freeze.
        let rms_db = if raw_rms > 0.0 {
            20.0 * raw_rms.log10()
        } else {
            -100.0
        };
        if rms_db < self.config.noise_floor {
            self.rms_smoother.update(0.0);
            self.centroid_smoother.update(0.0);
            self.flux_smoother.update(0.0);
            // Feed silence to beat detector so pulses decay properly.
            self.beat.update(&BeatFrame {
                bass: 0.0,
                mid: 0.0,
                treble: 0.0,
                spectral_flux: 0.0,
                dt,
                current_time: self.elapsed,
            });
            return Ok(Some(AudioData::silence()));
        }

        // FFT.
        let fft_result = self.fft.process(&self.time_buf)?;

        // Mel bands from raw linear magnitudes.
        let raw_mel = self.mel.apply(&fft_result.raw_magnitudes);

        // Chromagram from raw linear magnitudes.
        let raw_chroma =
            compute_chromagram(&fft_result.raw_magnitudes, self.fft.sample_rate(), fft_size);

        // Band energies from the 200-bin log spectrum.
        let (bass, mid, treble) = band_energies(&fft_result.spectrum);

        // Spectral centroid.
        let raw_centroid = spectral_centroid(&fft_result.spectrum);

        // Smoothing.
        self.spectrum_smoother.update(&fft_result.spectrum);
        self.mel_smoother.update(&raw_mel);
        self.chroma_smoother.update(&raw_chroma);
        self.rms_smoother.update(raw_rms);
        self.centroid_smoother.update(raw_centroid);
        self.flux_smoother.update(fft_result.spectral_flux);

        // Beat detection.
        let beat_state = self.beat.update(&BeatFrame {
            bass,
            mid,
            treble,
            spectral_flux: fft_result.spectral_flux,
            dt,
            current_time: self.elapsed,
        });

        Ok(Some(AudioData {
            spectrum: self.spectrum_smoother.values().to_vec(),
            mel_bands: self.mel_smoother.values().to_vec(),
            chromagram: self.chroma_smoother.values().to_vec(),
            beat_detected: beat_state.beat_detected,
            beat_confidence: beat_state.beat_confidence,
            bpm: beat_state.bpm,
            rms_level: self.rms_smoother.value(),
            peak_level: peak,
            spectral_centroid: self.centroid_smoother.value(),
            spectral_flux: self.flux_smoother.value(),
            onset_detected: beat_state.onset_detected,
        }))
    }

    /// Reset all internal state (e.g. on source change).
    pub fn reset(&mut self) {
        self.beat.reset();
        self.spectrum_smoother.reset();
        self.mel_smoother.reset();
        self.chroma_smoother.reset();
        self.rms_smoother.reset(0.0);
        self.centroid_smoother.reset(0.0);
        self.flux_smoother.reset(0.0);
        self.elapsed = 0.0;
    }
}

// ── AudioInput (InputSource implementation) ──────────────────────────────

/// Audio input source implementing [`InputSource`].
///
/// Wraps [`AudioAnalyzer`] and provides the standard lifecycle methods.
/// In production, a cpal audio stream pushes samples into the analyzer
/// from a callback thread. For testing, call [`push_samples`](AudioInput::push_samples)
/// directly with synthetic data.
pub struct AudioInput {
    name: String,
    running: bool,
    analyzer: AudioAnalyzer,
}

impl AudioInput {
    /// Create a new audio input with the given config.
    #[must_use]
    pub fn new(config: &AudioPipelineConfig) -> Self {
        Self {
            name: "AudioInput".to_owned(),
            running: false,
            analyzer: AudioAnalyzer::new(config),
        }
    }

    /// Create with a custom display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Push raw samples into the analysis pipeline.
    ///
    /// This is the entry point for both cpal callbacks and test harnesses.
    pub fn push_samples(&mut self, samples: &[f32]) {
        self.analyzer.push_samples(samples);
    }

    /// Access the underlying analyzer (for advanced usage / testing).
    #[must_use]
    pub fn analyzer(&self) -> &AudioAnalyzer {
        &self.analyzer
    }
}

impl InputSource for AudioInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.analyzer.reset();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        // Use a fixed dt of ~16ms (60fps). In production this would
        // come from the actual frame timer.
        let dt = 1.0 / 60.0;
        match self.analyzer.analyze(dt)? {
            Some(data) => Ok(InputData::Audio(data)),
            None => Ok(InputData::None),
        }
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
