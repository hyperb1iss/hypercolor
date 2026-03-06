//! Beat detection — spectral flux onset, adaptive threshold, BPM estimation.
//!
//! Pure computation on per-frame energy and spectrum data. No OS dependencies.
//! Three complementary methods fuse into a single beat signal:
//!
//! 1. **Energy onset** — instantaneous band energy vs. running average
//! 2. **Spectral flux** — rate of spectral change with adaptive threshold
//! 3. **Tempo tracker** — inter-onset interval histogram for BPM estimation

use std::collections::VecDeque;

// ── Constants ────────────────────────────────────────────────────────────

/// Maximum onset history (seconds). Older onsets are discarded.
const ONSET_HISTORY_SECS: f64 = 10.0;

/// Spectral flux history length (~1 second at 60 fps).
const FLUX_HISTORY_LEN: usize = 60;

/// Minimum BPM for normalization.
const BPM_MIN: f32 = 60.0;

/// Maximum BPM for normalization.
const BPM_MAX: f32 = 180.0;

/// Beat pulse decay rate (units per second). ~200ms to reach zero.
const PULSE_DECAY_RATE: f32 = 5.0;

/// Default minimum inter-onset interval (seconds). Caps at ~400 BPM.
const DEFAULT_MIN_INTERVAL: f32 = 0.15;

/// Default energy onset threshold multiplier.
const DEFAULT_THRESHOLD: f32 = 1.5;

/// Fast EMA alpha for short-term energy tracking.
const ENERGY_SHORT_ALPHA: f32 = 0.4;

/// Slow EMA alpha for long-term energy baseline.
const ENERGY_LONG_ALPHA: f32 = 0.02;

/// Minimum number of onset intervals needed for a BPM estimate.
const MIN_INTERVALS_FOR_BPM: usize = 4;

// ── Energy Onset Detector ────────────────────────────────────────────────

/// Detects energy-based onsets in a single frequency band.
///
/// Compares a fast EMA (short-term energy) against a slow EMA (long-term
/// baseline) with a dynamic threshold. Enforces a minimum cooldown between
/// consecutive onsets to prevent retriggering.
pub struct EnergyOnsetDetector {
    energy_short: f32,
    energy_long: f32,
    cooldown: f32,
    min_interval: f32,
    threshold: f32,
}

impl EnergyOnsetDetector {
    /// Create a detector with the given sensitivity threshold.
    #[must_use]
    pub fn new(threshold: f32) -> Self {
        Self {
            energy_short: 0.0,
            energy_long: 0.0,
            cooldown: 0.0,
            min_interval: DEFAULT_MIN_INTERVAL,
            threshold,
        }
    }

    /// Feed a new energy value and time delta. Returns `true` if an onset is detected.
    pub fn update(&mut self, energy: f32, dt: f32) -> bool {
        self.energy_short = lerp(self.energy_short, energy, ENERGY_SHORT_ALPHA);
        self.energy_long = lerp(self.energy_long, energy, ENERGY_LONG_ALPHA);
        self.cooldown -= dt;

        let dynamic_thresh = self.energy_long * self.threshold + 0.02;

        if self.energy_short > dynamic_thresh && self.cooldown <= 0.0 {
            self.cooldown = self.min_interval;
            true
        } else {
            false
        }
    }

    /// Reset detector state (e.g. on source change).
    pub fn reset(&mut self) {
        self.energy_short = 0.0;
        self.energy_long = 0.0;
        self.cooldown = 0.0;
    }
}

// ── Spectral Flux Detector ───────────────────────────────────────────────

/// Detects onsets via spectral flux — the rate of spectral change.
///
/// Captures onsets that pure energy detection misses, like a snare hit
/// during sustained bass. Uses an adaptive threshold based on running
/// mean + standard deviation of recent flux values.
pub struct SpectralFluxDetector {
    flux_history: VecDeque<f32>,
    threshold_multiplier: f32,
}

impl SpectralFluxDetector {
    /// Create a detector with the given threshold multiplier.
    #[must_use]
    pub fn new(threshold_multiplier: f32) -> Self {
        Self {
            flux_history: VecDeque::with_capacity(FLUX_HISTORY_LEN + 1),
            threshold_multiplier,
        }
    }

    /// Record a new flux value (typically from [`crate::input::audio::fft::spectral_flux`]).
    pub fn push(&mut self, flux: f32) {
        self.flux_history.push_back(flux);
        while self.flux_history.len() > FLUX_HISTORY_LEN {
            self.flux_history.pop_front();
        }
    }

    /// Check if the most recent flux value exceeds the adaptive threshold.
    #[must_use]
    pub fn is_onset(&self) -> bool {
        let Some(&flux) = self.flux_history.back() else {
            return false;
        };
        if self.flux_history.len() < 2 {
            return false;
        }

        let (mean, std_dev) = self.stats();
        flux > mean + self.threshold_multiplier * std_dev
    }

    /// Compute mean and standard deviation of the flux history.
    fn stats(&self) -> (f32, f32) {
        if self.flux_history.is_empty() {
            return (0.0, 0.0);
        }
        #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
        let count = self.flux_history.len() as f32;
        let mean: f32 = self.flux_history.iter().sum::<f32>() / count;
        let variance: f32 = self
            .flux_history
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f32>()
            / count;
        (mean, variance.sqrt())
    }

    /// Reset the flux history.
    pub fn reset(&mut self) {
        self.flux_history.clear();
    }
}

// ── Tempo Tracker ────────────────────────────────────────────────────────

/// Estimates BPM from inter-onset intervals and tracks beat phase.
///
/// Maintains a history of onset timestamps, computes intervals between them,
/// clusters intervals to find the dominant tempo, and tracks phase
/// (0.0 = on beat, 1.0 = just before next beat).
pub struct TempoTracker {
    onset_times: VecDeque<f64>,
    bpm: f32,
    phase: f32,
    confidence: f32,
    last_beat_time: f64,
    beat_interval: f64,
}

impl TempoTracker {
    /// Create a new tempo tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            onset_times: VecDeque::with_capacity(128),
            bpm: 0.0,
            phase: 0.0,
            confidence: 0.0,
            last_beat_time: 0.0,
            beat_interval: 0.0,
        }
    }

    /// Record an onset at the given timestamp (seconds since start).
    pub fn record_onset(&mut self, time: f64) {
        self.onset_times.push_back(time);

        // Prune old onsets.
        while let Some(&front) = self.onset_times.front() {
            if time - front > ONSET_HISTORY_SECS {
                self.onset_times.pop_front();
            } else {
                break;
            }
        }

        self.last_beat_time = time;
        self.estimate_bpm();
    }

    /// Update phase tracking for the current timestamp (call every frame).
    pub fn update_phase(&mut self, current_time: f64) {
        if self.beat_interval <= 0.0 {
            self.phase = 0.0;
            return;
        }

        let elapsed = current_time - self.last_beat_time;
        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let phase = ((elapsed / self.beat_interval) % 1.0).clamp(0.0, 1.0) as f32;
        self.phase = phase;
    }

    /// Current estimated BPM (0.0 if unknown).
    #[must_use]
    pub fn bpm(&self) -> f32 {
        self.bpm
    }

    /// Beat phase: 0.0 = on beat, 1.0 = just before next beat.
    #[must_use]
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Confidence in the current BPM estimate (0.0–1.0).
    #[must_use]
    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    /// Beat interval in seconds (0.0 if no tempo established).
    #[must_use]
    pub fn beat_interval(&self) -> f64 {
        self.beat_interval
    }

    /// Reset all tracking state.
    pub fn reset(&mut self) {
        self.onset_times.clear();
        self.bpm = 0.0;
        self.phase = 0.0;
        self.confidence = 0.0;
        self.last_beat_time = 0.0;
        self.beat_interval = 0.0;
    }

    /// Estimate BPM from inter-onset intervals using a simple median approach.
    fn estimate_bpm(&mut self) {
        if self.onset_times.len() < 2 {
            self.confidence = 0.0;
            return;
        }

        // Collect inter-onset intervals.
        let intervals: Vec<f64> = self
            .onset_times
            .iter()
            .zip(self.onset_times.iter().skip(1))
            .map(|(&a, &b)| b - a)
            .filter(|&interval| interval > 0.05 && interval < 2.0) // 30–1200 BPM range
            .collect();

        if intervals.len() < MIN_INTERVALS_FOR_BPM {
            self.confidence = 0.0;
            return;
        }

        // Use median interval for robustness against outliers.
        let mut sorted = intervals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];

        if median < f64::EPSILON {
            self.confidence = 0.0;
            return;
        }

        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let raw_bpm = (60.0 / median) as f32;
        self.bpm = normalize_bpm(raw_bpm);
        self.beat_interval = if self.bpm > 0.0 {
            60.0 / f64::from(self.bpm)
        } else {
            0.0
        };

        // Confidence: how consistent are the intervals?
        // Low variance relative to median = high confidence.
        #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
        let interval_count = intervals.len() as f64;
        let mean_interval: f64 = intervals.iter().sum::<f64>() / interval_count;
        let variance: f64 = intervals
            .iter()
            .map(|&x| (x - mean_interval).powi(2))
            .sum::<f64>()
            / interval_count;
        let cv = (variance.sqrt() / mean_interval).clamp(0.0, 1.0);
        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let cv_f32 = cv as f32;
        self.confidence = (1.0 - cv_f32).clamp(0.0, 1.0);
    }
}

impl Default for TempoTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Beat Pulse ───────────────────────────────────────────────────────────

/// Sharp-attack, exponential-decay beat pulse envelope.
///
/// Jumps to 1.0 on beat, decays at `PULSE_DECAY_RATE` per second.
/// Perfect for driving brightness spikes and particle bursts.
pub struct BeatPulse {
    value: f32,
}

impl BeatPulse {
    /// Create a new pulse envelope at zero.
    #[must_use]
    pub fn new() -> Self {
        Self { value: 0.0 }
    }

    /// Update the pulse. `beat` = true triggers a spike to 1.0.
    pub fn update(&mut self, beat: bool, dt: f32) {
        if beat {
            self.value = 1.0;
        } else {
            self.value = (self.value - PULSE_DECAY_RATE * dt).max(0.0);
        }
    }

    /// Current pulse value (0.0–1.0).
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Reset to zero.
    pub fn reset(&mut self) {
        self.value = 0.0;
    }
}

impl Default for BeatPulse {
    fn default() -> Self {
        Self::new()
    }
}

// ── Beat Detector (Composite) ────────────────────────────────────────────

/// Composite beat detector fusing energy onset, spectral flux, and tempo tracking.
///
/// Call [`update`](BeatDetector::update) once per frame with the current
/// spectrum and band energies. The detector handles all internal state.
pub struct BeatDetector {
    bass_onset: EnergyOnsetDetector,
    mid_onset: EnergyOnsetDetector,
    treble_onset: EnergyOnsetDetector,
    flux_detector: SpectralFluxDetector,
    tempo: TempoTracker,
    beat_pulse: BeatPulse,
    onset_pulse: BeatPulse,
}

impl BeatDetector {
    /// Create a new composite beat detector with the given sensitivity.
    #[must_use]
    pub fn new(sensitivity: f32) -> Self {
        Self {
            bass_onset: EnergyOnsetDetector::new(sensitivity),
            mid_onset: EnergyOnsetDetector::new(sensitivity * 1.2),
            treble_onset: EnergyOnsetDetector::new(sensitivity * 1.5),
            flux_detector: SpectralFluxDetector::new(sensitivity),
            tempo: TempoTracker::new(),
            beat_pulse: BeatPulse::new(),
            onset_pulse: BeatPulse::new(),
        }
    }

    /// Process one frame of audio analysis.
    ///
    /// `bass`, `mid`, `treble` are band energies. `spectral_flux` is the
    /// half-wave rectified spectral change. `dt` is the time delta in seconds.
    /// `current_time` is the absolute timestamp for tempo tracking.
    pub fn update(&mut self, frame: &BeatFrame) -> BeatState {
        // Energy onset detection per band.
        let bass_hit = self.bass_onset.update(frame.bass, frame.dt);
        let mid_hit = self.mid_onset.update(frame.mid, frame.dt);
        let treble_hit = self.treble_onset.update(frame.treble, frame.dt);

        // Spectral flux onset.
        self.flux_detector.push(frame.spectral_flux);
        let flux_hit = self.flux_detector.is_onset();

        // Any onset is a transient.
        let onset_detected = bass_hit || mid_hit || treble_hit || flux_hit;

        // Beat = bass onset or strong spectral flux onset.
        // Bass is the primary beat carrier in most music.
        let beat_detected = bass_hit || (flux_hit && frame.bass > 0.1);

        // Update tempo tracker on beat.
        if beat_detected {
            self.tempo.record_onset(frame.current_time);
        }
        self.tempo.update_phase(frame.current_time);

        // Update pulse envelopes.
        self.beat_pulse.update(beat_detected, frame.dt);
        self.onset_pulse.update(onset_detected, frame.dt);

        BeatState {
            beat_detected,
            onset_detected,
            beat_confidence: self.tempo.confidence(),
            bpm: self.tempo.bpm(),
            beat_pulse: self.beat_pulse.value(),
            onset_pulse: self.onset_pulse.value(),
        }
    }

    /// Current beat phase from the internal tempo tracker.
    #[must_use]
    pub fn phase(&self) -> f32 {
        self.tempo.phase()
    }

    /// Reset all detection state.
    pub fn reset(&mut self) {
        self.bass_onset.reset();
        self.mid_onset.reset();
        self.treble_onset.reset();
        self.flux_detector.reset();
        self.tempo.reset();
        self.beat_pulse.reset();
        self.onset_pulse.reset();
    }
}

impl Default for BeatDetector {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD)
    }
}

/// Input data for one beat detection frame.
pub struct BeatFrame {
    /// Bass band energy (0.0–1.0).
    pub bass: f32,
    /// Mid band energy (0.0–1.0).
    pub mid: f32,
    /// Treble band energy (0.0–1.0).
    pub treble: f32,
    /// Spectral flux from FFT pipeline.
    pub spectral_flux: f32,
    /// Frame time delta in seconds.
    pub dt: f32,
    /// Absolute timestamp in seconds since start.
    pub current_time: f64,
}

/// Beat detection output for one frame.
pub struct BeatState {
    /// True on the frame a beat is detected.
    pub beat_detected: bool,
    /// True on the frame any transient onset is detected.
    pub onset_detected: bool,
    /// Confidence in the current tempo estimate (0.0–1.0).
    pub beat_confidence: f32,
    /// Estimated BPM (0.0 if unknown).
    pub bpm: f32,
    /// Beat pulse envelope (1.0 on beat, decays to 0.0).
    pub beat_pulse: f32,
    /// Onset pulse envelope (1.0 on onset, decays to 0.0).
    pub onset_pulse: f32,
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Linear interpolation.
fn lerp(current: f32, target: f32, alpha: f32) -> f32 {
    current + alpha * (target - current)
}

/// Normalize a raw BPM to the 60–180 range by halving or doubling.
///
/// Returns 0.0 for non-positive or non-finite inputs.
fn normalize_bpm(raw_bpm: f32) -> f32 {
    if !raw_bpm.is_finite() || raw_bpm <= 0.0 {
        return 0.0;
    }
    let mut bpm = raw_bpm;
    while bpm < BPM_MIN {
        bpm *= 2.0;
    }
    while bpm > BPM_MAX {
        bpm /= 2.0;
    }
    bpm
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_bpm_in_range() {
        assert!((normalize_bpm(120.0) - 120.0).abs() < f32::EPSILON);
    }

    #[test]
    fn normalize_bpm_doubles_slow() {
        assert!((normalize_bpm(30.0) - 60.0).abs() < f32::EPSILON);
        assert!((normalize_bpm(45.0) - 90.0).abs() < f32::EPSILON);
    }

    #[test]
    fn normalize_bpm_halves_fast() {
        assert!((normalize_bpm(240.0) - 120.0).abs() < f32::EPSILON);
        // 360 / 2 = 180, which is exactly at BPM_MAX — stays there.
        assert!((normalize_bpm(360.0) - 180.0).abs() < f32::EPSILON);
        // 400 / 2 = 200 > 180, so 200 / 2 = 100.
        assert!((normalize_bpm(400.0) - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn beat_pulse_spike_and_decay() {
        let mut pulse = BeatPulse::new();
        pulse.update(true, 0.016);
        assert!((pulse.value() - 1.0).abs() < f32::EPSILON);

        // Decay for 200ms (~12 frames at 60fps).
        for _ in 0..12 {
            pulse.update(false, 0.016);
        }
        assert!(
            pulse.value() < 0.1,
            "pulse should have decayed: {}",
            pulse.value()
        );
    }

    #[test]
    fn beat_pulse_reset() {
        let mut pulse = BeatPulse::new();
        pulse.update(true, 0.016);
        pulse.reset();
        assert!(pulse.value().abs() < f32::EPSILON);
    }

    #[test]
    fn energy_onset_fires_on_spike() {
        let mut detector = EnergyOnsetDetector::new(1.5);

        // Feed silence for a while to establish baseline.
        for _ in 0..60 {
            detector.update(0.01, 0.016);
        }

        // Spike — should trigger.
        let fired = detector.update(0.8, 0.016);
        assert!(fired, "onset should fire on energy spike");
    }

    #[test]
    fn energy_onset_respects_cooldown() {
        let mut detector = EnergyOnsetDetector::new(1.5);

        // Establish baseline.
        for _ in 0..60 {
            detector.update(0.01, 0.016);
        }

        // First spike fires.
        assert!(detector.update(0.8, 0.016));

        // Immediate second spike should NOT fire (cooldown).
        assert!(!detector.update(0.8, 0.001));
    }

    #[test]
    fn spectral_flux_detector_onset() {
        let mut detector = SpectralFluxDetector::new(1.5);

        // Feed stable flux to establish baseline.
        for _ in 0..30 {
            detector.push(0.01);
        }

        // Spike in flux.
        detector.push(0.5);
        assert!(detector.is_onset(), "should detect onset on flux spike");
    }

    #[test]
    fn spectral_flux_detector_no_false_positive() {
        let mut detector = SpectralFluxDetector::new(1.5);

        // Feed uniform flux.
        for _ in 0..60 {
            detector.push(0.1);
        }

        assert!(!detector.is_onset(), "should not fire on steady-state flux");
    }

    #[test]
    fn tempo_tracker_basic_bpm() {
        let mut tracker = TempoTracker::new();
        // Simulate beats at 120 BPM = 0.5s intervals.
        for i in 0..10 {
            tracker.record_onset(f64::from(i) * 0.5);
        }

        let bpm = tracker.bpm();
        assert!((bpm - 120.0).abs() < 5.0, "expected ~120 BPM, got {bpm}");
        assert!(tracker.confidence() > 0.5, "confidence should be decent");
    }

    #[test]
    fn tempo_tracker_phase_ramps() {
        let mut tracker = TempoTracker::new();
        // Establish 120 BPM.
        for i in 0..10 {
            tracker.record_onset(f64::from(i) * 0.5);
        }

        // Phase at beat time should be ~0.
        tracker.update_phase(4.5);
        let phase_at_beat = tracker.phase();
        assert!(
            phase_at_beat < 0.1,
            "phase at beat should be near 0: {phase_at_beat}"
        );

        // Phase halfway between beats should be ~0.5.
        tracker.update_phase(4.75);
        let phase_mid = tracker.phase();
        assert!(
            (phase_mid - 0.5).abs() < 0.15,
            "phase midway should be ~0.5: {phase_mid}"
        );
    }

    #[test]
    fn tempo_tracker_reset() {
        let mut tracker = TempoTracker::new();
        tracker.record_onset(0.0);
        tracker.record_onset(0.5);
        tracker.reset();
        assert!(tracker.bpm().abs() < f32::EPSILON);
        assert!(tracker.confidence().abs() < f32::EPSILON);
    }

    #[test]
    fn lerp_basic() {
        assert!((lerp(0.0, 1.0, 0.5) - 0.5).abs() < f32::EPSILON);
        assert!((lerp(0.0, 1.0, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((lerp(0.0, 1.0, 1.0) - 1.0).abs() < f32::EPSILON);
    }
}
