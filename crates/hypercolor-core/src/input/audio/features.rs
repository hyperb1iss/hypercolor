//! Audio feature extraction — mel filterbank, chromagram, band energy, RMS, smoothing.
//!
//! Pure computation on `&[f32]` magnitude buffers. No OS dependencies.
//! All features operate on the linear-bin FFT output (`fft_size/2 + 1` bins)
//! or on the 200-bin log-frequency spectrum, as documented per function.

use crate::types::audio::{CHROMA_BINS, SPECTRUM_BINS};

#[cfg(test)]
use crate::types::audio::MEL_BANDS;

// ── Mel Filterbank ───────────────────────────────────────────────────────

/// 24-band mel-scale filterbank.
///
/// Each filter is a sparse vector of `(fft_bin_index, weight)` pairs forming
/// a triangular shape. Constructed once at startup, then reused every frame.
pub struct MelFilterbank {
    filters: Vec<Vec<(usize, f32)>>,
}

impl MelFilterbank {
    /// Build a mel filterbank for the given FFT size and sample rate.
    ///
    /// Generates `n_mels` triangular filters spanning 20 Hz to `min(nyquist, 20 kHz)`.
    #[must_use]
    pub fn new(fft_size: usize, sample_rate: u32, n_mels: usize) -> Self {
        #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
        let nyquist = sample_rate as f32 / 2.0;
        let mel_min = hz_to_mel(20.0);
        let mel_max = hz_to_mel(nyquist.min(20_000.0));
        let half = fft_size / 2;

        // n_mels + 2 mel points define n_mels triangular filters.
        let mel_points: Vec<f32> = (0..=n_mels + 1)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let t = i as f32 / (n_mels + 1) as f32;
                mel_min + (mel_max - mel_min) * t
            })
            .collect();

        let freq_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();

        #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
        let half_f = half as f32;
        let bin_points: Vec<usize> = freq_points
            .iter()
            .map(|&f| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::as_conversions
                )]
                let bin = (f / nyquist * half_f) as usize;
                bin
            })
            .collect();

        let mut filters = Vec::with_capacity(n_mels);
        for i in 0..n_mels {
            let mut filter = Vec::new();
            let lo = bin_points[i];
            let center = bin_points[i + 1];
            let hi = bin_points[i + 2];

            // Rising slope: lo -> center
            if center > lo {
                for b in lo..center {
                    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                    let weight = (b - lo) as f32 / (center - lo) as f32;
                    filter.push((b, weight));
                }
            }

            // Falling slope: center -> hi (inclusive)
            let denom = if hi > center { hi - center } else { 1 };
            for b in center..=hi {
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let weight = (hi.saturating_sub(b)) as f32 / denom as f32;
                filter.push((b, weight));
            }

            filters.push(filter);
        }

        Self { filters }
    }

    /// Apply the filterbank to FFT magnitude data, producing `MEL_BANDS` energy values.
    ///
    /// `fft_magnitudes` should be the linear-bin magnitudes (`fft_size/2 + 1` entries).
    #[must_use]
    pub fn apply(&self, fft_magnitudes: &[f32]) -> Vec<f32> {
        self.filters
            .iter()
            .map(|filter| {
                filter
                    .iter()
                    .map(|&(bin, weight)| fft_magnitudes.get(bin).copied().unwrap_or(0.0) * weight)
                    .sum()
            })
            .collect()
    }

    /// Number of mel bands.
    #[must_use]
    pub fn num_bands(&self) -> usize {
        self.filters.len()
    }

    /// Access the filter shapes (for testing/debugging).
    #[must_use]
    pub fn filters(&self) -> &[Vec<(usize, f32)>] {
        &self.filters
    }
}

/// Convert Hz to mel scale: `mel = 2595 * log10(1 + f/700)`.
#[must_use]
pub fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// Convert mel to Hz: `f = 700 * (10^(mel/2595) - 1)`.
#[must_use]
pub fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0)
}

// ── Chromagram ───────────────────────────────────────────────────────────

/// Compute a 12-bin chromagram from linear FFT magnitudes.
///
/// Maps each FFT bin to a pitch class (C=0 through B=11) and sums
/// squared magnitudes. The result is normalized so the maximum bin = 1.0.
///
/// Uses A4 = 440 Hz reference. Bins below 20 Hz or above 10 kHz are ignored.
#[must_use]
pub fn compute_chromagram(fft_magnitudes: &[f32], sample_rate: u32, fft_size: usize) -> Vec<f32> {
    let mut chroma = vec![0.0_f32; CHROMA_BINS];
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let freq_resolution = sample_rate as f32 / fft_size as f32;

    for (bin, &magnitude) in fft_magnitudes.iter().enumerate().skip(1) {
        #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
        let freq = bin as f32 * freq_resolution;
        if !(20.0..=10_000.0).contains(&freq) {
            continue;
        }

        // Semitones from A4, mapped to pitch class 0–11.
        let semitones = 12.0 * (freq / 440.0).log2();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::as_conversions,
            clippy::cast_sign_loss
        )]
        let pitch_class = ((semitones.round() as i32 % 12 + 12) % 12) as usize;

        // Shift so C=0: raw formula gives A=0, and C is 3 above A in the raw
        // numbering, so (raw + 9) % 12 maps A→9, C→0.
        let mapped = (pitch_class + 9) % 12;
        chroma[mapped] += magnitude * magnitude; // Energy = squared magnitude
    }

    // Normalize: max bin = 1.0.
    let max = chroma.iter().copied().fold(0.0_f32, f32::max);
    if max > 0.0 {
        for c in &mut chroma {
            *c /= max;
        }
    }

    chroma
}

// ── Band Energy ──────────────────────────────────────────────────────────

/// Bass range bin boundary in the 200-bin spectrum.
const BASS_END: usize = 40;
/// Mid range bin boundary in the 200-bin spectrum.
const MID_END: usize = 130;

/// Compute bass, mid, and treble band energies from the 200-bin spectrum.
///
/// Returns `(bass, mid, treble)`, each normalized 0.0–1.0. Uses RMS of
/// the respective bin ranges.
#[must_use]
pub fn band_energies(spectrum_200: &[f32]) -> (f32, f32, f32) {
    let bass = band_rms(spectrum_200, 0, BASS_END);
    let mid = band_rms(spectrum_200, BASS_END, MID_END);
    let treble = band_rms(spectrum_200, MID_END, SPECTRUM_BINS);
    (bass, mid, treble)
}

/// RMS of a spectrum slice.
fn band_rms(spectrum: &[f32], start: usize, end: usize) -> f32 {
    let end = end.min(spectrum.len());
    let start = start.min(end);
    let slice = &spectrum[start..end];
    if slice.is_empty() {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let count = slice.len() as f32;
    (slice.iter().map(|x| x * x).sum::<f32>() / count).sqrt()
}

// ── RMS Level ────────────────────────────────────────────────────────────

/// Compute the RMS level of a time-domain sample buffer.
///
/// Returns a value in [0.0, 1.0] assuming input samples are in [-1.0, 1.0].
#[must_use]
pub fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let count = samples.len() as f32;
    (samples.iter().map(|&s| s * s).sum::<f32>() / count).sqrt()
}

/// Compute the peak sample magnitude.
#[must_use]
pub fn compute_peak(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max)
}

/// Compute spectral centroid (brightness) from the 200-bin spectrum.
///
/// Returns a value in [0.0, 1.0] where 0.0 means all energy at the lowest
/// bin and 1.0 means all energy at the highest bin.
#[must_use]
pub fn spectral_centroid(spectrum_200: &[f32]) -> f32 {
    let total_energy: f32 = spectrum_200.iter().sum();
    if total_energy < 1e-10 {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let weighted_sum: f32 = spectrum_200
        .iter()
        .enumerate()
        .map(|(i, &v)| i as f32 * v)
        .sum();
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let max_index = (spectrum_200.len().saturating_sub(1)) as f32;
    if max_index < f32::EPSILON {
        return 0.0;
    }
    (weighted_sum / total_energy / max_index).clamp(0.0, 1.0)
}

// ── Asymmetric EMA Smoothing ─────────────────────────────────────────────

/// Asymmetric exponential moving average smoother.
///
/// Fast attack (snaps to rises), slow decay (fades naturally). This matches
/// how human perception works with sound — we notice onsets immediately but
/// expect energy to fade gradually.
pub struct Smoother {
    value: f32,
    attack: f32,
    decay: f32,
}

impl Smoother {
    /// Create a smoother with the given attack/decay factors.
    ///
    /// Both `attack` and `decay` should be in [0.0, 1.0].
    /// Higher values = faster tracking. `attack=0.3, decay=0.05` is a good default.
    #[must_use]
    pub fn new(attack: f32, decay: f32) -> Self {
        Self {
            value: 0.0,
            attack,
            decay,
        }
    }

    /// Update the smoother with a new target value.
    pub fn update(&mut self, target: f32) {
        let factor = if target > self.value {
            self.attack
        } else {
            self.decay
        };
        self.value += factor * (target - self.value);
    }

    /// Current smoothed value.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Reset to a specific value (e.g. 0.0 on silence).
    pub fn reset(&mut self, value: f32) {
        self.value = value;
    }
}

/// Batch smoother for array-like data (spectrum bins, mel bands, etc.).
pub struct ArraySmoother {
    values: Vec<f32>,
    attack: f32,
    decay: f32,
}

impl ArraySmoother {
    /// Create a batch smoother for `size` elements.
    #[must_use]
    pub fn new(size: usize, attack: f32, decay: f32) -> Self {
        Self {
            values: vec![0.0; size],
            attack,
            decay,
        }
    }

    /// Update all elements from a new frame of raw values.
    pub fn update(&mut self, targets: &[f32]) {
        for (val, &target) in self.values.iter_mut().zip(targets) {
            let factor = if target > *val {
                self.attack
            } else {
                self.decay
            };
            *val += factor * (target - *val);
        }
    }

    /// Current smoothed values.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Reset all values to zero.
    pub fn reset(&mut self) {
        self.values.fill(0.0);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_hz_roundtrip() {
        let freqs = [20.0, 440.0, 1000.0, 8000.0, 20_000.0];
        for &f in &freqs {
            let mel = hz_to_mel(f);
            let back = mel_to_hz(mel);
            assert!(
                (f - back).abs() < 0.1,
                "roundtrip failed for {f}: got {back}"
            );
        }
    }

    #[test]
    fn mel_filterbank_has_correct_count() {
        let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
        assert_eq!(fb.num_bands(), MEL_BANDS);
    }

    #[test]
    fn mel_filterbank_triangular_shape() {
        let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
        for (band_idx, filter) in fb.filters().iter().enumerate() {
            assert!(
                !filter.is_empty(),
                "filter {band_idx} should have at least one tap"
            );
            // All weights should be in [0.0, 1.0].
            for &(_, w) in filter {
                assert!(
                    (0.0..=1.0).contains(&w),
                    "weight out of range in band {band_idx}: {w}"
                );
            }
        }
    }

    #[test]
    fn mel_filterbank_apply_silence() {
        let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
        let silence = vec![0.0; 513]; // fft_size/2 + 1
        let result = fb.apply(&silence);
        assert_eq!(result.len(), MEL_BANDS);
        for &v in &result {
            assert!(v.abs() < f32::EPSILON, "expected zero, got {v}");
        }
    }

    #[test]
    fn chromagram_silence() {
        let silence = vec![0.0; 513];
        let chroma = compute_chromagram(&silence, 48_000, 1024);
        assert_eq!(chroma.len(), CHROMA_BINS);
        for &v in &chroma {
            assert!(v.abs() < f32::EPSILON);
        }
    }

    #[test]
    fn band_energies_silence() {
        let spectrum = vec![0.0; SPECTRUM_BINS];
        let (bass, mid, treble) = band_energies(&spectrum);
        assert!(bass.abs() < f32::EPSILON);
        assert!(mid.abs() < f32::EPSILON);
        assert!(treble.abs() < f32::EPSILON);
    }

    #[test]
    fn rms_of_known_signal() {
        // A constant signal of 0.5 has RMS = 0.5.
        let signal = vec![0.5; 100];
        let rms = compute_rms(&signal);
        assert!((rms - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_of_silence() {
        let signal = vec![0.0; 100];
        assert!(compute_rms(&signal).abs() < f32::EPSILON);
    }

    #[test]
    fn rms_empty_is_zero() {
        assert!(compute_rms(&[]).abs() < f32::EPSILON);
    }

    #[test]
    fn peak_of_known_signal() {
        let signal = vec![0.1, -0.5, 0.3, -0.2];
        assert!((compute_peak(&signal) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn smoother_attack_faster_than_decay() {
        let mut s = Smoother::new(0.3, 0.05);

        // Attack: should converge quickly toward 1.0.
        for _ in 0..20 {
            s.update(1.0);
        }
        let after_attack = s.value();
        assert!(after_attack > 0.95, "attack too slow: {after_attack}");

        // Decay: should converge slowly toward 0.0.
        for _ in 0..20 {
            s.update(0.0);
        }
        let after_decay = s.value();
        assert!(
            after_decay > 0.2,
            "decay too fast: {after_decay} (should still be well above zero)"
        );
    }

    #[test]
    fn smoother_reset() {
        let mut s = Smoother::new(0.3, 0.05);
        s.update(1.0);
        s.reset(0.0);
        assert!(s.value().abs() < f32::EPSILON);
    }

    #[test]
    fn array_smoother_tracks_independently() {
        let mut s = ArraySmoother::new(3, 0.5, 0.1);
        s.update(&[1.0, 0.0, 0.5]);
        // After one step: [0.5, 0.0, 0.25]
        assert!((s.values()[0] - 0.5).abs() < 1e-6);
        assert!(s.values()[1].abs() < f32::EPSILON);
        assert!((s.values()[2] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn spectral_centroid_low_energy() {
        // All energy in the first bin -> centroid near 0.
        let mut spectrum = vec![0.0; SPECTRUM_BINS];
        spectrum[0] = 1.0;
        let c = spectral_centroid(&spectrum);
        assert!(c < 0.01, "centroid should be near zero: {c}");
    }

    #[test]
    fn spectral_centroid_high_energy() {
        // All energy in the last bin -> centroid near 1.
        let mut spectrum = vec![0.0; SPECTRUM_BINS];
        spectrum[SPECTRUM_BINS - 1] = 1.0;
        let c = spectral_centroid(&spectrum);
        assert!(c > 0.99, "centroid should be near one: {c}");
    }

    #[test]
    fn spectral_centroid_silence() {
        let spectrum = vec![0.0; SPECTRUM_BINS];
        assert!(spectral_centroid(&spectrum).abs() < f32::EPSILON);
    }
}
