# 08 — Audio Pipeline Specification

> From soundwave to photon. Every struct, every constant, every clock edge.

**Status:** Implemented
**Crate:** `hypercolor-core`
**Module path:** `hypercolor_core::input`

---

## 1. AudioData Struct -- The Contract

`AudioData` is the single source of truth for all audio-reactive rendering. Computed once per DSP frame on the audio thread, consumed by both the Servo (Lightscript) and wgpu (native shader) paths. Every field is documented here as the canonical API.

```rust
/// Complete audio analysis data, computed once per DSP frame.
/// Injected into Servo as `window.engine.audio` and
/// into wgpu shaders as a uniform buffer + array textures.
#[derive(Clone, Copy, Debug)]
pub struct AudioData {
    // ─── Level & Shape ──────────────────────────────────────
    /// Overall audio level (RMS of 200 bins, normalized 0.0-1.0).
    /// Maps to Lightscript `engine.audio.level`.
    pub level: f32,

    /// Audio density -- spectral flatness (0.0 = pure tone, 1.0 = white noise).
    /// Geometric mean / arithmetic mean of power spectrum.
    /// Maps to Lightscript `engine.audio.density`.
    pub density: f32,

    /// Stereo width (0.0 = mono, 1.0 = full stereo).
    /// Computed as 1.0 - abs(correlation(left, right)).
    /// Maps to Lightscript `engine.audio.width`.
    pub width: f32,

    // ─── Frequency Spectrum ─────────────────────────────────
    /// 200 logarithmically-spaced frequency bins (normalized 0.0-1.0).
    /// Covers 20 Hz - 20 kHz. See Section 2 for bin mapping.
    /// Maps to Lightscript `engine.audio.freq` (Int8Array in JS).
    pub freq: [f32; 200],

    // ─── Band Energy ────────────────────────────────────────
    /// Bass energy (20-250 Hz, bins 0-39, normalized 0.0-1.0).
    pub bass: f32,
    /// Mid energy (250-4000 Hz, bins 40-129, normalized 0.0-1.0).
    pub mid: f32,
    /// Treble energy (4000-20000 Hz, bins 130-199, normalized 0.0-1.0).
    pub treble: f32,

    // ─── Mel Scale ──────────────────────────────────────────
    /// 24 mel-spaced frequency bands (raw energy, unnormalized).
    /// See Section 3 for filter parameters.
    pub mel_bands: [f32; 24],

    /// 24 mel bands, each divided by its running maximum (0.0-1.0).
    /// Auto-scales to the current audio -- always visually active.
    pub mel_bands_normalized: [f32; 24],

    // ─── Chromagram ─────────────────────────────────────────
    /// 12 pitch-class energy bins [C, C#, D, D#, E, F, F#, G, G#, A, A#, B].
    /// Normalized so max bin = 1.0. Computed from 4096-point FFT.
    pub chromagram: [f32; 12],

    // ─── Spectral Features ──────────────────────────────────
    /// Spectral centroid (brightness). Center-of-mass frequency, normalized 0.0-1.0.
    /// 0.0 = all energy at 20 Hz, 1.0 = all energy at 20 kHz.
    pub spectral_centroid: f32,

    /// Spectral spread (bandwidth around centroid, normalized 0.0-1.0).
    /// 0.0 = pure tone, 1.0 = energy spread across entire spectrum.
    pub spectral_spread: f32,

    /// Spectral rolloff (frequency below which 85% of energy lies, normalized 0.0-1.0).
    pub spectral_rolloff: f32,

    /// Spectral flux (rate of spectral change between frames, normalized 0.0-1.0).
    pub spectral_flux: f32,

    // ─── Beat Detection ─────────────────────────────────────
    /// True on the frame a beat onset is detected.
    pub is_on_beat: bool,

    /// Beat phase (0.0 = on beat, 1.0 = just before next beat).
    /// Continuous ramp driven by tempo tracker.
    pub beat_phase: f32,

    /// Confidence in current tempo estimate (0.0-1.0).
    pub beat_confidence: f32,

    /// Beat anticipation (0.0 = not approaching beat, 1.0 = beat imminent).
    /// Ramps up 15-25ms before predicted beat to compensate for output latency.
    pub beat_anticipation: f32,

    // ─── Harmonic Analysis ──────────────────────────────────
    /// Dominant pitch class (0-11, mapping: 0=C, 1=C#, ..., 11=B).
    pub dominant_pitch_class: u8,

    /// True if chord mood leans major (major_third > minor_third energy).
    /// False if minor or ambiguous.
    pub is_major: bool,

    /// Ratio of minor-to-major third energy relative to dominant pitch.
    /// -1.0 = strongly minor, 0.0 = ambiguous, +1.0 = strongly major.
    pub minor_major_ratio: f32,

    // ─── Derived / Convenience ──────────────────────────────
    /// Beat pulse envelope (1.0 on beat, exponential decay to 0.0).
    /// Decay rate: 5.0/s (~200ms to reach zero).
    pub beat_pulse: f32,

    /// Onset pulse envelope (like beat_pulse but for all transients, not just beats).
    pub onset_pulse: f32,

    /// Estimated tempo in BPM (clamped to 60-180 range).
    pub tempo: f32,

    /// Per-band spectral flux [bass, mid, treble].
    pub spectral_flux_bands: [f32; 3],

    /// Harmonic hue (0.0-1.0, maps pitch to color wheel via circle of fifths).
    pub harmonic_hue: f32,
}
```

### Field Correspondence to Lightscript API

| Rust Field             | Lightscript JS Path               | JS Type            | Notes                          |
| ---------------------- | --------------------------------- | ------------------ | ------------------------------ |
| `level`                | `engine.audio.level`              | `number`           | 0.0-1.0                        |
| `density`              | `engine.audio.density`            | `number`           | 0.0-1.0, spectral flatness     |
| `width`                | `engine.audio.width`              | `number`           | 0.0-1.0, stereo correlation    |
| `freq`                 | `engine.audio.freq`               | `Int8Array(200)`   | Scaled to -128..127 for compat |
| `bass`                 | `engine.audio.bass`               | `number`           | Hypercolor extension           |
| `mid`                  | `engine.audio.mid`                | `number`           | Hypercolor extension           |
| `treble`               | `engine.audio.treble`             | `number`           | Hypercolor extension           |
| `mel_bands`            | `engine.audio.melBands`           | `Float32Array(24)` | Raw energy                     |
| `mel_bands_normalized` | `engine.audio.melBandsNormalized` | `Float32Array(24)` | Auto-scaled 0-1                |
| `chromagram`           | `engine.audio.chromagram`         | `Float32Array(12)` | Pitch class energy             |
| `spectral_centroid`    | `engine.audio.spectralCentroid`   | `number`           | 0.0-1.0                        |
| `spectral_spread`      | `engine.audio.spectralSpread`     | `number`           | 0.0-1.0                        |
| `spectral_rolloff`     | `engine.audio.spectralRolloff`    | `number`           | 0.0-1.0                        |
| `spectral_flux`        | `engine.audio.spectralFlux`       | `number`           | 0.0-1.0                        |
| `is_on_beat`           | `engine.audio.isOnBeat`           | `boolean`          | True on onset frame            |
| `beat_phase`           | `engine.audio.beatPhase`          | `number`           | 0.0-1.0 continuous ramp        |
| `beat_confidence`      | `engine.audio.beatConfidence`     | `number`           | 0.0-1.0                        |
| `beat_anticipation`    | `engine.audio.beatAnticipation`   | `number`           | 0.0-1.0                        |
| `dominant_pitch_class` | `engine.audio.dominantPitchClass` | `number`           | 0-11 integer                   |
| `is_major`             | `engine.audio.isMajor`            | `boolean`          | Chord quality                  |
| `minor_major_ratio`    | `engine.audio.minorMajorRatio`    | `number`           | -1.0 to +1.0                   |
| `beat_pulse`           | `engine.audio.beatPulse`          | `number`           | Envelope 0-1                   |
| `onset_pulse`          | `engine.audio.onsetPulse`         | `number`           | Envelope 0-1                   |
| `tempo`                | `engine.audio.tempo`              | `number`           | BPM (60-180)                   |
| `spectral_flux_bands`  | `engine.audio.spectralFluxBands`  | `Float32Array(3)`  | [bass, mid, treble]            |
| `harmonic_hue`         | `engine.audio.harmonicHue`        | `number`           | 0.0-1.0 hue                    |

### Default Values (Silence)

When no audio is playing or the capture source is unavailable, `AudioData` returns sensible silence defaults:

```rust
impl Default for AudioData {
    fn default() -> Self {
        Self {
            level: 0.0,
            density: 0.0,
            width: 0.0,
            freq: [0.0; 200],
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            mel_bands: [0.0; 24],
            mel_bands_normalized: [0.0; 24],
            chromagram: [0.0; 12],
            spectral_centroid: 0.0,
            spectral_spread: 0.0,
            spectral_rolloff: 0.0,
            spectral_flux: 0.0,
            is_on_beat: false,
            beat_phase: 0.0,
            beat_confidence: 0.0,
            beat_anticipation: 0.0,
            dominant_pitch_class: 0,
            is_major: false,
            minor_major_ratio: 0.0,
            beat_pulse: 0.0,
            onset_pulse: 0.0,
            tempo: 0.0,
            spectral_flux_bands: [0.0; 3],
            harmonic_hue: 0.0,
        }
    }
}
```

---

## 2. FFT Pipeline

### Parameters

| Parameter           | Value                          | Rationale                                                                         |
| ------------------- | ------------------------------ | --------------------------------------------------------------------------------- |
| Sample rate         | 48000 Hz                       | PipeWire default. WASAPI common default. Matched to system.                       |
| Primary FFT size    | 1024 samples                   | 46.88 Hz resolution, 21.3ms window. Balances latency vs. bass resolution.         |
| Secondary FFT size  | 4096 samples                   | 11.72 Hz resolution, 85.3ms window. Used for chromagram/pitch at 15 Hz.           |
| Window function     | Hann                           | 4-bin main lobe, -31 dB side lobes. Industry standard for audio visualization.    |
| Hop size            | 256 samples (5.3ms)            | 75% overlap. Fresh spectrum every 5.3ms without sacrificing frequency resolution. |
| Magnitude scaling   | 20 \* log10(\|z\| + 1e-10)     | dB scale, floored at -100 dB.                                                     |
| Normalization range | -80 dB (silence) to 0 dB (max) | Maps to 0.0-1.0 output range.                                                     |
| FFT crate           | `realfft`                      | Real-to-complex (r2c) in-place transform. ~15 us for 1024-point.                  |

### Hann Window

Pre-computed coefficient array, applied element-wise before FFT:

```rust
/// Pre-compute Hann window coefficients (done once at startup)
fn precompute_hann(size: usize) -> Vec<f32> {
    (0..size)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / size as f32).cos()))
        .collect()
}
```

### DC Offset Removal

Subtract the buffer mean before windowing. Without this, a DC offset inflates bin 0 and bleeds into adjacent bins:

```rust
fn remove_dc_offset(samples: &mut [f32]) {
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    for s in samples.iter_mut() {
        *s -= mean;
    }
}
```

### Magnitude Computation

```rust
fn complex_to_db(re: f32, im: f32) -> f32 {
    let magnitude = (re * re + im * im).sqrt();
    let db = 20.0 * (magnitude + 1e-10).log10();
    db.max(-100.0)
}

fn db_to_normalized(db: f32) -> f32 {
    ((db - (-80.0)) / (0.0 - (-80.0))).clamp(0.0, 1.0)
}
```

### 200-Bin Logarithmic Mapping

The raw 1024-point FFT produces 512 linearly-spaced bins (each 46.88 Hz wide at 48 kHz). These are remapped to 200 logarithmically-spaced bins covering 20 Hz - 20 kHz.

**Mapping formula:**

For output bin `i` (0..199):

```
t_lo = i / 200
t_hi = (i + 1) / 200
freq_lo = exp(ln(20) + t_lo * (ln(20000) - ln(20)))
freq_hi = exp(ln(20) + t_hi * (ln(20000) - ln(20)))
fft_bin_lo = floor(freq_lo / nyquist * 512)
fft_bin_hi = max(ceil(freq_hi / nyquist * 512), fft_bin_lo + 1)
output[i] = mean(fft_magnitudes[fft_bin_lo..fft_bin_hi])
```

```rust
fn map_to_200_bins(fft_magnitudes: &[f32; 512], sample_rate: u32) -> [f32; 200] {
    let mut output = [0.0f32; 200];
    let nyquist = sample_rate as f32 / 2.0;
    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();

    for i in 0..200 {
        let t0 = i as f32 / 200.0;
        let t1 = (i + 1) as f32 / 200.0;
        let freq_lo = (log_min + t0 * (log_max - log_min)).exp();
        let freq_hi = (log_min + t1 * (log_max - log_min)).exp();

        let bin_lo = (freq_lo / nyquist * 512.0) as usize;
        let bin_hi = ((freq_hi / nyquist * 512.0) as usize)
            .max(bin_lo + 1)
            .min(512);

        let sum: f32 = fft_magnitudes[bin_lo..bin_hi].iter().sum();
        output[i] = sum / (bin_hi - bin_lo) as f32;
    }

    output
}
```

### Frequency Bin Mapping Table

Bin boundaries at 48 kHz sample rate (nyquist = 24000 Hz):

| Output Bin Range | Frequency Range | Musical Region                 | FFT Bins Covered | Bins/Output            |
| ---------------- | --------------- | ------------------------------ | ---------------- | ---------------------- |
| 0-9              | 20-46 Hz        | Sub-bass (lowest octave)       | 0-1              | ~1 each (interpolated) |
| 10-19            | 46-107 Hz       | Sub-bass / kick fundamental    | 1-2              | ~1 each                |
| 20-34            | 107-303 Hz      | Bass guitar, low synths        | 2-6              | 1-2 each               |
| 35-54            | 303-858 Hz      | Guitar body, vocal fundamental | 6-18             | 1-3 each               |
| 55-79            | 858-2.9 kHz     | Vocal presence, snare attack   | 18-62            | 2-4 each               |
| 80-109           | 2.9-8.5 kHz     | Presence, cymbal body          | 62-181           | 3-5 each               |
| 110-139          | 8.5-14.5 kHz    | Brilliance, shimmer            | 181-309          | 4-5 each               |
| 140-169          | 14.5-18.3 kHz   | Air, sibilance                 | 309-390          | 3-4 each               |
| 170-199          | 18.3-20 kHz     | Ultra-high (near inaudible)    | 390-426          | 1-2 each               |

**Key observation:** Low-frequency output bins map to very few FFT bins (sometimes < 1). This means sub-bass resolution is limited by the 1024-point FFT's 46.88 Hz spacing. The secondary 4096-point FFT (11.72 Hz resolution) is used for pitch detection precisely because of this limitation.

### Band Energy Extraction

Three summary bands computed from the 200-bin output:

| Band   | Bin Range | Frequency Range | Computation           |
| ------ | --------- | --------------- | --------------------- |
| Bass   | 0-39      | 20-250 Hz       | RMS of bins 0..=39    |
| Mid    | 40-129    | 250-4000 Hz     | RMS of bins 40..=129  |
| Treble | 130-199   | 4000-20000 Hz   | RMS of bins 130..=199 |

```rust
fn band_energy(bins: &[f32; 200], lo: usize, hi: usize) -> f32 {
    let slice = &bins[lo..=hi];
    (slice.iter().map(|x| x * x).sum::<f32>() / slice.len() as f32).sqrt()
}
```

---

## 3. Mel Filterbank

24 triangular filters mapping FFT bins to perceptually-uniform frequency bands.

### Mel Conversion Formulas

```
mel(f) = 2595 * log10(1 + f / 700)
f(mel) = 700 * (10^(mel / 2595) - 1)
```

### Filter Construction

Given `N_fft = 1024`, `sample_rate = 48000`, `n_mels = 24`:

1. Compute mel-scale bounds: `mel_min = mel(20) = 31.5`, `mel_max = mel(20000) = 3817.0`
2. Generate 26 evenly-spaced mel points (24 filters + 2 edges)
3. Convert mel points back to Hz, then to FFT bin indices
4. For each filter `i`, construct a triangle spanning `bin[i]` to `bin[i+2]` with peak at `bin[i+1]`

```rust
pub struct MelFilterbank {
    /// 24 triangular filters: Vec of (fft_bin_index, weight) pairs
    filters: Vec<Vec<(usize, f32)>>,
}

impl MelFilterbank {
    pub fn new(fft_size: usize, sample_rate: u32, n_mels: usize) -> Self {
        let nyquist = sample_rate as f32 / 2.0;
        let mel_min = hz_to_mel(20.0);
        let mel_max = hz_to_mel(nyquist.min(20000.0));

        // 26 mel points for 24 triangular filters
        let mel_points: Vec<f32> = (0..=n_mels + 1)
            .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32)
            .collect();

        let freq_points: Vec<f32> = mel_points.iter()
            .map(|&m| mel_to_hz(m))
            .collect();

        let bin_points: Vec<usize> = freq_points.iter()
            .map(|&f| (f / nyquist * (fft_size / 2) as f32) as usize)
            .collect();

        let mut filters = Vec::with_capacity(n_mels);
        for i in 0..n_mels {
            let mut filter = Vec::new();
            let (lo, center, hi) = (bin_points[i], bin_points[i + 1], bin_points[i + 2]);

            // Rising slope: lo → center
            for b in lo..center {
                let weight = (b - lo) as f32 / (center - lo).max(1) as f32;
                filter.push((b, weight));
            }
            // Falling slope: center → hi
            for b in center..=hi {
                let weight = (hi - b) as f32 / (hi - center).max(1) as f32;
                filter.push((b, weight));
            }

            filters.push(filter);
        }

        Self { filters }
    }

    pub fn apply(&self, fft_magnitudes: &[f32]) -> [f32; 24] {
        let mut out = [0.0f32; 24];
        for (i, filter) in self.filters.iter().enumerate() {
            out[i] = filter.iter()
                .map(|&(bin, weight)| fft_magnitudes.get(bin).unwrap_or(&0.0) * weight)
                .sum();
        }
        out
    }

    fn hz_to_mel(hz: f32) -> f32 { 2595.0 * (1.0 + hz / 700.0).log10() }
    fn mel_to_hz(mel: f32) -> f32 { 700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0) }
}
```

### 24-Band Mel Filter Parameters

Computed for FFT size 1024, sample rate 48000 Hz:

| Band | Center Freq (Hz) | Bandwidth (Hz) | Low Edge (Hz) | High Edge (Hz) | Musical Region        |
| ---- | ---------------- | -------------- | ------------- | -------------- | --------------------- |
| 0    | 47               | 30             | 20            | 73             | Sub-bass fundamental  |
| 1    | 73               | 35             | 47            | 104            | Kick drum fundamental |
| 2    | 104              | 42             | 73            | 141            | Bass guitar low       |
| 3    | 141              | 50             | 104           | 187            | Bass guitar high      |
| 4    | 187              | 60             | 141           | 243            | Low vocals            |
| 5    | 243              | 72             | 187           | 313            | Vocal fundamental     |
| 6    | 313              | 86             | 243           | 399            | Guitar body           |
| 7    | 399              | 103            | 313           | 507            | Snare body            |
| 8    | 507              | 123            | 399           | 641            | Low-mid               |
| 9    | 641              | 148            | 507           | 808            | Mid                   |
| 10   | 808              | 177            | 641           | 1016           | Vocal presence        |
| 11   | 1016             | 213            | 808           | 1280           | Upper mid             |
| 12   | 1280             | 256            | 1016          | 1613           | Guitar bite           |
| 13   | 1613             | 307            | 1280          | 2031           | Vocal clarity         |
| 14   | 2031             | 369            | 1613          | 2554           | Snare attack          |
| 15   | 2554             | 443            | 2031          | 3214           | Presence              |
| 16   | 3214             | 532            | 2554          | 4044           | High presence         |
| 17   | 4044             | 639            | 3214          | 5088           | Cymbal body           |
| 18   | 5088             | 767            | 4044          | 6405           | Brilliance            |
| 19   | 6405             | 921            | 5088          | 8063           | High brilliance       |
| 20   | 8063             | 1106           | 6405          | 10147          | Air                   |
| 21   | 10147            | 1330           | 8063          | 12773          | Upper air             |
| 22   | 12773            | 1594           | 10147         | 16078          | Shimmer               |
| 23   | 16078            | 1922           | 12773         | 20000          | Ultra-high presence   |

### Auto-Normalization

`mel_bands_normalized` divides each band by its running maximum, tracked with a slow decay:

```rust
pub struct MelNormalizer {
    running_max: [f32; 24],
    decay: f32, // 0.999 per frame (~16 seconds to halve at 60 fps)
}

impl MelNormalizer {
    pub fn normalize(&mut self, raw: &[f32; 24]) -> [f32; 24] {
        let mut out = [0.0f32; 24];
        for i in 0..24 {
            self.running_max[i] = self.running_max[i].max(raw[i]) * self.decay;
            self.running_max[i] = self.running_max[i].max(0.001); // prevent div-by-zero
            out[i] = (raw[i] / self.running_max[i]).clamp(0.0, 1.0);
        }
        out
    }
}
```

---

## 4. Chromagram

12 pitch-class energy bins computed from the 4096-point FFT for superior frequency resolution (11.72 Hz per bin at 48 kHz).

### Pitch Class Mapping

A4 = 440 Hz reference. Semitones from A4: `12 * log2(f / 440)`. Map to pitch class (0=C through 11=B):

| Index | Pitch | Frequency Examples (Hz)                                        | Octaves in 20-20kHz |
| ----- | ----- | -------------------------------------------------------------- | ------------------- |
| 0     | C     | 32.7, 65.4, 130.8, 261.6, 523.3, 1047, 2093, 4186, 8372, 16744 | 10                  |
| 1     | C#    | 34.6, 69.3, 138.6, 277.2, 554.4, 1109, 2217, 4435, 8870, 17740 | 10                  |
| 2     | D     | 36.7, 73.4, 146.8, 293.7, 587.3, 1175, 2349, 4699, 9397        | 9                   |
| 3     | D#    | 38.9, 77.8, 155.6, 311.1, 622.3, 1245, 2489, 4978, 9956        | 9                   |
| 4     | E     | 41.2, 82.4, 164.8, 329.6, 659.3, 1319, 2637, 5274, 10548       | 9                   |
| 5     | F     | 43.7, 87.3, 174.6, 349.2, 698.5, 1397, 2794, 5588, 11175       | 9                   |
| 6     | F#    | 46.2, 92.5, 185.0, 370.0, 740.0, 1480, 2960, 5920, 11840       | 9                   |
| 7     | G     | 49.0, 98.0, 196.0, 392.0, 784.0, 1568, 3136, 6272, 12544       | 9                   |
| 8     | G#    | 51.9, 103.8, 207.7, 415.3, 830.6, 1661, 3322, 6645, 13290      | 9                   |
| 9     | A     | 55.0, 110.0, 220.0, 440.0, 880.0, 1760, 3520, 7040, 14080      | 9                   |
| 10    | A#    | 58.3, 116.5, 233.1, 466.2, 932.3, 1865, 3729, 7459, 14917      | 9                   |
| 11    | B     | 61.7, 123.5, 246.9, 493.9, 987.8, 1976, 3951, 7902, 15804      | 9                   |

### Frequency-to-Pitch Mapping

```rust
pub fn compute_chromagram(
    fft_magnitudes: &[f32],
    sample_rate: u32,
    fft_size: usize,
) -> [f32; 12] {
    let mut chroma = [0.0f32; 12];
    let freq_resolution = sample_rate as f32 / fft_size as f32;

    for (bin, &magnitude) in fft_magnitudes.iter().enumerate().skip(1) {
        let freq = bin as f32 * freq_resolution;
        if freq < 20.0 || freq > 10000.0 { continue; }

        // Semitones from A4, mapped to pitch class 0-11
        let semitones = 12.0 * (freq / 440.0).log2();
        let pitch_class = ((semitones.round() as i32 % 12 + 12) % 12) as usize;

        // Shift so C=0 (A is naturally index 0 in this formula)
        let mapped = (pitch_class + 3) % 12;
        chroma[mapped] += magnitude * magnitude; // energy = squared magnitude
    }

    // Normalize: max bin = 1.0
    let max = chroma.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        for c in &mut chroma { *c /= max; }
    }

    chroma
}
```

### Dominant Pitch and Chord Mood

```rust
/// Returns (pitch_class 0-11, confidence 0-1)
pub fn dominant_pitch(chroma: &[f32; 12]) -> (u8, f32) {
    let (idx, &max_val) = chroma.iter().enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .unwrap();
    let sum: f32 = chroma.iter().sum();
    let confidence = if sum > 0.0 { max_val / sum } else { 0.0 };
    (idx as u8, confidence)
}

/// Chord mood: -1.0 (minor) to +1.0 (major)
pub fn chord_mood(chroma: &[f32; 12], root: usize) -> f32 {
    let major_third = chroma[(root + 4) % 12];
    let minor_third = chroma[(root + 3) % 12];
    let fifth = chroma[(root + 7) % 12];

    let fifth_weight = 0.5 + 0.5 * fifth;

    if major_third + minor_third < 0.01 { return 0.0; }

    ((major_third - minor_third) / (major_third + minor_third) * fifth_weight)
        .clamp(-1.0, 1.0)
}
```

### Execution Cadence

The 4096-point FFT runs at **15 Hz** (every 4th frame at 60fps). Chromagram, dominant pitch, and chord mood are computed from this secondary FFT. Between updates, previous values are held. Cost: ~60 us every 4th frame, negligible when amortized.

---

## 5. Beat Detection Algorithm

Three complementary methods are fused to produce reliable beat detection across genres.

### Architecture

```
                    +--------------------+
                    |   Band Energy      |--> Bass onset
                    |   Onset            |--> Mid onset
                    |   Detection        |--> Treble onset
                    +--------+-----------+
                             |
+----------------+           |         +--------------------+
| Spectral       |-----------+-------->|  Beat Fusion       |--> is_on_beat
| Flux           |           |         |  Engine            |--> beat_pulse
| Detection      |-----------+    +--->|                    |--> beat_phase
+----------------+                |    |                    |--> beat_confidence
                                  |    |                    |--> beat_anticipation
+----------------+                |    |                    |--> tempo
| Tempo          |----------------+    +--------------------+
| Tracker        |
| (auto-corr.)   |
+----------------+
```

### Method 1: Energy-Based Onset Detection

Instantaneous band energy compared against a running average with dynamic threshold:

```rust
pub struct EnergyOnsetDetector {
    energy_short: f32,    // Fast EMA (~50ms window), alpha = 0.4
    energy_long: f32,     // Slow EMA (~1-2s window), alpha = 0.02
    cooldown: f32,        // Seconds remaining before next onset allowed
    min_interval: f32,    // Minimum inter-onset interval (default: 0.15s = 400 BPM max)
    threshold: f32,       // Sensitivity multiplier (default: 1.5)
}

impl EnergyOnsetDetector {
    pub fn update(&mut self, energy: f32, dt: f32) -> bool {
        self.energy_short = lerp(self.energy_short, energy, 0.4);
        self.energy_long = lerp(self.energy_long, energy, 0.02);
        self.cooldown -= dt;

        let dynamic_thresh = self.energy_long * self.threshold + 0.02;

        if self.energy_short > dynamic_thresh && self.cooldown <= 0.0 {
            self.cooldown = self.min_interval;
            true
        } else {
            false
        }
    }
}
```

Three independent instances run on bass (20-250 Hz), mid (250-4 kHz), and treble (4-20 kHz) bands.

| Band         | Onset Character      | Typical Lighting Response         |
| ------------ | -------------------- | --------------------------------- |
| Bass onset   | Kick drum, bass drop | Full-room pulse, brightness spike |
| Mid onset    | Snare, vocal attack  | Flash, pattern shift              |
| Treble onset | Hi-hat, cymbal       | Sparkle, particle burst           |

### Method 2: Spectral Flux

Measures how much the frequency spectrum changed between frames. Catches onsets that energy detection misses (e.g., a snare during sustained bass):

```rust
pub struct SpectralFluxDetector {
    prev_spectrum: [f32; 200],
    flux_history: VecDeque<f32>, // ~1 second at 60fps (60 entries)
    threshold_multiplier: f32,   // Default: 1.5
}

impl SpectralFluxDetector {
    pub fn update(&mut self, spectrum: &[f32; 200]) -> f32 {
        let mut flux = 0.0;
        for i in 0..200 {
            // Half-wave rectification: only count increases
            let diff = spectrum[i] - self.prev_spectrum[i];
            if diff > 0.0 {
                flux += diff;
            }
        }

        self.prev_spectrum.copy_from_slice(spectrum);
        self.flux_history.push_back(flux);
        while self.flux_history.len() > 60 {
            self.flux_history.pop_front();
        }

        flux
    }

    pub fn is_onset(&self) -> bool {
        let flux = *self.flux_history.back().unwrap_or(&0.0);
        let mean: f32 = self.flux_history.iter().sum::<f32>()
            / self.flux_history.len() as f32;
        let variance: f32 = self.flux_history.iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f32>() / self.flux_history.len() as f32;
        let std_dev = variance.sqrt();

        flux > mean + self.threshold_multiplier * std_dev
    }
}
```

Banded spectral flux is computed identically over bass/mid/treble bin ranges, producing `spectral_flux_bands[3]`.

### Method 3: BPM Estimation and Phase Tracking

Auto-correlation of inter-onset intervals with histogram clustering:

```rust
pub struct TempoTracker {
    onset_times: VecDeque<f64>,  // Onset timestamps (seconds), 10s history
    bpm: f32,                     // Current estimated BPM
    phase: f32,                   // 0.0 = on beat, 1.0 = just before next
    confidence: f32,              // 0.0-1.0
    last_beat_time: f64,
    beat_interval: f64,           // Seconds between beats
}
```

**BPM range clamping:** Detected tempos outside 60-180 BPM are halved or doubled:

```rust
fn normalize_bpm(raw_bpm: f32) -> f32 {
    let mut bpm = raw_bpm;
    while bpm < 60.0 { bpm *= 2.0; }
    while bpm > 180.0 { bpm /= 2.0; }
    bpm
}
```

### Beat Anticipation

Once a stable BPM is established (confidence > 0.3), the anticipation signal ramps from 0.0 to 1.0 over the final 15-25ms before the predicted next beat:

```rust
pub struct BeatAnticipator {
    anticipation_ms: f32,  // Default: 20ms. Configurable per device latency profile.
    pub anticipation: f32, // 0.0-1.0 output
}

impl BeatAnticipator {
    pub fn update(&mut self, phase: f32, beat_interval: f64, confidence: f32) {
        if confidence < 0.3 {
            self.anticipation = 0.0;
            return;
        }

        let anticipation_phase = self.anticipation_ms / 1000.0 / beat_interval as f32;
        let time_to_beat = 1.0 - phase;

        if time_to_beat < anticipation_phase {
            self.anticipation = 1.0 - (time_to_beat / anticipation_phase);
        } else {
            self.anticipation = 0.0;
        }
    }
}
```

### Beat Pulse Envelope

Sharp attack, exponential decay:

```rust
pub struct BeatPulse {
    pub value: f32,
    decay_rate: f32, // Default: 5.0/s (~200ms to zero)
}

impl BeatPulse {
    pub fn update(&mut self, beat: bool, dt: f32) {
        if beat {
            self.value = 1.0;
        } else {
            self.value = (self.value - self.decay_rate * dt).max(0.0);
        }
    }
}
```

### Tempo Change Handling

| Scenario                        | Strategy                                                                                           |
| ------------------------------- | -------------------------------------------------------------------------------------------------- |
| Tempo change                    | BPM estimator adapts over 2-4 seconds as new intervals accumulate                                  |
| Breakdown (quiet section)       | Onset energy drops below threshold. Phase tracking continues on last known BPM. Confidence decays. |
| Drop (breakdown to full energy) | First strong onset after breakdown resets phase. Immediate lock.                                   |
| Stop-start                      | After 3 seconds of no onsets, reset BPM tracking. Fresh start.                                     |
| Rubato / live music             | Lower confidence, reduce anticipation. Fall back to pure onset detection.                          |

---

## 6. Smoothing

### Asymmetric Exponential Moving Average

All frequency and energy fields use asymmetric EMA: fast attack (lights snap on) and slow decay (lights fade naturally).

```rust
fn asymmetric_smooth(current: f32, previous: f32, attack: f32, decay: f32) -> f32 {
    let factor = if current > previous { attack } else { decay };
    previous + factor * (current - previous)
}
```

### Per-Field Smoothing Constants

| Field(s)                | Attack | Decay | Rationale                                     |
| ----------------------- | ------ | ----- | --------------------------------------------- |
| `freq[200]`             | 0.6    | 0.15  | Spectrum bars: snap up, slow fade             |
| `bass`, `mid`, `treble` | 0.5    | 0.12  | Band energy: slightly smoother than raw bins  |
| `mel_bands[24]`         | 0.6    | 0.15  | Same as freq -- perceptually equivalent       |
| `level`                 | 0.4    | 0.10  | Overall level: gentler tracking               |
| `spectral_centroid`     | 0.3    | 0.08  | Brightness: slow drift, not jitter            |
| `spectral_spread`       | 0.3    | 0.08  | Same cadence as centroid                      |
| `spectral_rolloff`      | 0.3    | 0.08  | Same cadence as centroid                      |
| `spectral_flux`         | 0.8    | 0.3   | Fast response -- flux is inherently transient |
| `chromagram[12]`        | 0.2    | 0.05  | Pitch: stable, not jumpy                      |
| `harmonic_hue`          | 0.15   | 0.03  | Color: very slow drift (avoids disco strobe)  |
| `minor_major_ratio`     | 0.15   | 0.05  | Chord mood: gradual emotional shift           |
| `beat_pulse`            | 1.0    | N/A   | Handled by BeatPulse decay, not EMA           |
| `beat_anticipation`     | 1.0    | N/A   | Handled by anticipator, not EMA               |
| `density`               | 0.2    | 0.05  | Spectral flatness: stable                     |
| `width`                 | 0.2    | 0.05  | Stereo width: stable                          |

### Peak Hold (Optional)

For spectrum visualizer effects, peaks can be held before decaying:

```rust
pub struct PeakHold {
    value: f32,
    hold_frames: u32,   // Default: 15 (250ms at 60fps)
    frames_held: u32,
}

impl PeakHold {
    pub fn update(&mut self, new_value: f32) -> f32 {
        if new_value >= self.value {
            self.value = new_value;
            self.frames_held = 0;
        } else if self.frames_held < self.hold_frames {
            self.frames_held += 1;
        } else {
            self.value *= 0.95; // Gravity decay
        }
        self.value
    }
}
```

---

## 7. AudioInput Struct

Owns the cpal stream, manages source selection, and handles monitor source detection.

```rust
pub struct AudioInput {
    /// Active cpal input stream (None if capture is disabled)
    stream: Option<cpal::Stream>,

    /// Ring buffer: audio thread writes, DSP thread reads
    sample_buffer: Arc<ArrayQueue<f32>>,

    /// Current source configuration
    config: AudioSourceConfig,

    /// Detected sample rate of the active device
    actual_sample_rate: u32,

    /// Detected channel count
    channels: u16,

    /// Whether the source is a monitor (loopback) device
    is_monitor: bool,

    /// Source metadata for UI display
    source_name: String,
}

pub struct AudioSourceConfig {
    /// Source type: system monitor, microphone, or specific device
    pub source: AudioSource,
    /// Target sample rate (resample if device differs)
    pub sample_rate: u32,
    /// Input gain multiplier (1.0 = unity)
    pub gain: f32,
    /// Noise gate threshold in dB (-60 to 0)
    pub noise_gate_db: f32,
}

pub enum AudioSource {
    /// Auto-detect system audio output monitor (default)
    SystemMonitor,
    /// Specific PulseAudio/PipeWire source or WASAPI device by name
    Named(String),
    /// Microphone input (default input device)
    Microphone,
    /// No audio input (effects receive silence)
    None,
}
```

### Monitor Source Discovery

**Linux (PipeWire / PulseAudio):**

1. Query `libpulse` via `libpulse-binding` for the default output sink name
2. Construct monitor source name: `{sink_name}.monitor`
3. Verify source exists via `pa_context_get_source_info_list()`, filtering for `PA_SOURCE_MONITOR` flag
4. Pass discovered source name to cpal as the input device

**Windows (WASAPI):**

1. Use cpal's WASAPI backend to enumerate output devices
2. Select the default output device
3. Open it as a loopback capture stream (WASAPI loopback mode)
4. cpal on Windows supports this via `SupportedStreamConfig` on the output device used as input

### Stream Lifecycle

```rust
impl AudioInput {
    pub fn start(&mut self) -> Result<()> {
        let device = self.resolve_device()?;
        let config = device.default_input_config()?;
        self.actual_sample_rate = config.sample_rate().0;
        self.channels = config.channels();

        let buffer = self.sample_buffer.clone();
        let gain = self.config.gain;
        let channels = self.channels;

        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Downmix to mono, apply gain, push to ring buffer
                for chunk in data.chunks(channels as usize) {
                    let mono = chunk.iter().sum::<f32>() / channels as f32;
                    let _ = buffer.push(mono * gain); // Drop if full (backpressure)
                }
            },
            |err| eprintln!("Audio capture error: {err}"),
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stream = None; // Drop stops the stream
    }
}
```

---

## 8. Thread Model

### Architecture

```
+----------------------------------------------------------------+
|  Main Thread (tokio async runtime)                              |
|  +- Render loop (60fps tick)                                    |
|  +- Device output (USB HID, WLED, Hue)                         |
|  +- API / WebSocket server                                      |
|  +- Reads AudioData via Arc<TripleBuffer<AudioData>>            |
+----------------------------------------------------------------+

+----------------------------------------------------------------+
|  Audio Capture Thread (OS-managed, cpal callback)               |
|  +- Invoked by OS audio subsystem at device buffer cadence      |
|  +- Writes f32 samples to lock-free SPSC ring buffer            |
|  +- MUST NOT block: no allocations, no locks, no I/O            |
+----------------------------------------------------------------+

+----------------------------------------------------------------+
|  DSP Thread (dedicated std::thread, optionally core-pinned)     |
|  +- Wakes every 5.3ms (hop size) via condvar or spin            |
|  +- Reads samples from ring buffer                              |
|  +- Runs full DSP pipeline (~86 us per frame)                   |
|  +- Writes completed AudioData to triple buffer back slot       |
|  +- Publishes via atomic swap                                   |
+----------------------------------------------------------------+
```

### Lock-Free Data Transfer

**Capture to DSP: SPSC ring buffer**

```rust
use crossbeam::queue::ArrayQueue;

/// Capacity: 4096 samples (~85ms at 48kHz)
/// Enough for a full 4096-point FFT window
let sample_buffer: Arc<ArrayQueue<f32>> = Arc::new(ArrayQueue::new(4096));
```

The cpal callback pushes samples. The DSP thread pops them. If the buffer fills (DSP fell behind), new samples are silently dropped -- preferable to blocking the audio thread.

**DSP to render: triple buffer**

A triple buffer provides always-available latest data with zero contention:

```rust
use triple_buffer::TripleBuffer;

let (mut writer, reader) = TripleBuffer::new(&AudioData::default()).split();

// DSP thread:
writer.write(computed_audio_data);

// Render thread:
let audio = reader.read(); // Always returns latest, never blocks
```

Alternative: `crossbeam::utils::CachePadded<AtomicCell<AudioData>>` if `AudioData` fits in a cache line (it does not at ~1200 bytes). For structs this large, triple buffer or `Arc<ArcSwap<AudioData>>` is the right primitive.

### Why Not Tokio?

The DSP pipeline is pure computation -- no I/O waits, no async boundaries. Running it on a dedicated `std::thread` with a tight timing loop guarantees:

1. No task scheduler jitter from competing async tasks
2. Deterministic wake-up timing (condvar or high-resolution timer)
3. Option to pin to a specific CPU core for cache locality
4. No risk of accidentally holding an executor thread during FFT computation

---

## 9. Servo Injection

For the Servo (HTML/Canvas) rendering path, `AudioData` is serialized as JavaScript and injected into `window.engine.audio` every frame via `evaluate_javascript()`.

### JavaScript Injection Code

```rust
impl AudioData {
    pub fn to_js_injection(&self) -> String {
        format!(
            r#"window.engine = window.engine || {{}};
            window.engine.audio = {{
                level: {level},
                density: {density},
                width: {width},
                freq: new Int8Array([{freq}]),
                bass: {bass},
                mid: {mid},
                treble: {treble},
                melBands: new Float32Array([{mel}]),
                melBandsNormalized: new Float32Array([{mel_norm}]),
                chromagram: new Float32Array([{chroma}]),
                spectralCentroid: {spectral_centroid},
                spectralSpread: {spectral_spread},
                spectralRolloff: {spectral_rolloff},
                spectralFlux: {spectral_flux},
                isOnBeat: {is_on_beat},
                beatPhase: {beat_phase},
                beatConfidence: {beat_confidence},
                beatAnticipation: {beat_anticipation},
                dominantPitchClass: {dominant_pitch_class},
                isMajor: {is_major},
                minorMajorRatio: {minor_major_ratio},
                beatPulse: {beat_pulse},
                onsetPulse: {onset_pulse},
                tempo: {tempo},
                spectralFluxBands: new Float32Array([{flux_bands}]),
                harmonicHue: {harmonic_hue}
            }};"#,
            level = self.level,
            density = self.density,
            width = self.width,
            freq = self.freq_to_int8_string(),
            bass = self.bass,
            mid = self.mid,
            treble = self.treble,
            mel = float_array_str(&self.mel_bands),
            mel_norm = float_array_str(&self.mel_bands_normalized),
            chroma = float_array_str(&self.chromagram),
            spectral_centroid = self.spectral_centroid,
            spectral_spread = self.spectral_spread,
            spectral_rolloff = self.spectral_rolloff,
            spectral_flux = self.spectral_flux,
            is_on_beat = self.is_on_beat,
            beat_phase = self.beat_phase,
            beat_confidence = self.beat_confidence,
            beat_anticipation = self.beat_anticipation,
            dominant_pitch_class = self.dominant_pitch_class,
            is_major = self.is_major,
            minor_major_ratio = self.minor_major_ratio,
            beat_pulse = self.beat_pulse,
            onset_pulse = self.onset_pulse,
            tempo = self.tempo,
            flux_bands = float_array_str(&self.spectral_flux_bands),
            harmonic_hue = self.harmonic_hue,
        )
    }

    /// Convert freq[200] to Int8Array values (matching LightScript convention).
    /// LightScript stores freq as signed 8-bit: -128 to 127.
    fn freq_to_int8_string(&self) -> String {
        self.freq.iter()
            .map(|&f| ((f * 255.0 - 128.0).clamp(-128.0, 127.0)) as i8)
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn float_array_str(arr: &[f32]) -> String {
    arr.iter()
        .map(|v| format!("{v:.4}"))
        .collect::<Vec<_>>()
        .join(",")
}
```

### Backward Compatibility

Existing community HTML effects (230+) access the original three fields: `level`, `density`, `width`, and `freq`. These are always populated. All Hypercolor extensions (`bass`, `mel_bands`, `chromagram`, `isOnBeat`, etc.) are additive -- effects that don't reference them are unaffected.

Effects can detect Hypercolor's extended API:

```javascript
const hasExtendedAudio = typeof engine.audio.melBands !== "undefined";
```

---

## 10. wgpu Injection

Native WGSL shaders receive audio data through two mechanisms: a uniform buffer for scalar/small-vector fields, and 1D textures for array data.

### Uniform Buffer Layout

```rust
/// GPU-side audio uniforms. Matches the WGSL struct byte-for-byte.
/// 16-byte aligned per wgpu uniform buffer requirements.
#[repr(C, align(16))]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AudioUniforms {
    // ─── Row 0 (bytes 0-15) ─────────────────────
    pub level: f32,              // offset 0
    pub bass: f32,               // offset 4
    pub mid: f32,                // offset 8
    pub treble: f32,             // offset 12

    // ─── Row 1 (bytes 16-31) ────────────────────
    pub beat_pulse: f32,         // offset 16
    pub beat_phase: f32,         // offset 20
    pub beat_confidence: f32,    // offset 24
    pub beat_anticipation: f32,  // offset 28

    // ─── Row 2 (bytes 32-47) ────────────────────
    pub tempo: f32,              // offset 32
    pub harmonic_hue: f32,       // offset 36
    pub spectral_flux: f32,      // offset 40
    pub spectral_centroid: f32,  // offset 44

    // ─── Row 3 (bytes 48-63) ────────────────────
    pub spectral_spread: f32,    // offset 48
    pub spectral_rolloff: f32,   // offset 52
    pub minor_major_ratio: f32,  // offset 56
    pub density: f32,            // offset 60

    // ─── Row 4 (bytes 64-79) ────────────────────
    pub width: f32,              // offset 64
    pub onset_pulse: f32,        // offset 68
    pub dominant_pitch_class: f32, // offset 72  (f32 for GPU compat, cast from u8)
    pub is_on_beat: f32,         // offset 76  (0.0 or 1.0)

    // ─── Row 5 (bytes 80-95) ────────────────────
    pub is_major: f32,           // offset 80  (0.0 or 1.0)
    pub _pad: [f32; 3],          // offset 84  (padding to 96 bytes)
}
// Total: 96 bytes (6 vec4 rows)
```

### WGSL Struct Definition

```wgsl
// audio.wgsl -- shared audio uniform interface
// Bind group 1 is reserved for audio data across all effect shaders.

struct AudioData {
    // Row 0
    level: f32,
    bass: f32,
    mid: f32,
    treble: f32,

    // Row 1
    beat_pulse: f32,
    beat_phase: f32,
    beat_confidence: f32,
    beat_anticipation: f32,

    // Row 2
    tempo: f32,
    harmonic_hue: f32,
    spectral_flux: f32,
    spectral_centroid: f32,

    // Row 3
    spectral_spread: f32,
    spectral_rolloff: f32,
    minor_major_ratio: f32,
    density: f32,

    // Row 4
    width: f32,
    onset_pulse: f32,
    dominant_pitch_class: f32,
    is_on_beat: f32,

    // Row 5
    is_major: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(1) @binding(0) var<uniform> audio: AudioData;

// Array data as 1D textures (R32Float format):
@group(1) @binding(1) var audio_spectrum: texture_1d<f32>;  // 200 texels (freq bins)
@group(1) @binding(2) var audio_mel: texture_1d<f32>;       // 24 texels (mel bands)
@group(1) @binding(3) var audio_chroma: texture_1d<f32>;    // 12 texels (chromagram)
```

### Texture Format

| Binding | Texture          | Width | Format     | Content                               |
| ------- | ---------------- | ----- | ---------- | ------------------------------------- |
| 1       | `audio_spectrum` | 200   | `R32Float` | `freq[200]` logarithmic bins, 0.0-1.0 |
| 2       | `audio_mel`      | 24    | `R32Float` | `mel_bands_normalized[24]`, 0.0-1.0   |
| 3       | `audio_chroma`   | 12    | `R32Float` | `chromagram[12]`, 0.0-1.0             |

Textures are updated each frame via `queue.write_texture()`. The 1D texture approach avoids the 256-element minimum for storage buffers and provides hardware-accelerated sampling with filtering.

### Shader Usage Example

```wgsl
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / uniforms.resolution;

    // Read a frequency bin by UV coordinate
    let bin = textureLoad(audio_spectrum, i32(uv.x * 200.0), 0).r;

    // Use scalar uniforms directly
    let pulse = audio.beat_pulse * audio.bass;
    let brightness = 0.2 + 0.8 * pulse;

    // Color from harmonic analysis
    let hue = audio.harmonic_hue;

    return hsv_to_rgb(hue, 0.8, brightness * bin);
}
```

---

## 11. Cross-Platform Audio

Hypercolor targets Linux (primary) and Windows (secondary) via cpal's platform backends.

### Linux: PipeWire / PulseAudio

| Layer      | Role                                      | Crate                                |
| ---------- | ----------------------------------------- | ------------------------------------ |
| PipeWire   | Graph-based audio routing, modern default | Accessed via PulseAudio compat layer |
| PulseAudio | Legacy systems, source enumeration        | `libpulse-binding`                   |
| ALSA       | Lowest level, no native loopback          | cpal fallback (mic input only)       |

**System audio capture:** PipeWire and PulseAudio expose every output sink as a `.monitor` source. Hypercolor detects the default sink's monitor automatically via `pa_context_get_source_info_list()` with `PA_SOURCE_MONITOR` flag filtering.

**Source discovery flow:**

1. Connect to PulseAudio server (works on both PulseAudio and PipeWire via `pipewire-pulse`)
2. Query default sink name
3. Construct monitor name: `{sink_name}.monitor`
4. Verify existence in source list
5. Set `PULSE_SOURCE` env var or pass device name to cpal

**JACK support:** PipeWire speaks JACK natively. Legacy JACK-only setups (rare) can use cpal's `jack` feature flag.

**Bare ALSA:** Not supported for system loopback. Requires manual `snd-aloop` kernel module setup. Hypercolor displays a diagnostic message recommending PipeWire installation.

### Windows: WASAPI

| Feature            | Behavior                                                                |
| ------------------ | ----------------------------------------------------------------------- |
| Backend            | cpal WASAPI host (default on Windows)                                   |
| System loopback    | WASAPI loopback capture mode on the default render endpoint             |
| Device enumeration | `IMMDeviceEnumerator` via cpal's device listing                         |
| Sample format      | f32 (cpal handles conversion from device native format)                 |
| Latency            | Shared mode: ~10ms. Exclusive mode: ~3ms (not recommended for loopback) |

**WASAPI loopback capture:**

WASAPI provides native loopback capture via `IAudioClient` initialized with `AUDCLNT_STREAMFLAGS_LOOPBACK`. cpal supports this on Windows when opening an output device as an input stream. The monitor source discovery is simpler than Linux -- WASAPI loopback "just works" on the default render device.

```rust
// Platform-specific monitor discovery
#[cfg(target_os = "linux")]
fn find_monitor_source() -> Result<cpal::Device> {
    // Use libpulse to find default sink's .monitor source
    let monitor_name = discover_pulse_monitor()?;
    find_cpal_device_by_name(&monitor_name)
}

#[cfg(target_os = "windows")]
fn find_monitor_source() -> Result<cpal::Device> {
    // WASAPI: use default output device in loopback mode
    let host = cpal::default_host();
    host.default_output_device()
        .ok_or_else(|| anyhow!("No default output device found"))
}
```

### Sample Rate Negotiation

The audio pipeline adapts to whatever sample rate the device provides:

| Device Rate | FFT Size | Freq Resolution | Nyquist  |
| ----------- | -------- | --------------- | -------- |
| 44100 Hz    | 1024     | 43.07 Hz        | 22050 Hz |
| 48000 Hz    | 1024     | 46.88 Hz        | 24000 Hz |
| 96000 Hz    | 1024     | 93.75 Hz        | 48000 Hz |

If the device sample rate differs from 48000, the mel filterbank and bin mapping tables are recomputed at startup. No runtime resampling is performed -- the FFT adapts to the native rate.

---

## 12. Configuration

### Audio Configuration Schema

```toml
[audio]
# Source selection
source = "system-monitor"    # "system-monitor" | "microphone" | "none" | "<device-name>"

# Gain and sensitivity
gain = 1.0                   # Input gain multiplier (0.1 - 5.0)
noise_gate_db = -60.0        # Noise floor in dB (-80 to -20). Below this = silence.

# Beat detection tuning
beat_threshold = 1.5         # Onset sensitivity multiplier (0.5 = very sensitive, 3.0 = only hard hits)
beat_cooldown_ms = 150       # Minimum ms between beats (prevents double-triggers)
anticipation_ms = 20         # Beat anticipation lead time in ms

# Smoothing overrides (defaults from Section 6 table)
smoothing_attack = 0.6       # Rising edge factor (0.0 - 1.0)
smoothing_decay = 0.15       # Falling edge factor (0.0 - 1.0)

# Band split points (Hz)
bass_ceiling = 250           # Bass/mid crossover frequency
treble_floor = 4000          # Mid/treble crossover frequency

# FFT configuration (rarely changed)
fft_size = 1024              # Primary FFT window size (256, 512, 1024, 2048, 4096)
hop_size = 256               # Samples between FFT frames

# Quality tier
quality = "full"             # "full" | "balanced" | "minimal"
```

### Quality Tiers

| Tier         | FFT Size    | DSP Rate | Features Enabled                                    | CPU Cost     |
| ------------ | ----------- | -------- | --------------------------------------------------- | ------------ |
| **Full**     | 1024 + 4096 | 60 Hz    | All: spectrum, mel, chroma, harmonics, beat         | ~86 us/frame |
| **Balanced** | 1024        | 30 Hz    | Spectrum, mel, beat detection. No chroma/harmonics. | ~40 us/frame |
| **Minimal**  | 512         | 30 Hz    | Spectrum, beat detection only. No mel, no chroma.   | ~20 us/frame |

```rust
pub enum AudioQuality {
    /// Full pipeline: FFT 1024 + 4096, mel + chroma + harmonics @ 60Hz
    Full,
    /// Balanced: FFT 1024, mel + beat @ 30Hz, no chroma/harmonics
    Balanced,
    /// Minimal: FFT 512, beat @ 30Hz, no mel/chroma
    Minimal,
}

impl AudioQuality {
    pub fn primary_fft_size(&self) -> usize {
        match self {
            Self::Full | Self::Balanced => 1024,
            Self::Minimal => 512,
        }
    }

    pub fn dsp_rate_hz(&self) -> u32 {
        match self {
            Self::Full => 60,
            Self::Balanced | Self::Minimal => 30,
        }
    }

    pub fn enable_chromagram(&self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn enable_mel(&self) -> bool {
        matches!(self, Self::Full | Self::Balanced)
    }
}
```

### Noise Floor Auto-Calibration

Measure ambient noise for 2 seconds with no music playing, set the gate 6 dB above:

```rust
pub fn auto_calibrate_noise_floor(samples: &[f32]) -> f32 {
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
    let db = 20.0 * (rms + 1e-10).log10();
    db + 6.0 // 6 dB headroom above measured floor
}
```

### Per-Device Latency Compensation

Different output transports have different latencies. Beat anticipation can be adjusted per device to maintain sync:

```rust
pub struct DeviceLatencyProfile {
    /// Known output latency for this device/transport (ms)
    pub output_latency_ms: f32,
    /// Additional anticipation offset to compensate (ms)
    pub anticipation_offset_ms: f32,
}
```

| Transport                    | output_latency_ms | anticipation_offset_ms |
| ---------------------------- | ----------------- | ---------------------- |
| USB HID (PrismRGB, keyboard) | 8                 | 0                      |
| WLED (DDP over LAN)          | 4                 | 0                      |
| Philips Hue (DTLS UDP)       | 35                | 25                     |

---

## Appendix A: Complete DSP Frame Execution Order

One frame of the DSP pipeline, executed every hop (5.3ms at 60 Hz, 10.6ms at 30 Hz):

```
 1. Read 256 new samples from ring buffer (append to 1024-sample sliding window)
 2. If < 1024 samples accumulated, return previous AudioData
 3. Copy window, remove DC offset (subtract mean)
 4. Apply Hann window coefficients (element-wise multiply)
 5. Execute 1024-point real-to-complex FFT
 6. Compute magnitude spectrum (512 bins, dB scale, normalize to 0-1)
 7. Map 512 linear bins -> 200 logarithmic bins
 8. Apply asymmetric EMA smoothing to freq[200]
 9. Compute band energy: bass, mid, treble (RMS of bin ranges)
10. Compute spectral features: centroid, spread, rolloff, flux
11. Apply mel filterbank -> mel_bands[24], normalize -> mel_bands_normalized[24]
12. Compute density (spectral flatness) and width (stereo correlation)
13. Every 4th frame (15 Hz): execute 4096-point FFT
14.   -> Compute chromagram[12]
15.   -> Compute dominant_pitch_class, minor_major_ratio, harmonic_hue
16. Run energy onset detectors (bass, mid, treble)
17. Run spectral flux onset detector
18. Update tempo tracker with onset events
19. Update beat phase, confidence, anticipation
20. Update beat_pulse and onset_pulse envelopes
21. Apply smoothing to all remaining fields
22. Write completed AudioData to triple buffer back slot
23. Publish (atomic swap) -- render thread sees new data on next read
```

## Appendix B: Crate Dependencies

| Crate              | Purpose                                                 | License                   |
| ------------------ | ------------------------------------------------------- | ------------------------- |
| `cpal`             | Cross-platform audio I/O (WASAPI, PulseAudio/ALSA)      | Apache-2.0                |
| `realfft`          | In-place real-to-complex FFT                            | Apache-2.0/MIT            |
| `libpulse-binding` | PulseAudio API for Linux monitor source discovery       | MIT OR Apache-2.0         |
| `crossbeam`        | Lock-free SPSC ring buffer (`ArrayQueue`)               | MIT OR Apache-2.0         |
| `triple-buffer`    | Lock-free single-producer single-consumer triple buffer | MIT                       |
| `bytemuck`         | Zero-copy GPU buffer marshaling (`Pod`, `Zeroable`)     | MIT OR Apache-2.0 OR Zlib |

## Appendix C: Spectral Feature Formulas

**Spectral Centroid** (center of mass):

```
centroid = sum(freq_i * magnitude_i) / sum(magnitude_i)
normalized = centroid / nyquist
```

**Spectral Spread** (standard deviation around centroid):

```
spread = sqrt(sum(magnitude_i * (freq_i - centroid)^2) / sum(magnitude_i))
normalized = spread / nyquist
```

**Spectral Rolloff** (frequency below which 85% of energy lies):

```
Find smallest freq_k such that:
  sum(magnitude_i for i <= k) >= 0.85 * sum(magnitude_i for all i)
normalized = freq_k / nyquist
```

**Spectral Flux** (half-wave rectified difference):

```
flux = sum(max(0, magnitude_i[t] - magnitude_i[t-1]) for all i)
normalized = flux / flux_history_max
```

**Density** (spectral flatness):

```
density = geometric_mean(power_spectrum) / arithmetic_mean(power_spectrum)
```
