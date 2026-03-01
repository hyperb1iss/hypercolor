//! Integration tests for the audio analysis pipeline.
//!
//! All tests use synthetic audio data — no microphone or sound card needed.
//! Tests verify FFT accuracy, feature extraction, beat detection, and the
//! full `AudioInput` → `InputSource` pipeline.

use std::f32::consts::PI;

use hypercolor_core::input::audio::AudioInput;
use hypercolor_core::input::audio::beat::{
    BeatDetector, BeatFrame, BeatPulse, EnergyOnsetDetector, SpectralFluxDetector, TempoTracker,
};
use hypercolor_core::input::audio::features::{
    ArraySmoother, MelFilterbank, Smoother, band_energies, compute_chromagram, compute_peak,
    compute_rms, spectral_centroid,
};
use hypercolor_core::input::audio::fft::{FftPipeline, RingBuffer, precompute_hann, spectral_flux};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_types::audio::{AudioConfig, AudioData, CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};

// ── Helpers ──────────────────────────────────────────────────────────────

/// Generate a mono sine wave at the given frequency and sample rate.
fn sine_wave(freq_hz: f32, sample_rate: u32, num_samples: usize) -> Vec<f32> {
    (0..num_samples)
        .map(|i| {
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let t = i as f32 / sample_rate as f32;
            (2.0 * PI * freq_hz * t).sin()
        })
        .collect()
}

/// Generate silence (all zeros).
fn silence(num_samples: usize) -> Vec<f32> {
    vec![0.0; num_samples]
}

/// Generate white noise (pseudo-random, deterministic for reproducibility).
fn white_noise(num_samples: usize) -> Vec<f32> {
    // Simple LCG for deterministic "noise".
    let mut state: u32 = 42;
    (0..num_samples)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            // Map to [-1.0, 1.0].
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let val = (state >> 16) as f32 / 32768.0 - 1.0;
            val
        })
        .collect()
}

/// Generate a synthetic kick drum pulse — a decaying low-frequency sine burst.
fn kick_pulse(sample_rate: u32, num_samples: usize) -> Vec<f32> {
    (0..num_samples)
        .map(|i| {
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            let t = i as f32 / sample_rate as f32;
            let env = (-t * 30.0).exp(); // Fast exponential decay
            let freq = 60.0 + 200.0 * (-t * 50.0).exp(); // Pitch drops
            env * (2.0 * PI * freq * t).sin()
        })
        .collect()
}

// ── FFT Pipeline Tests ───────────────────────────────────────────────────

#[test]
fn fft_sine_wave_peak_at_correct_bin() {
    let sample_rate = 48_000;
    let fft_size = 1024;
    let test_freq = 1000.0_f32; // 1 kHz

    let samples = sine_wave(test_freq, sample_rate, fft_size);
    let mut pipeline = FftPipeline::new(fft_size, sample_rate);
    let result = pipeline.process(&samples).expect("FFT should succeed");

    assert_eq!(result.spectrum.len(), SPECTRUM_BINS);

    // Find the peak bin.
    let (peak_bin, peak_val) = result
        .spectrum
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .expect("spectrum should not be empty");

    assert!(
        *peak_val > 0.1,
        "peak should be significant: {peak_val} at bin {peak_bin}"
    );

    // The peak should be in the region corresponding to ~1kHz.
    // In the 200-bin log scale from 20 Hz to 20 kHz:
    // bin = 200 * ln(f/20) / ln(1000)
    // For 1kHz: ~200 * ln(50) / ln(1000) ~= 200 * 3.912 / 6.908 ~= 113
    // Allow generous tolerance for windowing effects.
    assert!(
        (50..150).contains(&peak_bin),
        "1 kHz peak should be near bin ~113, found at {peak_bin}"
    );
}

#[test]
fn fft_silence_all_bins_near_zero() {
    let fft_size = 1024;
    let samples = silence(fft_size);
    let mut pipeline = FftPipeline::new(fft_size, 48_000);
    let result = pipeline.process(&samples).expect("FFT should succeed");

    for (i, &v) in result.spectrum.iter().enumerate() {
        assert!(
            v < 0.05,
            "silence should produce near-zero bins, but bin {i} = {v}"
        );
    }
}

#[test]
fn fft_white_noise_relatively_flat_spectrum() {
    let fft_size = 1024;
    let samples = white_noise(fft_size);
    let mut pipeline = FftPipeline::new(fft_size, 48_000);
    let result = pipeline.process(&samples).expect("FFT should succeed");

    // White noise should have energy spread across the spectrum.
    // Check that at least 80% of bins have non-negligible energy.
    let active_bins = result.spectrum.iter().filter(|&&v| v > 0.01).count();
    assert!(
        active_bins > SPECTRUM_BINS * 3 / 4,
        "white noise should activate most bins: {active_bins}/{SPECTRUM_BINS}"
    );

    // No single bin should dominate — check that max/mean ratio is reasonable.
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let mean: f32 = result.spectrum.iter().sum::<f32>() / SPECTRUM_BINS as f32;
    let max = result.spectrum.iter().copied().fold(0.0_f32, f32::max);
    if mean > 0.001 {
        let ratio = max / mean;
        assert!(
            ratio < 10.0,
            "white noise should be relatively flat, but max/mean = {ratio}"
        );
    }
}

// ── Hann Window Tests ────────────────────────────────────────────────────

#[test]
fn hann_window_endpoints_near_zero() {
    let w = precompute_hann(256);
    assert!(w[0].abs() < 1e-6, "first sample should be ~0: {}", w[0]);
    // Periodic Hann: last sample is NOT exactly zero, but close for large N.
    assert!(w[255] < 0.01, "last sample should be near 0: {}", w[255]);
}

#[test]
fn hann_window_symmetric() {
    // Periodic Hann: w(n) = 0.5 * (1 - cos(2*pi*n/N))
    // Symmetry: w(k) = w(N-k) for 1 <= k < N
    let n = 512;
    let w = precompute_hann(n);
    for i in 1..n / 2 {
        assert!(
            (w[i] - w[n - i]).abs() < 1e-5,
            "window should be symmetric at index {i}: {} vs {}",
            w[i],
            w[n - i]
        );
    }
}

#[test]
fn hann_window_peak_at_midpoint() {
    let w = precompute_hann(1024);
    let mid = 512;
    assert!(
        (w[mid] - 1.0).abs() < 1e-4,
        "window peak should be 1.0: {}",
        w[mid]
    );
}

// ── Mel Filterbank Tests ─────────────────────────────────────────────────

#[test]
fn mel_filterbank_correct_band_count() {
    let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
    assert_eq!(fb.num_bands(), MEL_BANDS);
}

#[test]
fn mel_filterbank_triangular_shapes() {
    let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
    for (band_idx, filter) in fb.filters().iter().enumerate() {
        assert!(
            !filter.is_empty(),
            "mel filter {band_idx} should have at least one tap"
        );
        // All weights in [0, 1].
        for &(_, w) in filter {
            assert!(
                (0.0..=1.001).contains(&w),
                "weight out of range in band {band_idx}: {w}"
            );
        }
    }
}

#[test]
fn mel_filterbank_center_frequencies_increase() {
    let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
    // The center of each filter (highest weight or midpoint bin) should increase.
    let centers: Vec<usize> = fb
        .filters()
        .iter()
        .map(|filter| {
            filter
                .iter()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map_or(0, |&(bin, _)| bin)
        })
        .collect();

    for i in 1..centers.len() {
        assert!(
            centers[i] >= centers[i - 1],
            "mel filter centers should increase: band {}: {}, band {i}: {}",
            i - 1,
            centers[i - 1],
            centers[i]
        );
    }
}

#[test]
fn mel_filterbank_silence_produces_zeros() {
    let fb = MelFilterbank::new(1024, 48_000, MEL_BANDS);
    let mags = vec![0.0; 513]; // fft_size/2 + 1
    let result = fb.apply(&mags);
    assert_eq!(result.len(), MEL_BANDS);
    for &v in &result {
        assert!(v.abs() < f32::EPSILON, "expected zero for silence, got {v}");
    }
}

// ── Chromagram Tests ─────────────────────────────────────────────────────

#[test]
fn chromagram_a440_produces_strong_a_class() {
    let sample_rate = 48_000;
    let fft_size = 4096;

    // Build a clean linear magnitude array with a spike at the 440 Hz bin.
    // freq_resolution = 48000 / 4096 = 11.72 Hz per bin
    // 440 Hz -> bin 37.5, so bins 37-38 should have energy.
    let num_bins = fft_size / 2 + 1;
    let mut magnitudes = vec![0.0_f32; num_bins];

    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let freq_res = sample_rate as f32 / fft_size as f32;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    let target_bin = (440.0 / freq_res) as usize;
    // Place energy at the target bin and its neighbors (simulates windowed peak).
    for offset in 0_u8..=2 {
        let weight = 1.0 / (1.0 + f32::from(offset));
        let off = usize::from(offset);
        if target_bin + off < num_bins {
            magnitudes[target_bin + off] = weight;
        }
        if target_bin >= off {
            magnitudes[target_bin - off] = weight;
        }
    }

    let chroma = compute_chromagram(&magnitudes, sample_rate, fft_size);
    assert_eq!(chroma.len(), CHROMA_BINS);

    // A = pitch class index 9 (C=0, C#=1, ... A=9, A#=10, B=11)
    let a_idx = 9;

    let max_idx = chroma
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .expect("chroma should not be empty");

    // The peak should be at A (index 9) or very close.
    // Chroma bins are always 0..12, so these casts are safe.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::as_conversions
    )]
    let mi = max_idx as i32;
    let distance = (mi - a_idx).abs().min(12 - (mi - a_idx).abs());
    assert!(
        distance <= 1,
        "peak chroma bin should be near A(9), got {max_idx} (distance {distance}), chroma: {chroma:?}"
    );
}

#[test]
fn chromagram_silence() {
    let silence_mags = vec![0.0; 513];
    let chroma = compute_chromagram(&silence_mags, 48_000, 1024);
    assert_eq!(chroma.len(), CHROMA_BINS);
    for &v in &chroma {
        assert!(v.abs() < f32::EPSILON);
    }
}

// ── Band Energy Tests ────────────────────────────────────────────────────

#[test]
fn band_energy_bass_heavy_signal() {
    let sample_rate = 48_000;
    let fft_size = 1024;
    // 80 Hz sine — solidly in the bass range.
    let samples = sine_wave(80.0, sample_rate, fft_size);
    let mut pipeline = FftPipeline::new(fft_size, sample_rate);
    let result = pipeline.process(&samples).expect("FFT should succeed");

    let (bass, _mid, treble) = band_energies(&result.spectrum);
    assert!(
        bass > treble,
        "80 Hz should have more bass than treble: bass={bass}, treble={treble}"
    );
}

#[test]
fn band_energy_treble_heavy_signal() {
    // Direct spectrum test: concentrate energy in the treble region (bins 130-199).
    let mut spectrum = vec![0.0; SPECTRUM_BINS];
    for val in &mut spectrum[150..190] {
        *val = 0.8;
    }
    let (bass, _mid, treble) = band_energies(&spectrum);
    assert!(
        treble > bass,
        "treble-heavy spectrum should have more treble than bass: treble={treble}, bass={bass}"
    );
    assert!(treble > 0.1, "treble should be significant: {treble}");
}

#[test]
fn band_energy_silence() {
    let spectrum = vec![0.0; SPECTRUM_BINS];
    let (bass, mid, treble) = band_energies(&spectrum);
    assert!(bass.abs() < f32::EPSILON);
    assert!(mid.abs() < f32::EPSILON);
    assert!(treble.abs() < f32::EPSILON);
}

// ── RMS Level Tests ──────────────────────────────────────────────────────

#[test]
fn rms_known_amplitude() {
    // RMS of a sine wave with amplitude A is A / sqrt(2) ≈ 0.707
    let samples = sine_wave(440.0, 48_000, 48_000); // 1 second
    let rms = compute_rms(&samples);
    let expected = 1.0 / 2.0_f32.sqrt();
    assert!(
        (rms - expected).abs() < 0.02,
        "RMS of unit sine should be ~{expected}: got {rms}"
    );
}

#[test]
fn rms_silence_is_zero() {
    let samples = silence(1024);
    assert!(compute_rms(&samples).abs() < f32::EPSILON);
}

#[test]
fn rms_constant_signal() {
    // RMS of a constant value K is |K|.
    let samples = vec![0.3; 1000];
    let rms = compute_rms(&samples);
    assert!(
        (rms - 0.3).abs() < 0.001,
        "RMS of constant 0.3 should be ~0.3: got {rms}"
    );
}

#[test]
fn peak_of_known_signal() {
    let mut samples = silence(100);
    samples[50] = 0.75;
    samples[25] = -0.9;
    let peak = compute_peak(&samples);
    assert!(
        (peak - 0.9).abs() < f32::EPSILON,
        "peak should be 0.9: got {peak}"
    );
}

// ── Spectral Centroid Tests ──────────────────────────────────────────────

#[test]
fn spectral_centroid_low_frequency() {
    let mut spectrum = vec![0.0; SPECTRUM_BINS];
    spectrum[0] = 1.0;
    let c = spectral_centroid(&spectrum);
    assert!(c < 0.01, "centroid with all bass should be near 0: {c}");
}

#[test]
fn spectral_centroid_high_frequency() {
    let mut spectrum = vec![0.0; SPECTRUM_BINS];
    spectrum[SPECTRUM_BINS - 1] = 1.0;
    let c = spectral_centroid(&spectrum);
    assert!(c > 0.99, "centroid with all treble should be near 1: {c}");
}

#[test]
fn spectral_centroid_silence() {
    let spectrum = vec![0.0; SPECTRUM_BINS];
    assert!(spectral_centroid(&spectrum).abs() < f32::EPSILON);
}

// ── Smoothing Tests ──────────────────────────────────────────────────────

#[test]
fn smoother_attack_decay_asymmetry() {
    let mut s = Smoother::new(0.3, 0.05);

    // Attack: converge quickly toward 1.0.
    for _ in 0..30 {
        s.update(1.0);
    }
    let attack_val = s.value();
    assert!(
        attack_val > 0.95,
        "after 30 attack steps, value should be >0.95: {attack_val}"
    );

    // Decay: converge slowly toward 0.0.
    for _ in 0..30 {
        s.update(0.0);
    }
    let decay_val = s.value();
    assert!(
        decay_val > 0.1,
        "after 30 decay steps, value should still be >0.1: {decay_val}"
    );
}

#[test]
fn smoother_convergence() {
    let mut s = Smoother::new(0.5, 0.5);
    // With symmetric alpha=0.5, should converge to target within ~20 steps.
    for _ in 0..50 {
        s.update(0.75);
    }
    assert!(
        (s.value() - 0.75).abs() < 0.01,
        "should converge to target: {}",
        s.value()
    );
}

#[test]
fn array_smoother_independent_channels() {
    let mut s = ArraySmoother::new(4, 0.5, 0.1);
    s.update(&[1.0, 0.0, 0.5, 0.2]);

    // Channel 0: attacked toward 1.0 → 0.5
    assert!((s.values()[0] - 0.5).abs() < 1e-6);
    // Channel 1: no change from 0
    assert!(s.values()[1].abs() < f32::EPSILON);
    // Channel 2: attacked toward 0.5 → 0.25
    assert!((s.values()[2] - 0.25).abs() < 1e-6);
}

// ── Beat Detection Tests ─────────────────────────────────────────────────

#[test]
fn beat_detection_synthetic_kicks() {
    let sample_rate = 48_000;
    let fft_size = 1024;
    let bpm = 120.0;
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    let beat_interval_samples = (sample_rate as f32 * 60.0 / bpm) as usize;
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let dt = fft_size as f32 / sample_rate as f32; // time per FFT frame

    let mut detector = BeatDetector::new(1.5);
    let mut pipeline = FftPipeline::new(fft_size, sample_rate);
    let mut ring = RingBuffer::new(fft_size * 4);

    let mut beat_count = 0;
    let total_duration = 5.0_f32; // 5 seconds
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    let total_samples = (sample_rate as f32 * total_duration) as usize;

    // Generate audio: kick pulses at 120 BPM with silence between.
    let mut audio = silence(total_samples);
    let kick = kick_pulse(sample_rate, fft_size);
    let mut pos = 0;
    while pos + kick.len() <= total_samples {
        for (j, &s) in kick.iter().enumerate() {
            audio[pos + j] += s;
        }
        pos += beat_interval_samples;
    }

    // Process in FFT-sized chunks.
    let mut current_time = 0.0_f64;
    let mut i = 0;
    while i + fft_size <= total_samples {
        ring.push_slice(&audio[i..i + fft_size]);
        let mut buf = vec![0.0; fft_size];
        ring.read_last(&mut buf);

        if let Ok(result) = pipeline.process(&buf) {
            let (bass, mid, treble) = band_energies(&result.spectrum);
            let state = detector.update(&BeatFrame {
                bass,
                mid,
                treble,
                spectral_flux: result.spectral_flux,
                dt,
                current_time,
            });
            if state.beat_detected {
                beat_count += 1;
            }
        }

        i += fft_size;
        current_time += f64::from(dt);
    }

    // At 120 BPM over 5 seconds, we expect ~10 beats.
    // Allow generous tolerance — the detector may miss some or trigger on transients.
    assert!(
        beat_count >= 3,
        "should detect at least a few beats from 120 BPM kick pattern: got {beat_count}"
    );
    assert!(
        beat_count < 30,
        "should not wildly over-detect: got {beat_count}"
    );
}

#[test]
fn beat_pulse_spike_and_decay() {
    let mut pulse = BeatPulse::new();
    pulse.update(true, 0.016);
    assert!((pulse.value() - 1.0).abs() < f32::EPSILON);

    // Decay over ~200ms.
    for _ in 0..12 {
        pulse.update(false, 0.016);
    }
    assert!(pulse.value() < 0.1, "pulse should decay: {}", pulse.value());
}

#[test]
fn energy_onset_fires_on_spike() {
    let mut detector = EnergyOnsetDetector::new(1.5);

    // Build up a low baseline.
    for _ in 0..60 {
        detector.update(0.01, 0.016);
    }

    assert!(
        detector.update(0.8, 0.016),
        "onset should fire on energy spike"
    );
}

#[test]
fn spectral_flux_detector_onset() {
    let mut detector = SpectralFluxDetector::new(1.5);
    for _ in 0..30 {
        detector.push(0.01);
    }
    detector.push(0.5);
    assert!(detector.is_onset(), "should detect onset on flux spike");
}

#[test]
fn tempo_tracker_120bpm() {
    let mut tracker = TempoTracker::new();
    for i in 0..12 {
        tracker.record_onset(f64::from(i) * 0.5); // 120 BPM = 0.5s interval
    }

    let bpm = tracker.bpm();
    assert!((bpm - 120.0).abs() < 5.0, "expected ~120 BPM, got {bpm}");
    assert!(
        tracker.confidence() > 0.5,
        "confidence should be decent: {}",
        tracker.confidence()
    );
}

// ── Spectral Flux Tests ──────────────────────────────────────────────────

#[test]
fn spectral_flux_identical_is_zero() {
    let a = vec![0.5; SPECTRUM_BINS];
    let b = vec![0.5; SPECTRUM_BINS];
    assert!(spectral_flux(&a, &b).abs() < f32::EPSILON);
}

#[test]
fn spectral_flux_only_positive_changes() {
    let prev = vec![0.8; SPECTRUM_BINS];
    let curr = vec![0.3; SPECTRUM_BINS]; // All decreases
    assert!(
        spectral_flux(&curr, &prev).abs() < f32::EPSILON,
        "flux should be zero when all bins decrease"
    );
}

#[test]
fn spectral_flux_increases_detected() {
    let prev = vec![0.0; SPECTRUM_BINS];
    let curr = vec![1.0; SPECTRUM_BINS]; // All increases
    let flux = spectral_flux(&curr, &prev);
    assert!(
        flux > 0.5,
        "flux should be high when all bins increase: {flux}"
    );
}

// ── Ring Buffer Tests ────────────────────────────────────────────────────

#[test]
#[expect(clippy::float_cmp)]
fn ring_buffer_wraps_correctly() {
    let mut rb = RingBuffer::new(4);
    rb.push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    let mut dst = [0.0f32; 4];
    rb.read_last(&mut dst);
    assert_eq!(dst, [3.0, 4.0, 5.0, 6.0]);
}

#[test]
#[expect(clippy::float_cmp)]
fn ring_buffer_partial_read() {
    let mut rb = RingBuffer::new(8);
    rb.push_slice(&[10.0, 20.0]);

    let mut dst = [0.0f32; 4];
    rb.read_last(&mut dst);
    assert_eq!(dst, [0.0, 0.0, 10.0, 20.0]);
}

// ── AudioInput Integration Tests ─────────────────────────────────────────

#[test]
fn audio_input_lifecycle() {
    let config = AudioConfig::default();
    let mut input = AudioInput::new(&config);

    assert!(!input.is_running());
    assert_eq!(input.name(), "AudioInput");

    input.start().expect("start should succeed");
    assert!(input.is_running());

    input.stop();
    assert!(!input.is_running());
}

#[test]
fn audio_input_returns_none_without_samples() {
    let config = AudioConfig::default();
    let mut input = AudioInput::new(&config);
    input.start().expect("start");

    let data = input.sample().expect("sample should not error");
    assert!(
        matches!(data, InputData::None),
        "should return None when no samples are pushed"
    );
}

#[test]
fn audio_input_produces_audio_data_with_samples() {
    let config = AudioConfig::default();
    let mut input = AudioInput::new(&config);
    input.start().expect("start");

    // Push enough samples for at least one FFT window.
    let samples = sine_wave(440.0, 48_000, 2048);
    input.push_samples(&samples);

    let data = input.sample().expect("sample should succeed");
    match data {
        InputData::Audio(audio) => {
            assert_eq!(audio.spectrum.len(), SPECTRUM_BINS);
            assert_eq!(audio.mel_bands.len(), MEL_BANDS);
            assert_eq!(audio.chromagram.len(), CHROMA_BINS);
            assert!(audio.rms_level > 0.0, "RMS should be nonzero for sine wave");
        }
        InputData::None => panic!("expected Audio data, got None"),
        InputData::Screen(_) => panic!("expected Audio data"),
    }
}

#[test]
fn audio_input_silence_produces_near_zero() {
    let config = AudioConfig {
        noise_floor: -120.0, // Very low floor so silence still gets processed
        ..AudioConfig::default()
    };
    let mut input = AudioInput::new(&config);
    input.start().expect("start");

    let samples = silence(2048);
    input.push_samples(&samples);

    let data = input.sample().expect("sample");
    match data {
        InputData::Audio(audio) => {
            // Silence: all values should be near zero.
            for &v in &audio.spectrum {
                assert!(v < 0.1, "spectrum bin too high for silence: {v}");
            }
            assert!(!audio.beat_detected);
        }
        InputData::None => {
            // Also acceptable — might not have enough data.
        }
        InputData::Screen(_) => panic!("unexpected variant"),
    }
}

#[test]
fn audio_input_custom_name() {
    let config = AudioConfig::default();
    let input = AudioInput::new(&config).with_name("PipeWire Monitor");
    assert_eq!(input.name(), "PipeWire Monitor");
}

#[test]
fn audio_input_multiple_frames() {
    let config = AudioConfig::default();
    let mut input = AudioInput::new(&config);
    input.start().expect("start");

    // Feed samples across multiple frames.
    for _ in 0..10 {
        let chunk = sine_wave(1000.0, 48_000, 512);
        input.push_samples(&chunk);
        let _ = input.sample(); // Don't care about result, just exercise the path
    }

    // After many frames, sampling should still work.
    let chunk = sine_wave(1000.0, 48_000, 1024);
    input.push_samples(&chunk);
    let data = input.sample().expect("sample after many frames");
    assert!(
        matches!(data, InputData::Audio(_)),
        "should produce audio data"
    );
}

#[test]
fn audio_data_silence_default_values() {
    let silence_data = AudioData::silence();
    assert_eq!(silence_data.spectrum.len(), SPECTRUM_BINS);
    assert_eq!(silence_data.mel_bands.len(), MEL_BANDS);
    assert_eq!(silence_data.chromagram.len(), CHROMA_BINS);
    assert!(!silence_data.beat_detected);
    assert!((silence_data.bpm - 0.0).abs() < f32::EPSILON);
    assert!((silence_data.rms_level - 0.0).abs() < f32::EPSILON);
}
