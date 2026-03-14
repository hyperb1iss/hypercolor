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
#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig};
#[cfg(target_os = "linux")]
use libpulse_binding as pulse;

use crate::input::traits::{InputData, InputSource};
use crate::types::audio::{AudioData, AudioPipelineConfig, AudioSourceType};

use beat::{BeatDetector, BeatFrame};
use features::{
    ArraySmoother, MelFilterbank, Smoother, band_energies, compute_chromagram, compute_peak,
    compute_rms, spectral_centroid,
};
use fft::{FftPipeline, RingBuffer};

#[cfg(target_os = "linux")]
use self::linux as linux_audio;
use crate::types::audio::{CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};

const DEFAULT_AUDIO_FRAME_DT: f32 = 1.0 / 60.0;

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
        Self::with_sample_rate(config, 48_000)
    }

    /// Create a new analyzer from the given audio config and runtime sample rate.
    #[must_use]
    pub fn with_sample_rate(config: &AudioPipelineConfig, sample_rate_hz: u32) -> Self {
        let fft_size = config.fft_size;
        // Ring buffer holds 4x the FFT size for comfortable overlap.
        let ring_capacity = fft_size * 4;

        Self {
            config: config.clone(),
            ring: RingBuffer::new(ring_capacity),
            fft: FftPipeline::new(fft_size, sample_rate_hz),
            mel: MelFilterbank::new(fft_size, sample_rate_hz, MEL_BANDS),
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
        let sample_rate = self.fft.sample_rate();

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
        let raw_mel = self.mel.apply(fft_result.raw_magnitudes);

        // Chromagram from raw linear magnitudes.
        let raw_chroma = compute_chromagram(fft_result.raw_magnitudes, sample_rate, fft_size);

        // Band energies from the 200-bin log spectrum.
        let (bass, mid, treble) = band_energies(fft_result.spectrum);

        // Spectral centroid.
        let raw_centroid = spectral_centroid(fft_result.spectrum);

        // Smoothing.
        self.spectrum_smoother.update(fft_result.spectrum);
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
            beat_phase: self.beat.phase(),
            beat_pulse: beat_state.beat_pulse,
            bpm: beat_state.bpm,
            rms_level: self.rms_smoother.value(),
            peak_level: peak,
            spectral_centroid: self.centroid_smoother.value(),
            spectral_flux: self.flux_smoother.value(),
            onset_detected: beat_state.onset_detected,
            onset_pulse: beat_state.onset_pulse,
        }))
    }

    /// Reset all internal state (e.g. on source change).
    pub fn reset(&mut self) {
        let config = self.config.clone();
        let sample_rate = self.fft.sample_rate();
        *self = Self::with_sample_rate(&config, sample_rate);
    }

    /// Current hardware sample rate used by the analyzer.
    #[must_use]
    pub fn sample_rate_hz(&self) -> u32 {
        self.fft.sample_rate()
    }

    /// Replace the pipeline config while preserving the current hardware sample rate.
    pub fn reconfigure(&mut self, config: &AudioPipelineConfig) {
        let sample_rate = self.fft.sample_rate();
        *self = Self::with_sample_rate(config, sample_rate);
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
    host: cpal::Host,
    name: String,
    running: bool,
    capture_active: bool,
    config: AudioPipelineConfig,
    analyzer: Arc<Mutex<AudioAnalyzer>>,
    capture: Option<CaptureHandle>,
    parked_source: Option<AudioSourceType>,
    degraded_to_silence: bool,
}

impl AudioInput {
    /// Create a new audio input with the given config.
    #[must_use]
    pub fn new(config: &AudioPipelineConfig) -> Self {
        Self {
            host: cpal::default_host(),
            name: "AudioInput".to_owned(),
            running: false,
            capture_active: false,
            config: config.clone(),
            analyzer: Arc::new(Mutex::new(AudioAnalyzer::new(config))),
            capture: None,
            parked_source: None,
            degraded_to_silence: false,
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
        if let Ok(mut analyzer) = self.analyzer.lock() {
            analyzer.push_samples(samples);
        }
    }

    /// Access the underlying analyzer (for advanced usage / testing).
    pub fn analyzer(&self) -> std::sync::MutexGuard<'_, AudioAnalyzer> {
        self.analyzer
            .lock()
            .expect("audio analyzer mutex should not be poisoned")
    }

    /// Set whether the capture stream should actively pull from hardware.
    ///
    /// This keeps the input source registered while allowing the render loop to
    /// demand-gate live audio capture based on the active effect.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream cannot be resumed after activation.
    pub fn set_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.set_capture_active_state(active)
    }

    fn sample_with_dt(&mut self, dt: f32) -> anyhow::Result<InputData> {
        if !self.running {
            return Ok(InputData::None);
        }
        if !self.capture_active {
            return Ok(InputData::Audio(AudioData::silence()));
        }

        let dt = if dt.is_finite() && dt > 0.0 {
            dt
        } else {
            DEFAULT_AUDIO_FRAME_DT
        };
        let mut analyzer = self
            .analyzer
            .lock()
            .map_err(|_| anyhow!("audio analyzer mutex poisoned"))?;
        match analyzer.analyze(dt)? {
            Some(data) => Ok(InputData::Audio(data)),
            None if self.degraded_to_silence
                || matches!(self.config.source, AudioSourceType::None) =>
            {
                Ok(InputData::Audio(AudioData::silence()))
            }
            None => Ok(InputData::None),
        }
    }

    fn apply_analyzer_config(&mut self, config: &AudioPipelineConfig) {
        if let Ok(mut analyzer) = self.analyzer.lock() {
            analyzer.reconfigure(config);
        } else {
            self.analyzer = Arc::new(Mutex::new(AudioAnalyzer::new(config)));
        }
    }

    fn start_stream_for_config(
        &self,
        config: &AudioPipelineConfig,
    ) -> anyhow::Result<(Arc<Mutex<AudioAnalyzer>>, Option<CaptureHandle>)> {
        let analyzer = Arc::new(Mutex::new(AudioAnalyzer::new(config)));
        if matches!(config.source, AudioSourceType::None) {
            return Ok((analyzer, None));
        }

        let capture = build_capture_stream(&self.host, config, Arc::clone(&analyzer))?;

        Ok((analyzer, Some(capture)))
    }

    fn reset_analyzer(&mut self) {
        if let Ok(mut analyzer) = self.analyzer.lock() {
            analyzer.reset();
        } else {
            self.analyzer = Arc::new(Mutex::new(AudioAnalyzer::new(&self.config)));
        }
    }

    fn pause_capture_stream(&mut self) {
        match self.capture.as_mut() {
            Some(CaptureHandle::Cpal(stream)) => {
                if let Err(error) = stream.pause() {
                    tracing::warn!(
                        input = %self.name,
                        source = ?self.config.source,
                        %error,
                        "Failed to pause audio capture stream"
                    );
                } else {
                    tracing::info!(
                        input = %self.name,
                        source = ?self.config.source,
                        "Audio capture stream paused"
                    );
                }
            }
            #[cfg(target_os = "linux")]
            Some(CaptureHandle::LinuxPulse(_)) => {
                if let Some(CaptureHandle::LinuxPulse(capture)) = self.capture.take() {
                    drop(capture);
                }
                tracing::info!(
                    input = %self.name,
                    source = ?self.config.source,
                    "Audio capture stream stopped while inactive"
                );
            }
            None => {}
        }

        self.degraded_to_silence = false;
        self.reset_analyzer();
    }

    fn drop_capture_stream(&mut self, reason: &'static str) {
        let stream_source = self
            .parked_source
            .clone()
            .unwrap_or_else(|| self.config.source.clone());
        let previous_capture = self.capture.take();
        self.parked_source = None;

        if previous_capture.is_none() {
            tracing::debug!(
                input = %self.name,
                source = ?stream_source,
                reason,
                "No audio capture stream to drop"
            );
            return;
        }

        drop(previous_capture);
        tracing::info!(
            input = %self.name,
            source = ?stream_source,
            reason,
            "Dropped audio capture stream"
        );
    }

    fn start_capture_stream(&mut self) -> anyhow::Result<()> {
        if matches!(self.config.source, AudioSourceType::None) {
            self.degraded_to_silence = false;
            return Ok(());
        }

        if let Some(capture) = &self.capture {
            match capture {
                CaptureHandle::Cpal(stream) => {
                    stream
                        .play()
                        .context("failed to resume audio capture stream")?;
                    self.degraded_to_silence = false;
                    tracing::info!(
                        input = %self.name,
                        source = ?self.config.source,
                        "Audio capture stream resumed"
                    );
                    return Ok(());
                }
                #[cfg(target_os = "linux")]
                CaptureHandle::LinuxPulse(pulse_capture) => {
                    let _ = pulse_capture;
                    self.degraded_to_silence = false;
                    return Ok(());
                }
            }
        }

        match build_capture_stream(&self.host, &self.config, Arc::clone(&self.analyzer)) {
            Ok(capture) => {
                tracing::info!(
                    input = %self.name,
                    source = ?self.config.source,
                    "Audio capture stream started"
                );
                self.degraded_to_silence = false;
                self.capture = Some(capture);
            }
            Err(error) => {
                self.degraded_to_silence = true;
                tracing::warn!(
                    input = %self.name,
                    source = ?self.config.source,
                    %error,
                    "Audio capture unavailable; LightScript audio input will fall back to silence"
                );
            }
        }

        Ok(())
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
            if active && self.running && self.capture.is_none() {
                self.start_capture_stream()?;
            }
            return Ok(());
        }

        self.capture_active = active;

        if !self.running {
            return Ok(());
        }

        if active {
            self.start_capture_stream()
        } else {
            self.pause_capture_stream();
            Ok(())
        }
    }

    /// Apply a runtime audio config change without rebuilding the whole input manager.
    ///
    /// Disabling audio parks any existing hardware stream so a plain toggle can
    /// resume it without touching the native backend. Switching between live
    /// sources still opens the replacement stream before dropping the previous
    /// one so the capture path does not go completely dark in between.
    ///
    /// # Errors
    ///
    /// Returns an error if the new stream cannot be created or started.
    pub fn reconfigure_live(
        &mut self,
        config: &AudioPipelineConfig,
        name: impl Into<String>,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        let next_name = name.into();
        let previous_source = self.config.source.clone();
        let effective_capture_active =
            capture_active && !matches!(config.source, AudioSourceType::None);
        let source_changed = previous_source != config.source;

        self.name = next_name;
        self.config = config.clone();
        self.running = true;

        if !source_changed {
            self.degraded_to_silence = false;
            self.apply_analyzer_config(config);
            self.set_capture_active_state(effective_capture_active)?;
            tracing::info!(
                input = %self.name,
                source = ?self.config.source,
                capture_active = effective_capture_active,
                "Live audio config updated without reopening capture stream"
            );
            return Ok(());
        }

        if matches!(config.source, AudioSourceType::None) {
            if !matches!(previous_source, AudioSourceType::None) && self.capture.is_some() {
                self.parked_source = Some(previous_source.clone());
            }
            self.set_capture_active_state(false)?;
            self.degraded_to_silence = false;
            self.apply_analyzer_config(config);
            tracing::info!(
                input = %self.name,
                previous_source = ?previous_source,
                parked_source = ?self.parked_source,
                source = ?self.config.source,
                "Live audio input disabled; parked existing capture stream for fast resume"
            );
            return Ok(());
        }

        if matches!(previous_source, AudioSourceType::None)
            && self.capture.is_some()
            && self.parked_source.as_ref() == Some(&config.source)
        {
            self.parked_source = None;
            self.degraded_to_silence = false;
            self.apply_analyzer_config(config);
            self.set_capture_active_state(effective_capture_active)?;
            tracing::info!(
                input = %self.name,
                previous_source = ?previous_source,
                source = ?self.config.source,
                capture_active = effective_capture_active,
                "Live audio input resumed parked capture stream"
            );
            return Ok(());
        }

        if effective_capture_active {
            let (next_analyzer, next_capture) = self.start_stream_for_config(config)?;
            let previous_capture = self.capture.take();
            self.parked_source = None;
            self.analyzer = next_analyzer;
            self.capture = next_capture;
            self.degraded_to_silence = false;
            drop(previous_capture);
        } else {
            let previous_capture = self.capture.take();
            self.parked_source = None;
            self.analyzer = Arc::new(Mutex::new(AudioAnalyzer::new(config)));
            self.degraded_to_silence = false;
            drop(previous_capture);
        }

        self.set_capture_active_state(effective_capture_active)?;

        tracing::info!(
            input = %self.name,
            previous_source = ?previous_source,
            source = ?self.config.source,
            capture_active = effective_capture_active,
            "Live audio capture source switched"
        );

        Ok(())
    }
}

impl InputSource for AudioInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }

        self.running = true;
        self.degraded_to_silence = false;
        self.capture = None;

        if matches!(self.config.source, AudioSourceType::None) {
            return Ok(());
        }

        if !self.capture_active {
            tracing::debug!(
                input = %self.name,
                source = ?self.config.source,
                "Audio input armed but idle until an audio-reactive effect requests capture"
            );
            return Ok(());
        }

        self.start_capture_stream()
    }

    fn stop(&mut self) {
        self.drop_capture_stream("input stopped");
        self.running = false;
        self.capture_active = false;
        self.degraded_to_silence = false;
        self.reset_analyzer();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.sample_with_dt(DEFAULT_AUDIO_FRAME_DT)
    }

    fn sample_with_delta_secs(&mut self, delta_secs: f32) -> anyhow::Result<InputData> {
        self.sample_with_dt(delta_secs)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_audio_source(&self) -> bool {
        true
    }

    fn reconfigure_audio(
        &mut self,
        config: &AudioPipelineConfig,
        name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        self.reconfigure_live(config, name, capture_active)
    }

    fn set_audio_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.set_capture_active_state(active)
    }
}

fn build_capture_stream(
    host: &cpal::Host,
    config: &AudioPipelineConfig,
    analyzer: Arc<Mutex<AudioAnalyzer>>,
) -> anyhow::Result<CaptureHandle> {
    let selected = select_input_device(host, &config.source)?;
    match selected {
        SelectedInputDevice::Cpal {
            device,
            display_name,
        } => {
            let supported_config = device.default_input_config().with_context(|| {
                format!("failed to get default input config for '{display_name}'")
            })?;
            reconfigure_analyzer(&analyzer, config, &supported_config);
            let stream_config: cpal::StreamConfig = supported_config.config();
            let channels = usize::from(stream_config.channels.max(1));
            let sample_format = supported_config.sample_format();
            let stream = match sample_format {
                SampleFormat::I8 => {
                    build_stream::<i8>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::I16 => {
                    build_stream::<i16>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::I24 => build_stream::<cpal::I24>(
                    &device,
                    &stream_config,
                    channels,
                    analyzer,
                    &display_name,
                ),
                SampleFormat::I32 => {
                    build_stream::<i32>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::I64 => {
                    build_stream::<i64>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::U8 => {
                    build_stream::<u8>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::U16 => {
                    build_stream::<u16>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::U24 => build_stream::<cpal::U24>(
                    &device,
                    &stream_config,
                    channels,
                    analyzer,
                    &display_name,
                ),
                SampleFormat::U32 => {
                    build_stream::<u32>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::U64 => {
                    build_stream::<u64>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::F32 => {
                    build_stream::<f32>(&device, &stream_config, channels, analyzer, &display_name)
                }
                SampleFormat::F64 => {
                    build_stream::<f64>(&device, &stream_config, channels, analyzer, &display_name)
                }
                sample_format => Err(anyhow!("unsupported audio sample format: {sample_format}")),
            }?;
            stream
                .play()
                .context("failed to start audio capture stream")?;

            tracing::info!(
                source = ?config.source,
                device = %display_name,
                sample_rate_hz = supported_config.sample_rate(),
                channels,
                sample_format = ?sample_format,
                "Audio capture stream configured"
            );

            Ok(CaptureHandle::Cpal(stream))
        }
        #[cfg(target_os = "linux")]
        SelectedInputDevice::LinuxPulse {
            source_name,
            display_name,
        } => build_linux_pulse_capture_stream(&source_name, &display_name, analyzer, config),
    }
}

fn reconfigure_analyzer(
    analyzer: &Arc<Mutex<AudioAnalyzer>>,
    config: &AudioPipelineConfig,
    supported_config: &SupportedStreamConfig,
) {
    if let Ok(mut guard) = analyzer.lock() {
        *guard = AudioAnalyzer::with_sample_rate(config, supported_config.sample_rate());
    }
}

fn select_input_device(
    host: &cpal::Host,
    source: &AudioSourceType,
) -> anyhow::Result<SelectedInputDevice> {
    match source {
        AudioSourceType::None => Err(anyhow!("audio source is disabled")),
        AudioSourceType::Named(name) => {
            #[cfg(target_os = "linux")]
            if linux_audio::pulse_source_exists(name)? {
                return Ok(find_linux_pulse_capture_device(name));
            }

            let device = find_named_input_device(host, name)?
                .ok_or_else(|| anyhow!("audio input device '{name}' not found"))?;
            Ok(SelectedInputDevice::new(device))
        }
        AudioSourceType::SystemMonitor => {
            #[cfg(target_os = "linux")]
            if let Some(selected) = find_linux_system_monitor_input_device(host)? {
                return Ok(selected);
            }

            let device = find_monitor_input_device(host)?
                .or_else(|| host.default_input_device())
                .ok_or_else(|| anyhow!("no input device available for system monitor capture"))?;
            Ok(SelectedInputDevice::new(device))
        }
        AudioSourceType::Microphone => {
            let device = find_microphone_input_device(host)?
                .or_else(|| host.default_input_device())
                .ok_or_else(|| anyhow!("no microphone input device available"))?;
            Ok(SelectedInputDevice::new(device))
        }
    }
}

fn find_named_input_device(host: &cpal::Host, name: &str) -> anyhow::Result<Option<cpal::Device>> {
    let wanted = name.trim().to_ascii_lowercase();
    let mut partial_match = None;

    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        let Ok(description) = device.description() else {
            continue;
        };
        let normalized = description.name().trim().to_ascii_lowercase();
        let driver_name = description
            .driver()
            .map(|driver| driver.trim().to_ascii_lowercase());
        let exact_match = normalized == wanted
            || driver_name
                .as_deref()
                .is_some_and(|driver| driver == wanted)
            || description
                .extended()
                .iter()
                .any(|line| line.trim().eq_ignore_ascii_case(name));
        if exact_match {
            return Ok(Some(device));
        }
        let partial_match_found = normalized.contains(&wanted)
            || driver_name
                .as_deref()
                .is_some_and(|driver| driver.contains(&wanted))
            || description
                .extended()
                .iter()
                .any(|line| line.to_ascii_lowercase().contains(&wanted));
        if partial_match.is_none() && partial_match_found {
            partial_match = Some(device);
        }
    }

    Ok(partial_match)
}

fn find_monitor_input_device(host: &cpal::Host) -> anyhow::Result<Option<cpal::Device>> {
    find_input_device_matching(host, is_monitorish_device_name)
}

fn find_microphone_input_device(host: &cpal::Host) -> anyhow::Result<Option<cpal::Device>> {
    find_input_device_matching(host, |name| {
        !is_monitorish_device_name(name) && !is_serverish_device_name(name)
    })
}

fn find_input_device_matching(
    host: &cpal::Host,
    predicate: impl Fn(&str) -> bool,
) -> anyhow::Result<Option<cpal::Device>> {
    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        let Ok(description) = device.description() else {
            continue;
        };
        let device_name = description.name();
        if predicate(device_name) {
            return Ok(Some(device));
        }
    }

    Ok(None)
}

fn is_monitorish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    ["monitor", "loopback", "what u hear", "stereo mix"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn is_serverish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "sound server",
        "default audio device",
        "discard all samples",
        "rate converter plugin",
        "plugin for channel",
        "plugin using speex",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

enum SelectedInputDevice {
    Cpal {
        device: cpal::Device,
        display_name: String,
    },
    #[cfg(target_os = "linux")]
    LinuxPulse {
        source_name: String,
        display_name: String,
    },
}

impl SelectedInputDevice {
    fn new(device: cpal::Device) -> Self {
        Self::Cpal {
            display_name: device_display_name(&device),
            device,
        }
    }
}

fn device_display_name(device: &cpal::Device) -> String {
    device.description().map_or_else(
        |_| "<unknown-audio-device>".to_owned(),
        |description| description.name().trim().to_owned(),
    )
}

#[cfg(target_os = "linux")]
fn find_linux_system_monitor_input_device(
    _host: &cpal::Host,
) -> anyhow::Result<Option<SelectedInputDevice>> {
    let Some(source_name) = linux_audio::default_monitor_source_name()? else {
        return Ok(None);
    };
    Ok(Some(find_linux_pulse_capture_device(&source_name)))
}

#[cfg(target_os = "linux")]
fn find_linux_pulse_capture_device(source_name: &str) -> SelectedInputDevice {
    SelectedInputDevice::LinuxPulse {
        source_name: source_name.to_owned(),
        display_name: format!("PulseAudio source {source_name}"),
    }
}

#[cfg(target_os = "linux")]
struct LinuxPulseCapture {
    stop_tx: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Drop for LinuxPulseCapture {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

enum CaptureHandle {
    Cpal(Stream),
    #[cfg(target_os = "linux")]
    LinuxPulse(LinuxPulseCapture),
}

#[cfg(target_os = "linux")]
fn build_linux_pulse_capture_stream(
    source_name: &str,
    display_name: &str,
    analyzer: Arc<Mutex<AudioAnalyzer>>,
    config: &AudioPipelineConfig,
) -> anyhow::Result<CaptureHandle> {
    const PULSE_CAPTURE_CHANNELS: usize = 2;
    const PULSE_CAPTURE_RATE_HZ: u32 = 48_000;

    if let Ok(mut guard) = analyzer.lock() {
        *guard = AudioAnalyzer::with_sample_rate(config, PULSE_CAPTURE_RATE_HZ);
    }

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (stop_tx, stop_rx) = mpsc::channel();
    let worker_source = source_name.to_owned();
    let worker_display = display_name.to_owned();
    let worker = thread::Builder::new()
        .name("hypercolor-pulse-capture".to_owned())
        .spawn(move || {
            run_linux_pulse_capture(
                &worker_source,
                &worker_display,
                analyzer,
                ready_tx,
                stop_rx,
                PULSE_CAPTURE_CHANNELS,
                PULSE_CAPTURE_RATE_HZ,
            );
        })
        .context("failed to spawn Linux PulseAudio capture worker")?;

    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => {
            tracing::info!(
                source = %source_name,
                device = %display_name,
                sample_rate_hz = PULSE_CAPTURE_RATE_HZ,
                channels = PULSE_CAPTURE_CHANNELS,
                sample_format = "f32",
                "Audio capture stream configured"
            );
            Ok(CaptureHandle::LinuxPulse(LinuxPulseCapture {
                stop_tx,
                worker: Some(worker),
            }))
        }
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(anyhow!(
                "failed to start Linux PulseAudio capture worker: {error}"
            ))
        }
        Err(error) => {
            let _ = stop_tx.send(());
            let _ = worker.join();
            Err(anyhow!(
                "timed out waiting for Linux PulseAudio capture worker to start: {error}"
            ))
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_arguments,
    reason = "worker startup passes a fixed set of runtime parameters into the capture thread"
)]
fn run_linux_pulse_capture(
    source_name: &str,
    display_name: &str,
    analyzer: Arc<Mutex<AudioAnalyzer>>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
    stop_rx: mpsc::Receiver<()>,
    channels: usize,
    sample_rate_hz: u32,
) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::FLOAT32NE,
        channels: u8::try_from(channels).unwrap_or(2),
        rate: sample_rate_hz,
    };
    if !spec.is_valid() {
        let _ = ready_tx.send(Err("invalid PulseAudio sample spec".to_owned()));
        return;
    }

    let Some(mut mainloop) = pulse::mainloop::standard::Mainloop::new() else {
        let _ = ready_tx.send(Err("failed to create PulseAudio mainloop".to_owned()));
        return;
    };
    let Some(mut context) = pulse::context::Context::new(&mainloop, "hypercolor-audio") else {
        let _ = ready_tx.send(Err("failed to create PulseAudio context".to_owned()));
        return;
    };
    if let Err(error) = context.connect(None, pulse::context::FlagSet::NOFLAGS, None) {
        let _ = ready_tx.send(Err(format!(
            "failed to connect to PulseAudio compatibility server: {error:?}"
        )));
        return;
    }

    if let Err(error) = wait_for_pulse_context(
        &mut mainloop,
        &context,
        &stop_rx,
        "before record stream setup",
    ) {
        let _ = ready_tx.send(Err(error));
        return;
    }

    let Some(stream) =
        pulse::stream::Stream::new(&mut context, "Hypercolor Audio Capture", &spec, None)
    else {
        let _ = ready_tx.send(Err("failed to create PulseAudio record stream".to_owned()));
        return;
    };
    let stream = Rc::new(RefCell::new(stream));
    let stream_for_callback = Rc::clone(&stream);
    let callback_analyzer = Arc::clone(&analyzer);
    let callback_source = source_name.to_owned();
    stream
        .borrow_mut()
        .set_read_callback(Some(Box::new(move |_bytes_available| {
            let mut guard = stream_for_callback.borrow_mut();
            loop {
                match guard.peek() {
                    Ok(pulse::stream::PeekResult::Data(bytes)) => {
                        let samples = bytes_to_f32_samples(bytes);
                        push_input_samples(&samples, channels, &callback_analyzer);
                        if let Err(error) = guard.discard() {
                            tracing::warn!(
                                source = %callback_source,
                                %error,
                                "Failed to discard PulseAudio record buffer"
                            );
                            break;
                        }
                    }
                    Ok(pulse::stream::PeekResult::Hole(_)) => {
                        if let Err(error) = guard.discard() {
                            tracing::warn!(
                                source = %callback_source,
                                %error,
                                "Failed to discard PulseAudio record hole"
                            );
                            break;
                        }
                    }
                    Ok(pulse::stream::PeekResult::Empty) => break,
                    Err(error) => {
                        tracing::warn!(
                            source = %callback_source,
                            %error,
                            "PulseAudio record stream peek failed"
                        );
                        break;
                    }
                }
            }
        })));

    if let Err(error) =
        stream
            .borrow_mut()
            .connect_record(Some(source_name), None, pulse::stream::FlagSet::NOFLAGS)
    {
        let _ = ready_tx.send(Err(format!(
            "failed to connect PulseAudio record stream for {display_name}: {error:?}"
        )));
        return;
    }

    if let Err(error) = wait_for_pulse_stream(&mut mainloop, &stream, &stop_rx) {
        let _ = ready_tx.send(Err(error));
        return;
    }

    if ready_tx.send(Ok(())).is_err() {
        let _ = stream.borrow_mut().disconnect();
        context.disconnect();
        return;
    }

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }

        match mainloop.iterate(false) {
            pulse::mainloop::standard::IterateResult::Success(_) => {}
            pulse::mainloop::standard::IterateResult::Quit(retval) => {
                tracing::warn!(
                    source = %source_name,
                    retval = retval.0,
                    "PulseAudio mainloop quit unexpectedly"
                );
                break;
            }
            pulse::mainloop::standard::IterateResult::Err(error) => {
                tracing::warn!(
                    source = %source_name,
                    %error,
                    "PulseAudio mainloop iteration failed"
                );
                break;
            }
        }

        match stream.borrow().get_state() {
            pulse::stream::State::Failed | pulse::stream::State::Terminated => {
                tracing::warn!(
                    source = %source_name,
                    state = ?stream.borrow().get_state(),
                    "PulseAudio record stream stopped unexpectedly"
                );
                break;
            }
            _ => {}
        }

        thread::sleep(Duration::from_millis(5));
    }

    let _ = stream.borrow_mut().disconnect();
    context.disconnect();
}

#[cfg(target_os = "linux")]
fn wait_for_pulse_context(
    mainloop: &mut pulse::mainloop::standard::Mainloop,
    context: &pulse::context::Context,
    stop_rx: &mpsc::Receiver<()>,
    phase: &str,
) -> Result<(), String> {
    loop {
        if stop_rx.try_recv().is_ok() {
            return Err(format!("PulseAudio capture stopped {phase}"));
        }

        match context.get_state() {
            pulse::context::State::Ready => return Ok(()),
            pulse::context::State::Failed | pulse::context::State::Terminated => {
                return Err(format!(
                    "PulseAudio context failed {phase}: {:?}",
                    context.errno()
                ));
            }
            _ => {}
        }

        match mainloop.iterate(false) {
            pulse::mainloop::standard::IterateResult::Success(_) => {}
            pulse::mainloop::standard::IterateResult::Quit(retval) => {
                return Err(format!(
                    "PulseAudio mainloop quit {phase} with status {}",
                    retval.0
                ));
            }
            pulse::mainloop::standard::IterateResult::Err(error) => {
                return Err(format!(
                    "PulseAudio mainloop iteration failed {phase}: {error:?}"
                ));
            }
        }

        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
fn wait_for_pulse_stream(
    mainloop: &mut pulse::mainloop::standard::Mainloop,
    stream: &std::rc::Rc<std::cell::RefCell<pulse::stream::Stream>>,
    stop_rx: &mpsc::Receiver<()>,
) -> Result<(), String> {
    loop {
        if stop_rx.try_recv().is_ok() {
            return Err("PulseAudio capture stopped before record stream became ready".to_owned());
        }

        match stream.borrow().get_state() {
            pulse::stream::State::Ready => return Ok(()),
            pulse::stream::State::Failed | pulse::stream::State::Terminated => {
                return Err(format!(
                    "PulseAudio record stream failed before becoming ready: {:?}",
                    stream.borrow().get_state()
                ));
            }
            _ => {}
        }

        match mainloop.iterate(false) {
            pulse::mainloop::standard::IterateResult::Success(_) => {}
            pulse::mainloop::standard::IterateResult::Quit(retval) => {
                return Err(format!(
                    "PulseAudio mainloop quit before record stream became ready: {}",
                    retval.0
                ));
            }
            pulse::mainloop::standard::IterateResult::Err(error) => {
                return Err(format!(
                    "PulseAudio mainloop iteration failed before record stream became ready: {error:?}"
                ));
            }
        }

        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
fn bytes_to_f32_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let array: [u8; std::mem::size_of::<f32>()] = chunk
                .try_into()
                .unwrap_or([0_u8; std::mem::size_of::<f32>()]);
            f32::from_ne_bytes(array)
        })
        .collect()
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    analyzer: Arc<Mutex<AudioAnalyzer>>,
    device_name: &str,
) -> anyhow::Result<Stream>
where
    T: Sample + SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let err_name = device_name.to_owned();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| push_input_samples(data, channels, &analyzer),
            move |error| {
                tracing::warn!(
                    device = %err_name,
                    %error,
                    "Audio capture stream reported an error"
                );
            },
            None,
        )
        .with_context(|| format!("failed to build audio capture stream for '{device_name}'"))
}

fn push_input_samples<T>(input: &[T], channels: usize, analyzer: &Arc<Mutex<AudioAnalyzer>>)
where
    T: Sample + Copy,
    f32: FromSample<T>,
{
    let channel_count = channels.max(1);
    let mut mono = Vec::with_capacity(input.len().max(1) / channel_count.max(1));

    for frame in input.chunks(channel_count) {
        let sum = frame.iter().copied().map(f32::from_sample).sum::<f32>();
        let sample_count = u16::try_from(frame.len()).unwrap_or(1);
        mono.push((sum / f32::from(sample_count)).clamp(-1.0, 1.0));
    }

    if let Ok(mut guard) = analyzer.lock() {
        guard.push_samples(&mono);
    }
}
