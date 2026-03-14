//! FFT processing pipeline — windowing, real-to-complex transform, log-frequency resampling.
//!
//! Pure computation on `&[f32]` sample buffers. No OS dependencies, no audio hardware.
//! The pipeline follows the spec: Hann window, DC removal, 1024-point r2c FFT,
//! dB-scaled magnitudes, and logarithmic remapping to 200 perceptual bins.

use std::sync::Arc;

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;

use crate::types::audio::SPECTRUM_BINS;

// ── Constants ────────────────────────────────────────────────────────────

/// Default FFT window size (1024 samples = 21.3ms at 48 kHz).
pub const DEFAULT_FFT_SIZE: usize = 1024;

/// Default sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Floor for dB conversion — prevents log(0).
const DB_EPSILON: f32 = 1e-10;

/// dB range mapped to 0.0–1.0 output. Anything below -80 dB is silence.
const DB_FLOOR: f32 = -80.0;

/// Minimum frequency for log mapping (Hz).
const FREQ_MIN: f32 = 20.0;

/// Maximum frequency for log mapping (Hz).
const FREQ_MAX: f32 = 20_000.0;

// ── Ring Buffer ──────────────────────────────────────────────────────────

/// A simple ring buffer for accumulating incoming audio samples.
///
/// Stores the last `capacity` samples, allowing the FFT pipeline to
/// grab a full window whenever it needs one.
pub struct RingBuffer {
    data: Vec<f32>,
    write_pos: usize,
    len: usize,
}

impl RingBuffer {
    /// Create a ring buffer with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity],
            write_pos: 0,
            len: 0,
        }
    }

    /// Push a slice of samples into the ring buffer.
    pub fn push_slice(&mut self, samples: &[f32]) {
        for &s in samples {
            self.data[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % self.data.len();
            if self.len < self.data.len() {
                self.len += 1;
            }
        }
    }

    /// Number of samples currently stored (up to capacity).
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer has been completely filled at least once.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.len == self.data.len()
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Copy the last `count` samples into `dst`, in chronological order.
    ///
    /// If fewer than `count` samples are stored, the beginning of `dst`
    /// is zero-filled.
    pub fn read_last(&self, dst: &mut [f32]) {
        let count = dst.len();
        if self.len < count {
            // Zero-fill the prefix, then copy what we have.
            let zeros = count - self.len;
            dst[..zeros].fill(0.0);
            self.copy_tail(&mut dst[zeros..], self.len);
        } else {
            self.copy_tail(dst, count);
        }
    }

    /// Copy `count` samples from the tail of the ring into `dst`.
    fn copy_tail(&self, dst: &mut [f32], count: usize) {
        let cap = self.data.len();
        // Start position in the ring for the oldest of the `count` samples.
        let start = (self.write_pos + cap - count) % cap;

        if start + count <= cap {
            dst[..count].copy_from_slice(&self.data[start..start + count]);
        } else {
            let first = cap - start;
            dst[..first].copy_from_slice(&self.data[start..]);
            dst[first..count].copy_from_slice(&self.data[..count - first]);
        }
    }

    /// Total capacity of the ring buffer.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.data.len()
    }
}

// ── Hann Window ──────────────────────────────────────────────────────────

/// Pre-computed Hann window coefficients.
///
/// Applied element-wise to the time-domain buffer before FFT to reduce
/// spectral leakage. Symmetric, with endpoints at zero.
#[must_use]
pub fn precompute_hann(size: usize) -> Vec<f32> {
    (0..size)
        .map(|n| {
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let t = 2.0 * std::f32::consts::PI * n as f32 / size as f32;
            0.5 * (1.0 - t.cos())
        })
        .collect()
}

// ── FFT Pipeline ─────────────────────────────────────────────────────────

/// Complete FFT analysis pipeline.
///
/// Owns the FFT planner, pre-computed window, scratch buffers, and
/// log-frequency bin mapping. Feed it time-domain samples via
/// [`process`](FftPipeline::process) and get back a 200-bin normalized spectrum.
pub struct FftPipeline {
    fft_size: usize,
    sample_rate: u32,
    hann_window: Vec<f32>,
    /// Windowed time-domain buffer (reused each frame).
    time_buf: Vec<f32>,
    /// Complex spectrum output from the r2c FFT.
    spectrum_buf: Vec<Complex<f32>>,
    /// The forward FFT plan.
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    /// Pre-computed log-frequency bin mapping: for each of the 200 output bins,
    /// stores `(fft_bin_lo, fft_bin_hi)`.
    bin_map: Vec<(usize, usize)>,
    /// Raw dB-normalized magnitudes for all FFT bins (reused each frame).
    raw_magnitudes_buf: Vec<f32>,
    /// Log-frequency resample buffer (reused each frame).
    spectrum_bins_buf: Vec<f32>,
    /// Previous spectrum frame (for spectral flux computation).
    prev_magnitudes: Vec<f32>,
}

impl FftPipeline {
    /// Create a new pipeline with the given FFT size and sample rate.
    ///
    /// # Panics
    ///
    /// Panics if `fft_size` is zero (the `realfft` crate requires a non-zero length).
    #[must_use]
    pub fn new(fft_size: usize, sample_rate: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);

        let spectrum_buf = fft.make_output_vec();

        let half = fft_size / 2;
        let bin_map = build_log_bin_map(half, sample_rate);

        Self {
            fft_size,
            sample_rate,
            hann_window: precompute_hann(fft_size),
            time_buf: vec![0.0; fft_size],
            spectrum_buf,
            fft,
            bin_map,
            raw_magnitudes_buf: vec![0.0; half + 1],
            spectrum_bins_buf: vec![0.0; SPECTRUM_BINS],
            prev_magnitudes: vec![0.0; SPECTRUM_BINS],
        }
    }

    /// FFT size (number of time-domain samples per window).
    #[must_use]
    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Process a time-domain buffer and return the 200-bin normalized spectrum.
    ///
    /// `samples` must contain exactly `fft_size` elements. If the caller
    /// provides fewer, the behaviour is defined by [`RingBuffer::read_last`]
    /// which zero-fills the prefix.
    ///
    /// Returns `(spectrum_200, raw_magnitudes, spectral_flux)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the FFT computation fails (buffer size mismatch).
    pub fn process(&mut self, samples: &[f32]) -> anyhow::Result<FftResult<'_>> {
        // Copy into work buffer.
        let n = samples.len().min(self.fft_size);
        self.time_buf[..n].copy_from_slice(&samples[..n]);
        self.time_buf[n..].fill(0.0);

        // DC offset removal.
        remove_dc_offset(&mut self.time_buf);

        // Apply Hann window.
        for (sample, &coeff) in self.time_buf.iter_mut().zip(&self.hann_window) {
            *sample *= coeff;
        }

        // Forward FFT (r2c). The scratch buffer for realfft is Vec<Complex<f32>>,
        // but we stored Vec<f32>. We need to reinterpret or use `process` directly.
        // `realfft` process_with_scratch expects &mut [Complex<f32>] for scratch.
        // Let's just use `process` which allocates scratch internally.
        self.fft
            .process(&mut self.time_buf, &mut self.spectrum_buf)
            .map_err(|e| anyhow::anyhow!("FFT processing failed: {e}"))?;

        // Compute magnitudes in dB, normalized to [0, 1].
        let half = self.fft_size / 2;
        self.compute_raw_magnitudes(half);

        // Log-frequency resample to 200 bins.
        self.resample_log();

        // Spectral flux (half-wave rectified difference from previous frame).
        let flux = spectral_flux(&self.spectrum_bins_buf, &self.prev_magnitudes);
        self.prev_magnitudes
            .copy_from_slice(&self.spectrum_bins_buf);

        Ok(FftResult {
            spectrum: &self.spectrum_bins_buf,
            raw_magnitudes: &self.raw_magnitudes_buf,
            spectral_flux: flux,
        })
    }

    /// Compute dB-normalized magnitudes from the complex spectrum.
    fn compute_raw_magnitudes(&mut self, half: usize) {
        for (slot, complex) in self
            .raw_magnitudes_buf
            .iter_mut()
            .zip(self.spectrum_buf[..=half].iter())
        {
            let mag = (complex.re * complex.re + complex.im * complex.im).sqrt();
            let db = 20.0 * (mag + DB_EPSILON).log10();
            *slot = db_to_normalized(db);
        }
    }

    /// Resample linearly-spaced FFT magnitudes into 200 log-spaced bins.
    fn resample_log(&mut self) {
        for (slot, &(lo, hi)) in self.spectrum_bins_buf.iter_mut().zip(&self.bin_map) {
            if lo >= self.raw_magnitudes_buf.len() {
                *slot = 0.0;
                continue;
            }

            let hi = hi.min(self.raw_magnitudes_buf.len());
            let slice = &self.raw_magnitudes_buf[lo..hi];
            *slot = if slice.is_empty() {
                0.0
            } else {
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let count = slice.len() as f32;
                slice.iter().sum::<f32>() / count
            };
        }
    }
}

/// Result from a single FFT frame.
pub struct FftResult<'a> {
    /// 200 log-frequency bins, normalized 0.0–1.0.
    pub spectrum: &'a [f32],
    /// Raw dB-normalized magnitudes for all FFT bins (`fft_size/2 + 1` entries).
    pub raw_magnitudes: &'a [f32],
    /// Spectral flux (half-wave rectified sum of positive changes).
    pub spectral_flux: f32,
}

// ── Helper Functions ─────────────────────────────────────────────────────

/// Remove DC offset by subtracting the buffer mean.
fn remove_dc_offset(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    for s in samples.iter_mut() {
        *s -= mean;
    }
}

/// Map a dB value to [0.0, 1.0]. Below -80 dB is silence, 0 dB is maximum.
fn db_to_normalized(db: f32) -> f32 {
    ((db - DB_FLOOR) / (0.0 - DB_FLOOR)).clamp(0.0, 1.0)
}

/// Build the log-frequency bin mapping for 200 output bins.
///
/// For each output bin `i`, returns `(fft_bin_lo, fft_bin_hi)` covering
/// the frequency range `[freq_lo, freq_hi)` on a logarithmic scale from
/// 20 Hz to 20 kHz.
fn build_log_bin_map(half_fft: usize, sample_rate: u32) -> Vec<(usize, usize)> {
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let nyquist = sample_rate as f32 / 2.0;
    let log_min = FREQ_MIN.ln();
    let log_max = FREQ_MAX.ln();

    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let half_f = half_fft as f32;

    (0..SPECTRUM_BINS)
        .map(|i| {
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let t0 = i as f32 / SPECTRUM_BINS as f32;
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let t1 = (i + 1) as f32 / SPECTRUM_BINS as f32;

            let freq_lo = (log_min + t0 * (log_max - log_min)).exp();
            let freq_hi = (log_min + t1 * (log_max - log_min)).exp();

            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            let bin_lo = (freq_lo / nyquist * half_f) as usize;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            let bin_hi = ((freq_hi / nyquist * half_f) as usize)
                .max(bin_lo + 1)
                .min(half_fft + 1);

            (bin_lo, bin_hi)
        })
        .collect()
}

/// Compute half-wave rectified spectral flux between two frames.
///
/// Only positive changes (energy increases) contribute. Returns the sum
/// normalized by the number of bins.
pub fn spectral_flux(current: &[f32], previous: &[f32]) -> f32 {
    if current.len() != previous.len() || current.is_empty() {
        return 0.0;
    }
    let flux: f32 = current
        .iter()
        .zip(previous)
        .map(|(&c, &p)| (c - p).max(0.0))
        .sum();
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let count = current.len() as f32;
    flux / count
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_basics() {
        let mut rb = RingBuffer::new(4);
        assert!(rb.is_empty());
        assert!(!rb.is_full());

        rb.push_slice(&[1.0, 2.0, 3.0]);
        assert_eq!(rb.len(), 3);
        assert!(!rb.is_full());

        rb.push_slice(&[4.0]);
        assert!(rb.is_full());
        assert_eq!(rb.len(), 4);

        let mut dst = [0.0f32; 4];
        rb.read_last(&mut dst);
        assert_eq!(dst, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn ring_buffer_wraps() {
        let mut rb = RingBuffer::new(3);
        rb.push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(rb.len(), 3);

        let mut dst = [0.0f32; 3];
        rb.read_last(&mut dst);
        assert_eq!(dst, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn ring_buffer_read_with_zero_fill() {
        let mut rb = RingBuffer::new(8);
        rb.push_slice(&[10.0, 20.0]);

        let mut dst = [0.0f32; 4];
        rb.read_last(&mut dst);
        // First 2 should be zero-filled, last 2 are the data.
        assert_eq!(dst, [0.0, 0.0, 10.0, 20.0]);
    }

    #[test]
    fn hann_window_endpoints_are_zero() {
        let w = precompute_hann(64);
        assert!(w[0].abs() < 1e-6);
        // The last sample in a periodic Hann window is *not* zero,
        // but for size=64 it should be very close.
        assert!(w[63] < 0.01);
    }

    #[test]
    fn hann_window_symmetric() {
        // Periodic Hann: w(k) = w(N-k) for 1 <= k < N
        let n = 128;
        let w = precompute_hann(n);
        for i in 1..n / 2 {
            assert!((w[i] - w[n - i]).abs() < 1e-6, "asymmetry at index {i}");
        }
    }

    #[test]
    fn hann_window_peak_at_center() {
        let w = precompute_hann(256);
        let mid = 128;
        assert!((w[mid] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn db_to_normalized_bounds() {
        assert!((db_to_normalized(0.0) - 1.0).abs() < f32::EPSILON);
        assert!((db_to_normalized(-80.0) - 0.0).abs() < f32::EPSILON);
        assert!((db_to_normalized(-40.0) - 0.5).abs() < f32::EPSILON);
        // Below floor clamps to 0
        assert!((db_to_normalized(-100.0) - 0.0).abs() < f32::EPSILON);
        // Above 0 clamps to 1
        assert!((db_to_normalized(10.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn spectral_flux_identical_frames() {
        let a = vec![0.5; 200];
        let b = vec![0.5; 200];
        assert!(spectral_flux(&a, &b).abs() < f32::EPSILON);
    }

    #[test]
    fn spectral_flux_positive_only() {
        let prev = vec![0.5; 200];
        let curr = vec![0.3; 200]; // all decreases
        assert!(spectral_flux(&curr, &prev).abs() < f32::EPSILON);
    }
}
