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
pub mod realtime;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use arc_swap::ArcSwapOption;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream};
#[cfg(target_os = "linux")]
use libpulse_binding as pulse;

use crate::input::status::SourceStatusPolicy;
use crate::input::traits::{
    AudioSource, AudioSourceRole, InputData, InputSource, SourceRoleBinding,
};
use crate::input::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};
use crate::types::audio::{AudioData, AudioPipelineConfig, AudioSourceType};
use crate::types::event::TimedInputEvent;
use hypercolor_worker_retention::{retain_worker, spawn_worker};

use beat::{BeatDetector, BeatFrame};
use features::{
    ArraySmoother, MelFilterbank, Smoother, band_energies, compute_chromagram, compute_peak,
    compute_rms, spectral_centroid,
};
use fft::{FftPipeline, RingBuffer};
use realtime::{AudioFrameRing, push_interleaved_frames};

#[cfg(target_os = "linux")]
use self::linux as linux_audio;
use crate::types::audio::{CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};

const DEFAULT_AUDIO_FRAME_DT: f32 = 1.0 / 60.0;
const AUDIO_RING_DURATION_MS: usize = 500;
const AUDIO_ANALYSIS_IDLE: Duration = Duration::from_millis(2);
const AUDIO_SNAPSHOT_FRESHNESS: Duration = Duration::from_millis(250);
const AUDIO_OVERFLOW_REPORT_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(target_os = "linux")]
const PULSE_CAPTURE_TARGET_LATENCY_MS: u64 = 20;
#[cfg(target_os = "linux")]
const PULSE_CAPTURE_MAX_BUFFER_FRAGMENTS: u32 = 4;
#[cfg(target_os = "linux")]
const PULSE_CAPTURE_CHANNELS: usize = 2;
#[cfg(target_os = "linux")]
const PULSE_CAPTURE_RATE_HZ: u32 = 48_000;

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
            for &sample in samples {
                self.ring.push(sample * gain);
            }
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

struct CapturedAudioSnapshot {
    data: Arc<InputData>,
    captured_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum AudioFailureKind {
    None = 0,
    DeviceLost = 1,
    PermissionDenied = 2,
    BackendUnavailable = 3,
    AnalysisWorkerExited = 4,
}

impl AudioFailureKind {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::DeviceLost,
            2 => Self::PermissionDenied,
            3 => Self::BackendUnavailable,
            4 => Self::AnalysisWorkerExited,
            _ => Self::None,
        }
    }

    const fn issue_code(self) -> &'static str {
        match self {
            Self::None => "audio_capture_unavailable",
            Self::DeviceLost => "audio_device_lost",
            Self::PermissionDenied => "audio_permission_denied",
            Self::BackendUnavailable => "audio_backend_unavailable",
            Self::AnalysisWorkerExited => "audio_analysis_worker_exited",
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::None => "audio capture is unavailable",
            Self::DeviceLost => "audio device was disconnected or invalidated",
            Self::PermissionDenied => "audio capture permission was denied",
            Self::BackendUnavailable => "audio capture backend stopped unexpectedly",
            Self::AnalysisWorkerExited => "audio analysis worker stopped unexpectedly",
        }
    }
}

fn classify_audio_start_failure(error: &anyhow::Error) -> AudioFailureKind {
    let message = format!("{error:#}").to_ascii_lowercase();
    if ["permission", "access denied", "not authorized"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        AudioFailureKind::PermissionDenied
    } else if [
        "not found",
        "no input device",
        "no microphone",
        "no longer available",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        AudioFailureKind::DeviceLost
    } else {
        AudioFailureKind::BackendUnavailable
    }
}

struct AnalysisWorker {
    ring: Arc<AudioFrameRing>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AnalysisWorker {
    fn spawn(
        config: &AudioPipelineConfig,
        sample_rate_hz: u32,
        channels: usize,
        latest_snapshot: Arc<ArcSwapOption<CapturedAudioSnapshot>>,
        terminal_failure: Arc<AtomicU8>,
    ) -> anyhow::Result<Self> {
        let samples_per_window = usize::try_from(sample_rate_hz)
            .unwrap_or(48_000)
            .saturating_mul(AUDIO_RING_DURATION_MS)
            / 1_000;
        let ring = Arc::new(AudioFrameRing::with_channels(
            samples_per_window.max(config.fft_size.saturating_mul(4)),
            channels,
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_ring = Arc::clone(&ring);
        let worker_stop = Arc::clone(&stop);
        let analyzer = AudioAnalyzer::with_sample_rate(config, sample_rate_hz);
        let samples = Vec::with_capacity(config.fft_size.saturating_mul(4).max(4_096));
        let worker = spawn_worker(
            thread::Builder::new().name("hypercolor-audio-analysis".to_owned()),
            move || {
                run_analysis_worker(
                    analyzer,
                    samples,
                    &worker_ring,
                    &worker_stop,
                    &latest_snapshot,
                    &terminal_failure,
                );
            },
        )
        .context("failed to spawn audio analysis worker")?;
        Ok(Self {
            ring,
            stop,
            worker: Some(worker),
        })
    }

    fn ring(&self) -> Arc<AudioFrameRing> {
        Arc::clone(&self.ring)
    }

    fn is_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(thread::JoinHandle::is_finished)
    }
}

impl Drop for AnalysisWorker {
    fn drop(&mut self) {
        self.ring.close();
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if let Err(panic) = worker.join() {
                tracing::warn!(?panic, "Audio analysis worker panicked during shutdown");
            }
        }
    }
}

struct RecoveryWorker {
    result_rx: mpsc::Receiver<CaptureRuntime>,
    failure: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

enum RecoveryPoll {
    Pending,
    Ready(CaptureRuntime),
    Terminated,
}

impl RecoveryWorker {
    fn spawn(
        config: &AudioPipelineConfig,
        initial_failure: AudioFailureKind,
    ) -> anyhow::Result<Self> {
        // A rendezvous prevents an unclaimed native runtime from being dropped
        // on the render thread when demand disappears during startup.
        let (result_tx, result_rx) = mpsc::sync_channel(0);
        let failure = Arc::new(AtomicU8::new(initial_failure as u8));
        let worker_failure = Arc::clone(&failure);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_config = config.clone();
        let worker = spawn_worker(
            thread::Builder::new().name("hypercolor-audio-recovery".to_owned()),
            move || {
                let mut attempt = 0_u32;
                let mut failure = initial_failure;
                loop {
                    if attempt > 0 {
                        thread::park_timeout(audio_reconnect_delay(failure, attempt - 1));
                    }
                    if worker_stop.load(Ordering::Acquire) {
                        return;
                    }
                    let host = cpal::default_host();
                    match build_capture_stream(&host, &worker_config) {
                        Ok(capture) => {
                            let _ = result_tx.send(capture);
                            return;
                        }
                        Err(error) => {
                            failure = classify_audio_start_failure(&error);
                            worker_failure.store(failure as u8, Ordering::Release);
                            attempt = attempt.saturating_add(1);
                        }
                    }
                }
            },
        )
        .context("failed to spawn audio recovery worker")?;
        Ok(Self {
            result_rx,
            failure,
            stop,
            worker: Some(worker),
        })
    }

    fn poll(&mut self) -> RecoveryPoll {
        match self.result_rx.try_recv() {
            Ok(capture) => RecoveryPoll::Ready(capture),
            Err(mpsc::TryRecvError::Empty) => RecoveryPoll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => RecoveryPoll::Terminated,
        }
    }

    fn failure(&self) -> AudioFailureKind {
        AudioFailureKind::from_code(self.failure.load(Ordering::Acquire))
    }
}

impl Drop for RecoveryWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let Some(worker) = self.worker.take() else {
            return;
        };
        worker.thread().unpark();
        if worker.is_finished() {
            let _ = worker.join();
        } else {
            retain_worker(worker, "audio recovery worker");
        }
    }
}

fn audio_reconnect_delay(failure: AudioFailureKind, attempt: u32) -> Duration {
    let exponent = attempt.min(4);
    let multiplier = 1_u32 << exponent;
    match failure {
        AudioFailureKind::DeviceLost => Duration::from_millis(250 * u64::from(multiplier)),
        AudioFailureKind::AnalysisWorkerExited => {
            Duration::from_millis(500 * u64::from(multiplier))
        }
        AudioFailureKind::BackendUnavailable | AudioFailureKind::None => {
            Duration::from_secs(2 * u64::from(multiplier))
        }
        AudioFailureKind::PermissionDenied => Duration::from_secs(30),
    }
}

fn run_analysis_worker(
    mut analyzer: AudioAnalyzer,
    mut samples: Vec<f32>,
    ring: &AudioFrameRing,
    stop: &AtomicBool,
    latest_snapshot: &Arc<ArcSwapOption<CapturedAudioSnapshot>>,
    terminal_failure: &AtomicU8,
) {
    struct ExitGuard<'a> {
        stop: &'a AtomicBool,
        terminal_failure: &'a AtomicU8,
    }

    impl Drop for ExitGuard<'_> {
        fn drop(&mut self) {
            if !self.stop.load(Ordering::Acquire) {
                let _ = self.terminal_failure.compare_exchange(
                    AudioFailureKind::None as u8,
                    AudioFailureKind::AnalysisWorkerExited as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    }

    let _exit_guard = ExitGuard {
        stop,
        terminal_failure,
    };
    while !stop.load(Ordering::Acquire) {
        samples.clear();
        while samples.len() < samples.capacity() {
            let Some(sample) = ring.pop_frame() else {
                break;
            };
            samples.push(sample);
        }
        if samples.is_empty() {
            thread::park_timeout(AUDIO_ANALYSIS_IDLE);
            continue;
        }
        analyzer.push_samples(&samples);
        publish_latest_snapshot(&mut analyzer, samples.len(), latest_snapshot);
    }
}

/// Control-plane identity of the committed audio capture backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCaptureBackend {
    /// Backend selection is deferred until capture starts.
    Auto,
    /// Cross-platform capture through cpal.
    Cpal,
    /// Direct PulseAudio compatibility-protocol capture on Linux.
    PulseAudio,
    /// Audio capture is intentionally disabled.
    Disabled,
}

/// Native audio state prepared away from the render-owned input manager.
///
/// Construction may enumerate devices and wait for a backend. Committing the
/// prepared value only swaps in-memory state and retires the previous runtime
/// asynchronously.
#[must_use = "prepared native audio state must be committed or explicitly dropped"]
pub struct PreparedAudioReconfiguration {
    pub(super) expected_graph_generation: u64,
    pub(super) expected_source_present: bool,
    pub(super) expected_source_running: bool,
    pub(super) enabled: bool,
    config: AudioPipelineConfig,
    pub(super) name: String,
    pub(super) capture_active: bool,
    capture: Option<CaptureRuntime>,
    analyzer: Option<Arc<Mutex<AudioAnalyzer>>>,
    silence: Option<Arc<InputData>>,
    latest_snapshot: Option<Arc<ArcSwapOption<CapturedAudioSnapshot>>>,
    status: Option<SourceStatusReporter>,
    terminal_failure: Arc<AtomicU8>,
}

/// Previous audio workers detached by a successful in-memory commit.
///
/// Call [`Self::retire`] only after releasing the input manager lock.
#[must_use = "detached audio workers must be retired outside the input manager lock"]
pub struct AudioRuntimeRetirement {
    capture: Option<CaptureRuntime>,
    recovery: Option<RecoveryWorker>,
}

impl std::fmt::Debug for AudioRuntimeRetirement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AudioRuntimeRetirement")
            .field("capture", &self.capture.is_some())
            .field("recovery", &self.recovery.is_some())
            .finish()
    }
}

impl AudioRuntimeRetirement {
    pub(super) fn empty() -> Self {
        Self {
            capture: None,
            recovery: None,
        }
    }

    fn new(capture: Option<CaptureRuntime>, recovery: Option<RecoveryWorker>) -> Self {
        Self { capture, recovery }
    }

    /// Stop detached audio workers without blocking the caller.
    pub fn retire(self) {
        if self.capture.is_none() && self.recovery.is_none() {
            return;
        }
        retire_audio_runtime(self);
    }
}

pub(super) struct AudioPreparationRequest {
    pub expected_graph_generation: u64,
    pub expected_source_present: bool,
    pub expected_source_running: bool,
    pub enabled: bool,
    pub config: AudioPipelineConfig,
    pub name: String,
    pub capture_active: bool,
}

impl PreparedAudioReconfiguration {
    pub(super) fn prepare(request: AudioPreparationRequest) -> anyhow::Result<Self> {
        Self::prepare_with_capture(request, |config, terminal_failure| {
            build_capture_stream_with_failure(&cpal::default_host(), config, terminal_failure)
        })
    }

    pub(super) fn prepare_with_synthetic_capture_for_testing(
        request: AudioPreparationRequest,
    ) -> anyhow::Result<Self> {
        Self::prepare_with_capture(request, build_synthetic_capture_stream)
    }

    fn prepare_with_capture(
        request: AudioPreparationRequest,
        build_capture: impl FnOnce(
            &AudioPipelineConfig,
            Arc<AtomicU8>,
        ) -> anyhow::Result<CaptureRuntime>,
    ) -> anyhow::Result<Self> {
        let terminal_failure = Arc::new(AtomicU8::new(AudioFailureKind::None as u8));
        let should_start_capture = request.capture_active
            && if request.expected_source_present {
                request.expected_source_running
            } else {
                request.enabled
            };
        let capture = if should_start_capture {
            Some(build_capture(
                &request.config,
                Arc::clone(&terminal_failure),
            )?)
        } else {
            None
        };
        let capture_backend = classify_audio_capture_backend(
            &request.config.source,
            capture.as_ref().map(CaptureRuntime::backend),
        );
        let analyzer = Arc::new(Mutex::new(AudioAnalyzer::new(&request.config)));
        let silence = Arc::new(InputData::Audio(AudioData::silence()));
        let latest_snapshot = Some(capture.as_ref().map_or_else(
            || Arc::new(ArcSwapOption::empty()),
            |capture| Arc::clone(&capture.latest_snapshot),
        ));
        let status = (!request.expected_source_present).then(|| {
            SourceStatusReporter::new(
                "audio",
                SourceKind::Audio,
                capture_backend.status_id(),
                !matches!(request.config.source, AudioSourceType::None),
                true,
                request.capture_active,
            )
        });
        Ok(Self {
            expected_graph_generation: request.expected_graph_generation,
            expected_source_present: request.expected_source_present,
            expected_source_running: request.expected_source_running,
            enabled: request.enabled,
            config: request.config,
            name: request.name,
            capture_active: request.capture_active,
            capture,
            analyzer: Some(analyzer),
            silence: Some(silence),
            latest_snapshot,
            status,
            terminal_failure,
        })
    }

    pub(super) fn ensure_ready(&mut self) -> anyhow::Result<()> {
        if let Some(failure) = self
            .capture
            .as_mut()
            .and_then(CaptureRuntime::observe_failure)
        {
            self.terminal_failure
                .store(failure as u8, Ordering::Release);
        }
        let failure = AudioFailureKind::from_code(
            self.terminal_failure
                .swap(AudioFailureKind::None as u8, Ordering::AcqRel),
        );
        if failure == AudioFailureKind::None {
            return Ok(());
        }
        anyhow::bail!(
            "prepared audio runtime failed before commit: {}",
            failure.message()
        )
    }

    /// Inject a terminal pre-commit failure for deterministic transaction tests.
    #[doc(hidden)]
    pub fn fail_before_commit_for_testing(&mut self) {
        self.terminal_failure.store(
            AudioFailureKind::BackendUnavailable as u8,
            Ordering::Release,
        );
    }
}

impl Drop for PreparedAudioReconfiguration {
    fn drop(&mut self) {
        if let Some(capture) = self.capture.take() {
            retire_capture_runtime(capture);
        }
    }
}

impl AudioCaptureBackend {
    /// Stable status identifier.
    #[must_use]
    pub const fn status_id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpal => "cpal",
            Self::PulseAudio => "pulse_audio",
            Self::Disabled => "disabled",
        }
    }
}

/// Classify backend identity before or after native device selection.
#[must_use]
pub fn classify_audio_capture_backend(
    source: &AudioSourceType,
    selected: Option<AudioCaptureBackend>,
) -> AudioCaptureBackend {
    match source {
        AudioSourceType::None => AudioCaptureBackend::Disabled,
        AudioSourceType::Microphone => selected.unwrap_or(AudioCaptureBackend::Cpal),
        AudioSourceType::SystemMonitor | AudioSourceType::Named(_) => {
            selected.unwrap_or(AudioCaptureBackend::Auto)
        }
    }
}

/// Audio input source implementing [`InputSource`].
///
/// Wraps [`AudioAnalyzer`] and provides the standard lifecycle methods.
/// In production, a cpal audio stream pushes samples into the analyzer
/// from a callback thread. For testing, call [`push_samples`](AudioInput::push_samples)
/// directly with synthetic data.
pub struct AudioInput {
    name: String,
    running: bool,
    capture_active: bool,
    config: AudioPipelineConfig,
    analyzer: Arc<Mutex<AudioAnalyzer>>,
    silence: Arc<InputData>,
    latest_snapshot: Arc<ArcSwapOption<CapturedAudioSnapshot>>,
    last_status_snapshot: Option<Arc<CapturedAudioSnapshot>>,
    capture: Option<CaptureRuntime>,
    recovery: Option<RecoveryWorker>,
    degraded_to_silence: bool,
    last_failure: AudioFailureKind,
    status: SourceStatusReporter,
}

impl AudioInput {
    /// Create a new audio input with the given config.
    #[must_use]
    pub fn new(config: &AudioPipelineConfig) -> Self {
        let capture_backend = classify_audio_capture_backend(&config.source, None);
        Self {
            name: "AudioInput".to_owned(),
            running: false,
            capture_active: false,
            config: config.clone(),
            analyzer: Arc::new(Mutex::new(AudioAnalyzer::new(config))),
            silence: Arc::new(InputData::Audio(AudioData::silence())),
            latest_snapshot: Arc::new(ArcSwapOption::empty()),
            last_status_snapshot: None,
            capture: None,
            recovery: None,
            degraded_to_silence: false,
            last_failure: AudioFailureKind::None,
            status: SourceStatusReporter::new(
                "audio",
                SourceKind::Audio,
                capture_backend.status_id(),
                !matches!(config.source, AudioSourceType::None),
                true,
                false,
            ),
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
    /// This deterministic seam is available while no committed native capture
    /// owns publication. A staged recovery cannot publish until it is committed.
    pub fn push_samples(&mut self, samples: &[f32]) {
        if self.capture.is_some() {
            return;
        }
        if let Ok(mut analyzer) = self.analyzer.lock() {
            analyzer.push_samples(samples);
            publish_latest_snapshot(&mut analyzer, samples.len(), &self.latest_snapshot);
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

    fn schedule_recovery(&mut self, failure: AudioFailureKind) -> anyhow::Result<()> {
        self.last_failure = failure;
        self.degraded_to_silence = failure != AudioFailureKind::None;
        if self.recovery.is_none() && !matches!(self.config.source, AudioSourceType::None) {
            self.recovery = Some(RecoveryWorker::spawn(&self.config, failure)?);
        }
        Ok(())
    }

    fn poll_recovery(&mut self) -> anyhow::Result<()> {
        let failure = self
            .recovery
            .as_ref()
            .map_or(AudioFailureKind::None, RecoveryWorker::failure);
        if failure != AudioFailureKind::None && failure != self.last_failure {
            self.last_failure = failure;
            if let Some(status) = self.status.session() {
                status.degraded(SourceIssue::new(
                    failure.issue_code(),
                    failure.message(),
                    true,
                ));
            }
        }

        let recovery_poll = self
            .recovery
            .as_mut()
            .map_or(RecoveryPoll::Pending, RecoveryWorker::poll);
        let capture = match recovery_poll {
            RecoveryPoll::Pending => return Ok(()),
            RecoveryPoll::Ready(capture) => capture,
            RecoveryPoll::Terminated => {
                self.recovery = None;
                let failure = AudioFailureKind::AnalysisWorkerExited;
                self.schedule_recovery(failure)?;
                if let Some(status) = self.status.session() {
                    status.degraded(SourceIssue::new(
                        failure.issue_code(),
                        failure.message(),
                        true,
                    ));
                }
                return Ok(());
            }
        };
        let capture_backend =
            classify_audio_capture_backend(&self.config.source, Some(capture.backend()));
        if let Err(error) = self.status.set_backend(capture_backend.status_id()) {
            retire_capture_runtime(capture);
            return Err(error.into());
        }
        self.latest_snapshot = Arc::clone(&capture.latest_snapshot);
        self.capture = Some(capture);
        self.recovery = None;
        self.degraded_to_silence = false;
        self.last_failure = AudioFailureKind::None;
        tracing::info!(
            input = %self.name,
            source = ?self.config.source,
            backend = capture_backend.status_id(),
            "Audio capture recovered without restarting the daemon"
        );
        Ok(())
    }

    fn sample_shared_with_dt(&mut self, _dt: f32) -> anyhow::Result<Option<Arc<InputData>>> {
        if !self.running {
            return Ok(None);
        }
        if !self.capture_active {
            return Ok(Some(Arc::clone(&self.silence)));
        }

        if let Some(failure) = self
            .capture
            .as_mut()
            .and_then(CaptureRuntime::observe_failure)
        {
            let failed_capture = self
                .capture
                .take()
                .expect("observed audio failure has an active capture runtime");
            retire_capture_runtime(failed_capture);
            self.latest_snapshot = Arc::new(ArcSwapOption::empty());
            self.last_status_snapshot = None;
            self.schedule_recovery(failure)?;
            if let Some(status) = self.status.session() {
                status.degraded(SourceIssue::new(
                    failure.issue_code(),
                    failure.message(),
                    true,
                ));
            }
        }
        self.poll_recovery()?;

        if let Some(capture) = self.capture.as_mut()
            && let Some((dropped, dropped_total)) = capture.take_drop_report(Instant::now())
        {
            tracing::warn!(
                input = %self.name,
                source = ?self.config.source,
                dropped,
                dropped_total,
                "Audio callback ring saturated; oldest frames were shed"
            );
        }

        if let Some(snapshot) = self.latest_snapshot.load_full() {
            if self
                .last_status_snapshot
                .as_ref()
                .is_none_or(|previous| !Arc::ptr_eq(previous, &snapshot))
            {
                if let Some(status) = self.status.session() {
                    status.record_sample(
                        snapshot.captured_at,
                        snapshot.captured_at + AUDIO_SNAPSHOT_FRESHNESS,
                        usize::from(self.capture.is_some()),
                    )?;
                }
                self.last_status_snapshot = Some(Arc::clone(&snapshot));
            }
            if snapshot.captured_at.elapsed() > AUDIO_SNAPSHOT_FRESHNESS {
                return Ok(Some(Arc::clone(&self.silence)));
            }
            return Ok(Some(Arc::clone(&snapshot.data)));
        }

        if self.degraded_to_silence
            || self.recovery.is_some()
            || matches!(self.config.source, AudioSourceType::None)
        {
            return Ok(Some(Arc::clone(&self.silence)));
        }

        Ok(None)
    }

    fn sample_with_dt(&mut self, dt: f32) -> anyhow::Result<InputData> {
        Ok(self
            .sample_shared_with_dt(dt)?
            .map_or(InputData::None, |sample| sample.as_ref().clone()))
    }

    pub(super) fn commit_prepared(
        &mut self,
        prepared: &mut PreparedAudioReconfiguration,
    ) -> anyhow::Result<AudioRuntimeRetirement> {
        prepared.ensure_ready()?;
        let previous_source = self.config.source.clone();
        let was_running = self.running;
        let capture_backend = classify_audio_capture_backend(
            &prepared.config.source,
            prepared.capture.as_ref().map(CaptureRuntime::backend),
        );
        self.status.reconfigure(
            capture_backend.status_id(),
            SourceStatusPolicy::new(
                !matches!(prepared.config.source, AudioSourceType::None),
                true,
                prepared.capture_active,
            ),
            was_running && prepared.capture_active,
        )?;
        let next_analyzer = prepared
            .analyzer
            .take()
            .expect("prepared audio analyzer is committed exactly once");
        let next_silence = prepared
            .silence
            .take()
            .expect("prepared audio silence snapshot is committed exactly once");
        let next_snapshot = prepared
            .latest_snapshot
            .take()
            .expect("prepared audio snapshot slot is committed exactly once");
        let previous_capture = self.capture.take();
        let previous_recovery = self.recovery.take();
        self.name = std::mem::take(&mut prepared.name);
        self.config = std::mem::take(&mut prepared.config);
        self.analyzer = next_analyzer;
        self.silence = next_silence;
        self.latest_snapshot = next_snapshot;
        self.capture = prepared.capture.take();
        self.capture_active = prepared.capture_active;
        self.degraded_to_silence = false;
        self.last_failure = AudioFailureKind::None;
        self.last_status_snapshot = None;

        tracing::info!(
            input = %self.name,
            previous_source = ?previous_source,
            source = ?self.config.source,
            capture_active = self.capture_active,
            "Live audio capture configuration committed"
        );
        Ok(AudioRuntimeRetirement::new(
            previous_capture,
            previous_recovery,
        ))
    }

    pub(super) fn from_prepared(
        prepared: &mut PreparedAudioReconfiguration,
        source_graph_generation: u64,
    ) -> anyhow::Result<Self> {
        prepared.ensure_ready()?;
        let capture_backend = classify_audio_capture_backend(
            &prepared.config.source,
            prepared.capture.as_ref().map(CaptureRuntime::backend),
        );
        let mut status = prepared
            .status
            .take()
            .expect("new prepared audio source owns staged status state");
        if let Err(error) = status.set_backend(capture_backend.status_id()) {
            prepared.status = Some(status);
            return Err(error.into());
        }
        status.set_source_graph_generation(source_graph_generation);
        let should_capture =
            prepared.capture_active && !matches!(prepared.config.source, AudioSourceType::None);
        if should_capture && prepared.capture.is_none() {
            prepared.status = Some(status);
            anyhow::bail!("active prepared audio source is missing its capture runtime");
        }
        if should_capture && let Err(error) = status.begin_session() {
            prepared.status = Some(status);
            return Err(error.into());
        }
        let capture = prepared.capture.take();
        let latest_snapshot = prepared
            .latest_snapshot
            .take()
            .expect("prepared audio snapshot slot is committed exactly once");
        Ok(Self {
            name: std::mem::take(&mut prepared.name),
            running: true,
            capture_active: prepared.capture_active,
            config: std::mem::take(&mut prepared.config),
            analyzer: prepared
                .analyzer
                .take()
                .expect("prepared audio analyzer is committed exactly once"),
            silence: prepared
                .silence
                .take()
                .expect("prepared audio silence snapshot is committed exactly once"),
            latest_snapshot,
            last_status_snapshot: None,
            capture,
            recovery: None,
            degraded_to_silence: false,
            last_failure: AudioFailureKind::None,
            status,
        })
    }

    fn reset_analyzer(&mut self) {
        if let Ok(mut analyzer) = self.analyzer.lock() {
            analyzer.reset();
        } else {
            self.analyzer = Arc::new(Mutex::new(AudioAnalyzer::new(&self.config)));
        }
        self.clear_latest_snapshot();
    }

    fn clear_latest_snapshot(&self) {
        self.latest_snapshot.store(None);
    }

    fn pause_capture_stream(&mut self) {
        if let Some(capture) = self.capture.take() {
            retire_capture_runtime(capture);
        }
        self.recovery = None;

        self.degraded_to_silence = false;
        self.last_failure = AudioFailureKind::None;
        self.reset_analyzer();
    }

    fn drop_capture_stream(&mut self, reason: &'static str) {
        let stream_source = self.config.source.clone();
        let previous_capture = self.capture.take();
        self.recovery = None;

        if previous_capture.is_none() {
            tracing::debug!(
                input = %self.name,
                source = ?stream_source,
                reason,
                "No audio capture stream to drop"
            );
            return;
        }

        if let Some(capture) = previous_capture {
            retire_capture_runtime(capture);
        }
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
            self.recovery = None;
            self.last_failure = AudioFailureKind::None;
            return Ok(());
        }

        if self.capture.is_some() {
            self.degraded_to_silence = false;
            self.recovery = None;
            return Ok(());
        }

        self.latest_snapshot = Arc::new(ArcSwapOption::empty());
        self.schedule_recovery(AudioFailureKind::None)?;
        tracing::debug!(
            input = %self.name,
            source = ?self.config.source,
            "Audio capture startup scheduled off the render thread"
        );
        Ok(())
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        let previous = self.capture_active;
        self.status.set_policy(
            !matches!(self.config.source, AudioSourceType::None),
            true,
            active,
        )?;
        if previous == active {
            if active && self.running && self.capture.is_none() {
                self.start_capture_stream()?;
            }
            return Ok(());
        }

        if !self.running {
            self.capture_active = active;
            return Ok(());
        }

        if active {
            let status = self.status.begin_session()?;
            if let Err(error) = self.start_capture_stream() {
                self.status.stop();
                self.status.set_policy(
                    !matches!(self.config.source, AudioSourceType::None),
                    true,
                    previous,
                )?;
                self.drop_capture_stream("capture activation rolled back");
                return Err(error);
            }
            if self.degraded_to_silence
                && let Some(status) = status
            {
                status.degraded(SourceIssue::new(
                    self.last_failure.issue_code(),
                    self.last_failure.message(),
                    true,
                ));
            }
            self.capture_active = true;
            Ok(())
        } else {
            self.pause_capture_stream();
            self.capture_active = false;
            Ok(())
        }
    }

    /// Apply a runtime audio config change without rebuilding the whole input manager.
    ///
    /// A replacement native stream and analysis worker become ready against a
    /// private publication slot before any committed state changes. A failed
    /// replacement therefore leaves the current stream live and retryable.
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
        let was_running = self.running;
        let effective_capture_active =
            capture_active && !matches!(config.source, AudioSourceType::None);
        let prepared = PreparedAudioReconfiguration::prepare(AudioPreparationRequest {
            expected_graph_generation: 0,
            expected_source_present: true,
            expected_source_running: was_running,
            enabled: !matches!(config.source, AudioSourceType::None),
            config: config.clone(),
            name: next_name,
            capture_active: effective_capture_active,
        })?;
        let mut prepared = prepared;
        self.commit_prepared(&mut prepared)?.retire();
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

        self.degraded_to_silence = false;
        self.recovery = None;
        self.last_status_snapshot = None;
        self.last_failure = AudioFailureKind::None;

        if matches!(self.config.source, AudioSourceType::None) {
            self.running = true;
            return Ok(());
        }

        if !self.capture_active {
            tracing::debug!(
                input = %self.name,
                source = ?self.config.source,
                "Audio input armed but idle until an audio-reactive effect requests capture"
            );
            self.running = true;
            return Ok(());
        }

        let status = self.status.begin_session()?;
        if let Err(error) = self.start_capture_stream() {
            self.status.stop();
            self.drop_capture_stream("startup rolled back");
            self.clear_latest_snapshot();
            return Err(error);
        }
        if self.degraded_to_silence
            && let Some(status) = status
        {
            status.degraded(SourceIssue::new(
                self.last_failure.issue_code(),
                self.last_failure.message(),
                true,
            ));
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.drop_capture_stream("input stopped");
        self.running = false;
        self.capture_active = false;
        self.degraded_to_silence = false;
        self.recovery = None;
        self.last_failure = AudioFailureKind::None;
        self.last_status_snapshot = None;
        self.reset_analyzer();
        self.status.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.sample_with_dt(DEFAULT_AUDIO_FRAME_DT)
    }

    fn sample_with_delta_secs(&mut self, delta_secs: f32) -> anyhow::Result<InputData> {
        self.sample_with_dt(delta_secs)
    }

    fn sample_shared_and_drain_into(
        &mut self,
        delta_secs: f32,
        _events: &mut Vec<TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        self.sample_shared_with_dt(delta_secs)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

impl AudioSource for AudioInput {
    fn reconfigure_audio(
        &mut self,
        config: &AudioPipelineConfig,
        name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        self.reconfigure_live(config, name, capture_active)
    }

    fn supports_prepared_audio_reconfiguration(&self) -> bool {
        true
    }

    fn commit_prepared_audio_reconfiguration(
        &mut self,
        prepared: &mut PreparedAudioReconfiguration,
    ) -> anyhow::Result<AudioRuntimeRetirement> {
        self.commit_prepared(prepared)
    }

    fn set_audio_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.set_capture_active_state(active)
    }
}

impl SourceRoleBinding for AudioInput {
    type Role = AudioSourceRole;
}

impl Drop for AudioInput {
    fn drop(&mut self) {
        if let Some(capture) = self.capture.take() {
            retire_capture_runtime(capture);
        }
        self.recovery = None;
    }
}

fn build_capture_stream(
    host: &cpal::Host,
    config: &AudioPipelineConfig,
) -> anyhow::Result<CaptureRuntime> {
    build_capture_stream_with_failure(
        host,
        config,
        Arc::new(AtomicU8::new(AudioFailureKind::None as u8)),
    )
}

fn build_capture_stream_with_failure(
    host: &cpal::Host,
    config: &AudioPipelineConfig,
    terminal_failure: Arc<AtomicU8>,
) -> anyhow::Result<CaptureRuntime> {
    let selected = select_input_device(host, &config.source)?;
    match selected {
        SelectedInputDevice::Cpal {
            device,
            display_name,
        } => {
            let supported_config = device.default_input_config().with_context(|| {
                format!("failed to get default input config for '{display_name}'")
            })?;
            let stream_config: cpal::StreamConfig = supported_config.config();
            let channels = usize::from(stream_config.channels.max(1));
            let sample_format = supported_config.sample_format();
            let latest_snapshot = Arc::new(ArcSwapOption::empty());
            let analysis = AnalysisWorker::spawn(
                config,
                supported_config.sample_rate(),
                channels,
                Arc::clone(&latest_snapshot),
                Arc::clone(&terminal_failure),
            )?;
            let ring = analysis.ring();
            let stream = match sample_format {
                SampleFormat::I8 => build_stream::<i8>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::I16 => build_stream::<i16>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::I24 => build_stream::<cpal::I24>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::I32 => build_stream::<i32>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::I64 => build_stream::<i64>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::U8 => build_stream::<u8>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::U16 => build_stream::<u16>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::U24 => build_stream::<cpal::U24>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::U32 => build_stream::<u32>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::U64 => build_stream::<u64>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::F32 => build_stream::<f32>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                SampleFormat::F64 => build_stream::<f64>(
                    &device,
                    &stream_config,
                    channels,
                    &ring,
                    &display_name,
                    Arc::clone(&terminal_failure),
                ),
                sample_format => Err(anyhow!("unsupported audio sample format: {sample_format}")),
            }?;
            stream
                .stream
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

            Ok(CaptureRuntime {
                capture: CaptureHandle::Cpal { _capture: stream },
                analysis,
                latest_snapshot,
                observed_drops: 0,
                pending_drop_report: 0,
                last_drop_report: None,
                terminal_failure,
            })
        }
        #[cfg(target_os = "linux")]
        SelectedInputDevice::LinuxPulse {
            source_name,
            display_name,
        } => {
            let latest_snapshot = Arc::new(ArcSwapOption::empty());
            let analysis = AnalysisWorker::spawn(
                config,
                PULSE_CAPTURE_RATE_HZ,
                PULSE_CAPTURE_CHANNELS,
                Arc::clone(&latest_snapshot),
                Arc::clone(&terminal_failure),
            )?;
            let capture = build_linux_pulse_capture_stream(
                &source_name,
                &display_name,
                analysis.ring(),
                Arc::clone(&terminal_failure),
            )?;
            Ok(CaptureRuntime {
                capture,
                analysis,
                latest_snapshot,
                observed_drops: 0,
                pending_drop_report: 0,
                last_drop_report: None,
                terminal_failure,
            })
        }
    }
}

fn build_synthetic_capture_stream(
    config: &AudioPipelineConfig,
    terminal_failure: Arc<AtomicU8>,
) -> anyhow::Result<CaptureRuntime> {
    let latest_snapshot = Arc::new(ArcSwapOption::empty());
    let analysis = AnalysisWorker::spawn(
        config,
        48_000,
        2,
        Arc::clone(&latest_snapshot),
        Arc::clone(&terminal_failure),
    )?;
    Ok(CaptureRuntime {
        capture: CaptureHandle::Synthetic,
        analysis,
        latest_snapshot,
        observed_drops: 0,
        pending_drop_report: 0,
        last_drop_report: None,
        terminal_failure,
    })
}

/// Downgrade a PulseAudio query failure into "Pulse has no answer".
///
/// An unreachable sound server says nothing about whether the requested device
/// exists, so propagating the connection error would strand every ALSA-only or
/// headless machine on an opaque failure instead of letting the cpal lookup
/// below decide.
#[cfg(target_os = "linux")]
fn pulse_lookup<T>(result: anyhow::Result<T>, query: &str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(%error, query, "PulseAudio lookup unavailable; falling back to cpal");
            None
        }
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
            if pulse_lookup(linux_audio::pulse_source_exists(name), "named source").unwrap_or(false)
            {
                return Ok(find_linux_pulse_capture_device(name));
            }

            let device = find_named_input_device(host, name)?
                .ok_or_else(|| anyhow!("audio input device '{name}' not found"))?;
            Ok(SelectedInputDevice::new(device))
        }
        AudioSourceType::SystemMonitor => {
            #[cfg(target_os = "linux")]
            if let Some(selected) = pulse_lookup(
                find_linux_system_monitor_input_device(host),
                "system monitor",
            )
            .flatten()
            {
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
    exit_rx: mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<()>>,
    publish_enabled: Arc<AtomicBool>,
    failure: Arc<AtomicU8>,
}

#[cfg(target_os = "linux")]
impl LinuxPulseCapture {
    fn stop(&mut self) -> bool {
        self.publish_enabled.store(false, Ordering::Release);
        let Some(worker) = self.worker.as_ref() else {
            return true;
        };
        let _ = self.stop_tx.send(());
        let _ = self.exit_rx.recv_timeout(Duration::from_secs(1));
        if !worker.is_finished() {
            tracing::warn!(
                "PulseAudio capture worker did not stop before the deadline; retaining its join handle"
            );
            return false;
        }
        let worker = self
            .worker
            .take()
            .expect("finished PulseAudio worker remains owned");
        let _ = worker.join();
        true
    }

    fn observe_exit(&mut self) -> Option<AudioFailureKind> {
        let code = self
            .failure
            .swap(AudioFailureKind::None as u8, Ordering::AcqRel);
        let failure = AudioFailureKind::from_code(code);
        if failure != AudioFailureKind::None {
            return Some(failure);
        }
        let worker = self.worker.as_ref()?;
        if !worker.is_finished() {
            return None;
        }
        let worker = self
            .worker
            .take()
            .expect("finished PulseAudio worker remains owned");
        let _ = worker.join();
        Some(AudioFailureKind::BackendUnavailable)
    }
}

#[cfg(target_os = "linux")]
impl Drop for LinuxPulseCapture {
    fn drop(&mut self) {
        if self.stop() {
            return;
        }
        let Some(worker) = self.worker.take() else {
            return;
        };
        retain_worker(worker, "PulseAudio capture worker");
    }
}

enum CaptureHandle {
    Cpal {
        _capture: CpalCapture,
    },
    Synthetic,
    #[cfg(target_os = "linux")]
    LinuxPulse(LinuxPulseCapture),
}

struct CpalCapture {
    stream: Stream,
}

impl CaptureHandle {
    fn backend(&self) -> AudioCaptureBackend {
        match self {
            Self::Cpal { .. } => AudioCaptureBackend::Cpal,
            Self::Synthetic => AudioCaptureBackend::Cpal,
            #[cfg(target_os = "linux")]
            Self::LinuxPulse(_) => AudioCaptureBackend::PulseAudio,
        }
    }

    fn observe_failure(&mut self) -> Option<AudioFailureKind> {
        match self {
            Self::Cpal { .. } => None,
            Self::Synthetic => None,
            #[cfg(target_os = "linux")]
            Self::LinuxPulse(capture) => capture.observe_exit(),
        }
    }
}

struct CaptureRuntime {
    capture: CaptureHandle,
    analysis: AnalysisWorker,
    latest_snapshot: Arc<ArcSwapOption<CapturedAudioSnapshot>>,
    observed_drops: u64,
    pending_drop_report: u64,
    last_drop_report: Option<Instant>,
    terminal_failure: Arc<AtomicU8>,
}

impl CaptureRuntime {
    fn backend(&self) -> AudioCaptureBackend {
        self.capture.backend()
    }

    fn observe_failure(&mut self) -> Option<AudioFailureKind> {
        let failure = AudioFailureKind::from_code(
            self.terminal_failure
                .swap(AudioFailureKind::None as u8, Ordering::AcqRel),
        );
        if failure != AudioFailureKind::None {
            return Some(failure);
        }
        if self.analysis.is_finished() {
            return Some(AudioFailureKind::AnalysisWorkerExited);
        }
        self.capture.observe_failure()
    }

    fn take_drop_report(&mut self, now: Instant) -> Option<(u64, u64)> {
        let total = self.analysis.ring.dropped_total();
        self.pending_drop_report = self
            .pending_drop_report
            .saturating_add(total.saturating_sub(self.observed_drops));
        self.observed_drops = total;
        if self.pending_drop_report == 0
            || self
                .last_drop_report
                .is_some_and(|last| now.duration_since(last) < AUDIO_OVERFLOW_REPORT_INTERVAL)
        {
            return None;
        }
        let pending = std::mem::take(&mut self.pending_drop_report);
        self.last_drop_report = Some(now);
        Some((pending, total))
    }
}

fn retire_capture_runtime(capture: CaptureRuntime) {
    retire_audio_runtime(AudioRuntimeRetirement::new(Some(capture), None));
}

fn retire_audio_runtime(runtime: AudioRuntimeRetirement) {
    match spawn_worker(
        thread::Builder::new().name("hypercolor-audio-retire".to_owned()),
        move || drop(runtime),
    ) {
        Ok(worker) => retain_worker(worker, "audio capture retirement"),
        Err(error) => tracing::warn!(%error, "Failed to spawn audio capture retirement worker"),
    }
}

#[cfg(target_os = "linux")]
fn build_linux_pulse_capture_stream(
    source_name: &str,
    display_name: &str,
    ring: Arc<AudioFrameRing>,
    failure: Arc<AtomicU8>,
) -> anyhow::Result<CaptureHandle> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (stop_tx, stop_rx) = mpsc::channel();
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let publish_enabled = Arc::new(AtomicBool::new(true));
    let worker_publish_enabled = Arc::clone(&publish_enabled);
    let worker_failure = Arc::clone(&failure);
    let worker_source = source_name.to_owned();
    let worker_display = display_name.to_owned();
    let worker = spawn_worker(
        thread::Builder::new().name("hypercolor-pulse-capture".to_owned()),
        move || {
            run_linux_pulse_capture(
                &worker_source,
                &worker_display,
                ring,
                ready_tx,
                stop_rx,
                PULSE_CAPTURE_CHANNELS,
                PULSE_CAPTURE_RATE_HZ,
                worker_publish_enabled,
                worker_failure,
            );
            let _ = exit_tx.send(());
        },
    )
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
                exit_rx,
                worker: Some(worker),
                publish_enabled,
                failure,
            }))
        }
        Ok(Err(error)) => {
            let mut capture = LinuxPulseCapture {
                stop_tx,
                exit_rx,
                worker: Some(worker),
                publish_enabled,
                failure,
            };
            capture.stop();
            Err(anyhow!(
                "failed to start Linux PulseAudio capture worker: {error}"
            ))
        }
        Err(error) => {
            let mut capture = LinuxPulseCapture {
                stop_tx,
                exit_rx,
                worker: Some(worker),
                publish_enabled,
                failure,
            };
            capture.stop();
            Err(anyhow!(
                "timed out waiting for Linux PulseAudio capture worker to start: {error}"
            ))
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    reason = "worker startup passes a fixed set of runtime parameters into the capture thread"
)]
fn run_linux_pulse_capture(
    source_name: &str,
    display_name: &str,
    ring: Arc<AudioFrameRing>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
    stop_rx: mpsc::Receiver<()>,
    channels: usize,
    sample_rate_hz: u32,
    publish_enabled: Arc<AtomicBool>,
    failure: Arc<AtomicU8>,
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
    let buffer_attr = pulse_record_buffer_attr(&spec);
    let stream_for_callback = Rc::clone(&stream);
    let callback_ring = Arc::clone(&ring);
    stream
        .borrow_mut()
        .set_read_callback(Some(Box::new(move |_bytes_available| {
            let mut guard = stream_for_callback.borrow_mut();
            loop {
                match guard.peek() {
                    Ok(pulse::stream::PeekResult::Data(bytes)) => {
                        if publish_enabled.load(Ordering::Acquire) {
                            push_native_f32_bytes(bytes, channels, &callback_ring);
                        }
                        if let Err(error) = guard.discard() {
                            let _ = error;
                            failure.store(
                                AudioFailureKind::BackendUnavailable as u8,
                                Ordering::Release,
                            );
                            break;
                        }
                    }
                    Ok(pulse::stream::PeekResult::Hole(_)) => {
                        if let Err(error) = guard.discard() {
                            let _ = error;
                            failure.store(
                                AudioFailureKind::BackendUnavailable as u8,
                                Ordering::Release,
                            );
                            break;
                        }
                    }
                    Ok(pulse::stream::PeekResult::Empty) => break,
                    Err(error) => {
                        let _ = error;
                        failure.store(
                            AudioFailureKind::BackendUnavailable as u8,
                            Ordering::Release,
                        );
                        break;
                    }
                }
            }
        })));

    if let Err(error) = stream.borrow_mut().connect_record(
        Some(source_name),
        Some(&buffer_attr),
        pulse_record_stream_flags(),
    ) {
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

    let actual_fragsize = stream
        .borrow_mut()
        .get_buffer_attr()
        .map(|attr| attr.fragsize);
    tracing::info!(
        source = %source_name,
        target_fragment_bytes = buffer_attr.fragsize,
        target_fragment_ms = PULSE_CAPTURE_TARGET_LATENCY_MS,
        max_buffer_bytes = buffer_attr.maxlength,
        actual_fragment_bytes = actual_fragsize,
        "PulseAudio record buffer configured for low-latency capture"
    );

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

        match stop_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    let _ = stream.borrow_mut().disconnect();
    context.disconnect();
}

#[cfg(target_os = "linux")]
fn pulse_record_buffer_attr(spec: &pulse::sample::Spec) -> pulse::def::BufferAttr {
    let target_latency = pulse::time::MicroSeconds(PULSE_CAPTURE_TARGET_LATENCY_MS * 1_000);
    let frame_size = u32::try_from(spec.frame_size()).unwrap_or(1);
    let fragment_bytes = u32::try_from(spec.usec_to_bytes(target_latency))
        .unwrap_or(u32::MAX)
        .max(frame_size);
    let max_buffer_bytes = fragment_bytes.saturating_mul(PULSE_CAPTURE_MAX_BUFFER_FRAGMENTS);

    pulse::def::BufferAttr {
        // The Pulse defaults are on the order of seconds, which makes monitor
        // capture look alive but badly delayed after pause/play transitions.
        maxlength: max_buffer_bytes,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: fragment_bytes,
    }
}

#[cfg(target_os = "linux")]
fn pulse_record_stream_flags() -> pulse::stream::FlagSet {
    pulse::stream::FlagSet::ADJUST_LATENCY
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

        match stop_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!("PulseAudio capture stopped {phase}"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
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

        match stop_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(
                    "PulseAudio capture stopped before record stream became ready".to_owned(),
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(target_os = "linux")]
fn push_native_f32_bytes(bytes: &[u8], channels: usize, ring: &AudioFrameRing) {
    let channel_count = channels.max(1);
    debug_assert_eq!(ring.channels(), channel_count);
    let frame_bytes = std::mem::size_of::<f32>().saturating_mul(channel_count);
    let mut stats = realtime::PushStats::default();
    for frame in bytes.chunks_exact(frame_bytes) {
        let pushed = ring.push_frame_with(|channel| {
            let start = channel * std::mem::size_of::<f32>();
            let sample = frame[start..start + std::mem::size_of::<f32>()]
                .try_into()
                .expect("f32 audio chunks have exact width");
            f32::from_ne_bytes(sample)
        });
        stats.accepted += pushed.accepted;
        stats.dropped += pushed.dropped;
    }
    ring.record(stats);
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    ring: &Arc<AudioFrameRing>,
    device_name: &str,
    failure: Arc<AtomicU8>,
) -> anyhow::Result<CpalCapture>
where
    T: Sample + SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    debug_assert_eq!(ring.channels(), channels.max(1));
    let callback_ring = Arc::clone(ring);
    let callback_failure = Arc::clone(&failure);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let _ = push_interleaved_frames(data, &callback_ring, f32::from_sample);
            },
            move |error| {
                let failure = match error {
                    cpal::StreamError::DeviceNotAvailable
                    | cpal::StreamError::StreamInvalidated => AudioFailureKind::DeviceLost,
                    cpal::StreamError::BackendSpecific { .. } => {
                        AudioFailureKind::BackendUnavailable
                    }
                    cpal::StreamError::BufferUnderrun => AudioFailureKind::None,
                };
                if failure != AudioFailureKind::None {
                    callback_failure.store(failure as u8, Ordering::Release);
                }
            },
            None,
        )
        .with_context(|| format!("failed to build audio capture stream for '{device_name}'"))?;
    Ok(CpalCapture { stream })
}

fn publish_latest_snapshot(
    analyzer: &mut AudioAnalyzer,
    sample_count: usize,
    latest_snapshot: &Arc<ArcSwapOption<CapturedAudioSnapshot>>,
) {
    match analyzer.analyze(analysis_dt_seconds(sample_count, analyzer.sample_rate_hz())) {
        Ok(Some(snapshot)) => latest_snapshot.store(Some(Arc::new(CapturedAudioSnapshot {
            data: Arc::new(InputData::Audio(snapshot)),
            captured_at: Instant::now(),
        }))),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "Audio snapshot analysis failed"),
    }
}

fn analysis_dt_seconds(sample_count: usize, sample_rate_hz: u32) -> f32 {
    if sample_rate_hz == 0 {
        return DEFAULT_AUDIO_FRAME_DT;
    }

    let safe_sample_count = u32::try_from(sample_count.max(1)).unwrap_or(u32::MAX);
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "audio callback chunk durations only need frame-scale precision"
    )]
    let dt = (f64::from(safe_sample_count) / f64::from(sample_rate_hz)) as f32;
    dt
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn pulse_record_buffer_attr_targets_twenty_milliseconds() {
        let spec = pulse::sample::Spec {
            format: pulse::sample::Format::FLOAT32NE,
            channels: 2,
            rate: 48_000,
        };

        let attr = pulse_record_buffer_attr(&spec);

        assert_eq!(attr.fragsize, 7_680);
        assert_eq!(attr.maxlength, 30_720);
    }
}
