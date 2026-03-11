# 04 — Audio System

> Making the room pulse. The signal chain from soundwave to photon.

---

## Overview

Audio-reactive lighting transforms sound into color, motion, and rhythm. When the bass drops and every LED in the room detonates in sync -- that is the moment Hypercolor exists to deliver.

This document covers the complete audio pipeline: capturing system audio on Linux, transforming it through DSP into rich frequency/beat/harmonic data, and injecting that data into effects via both the Servo (Lightscript) and wgpu (native shader) paths. The target API surface matches and extends the Lightscript audio model for full effect compatibility.

### Design Goals

| Goal | Target |
|---|---|
| Audio-to-photon latency | < 30ms total (capture + DSP + render + device output) |
| DSP frame rate | 60 Hz (one analysis per render frame) |
| CPU budget (audio thread) | < 5% single core on i7-14700K |
| Capture source | System audio loopback (PipeWire/PulseAudio monitor) |
| API compatibility | Full Lightscript `window.engine.audio` contract |
| Zero-config | Auto-detect default audio output monitor source |

---

## 1. Audio Capture Pipeline

### Signal Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         Linux Audio Stack                               │
│                                                                         │
│  ┌──────────┐    ┌───────────┐    ┌──────────────┐    ┌─────────────┐  │
│  │ App      │───>│ PipeWire  │───>│ Hardware     │───>│ Speakers /  │  │
│  │ (Spotify,│    │ Graph     │    │ Output Sink  │    │ DAC         │  │
│  │  Game)   │    │           │    │              │    │             │  │
│  └──────────┘    └─────┬─────┘    └──────────────┘    └─────────────┘  │
│                        │                                                │
│                        │ Monitor source                                 │
│                        │ (loopback tap)                                 │
│                        ▼                                                │
│               ┌────────────────┐                                        │
│               │ Hypercolor     │                                        │
│               │ Audio Capture  │                                        │
│               │ (cpal stream)  │                                        │
│               └───────┬────────┘                                        │
└───────────────────────┼─────────────────────────────────────────────────┘
                        │
                        │ f32 PCM samples (ring buffer)
                        ▼
┌───────────────────────────────────────────────────────────────────────┐
│                      DSP Pipeline (dedicated thread)                  │
│                                                                       │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐  ┌───────────────────┐   │
│  │ Window   │─>│ FFT      │─>│ Frequency │─>│ Feature           │   │
│  │ Function │  │ (realfft)│  │ Mapping   │  │ Extraction        │   │
│  │ (Hann)   │  │ r2c      │  │ & Binning │  │                   │   │
│  └──────────┘  └──────────┘  └───────────┘  │ - Mel bands (24)  │   │
│                                              │ - Chromagram (12) │   │
│                                              │ - Spectral feat.  │   │
│                                              │ - Beat detection  │   │
│                                              │ - Harmonic anal.  │   │
│                                              └────────┬──────────┘   │
│                                                       │              │
└───────────────────────────────────────────────────────┼──────────────┘
                                                        │
                                        AudioData struct (lock-free)
                                                        │
                        ┌───────────────────────────────┼───────────┐
                        │                               │           │
                        ▼                               ▼           ▼
              ┌──────────────────┐          ┌────────────┐  ┌──────────┐
              │ Servo Renderer   │          │ wgpu       │  │ Event Bus│
              │ inject into      │          │ Uniform    │  │ spectrum │
              │ window.engine    │          │ Buffer     │  │ watch    │
              │ .audio           │          │            │  │ channel  │
              └──────────────────┘          └────────────┘  └──────────┘
```

### Linux Audio Landscape

Hypercolor must navigate three audio subsystems. PipeWire is the clear primary target, with PulseAudio and ALSA as fallbacks.

#### PipeWire (Primary -- Modern Linux)

PipeWire is the convergence point. It replaces both PulseAudio and JACK, speaks both protocols natively, and is the default on Fedora, Ubuntu 22.10+, Arch, CachyOS, and every modern distro that matters.

**System audio capture via monitor source:**

PipeWire exposes every output sink as a corresponding monitor source. When audio plays through `alsa_output.pci-0000_00_1f.3.analog-stereo`, PipeWire creates `alsa_output.pci-0000_00_1f.3.analog-stereo.monitor` -- a loopback tap of everything going to that sink.

```
PipeWire Graph:
  Spotify ──────┐
  Firefox ──────┤──> Output Sink ──> Hardware
  Game ─────────┘        │
                         │ .monitor
                         ▼
                    Hypercolor (cpal input stream)
```

**How cpal connects:**

`cpal` on Linux links against `libpulse` (PulseAudio client library), which PipeWire intercepts via its PulseAudio compatibility layer (`pipewire-pulse`). When Hypercolor requests the default input device, it gets whatever PipeWire exposes. To capture system audio specifically, we need to select the monitor source.

**Monitor source selection strategies:**

1. **Automatic (recommended):** Query PipeWire for the default sink's monitor via `pw-cli` or the PulseAudio API. The monitor source name follows the pattern `{sink_name}.monitor`.

2. **PulseAudio API (`libpulse`):** Use `pa_context_get_source_info_list()` to enumerate sources, filter for `PA_SOURCE_MONITOR` flag. This works identically under PipeWire's PulseAudio layer.

3. **PipeWire native:** Use `pipewire-rs` (Rust bindings) to enumerate nodes, find the output sink, and create a stream connected to its monitor port. More control, but heavier dependency.

4. **Environment variable:** `PULSE_SOURCE=alsa_output.pci-xxx.monitor` forces cpal to use a specific source.

**Recommendation:** Use the PulseAudio API via `libpulse-binding` (Rust crate) for monitor source discovery, then pass the source name to cpal. This works on both PipeWire and legacy PulseAudio without code changes.

#### PulseAudio (Legacy Fallback)

Identical monitor source mechanism. PulseAudio invented the concept. The same `libpulse` API works here natively. Systems still running bare PulseAudio (older Ubuntu, some enterprise distros) get first-class support through the same code path.

**Key command for users:**
```bash
# List available monitor sources
pactl list sources short | grep monitor

# Example output:
# 47  alsa_output.pci-0000_00_1f.3.analog-stereo.monitor  PipeWireNode  s32le 2ch 48000Hz  IDLE
```

#### ALSA (Lowest Level)

ALSA has no native monitor source concept. System audio loopback requires either:

1. **`snd-aloop` kernel module:** Creates a virtual loopback device. Audio routed to the loopback output appears on its input. Requires manual setup and routing.

2. **ALSA dmix + dsnoop:** Complex `.asound.rc` configuration to tap the mix bus.

**Recommendation:** Don't actively support bare ALSA for system audio capture. It's a configuration maze. If someone runs without PulseAudio/PipeWire in 2026, they can configure `snd-aloop` themselves. Hypercolor detects this and shows a clear "install PipeWire for system audio capture" message.

ALSA microphone input (direct capture, not loopback) works fine through cpal with no special handling.

#### JACK (Pro Audio)

PipeWire speaks JACK protocol natively. Users running PipeWire with JACK compatibility (the default on CachyOS/Arch with `pipewire-jack`) get JACK routing for free. Hypercolor doesn't need to handle JACK directly -- it's just another PipeWire client from our perspective.

For legacy JACK-only setups (rare in 2026): cpal supports JACK as a host backend. A `jack` feature flag can enable this.

### Capture Parameters

| Parameter | Default | Range | Notes |
|---|---|---|---|
| Sample rate | 48000 Hz | 44100-96000 | Match system default. 48kHz is PipeWire default |
| Buffer size | 1024 samples | 256-4096 | ~21ms at 48kHz. Good latency/stability balance |
| Channels | 2 (stereo) | 1-2 | Downmix to mono for FFT. Stereo for width analysis |
| Bit depth | f32 | -- | cpal delivers f32 normalized [-1.0, 1.0] |
| Ring buffer | 4096 samples | -- | ~85ms history. Enough for 4096-point FFT |

### Audio Source Configuration

```rust
/// Audio source configuration
pub struct AudioSourceConfig {
    /// Source type: system monitor, microphone, or specific device
    pub source: AudioSource,
    /// Target sample rate (will resample if device differs)
    pub sample_rate: u32,
    /// Input gain multiplier (1.0 = unity)
    pub gain: f32,
    /// Noise gate threshold in dB (-60 to 0)
    pub noise_gate_db: f32,
}

pub enum AudioSource {
    /// Auto-detect system audio output monitor (default)
    SystemMonitor,
    /// Specific PulseAudio/PipeWire source by name
    Named(String),
    /// Microphone input (default input device)
    Microphone,
    /// No audio input (effects get silence)
    None,
}
```

### Multi-Source Mixing

For advanced scenarios (game audio + music, multiple app capture):

**PipeWire approach:** PipeWire's graph model allows creating custom routing. Hypercolor can request multiple capture streams connected to different application outputs. PipeWire handles the mixing internally.

**Practical approach for v1:** Capture the default output monitor (which already mixes all application audio). Users who want per-app filtering can use PipeWire's `pw-link` or `qpwgraph` to route specific apps to a dedicated virtual sink, then point Hypercolor at that sink's monitor.

**Voice chat filtering (Luna's scenario):** Route Discord to a different sink than Spotify. Hypercolor captures only the music sink's monitor. This is a PipeWire routing solution, not a Hypercolor DSP problem.

---

## 2. FFT & Frequency Analysis

### The DSP Pipeline

```
Raw PCM ──> DC Offset ──> Window ──> FFT ──> Magnitude ──> Log Scale ──> Bin ──> Smooth ──> Output
Samples     Removal       Function          Spectrum      (dB)          Mapping   (EMA)     [200]
(f32)       (subtract     (Hann)            (complex      (20*log10)    (200      (decay)
            mean)                            → |z|)                     bins)
```

### Window Function Selection

The window function shapes the frequency resolution vs. spectral leakage tradeoff.

| Window | Main Lobe Width | Side Lobe Level | Best For |
|---|---|---|---|
| **Hann** | 4 bins | -31 dB | General purpose. Good balance. Our default. |
| Hamming | 4 bins | -42 dB | Slightly better side lobe rejection |
| Blackman-Harris | 8 bins | -92 dB | Maximum leakage rejection. Wider main lobe. |
| Flat-top | 10 bins | -44 dB | Amplitude accuracy. Not useful for lighting. |

**Choice: Hann window.** Industry standard for audio visualization. The slight spectral leakage is invisible in lighting effects -- we're painting with broad strokes, not doing laboratory measurement. Hann is also cheap to compute:

```rust
fn hann_window(n: usize, total: usize) -> f32 {
    0.5 * (1.0 - (2.0 * std::f32::consts::PI * n as f32 / total as f32).cos())
}
```

### FFT Size Tradeoffs

The FFT size determines frequency resolution and latency. At 48 kHz sample rate:

| FFT Size | Frequency Resolution | Time Window | Latency | Best For |
|---|---|---|---|---|
| 512 | 93.75 Hz | 10.7 ms | Lowest | Beat detection, transient response |
| 1024 | 46.88 Hz | 21.3 ms | Low | Balanced (our primary) |
| 2048 | 23.44 Hz | 42.7 ms | Medium | Better bass resolution |
| 4096 | 11.72 Hz | 85.3 ms | High | Precise pitch detection |

**Primary FFT: 1024 samples at 48 kHz.**

This gives us 512 unique frequency bins spanning 0-24000 Hz, with 46.88 Hz resolution. The 21.3ms time window introduces minimal latency while resolving bass frequencies adequately (lowest resolvable: ~47 Hz, which is between A1 and B1).

**Secondary FFT: 4096 samples for harmonic analysis.**

Run at a lower rate (15 Hz instead of 60 Hz) for chromagram and pitch detection, where frequency resolution matters more than temporal resolution. The 11.72 Hz resolution cleanly separates adjacent semitones above ~200 Hz.

### DC Offset Removal

Subtract the mean of the windowed buffer before FFT. Without this, a DC offset creates a large bin-0 value that bleeds energy into adjacent bins and makes the entire spectrum appear louder than it is.

```rust
fn remove_dc_offset(samples: &mut [f32]) {
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    for s in samples.iter_mut() {
        *s -= mean;
    }
}
```

### Magnitude and Logarithmic Scaling

Convert complex FFT output to perceptual loudness:

```rust
fn complex_to_db(re: f32, im: f32) -> f32 {
    let magnitude = (re * re + im * im).sqrt();
    let db = 20.0 * (magnitude + 1e-10).log10(); // Avoid log(0)
    db.max(-100.0) // Floor at -100 dB
}
```

Normalize to 0.0-1.0 range for effects:

```rust
fn db_to_normalized(db: f32, floor: f32, ceiling: f32) -> f32 {
    ((db - floor) / (ceiling - floor)).clamp(0.0, 1.0)
}

// floor = -80 dB (silence), ceiling = 0 dB (max)
// Matches the LightScript internal scaling
```

### Frequency Bin Mapping: The 200-Bin Output

The LightScript API provides effects with `engine.audio.freq[200]` -- 200 frequency bins. We must match this format exactly for compatibility.

**Linear-to-logarithmic remapping:**

The raw FFT produces linearly spaced bins (each 46.88 Hz wide with 1024-point FFT at 48 kHz). Human hearing is logarithmic -- the difference between 100 Hz and 200 Hz is perceptually the same as 1000 Hz to 2000 Hz. The 200 output bins use logarithmic spacing.

```rust
/// Map 512 linear FFT bins → 200 logarithmically spaced output bins
fn map_to_200_bins(fft_magnitudes: &[f32; 512], sample_rate: u32) -> [f32; 200] {
    let mut output = [0.0f32; 200];
    let fft_size = 1024;
    let nyquist = sample_rate as f32 / 2.0;

    // Logarithmic spacing from ~20 Hz to ~20000 Hz
    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();

    for i in 0..200 {
        let t0 = i as f32 / 200.0;
        let t1 = (i + 1) as f32 / 200.0;
        let freq_lo = (log_min + t0 * (log_max - log_min)).exp();
        let freq_hi = (log_min + t1 * (log_max - log_min)).exp();

        // Map frequency range to FFT bin indices
        let bin_lo = (freq_lo / nyquist * fft_magnitudes.len() as f32) as usize;
        let bin_hi = ((freq_hi / nyquist * fft_magnitudes.len() as f32) as usize)
            .max(bin_lo + 1)
            .min(fft_magnitudes.len());

        // Average the FFT bins that fall within this output bin
        let sum: f32 = fft_magnitudes[bin_lo..bin_hi].iter().sum();
        output[i] = sum / (bin_hi - bin_lo) as f32;
    }

    output
}
```

**Bin distribution across the spectrum:**

| Output Bins | Frequency Range | Musical Range | Notes |
|---|---|---|---|
| 0-19 | 20-80 Hz | Sub-bass | Kick drums, 808 bass |
| 20-49 | 80-300 Hz | Bass | Bass guitar, low synths |
| 50-89 | 300-1200 Hz | Low-mid | Vocals, guitar body |
| 90-129 | 1.2-4 kHz | Mid | Vocal presence, snare attack |
| 130-169 | 4-12 kHz | High-mid | Cymbal body, synth brightness |
| 170-199 | 12-20 kHz | Treble | Air, sibilance, hi-hat shimmer |

### Smoothing and Decay

Raw FFT output is noisy frame-to-frame. Effects need smooth, musically meaningful values.

**Exponential Moving Average (EMA):**

```rust
fn smooth(current: f32, previous: f32, factor: f32) -> f32 {
    previous + factor * (current - previous)
}

// Attack (rising): factor = 0.6 (fast response to new energy)
// Decay (falling): factor = 0.15 (slower fade-out, natural feel)
```

**Asymmetric smoothing is critical.** Lights should snap ON with the beat but fade smoothly. Using the same factor for both directions makes the response feel sluggish.

```rust
fn asymmetric_smooth(current: f32, previous: f32, attack: f32, decay: f32) -> f32 {
    let factor = if current > previous { attack } else { decay };
    previous + factor * (current - previous)
}
```

**Peak hold (optional):**

For spectrum visualizer effects, hold peaks for a configurable number of frames before they decay. Classic "falling dots" visualization.

```rust
struct PeakHold {
    value: f32,
    hold_frames: u32,
    frames_held: u32,
}

impl PeakHold {
    fn update(&mut self, new_value: f32) -> f32 {
        if new_value >= self.value {
            self.value = new_value;
            self.frames_held = 0;
        } else if self.frames_held < self.hold_frames {
            self.frames_held += 1;
        } else {
            self.value *= 0.95; // Decay
        }
        self.value
    }
}
```

### Band Energy Extraction

Three summary bands matching Lightscript's `bass`, `mid`, `treble`:

| Band | Frequency Range | Output Bins | Use |
|---|---|---|---|
| **Bass** | 20-250 Hz | 0-39 | Kick detection, room pulse |
| **Mid** | 250-4000 Hz | 40-129 | Vocal/melody tracking |
| **Treble** | 4000-20000 Hz | 130-199 | Sparkle, shimmer, hi-hat |

```rust
fn band_energy(bins: &[f32; 200], lo: usize, hi: usize) -> f32 {
    let slice = &bins[lo..=hi];
    let rms = (slice.iter().map(|x| x * x).sum::<f32>() / slice.len() as f32).sqrt();
    rms
}

// level = overall RMS of all 200 bins
// bass = band_energy(bins, 0, 39)
// mid = band_energy(bins, 40, 129)
// treble = band_energy(bins, 130, 199)
```

---

## 3. Beat Detection

Beat detection is the most perceptually critical feature. When the beat drops and the lights are 50ms late, it feels wrong. When they're 20ms *early*, it feels magical -- the room *anticipates* the music.

### Multi-Algorithm Approach

We run three complementary beat detection methods and fuse their outputs:

```
                    ┌──────────────────┐
                    │   Band Energy    │──> Bass onset
                    │   Onset          │──> Snare onset
                    │   Detection      │──> Hi-hat onset
                    └────────┬─────────┘
                             │
┌──────────────┐             │         ┌──────────────────┐
│ Spectral     │─────────────┼────────>│  Beat Fusion     │──> beat (bool)
│ Flux         │             │         │  Engine           │──> beatPulse (0-1)
│ Detection    │─────────────┘    ┌───>│                   │──> beatPhase (0-1)
└──────────────┘                  │    │                   │──> beatConfidence
                                  │    │                   │──> beatAnticipation
┌──────────────┐                  │    │                   │──> tempo (BPM)
│ Tempo        │──────────────────┘    └──────────────────┘
│ Tracker      │
│ (auto-corr.) │
└──────────────┘
```

### Method 1: Energy-Based Onset Detection

The simplest and fastest method. Compare instantaneous band energy against a running average. When instantaneous energy exceeds the average by a threshold, declare an onset.

```rust
pub struct EnergyOnsetDetector {
    /// Short-term energy (fast EMA, ~50ms window)
    energy_short: f32,
    /// Long-term energy (slow EMA, ~1-2s window)
    energy_long: f32,
    /// Cooldown timer to prevent double-triggers
    cooldown: f32,
    /// Minimum time between onsets (seconds)
    min_interval: f32,
    /// Sensitivity threshold multiplier
    threshold: f32,
}

impl EnergyOnsetDetector {
    pub fn update(&mut self, energy: f32, dt: f32) -> bool {
        self.energy_short = lerp(self.energy_short, energy, 0.4);
        self.energy_long = lerp(self.energy_long, energy, 0.02);

        self.cooldown -= dt;

        // Dynamic threshold: onset when short-term exceeds long-term by threshold ratio
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

**Multi-band onset detection:**

Run separate onset detectors for bass (20-250 Hz), mid (250-4 kHz), and treble (4-20 kHz). This distinguishes kick drums from snare hits from hi-hats -- different physical impacts deserve different lighting responses.

| Band | Onset Character | Typical Lighting Response |
|---|---|---|
| **Bass onset** | Kick drum, bass drop | Full-room pulse, brightness spike |
| **Mid onset** | Snare, vocal attack | Flash, pattern shift |
| **Treble onset** | Hi-hat, cymbal | Sparkle, particle burst |

### Method 2: Spectral Flux

Spectral flux measures how much the frequency spectrum *changed* between frames. It catches onsets that energy detection misses (e.g., a snare hit during a sustained bass note, where total energy barely changes but the spectrum shifts dramatically).

```rust
pub struct SpectralFluxDetector {
    prev_spectrum: [f32; 200],
    flux_history: VecDeque<f32>, // ~1 second of history
    threshold_multiplier: f32,
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

        // Keep ~1 second of history at 60fps
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

**Banded spectral flux** (`spectralFluxBands[3]` in the Lightscript API):

Compute spectral flux separately for bass/mid/treble ranges. This feeds the multi-band beat detection and gives effects per-band reactivity.

### Method 3: BPM Estimation & Phase Tracking

Tempo tracking enables *predictive* beat timing -- the "beatAnticipation" and "beatPhase" fields that make lighting feel like it *knows* the music.

**Auto-correlation BPM estimation:**

```rust
pub struct TempoTracker {
    /// Onset history (timestamps in seconds)
    onset_times: VecDeque<f64>,
    /// Current estimated BPM
    bpm: f32,
    /// Phase within current beat (0.0 = beat, 1.0 = just before next beat)
    phase: f32,
    /// Confidence in current BPM estimate (0-1)
    confidence: f32,
    /// Time of last detected beat
    last_beat_time: f64,
    /// Expected beat interval (seconds)
    beat_interval: f64,
}

impl TempoTracker {
    pub fn update(&mut self, onset: bool, now: f64) {
        if onset {
            self.onset_times.push_back(now);
            // Keep 10 seconds of onset history
            while self.onset_times.len() > 600 {
                self.onset_times.pop_front();
            }
            self.last_beat_time = now;
            self.estimate_bpm();
        }

        // Update phase (continuous 0-1 ramp between beats)
        if self.beat_interval > 0.0 {
            let elapsed = now - self.last_beat_time;
            self.phase = (elapsed / self.beat_interval).fract() as f32;
        }
    }

    fn estimate_bpm(&mut self) {
        if self.onset_times.len() < 4 { return; }

        // Compute inter-onset intervals
        let intervals: Vec<f64> = self.onset_times.iter()
            .zip(self.onset_times.iter().skip(1))
            .map(|(a, b)| b - a)
            .filter(|i| *i > 0.15 && *i < 2.0) // 30-400 BPM range
            .collect();

        if intervals.is_empty() { return; }

        // Cluster intervals to find dominant tempo
        // Simple approach: histogram with 5ms bins
        let mut histogram = vec![0u32; 400]; // 0-2000ms in 5ms bins
        for interval in &intervals {
            let bin = ((interval * 200.0) as usize).min(399);
            histogram[bin] += 1;
            // Also count half and double tempo (harmonic relationships)
            let half_bin = ((interval * 100.0) as usize).min(399);
            let double_bin = ((interval * 400.0) as usize).min(399);
            histogram[half_bin] += 1;
            histogram[double_bin] += 1;
        }

        // Find peak
        let peak_bin = histogram.iter().enumerate()
            .max_by_key(|(_, count)| *count)
            .map(|(bin, _)| bin)
            .unwrap_or(0);

        let peak_interval = peak_bin as f64 / 200.0;
        if peak_interval > 0.0 {
            self.beat_interval = peak_interval;
            self.bpm = (60.0 / peak_interval) as f32;
            self.confidence = histogram[peak_bin] as f32
                / intervals.len() as f32;
        }
    }
}
```

**BPM range clamping:**

Most music falls between 60-180 BPM. Detected tempos outside this range are likely harmonics (half-time or double-time). Adjust accordingly:

```rust
fn normalize_bpm(raw_bpm: f32) -> f32 {
    let mut bpm = raw_bpm;
    while bpm < 60.0 { bpm *= 2.0; }
    while bpm > 180.0 { bpm /= 2.0; }
    bpm
}
```

### Beat Anticipation

The `beatAnticipation` field from Lightscript is what separates good audio-reactive lighting from great. It fires *before* the beat arrives, so the light reaches peak intensity at the exact moment the transient hits the ear.

**How it works:**

Once we have a stable BPM estimate, we know when the next beat *should* arrive. Fire the anticipation signal 15-25ms before that predicted moment. This compensates for:

1. Effect rendering latency (~2-5ms)
2. Device output latency (USB HID: ~5-10ms, network: ~3-8ms)
3. Human perceptual fusion window (~20ms)

```rust
pub struct BeatAnticipator {
    /// How far ahead to fire (seconds)
    anticipation_ms: f32,
    /// Current anticipation signal (0-1, peaks before beat)
    pub anticipation: f32,
}

impl BeatAnticipator {
    pub fn update(&mut self, phase: f32, beat_interval: f64, confidence: f32) {
        if confidence < 0.3 {
            self.anticipation = 0.0;
            return;
        }

        // Phase goes 0.0 → 1.0 between beats
        // Anticipation ramps up as we approach the predicted next beat
        let anticipation_phase = self.anticipation_ms / 1000.0 / beat_interval as f32;
        let time_to_beat = 1.0 - phase;

        if time_to_beat < anticipation_phase {
            // Ramp from 0 to 1 over the anticipation window
            self.anticipation = 1.0 - (time_to_beat / anticipation_phase);
        } else {
            self.anticipation = 0.0;
        }
    }
}
```

### Beat Pulse Envelope

The `beatPulse` output is a smoothed envelope that rises sharply on beat and decays over ~200ms. Effects use this for smooth pulsing rather than harsh on/off flashing.

```rust
pub struct BeatPulse {
    pub value: f32,
    decay_rate: f32, // Per-second decay (default: 5.0 → ~200ms to zero)
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

### Handling Tempo Changes and Breakdowns

Music isn't a metronome. The system must handle:

| Scenario | Strategy |
|---|---|
| **Tempo change** | BPM estimator adapts over 2-4 seconds as new intervals accumulate |
| **Breakdown (quiet section)** | Onset energy drops below threshold. Phase tracking continues based on last known BPM. Confidence decays. |
| **Drop (breakdown → full energy)** | First strong onset after breakdown resets phase. Immediate lock. |
| **Stop-start** | After 3 seconds of no onsets, reset BPM tracking. Fresh start. |
| **Rubato / live music** | Lower confidence, reduce anticipation. Fall back to pure onset detection. |

---

## 4. Advanced Audio Features

### Mel-Scale Bands (24 Bands)

The mel scale approximates human loudness perception. 24 mel bands provide a perceptually uniform representation of the spectrum -- equal-sized perceptual steps from low to high frequency.

**Mel frequency formula:**

```
mel(f) = 2595 * log10(1 + f/700)
f(mel) = 700 * (10^(mel/2595) - 1)
```

**24-band mel filterbank:**

| Band | Center Freq | Bandwidth | Musical Region |
|---|---|---|---|
| 0 | 47 Hz | 30 Hz | Sub-bass fundamental |
| 1 | 73 Hz | 35 Hz | Bass drum fundamental |
| 2 | 104 Hz | 42 Hz | Bass guitar low range |
| 3 | 141 Hz | 50 Hz | Bass guitar high range |
| 4 | 187 Hz | 60 Hz | Low vocals |
| 5 | 243 Hz | 72 Hz | Vocal fundamental |
| 6 | 313 Hz | 86 Hz | Guitar body |
| 7 | 399 Hz | 103 Hz | Snare body |
| 8 | 507 Hz | 123 Hz | Low-mid |
| 9 | 641 Hz | 148 Hz | Mid |
| 10 | 808 Hz | 177 Hz | Vocal presence |
| 11 | 1.02 kHz | 213 Hz | Upper mid |
| 12 | 1.28 kHz | 256 Hz | Guitar bite |
| 13 | 1.61 kHz | 307 Hz | Vocal clarity |
| 14 | 2.03 kHz | 369 Hz | Snare attack |
| 15 | 2.55 kHz | 443 Hz | Presence |
| 16 | 3.21 kHz | 532 Hz | High presence |
| 17 | 4.04 kHz | 639 Hz | Cymbal body |
| 18 | 5.09 kHz | 767 Hz | Brilliance |
| 19 | 6.41 kHz | 921 Hz | High brilliance |
| 20 | 8.08 kHz | 1.11 kHz | Air |
| 21 | 10.2 kHz | 1.33 kHz | Upper air |
| 22 | 12.8 kHz | 1.59 kHz | Shimmer |
| 23 | 16.1 kHz | 1.92 kHz | Ultra-high presence |

```rust
pub struct MelFilterbank {
    /// 24 triangular filters mapping FFT bins to mel bands
    filters: Vec<Vec<(usize, f32)>>, // (fft_bin_index, weight)
}

impl MelFilterbank {
    pub fn new(fft_size: usize, sample_rate: u32, n_mels: usize) -> Self {
        let nyquist = sample_rate as f32 / 2.0;
        let mel_min = Self::hz_to_mel(20.0);
        let mel_max = Self::hz_to_mel(nyquist.min(20000.0));

        // n_mels + 2 points for triangular filter edges
        let mel_points: Vec<f32> = (0..=n_mels + 1)
            .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32)
            .collect();

        let freq_points: Vec<f32> = mel_points.iter()
            .map(|&m| Self::mel_to_hz(m))
            .collect();

        let bin_points: Vec<usize> = freq_points.iter()
            .map(|&f| (f / nyquist * (fft_size / 2) as f32) as usize)
            .collect();

        let mut filters = Vec::with_capacity(n_mels);
        for i in 0..n_mels {
            let mut filter = Vec::new();
            let (lo, center, hi) = (bin_points[i], bin_points[i + 1], bin_points[i + 2]);

            // Rising slope
            for b in lo..center {
                let weight = (b - lo) as f32 / (center - lo).max(1) as f32;
                filter.push((b, weight));
            }
            // Falling slope
            for b in center..=hi {
                let weight = (hi - b) as f32 / (hi - center).max(1) as f32;
                filter.push((b, weight));
            }

            filters.push(filter);
        }

        Self { filters }
    }

    pub fn apply(&self, fft_magnitudes: &[f32]) -> Vec<f32> {
        self.filters.iter().map(|filter| {
            filter.iter()
                .map(|&(bin, weight)| fft_magnitudes.get(bin).unwrap_or(&0.0) * weight)
                .sum::<f32>()
        }).collect()
    }

    fn hz_to_mel(hz: f32) -> f32 { 2595.0 * (1.0 + hz / 700.0).log10() }
    fn mel_to_hz(mel: f32) -> f32 { 700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0) }
}
```

`melBandsNormalized[24]` divides each band by its running maximum for auto-scaled 0-1 range.

### Chromagram (12 Bins)

A chromagram maps all frequencies to their pitch class (C, C#, D, ..., B), collapsing octaves. This is the foundation for chord detection and harmonic color mapping.

**Pitch class frequencies (A4 = 440 Hz reference):**

| Bin | Note | Frequency (Hz) | Octaves Present in 20-20kHz |
|---|---|---|---|
| 0 | C | 32.7, 65.4, 130.8, 261.6, 523.3, 1047, 2093, 4186, 8372, 16744 | 10 |
| 1 | C# | 34.6, 69.3, 138.6, 277.2, 554.4, 1109, 2217, 4435, 8870, 17740 | 10 |
| 2 | D | 36.7, 73.4, 146.8, 293.7, 587.3, 1175, 2349, 4699, 9397 | 9 |
| 3 | D# | 38.9, 77.8, 155.6, 311.1, 622.3, 1245, 2489, 4978, 9956 | 9 |
| 4 | E | 41.2, 82.4, 164.8, 329.6, 659.3, 1319, 2637, 5274, 10548 | 9 |
| 5 | F | 43.7, 87.3, 174.6, 349.2, 698.5, 1397, 2794, 5588, 11175 | 9 |
| 6 | F# | 46.2, 92.5, 185.0, 370.0, 740.0, 1480, 2960, 5920, 11840 | 9 |
| 7 | G | 49.0, 98.0, 196.0, 392.0, 784.0, 1568, 3136, 6272, 12544 | 9 |
| 8 | G# | 51.9, 103.8, 207.7, 415.3, 830.6, 1661, 3322, 6645, 13290 | 9 |
| 9 | A | 55.0, 110.0, 220.0, 440.0, 880.0, 1760, 3520, 7040, 14080 | 9 |
| 10 | A# | 58.3, 116.5, 233.1, 466.2, 932.3, 1865, 3729, 7459, 14917 | 9 |
| 11 | B | 61.7, 123.5, 246.9, 493.9, 987.8, 1976, 3951, 7902, 15804 | 9 |

**Computation:** Use the 4096-point FFT for better frequency resolution. For each FFT bin, determine its closest pitch class and accumulate energy:

```rust
pub fn compute_chromagram(fft_magnitudes: &[f32], sample_rate: u32, fft_size: usize) -> [f32; 12] {
    let mut chroma = [0.0f32; 12];
    let freq_resolution = sample_rate as f32 / fft_size as f32;

    for (bin, &magnitude) in fft_magnitudes.iter().enumerate().skip(1) {
        let freq = bin as f32 * freq_resolution;
        if freq < 20.0 || freq > 10000.0 { continue; } // Useful range for pitch

        // Convert frequency to pitch class (0-11)
        // semitones from A4: 12 * log2(f / 440)
        let semitones = 12.0 * (freq / 440.0).log2();
        let pitch_class = ((semitones.round() as i32 % 12 + 12) % 12) as usize;

        // Map: A=9, so shift to C=0
        let mapped = (pitch_class + 3) % 12;
        chroma[mapped] += magnitude * magnitude; // Energy (squared magnitude)
    }

    // Normalize
    let max = chroma.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        for c in &mut chroma { *c /= max; }
    }

    chroma
}
```

### Spectral Features

Four features that describe the "shape" of the spectrum:

**Spectral Centroid** -- The "center of mass" of the spectrum. High centroid = bright/tinny sound. Low centroid = dark/bassy. Measured in Hz, normalized to 0-1.

```rust
pub fn spectral_centroid(magnitudes: &[f32], sample_rate: u32, fft_size: usize) -> f32 {
    let freq_resolution = sample_rate as f32 / fft_size as f32;
    let mut weighted_sum = 0.0f32;
    let mut total_magnitude = 0.0f32;

    for (bin, &mag) in magnitudes.iter().enumerate() {
        let freq = bin as f32 * freq_resolution;
        weighted_sum += freq * mag;
        total_magnitude += mag;
    }

    if total_magnitude > 0.0 { weighted_sum / total_magnitude } else { 0.0 }
}
```

**Spectral Spread** -- Bandwidth of the spectrum around the centroid. Wide spread = rich/complex sound. Narrow = pure tone.

**Spectral Rolloff** -- Frequency below which 85% of the spectral energy is concentrated. A rougher measure of brightness than centroid, but more robust to noise.

**Spectral Flux** -- Already computed for beat detection. Also exposed directly to effects for per-frame "change energy."

### Harmonic Analysis

**Dominant Pitch:** The chromagram bin with highest energy gives the dominant pitch class. Confidence is the ratio of the dominant bin to the sum of all bins.

```rust
pub fn dominant_pitch(chroma: &[f32; 12]) -> (usize, f32) {
    let (idx, &max_val) = chroma.iter().enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .unwrap();
    let sum: f32 = chroma.iter().sum();
    let confidence = if sum > 0.0 { max_val / sum } else { 0.0 };
    (idx, confidence) // (pitch_class 0-11, confidence 0-1)
}
```

**Chord Mood (Major/Minor):**

Simple heuristic: compare major third energy to minor third energy relative to the dominant pitch. The `chordMood` value ranges from -1.0 (strongly minor) to +1.0 (strongly major).

```rust
pub fn chord_mood(chroma: &[f32; 12], root: usize) -> f32 {
    let major_third = chroma[(root + 4) % 12]; // 4 semitones up
    let minor_third = chroma[(root + 3) % 12]; // 3 semitones up
    let fifth = chroma[(root + 7) % 12];       // 7 semitones up

    // Weight by fifth presence (confirms chord, not just interval)
    let fifth_weight = 0.5 + 0.5 * fifth;

    if major_third + minor_third < 0.01 { return 0.0; }

    ((major_third - minor_third) / (major_third + minor_third) * fifth_weight)
        .clamp(-1.0, 1.0)
}
```

### Harmonic Color Mapping

The `harmonicHue` field maps the dominant pitch to a color hue, creating synesthesia-inspired pitch-to-color correspondence. This is the "Synesthesia" color scheme in Lightscript effects.

**Pitch-to-hue mapping (circle of fifths → color wheel):**

| Pitch | Hue (degrees) | Color | Musical Character |
|---|---|---|---|
| C | 0 | Red | Root, grounding |
| G | 30 | Orange | Brightness, warmth |
| D | 60 | Yellow | Clarity |
| A | 90 | Yellow-green | Openness |
| E | 120 | Green | Natural, calm |
| B | 150 | Cyan-green | Ethereal |
| F# | 180 | Cyan | Tension |
| C# | 210 | Blue-cyan | Mystery |
| G# | 240 | Blue | Depth |
| D# | 270 | Purple | Richness |
| A# | 300 | Magenta | Intensity |
| F | 330 | Pink-red | Warmth, return |

```rust
const PITCH_TO_HUE: [f32; 12] = [
    0.0,   // C  → Red
    210.0, // C# → Blue-cyan
    60.0,  // D  → Yellow
    270.0, // D# → Purple
    120.0, // E  → Green
    330.0, // F  → Pink-red
    180.0, // F# → Cyan
    30.0,  // G  → Orange
    240.0, // G# → Blue
    90.0,  // A  → Yellow-green
    300.0, // A# → Magenta
    150.0, // B  → Cyan-green
];

pub fn harmonic_hue(chroma: &[f32; 12]) -> f32 {
    // Weighted average of all pitch hues by their chromagram energy
    let mut sin_sum = 0.0f32;
    let mut cos_sum = 0.0f32;
    let total: f32 = chroma.iter().sum::<f32>().max(0.001);

    for (i, &energy) in chroma.iter().enumerate() {
        let hue_rad = PITCH_TO_HUE[i].to_radians();
        let weight = energy / total;
        sin_sum += weight * hue_rad.sin();
        cos_sum += weight * hue_rad.cos();
    }

    let mut hue = sin_sum.atan2(cos_sum).to_degrees();
    if hue < 0.0 { hue += 360.0; }
    hue / 360.0 // Normalize to 0-1
}
```

---

## 5. Audio-to-Visual Mapping Patterns

These are the standard mappings that effect authors should reach for. Each maps an audio feature to a visual dimension.

### Canonical Mapping Table

| Audio Feature | Visual Parameter | Feel | Example |
|---|---|---|---|
| `bass` | Brightness / intensity | Power, impact | Room pulse on kick drum |
| `mid` | Color shift / saturation | Melody tracking | Palette warm-cool shift |
| `treble` | Speed / sparkle rate | Energy, air | Particle emission rate |
| `beat` | Flash / pulse | Rhythm | Strobe on beat |
| `beatPulse` | Smooth intensity envelope | Groove | Breathing glow |
| `beatPhase` | Animation phase | Flow | Wave position synced to tempo |
| `level` | Zone size / spread | Volume | Expanding rings |
| `spectralCentroid` | Color temperature | Brightness | High centroid → cool colors, low → warm |
| `spectralFlux` | Pattern change rate | Surprise | Trigger new pattern on flux spike |
| `chromagram` | Palette selection | Harmonic color | Pitch classes drive hue palette |
| `harmonicHue` | Direct hue mapping | Synesthesia | Music literally becomes color |
| `chordMood` | Warm/cool bias | Emotion | Major chords → warm glow, minor → cool |
| `melBands[n]` | Per-segment brightness | Detailed spectrum | 24-zone LED strip visualization |
| `beatAnticipation` | Pre-flash ramp | Precognition | Lights begin rising before the beat |
| `momentum` | Base animation speed | Energy level | Faster patterns during intense sections |

### Preset Mapping Recipes

**"Pulse" -- The universal audio-reactive default:**
```
brightness = 0.3 + 0.7 * beatPulse * bass
speed = 1.0 + 2.0 * treble
hue_shift = harmonicHue * 360
saturation = 0.5 + 0.5 * mid
```

**"Spectrum Bars" -- Classic visualizer:**
```
bar_height[i] = melBands[i] (for 24 zones)
bar_color[i] = hsl(i * 15, 100%, 50% + 30% * melBands[i])
```

**"Synesthesia" -- Music-to-color:**
```
hue = harmonicHue * 360
saturation = 0.6 + 0.4 * dominantPitchConfidence
brightness = 0.2 + 0.6 * level + 0.2 * beatPulse
warmth_bias = chordMood * 20  // shift hue +-20 degrees
```

**"Ambient" -- Soft, low-energy:**
```
brightness = 0.05 + 0.15 * level
color_drift = spectralCentroid * 0.01  // very slow
pattern_scale = 1.0 + 0.3 * bass
smoothing = 0.95  // heavy temporal filtering
```

**"Rave" -- Maximum reactivity:**
```
flash_trigger = beat AND bass > 0.7
strobe_rate = tempo / 60  // lock to BPM
hue = (time * colorSpeed + harmonicHue * 360) % 360
intensity = beatAnticipation * 0.3 + beatPulse * 0.7
```

---

## 6. Latency & Sync

### Latency Budget

The total audio-to-photon latency determines whether lights feel synchronized with music. Human perception merges audio and visual events within ~30ms. Beyond 50ms, the disconnect becomes noticeable.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         Audio-to-Photon Latency Budget                      │
│                                                                              │
│  Audio              DSP                Render            Device              │
│  Capture            Processing         Frame             Output              │
│                                                                              │
│  ├────────┤  ├──────────────┤  ├───────────────┤  ├──────────────────┤      │
│  0       5ms 5            8ms  8            13ms  13              28ms      │
│                                                                              │
│  PipeWire           FFT (1024)          wgpu/Servo          USB HID:  5-10ms│
│  monitor            + features          render +            WLED DDP: 3-5ms │
│  tap                + beat det.         pixel sampling      Hue:      30ms+ │
│                                                                              │
│  Target total: < 30ms (USB)    < 20ms (WLED)    < 60ms (Hue)               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Per-Stage Breakdown

| Stage | Latency | Deterministic? | Notes |
|---|---|---|---|
| **PipeWire capture** | 2-5 ms | Near-deterministic | Default quantum is 1024 samples / 48kHz = 21ms buffer, but PipeWire delivers partial buffers. Monitor source tapping adds ~1ms. |
| **Ring buffer fill** | 0-21 ms | Variable | Worst case: waiting for 1024 samples. Best case: buffer already full from overlap. Average: ~10ms. Mitigated by hop size (see below). |
| **FFT computation** | < 0.5 ms | Deterministic | 1024-point FFT is ~20 microseconds on modern hardware. Negligible. |
| **Feature extraction** | < 1 ms | Deterministic | Mel bands, chromagram, spectral features, beat detection. All O(N) on FFT output. |
| **Effect render** | 1-5 ms | Variable | wgpu shaders: < 1ms. Servo Canvas 2D: ~3-5ms. WebGL: ~2-4ms. |
| **Pixel sampling** | < 0.5 ms | Deterministic | 320x200 canvas, bilinear sampling at LED positions. Trivial. |
| **USB HID write** | 5-10 ms | Semi-deterministic | PrismRGB: 48 packets + commit at ~125 microseconds each. Worst case with USB scheduling jitter. |
| **WLED DDP** | 3-5 ms | Semi-deterministic | Single UDP packet for < 480 LEDs. Network latency on LAN. |
| **Hue Entertainment** | 20-40 ms | Variable | DTLS encrypted UDP. Bridge processing. Unavoidable. |

### Hop Size and Overlap

Instead of waiting for a full 1024 new samples (21ms), use a hop size of 256 samples (5.3ms). The FFT window overlaps 75%, reusing 768 samples from the previous frame. This gives us fresh frequency data every 5.3ms while maintaining the frequency resolution of a 1024-point FFT.

```
Time ───────────────────────────────────────────────────────>

Frame 1:  |████████████████████████████████████████████████|  1024 samples
Frame 2:       |███████████████████████████████████████████████|  256 new + 768 old
Frame 3:            |██████████████████████████████████████████████|
Frame 4:                 |█████████████████████████████████████████████|

             ↕ 5.3ms hop ↕
```

### Compensation Strategies

**For USB HID devices (PrismRGB, keyboard):**

5-10ms device latency. Combined with ~8-13ms capture+DSP+render, total is ~15-23ms. Within the 30ms perceptual window. No compensation needed for beat detection -- the `beatAnticipation` signal provides natural pre-flash.

**For WLED (DDP over LAN):**

3-5ms network latency. Total ~11-18ms. Excellent. WLED is the best-case scenario.

**For Philips Hue:**

30-60ms total latency. This pushes past the perceptual threshold. Compensation strategies:

1. Increase `beatAnticipation` lead time to 40ms for Hue devices specifically
2. Use phase-aligned rendering: advance the beat phase clock by the known Hue latency
3. Accept that Hue lighting will never feel as tight as direct USB -- it's ambient, not rhythmic

**Per-device latency configuration:**

```rust
pub struct DeviceLatencyProfile {
    /// Known output latency for this device/transport (ms)
    pub output_latency_ms: f32,
    /// Additional anticipation to compensate (ms)
    pub anticipation_offset_ms: f32,
}

// Defaults:
// USB HID:  output_latency = 8,  anticipation_offset = 0
// WLED DDP: output_latency = 4,  anticipation_offset = 0
// Hue:      output_latency = 35, anticipation_offset = 25
```

---

## 7. Audio Configuration UX

### Zero-Config Path (Default)

On first launch with no audio configuration:

1. Enumerate PulseAudio/PipeWire sources via `libpulse`
2. Find the default output sink
3. Select its `.monitor` source
4. Start capturing immediately
5. Display "Listening to: [sink name]" in the UI

If no monitor source is found, display a clear diagnostic:
```
Audio: No system audio source detected
  - PipeWire/PulseAudio not running, or
  - No audio output device configured

  Try: pactl list sources short | grep monitor
```

### Audio Settings Panel

```
┌─────────────────────────────────────────────────────────────┐
│  Audio Settings                                             │
│                                                             │
│  Source: [▼ System Audio (Starship/Matisse HD Audio) ────]  │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ ▁▂▃▄▅▆▇█▇▆▅▄▃▂▁▂▃▅▇█▇▅▃▁  ▁▃▅▆▅▃▁  ▁▂▃▂▁          │  │
│  │            Live Spectrum Preview                       │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  Gain:         [────────●──────] 1.2x                       │
│  Sensitivity:  [──────●────────] 60%                        │
│  Noise Floor:  [──●────────────] -60 dB  [Auto Calibrate]   │
│                                                             │
│  Beat Detection                                             │
│  ├ Threshold:  [────────●──────] 1.5x                       │
│  ├ Cooldown:   [──────●────────] 150ms                      │
│  └ Anticipation: [────●────────] 20ms                       │
│                                                             │
│  Advanced                                                   │
│  ├ FFT Size:   [▼ 1024 ──]   Buffer: [▼ Auto ──]           │
│  ├ Smoothing:  Attack [●─] 0.6   Decay [──●] 0.15          │
│  └ Band Split: Bass [250] Hz   Treble [4000] Hz            │
│                                                             │
│  Status: Capturing | 48000 Hz | Latency: ~8ms | BPM: 128   │
└─────────────────────────────────────────────────────────────┘
```

### Source Selection

Available sources populated from PulseAudio/PipeWire enumeration:

| Source | Description |
|---|---|
| System Audio (monitor) | Default output loopback -- captures all app audio |
| Microphone | Default input device |
| [Named PW node] | Specific PipeWire node by name |
| None | Disable audio capture |

### Noise Floor Calibration

"Auto Calibrate" button: captures 2 seconds of audio with no music playing, measures the noise floor, sets `noise_gate_db` just above it. This prevents fan noise, electrical hum, or background chatter from triggering false beats.

```rust
pub async fn auto_calibrate_noise_floor(samples: &[f32]) -> f32 {
    // Compute RMS of "silence"
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
    let db = 20.0 * (rms + 1e-10).log10();
    // Set gate 6 dB above measured floor
    db + 6.0
}
```

### Troubleshooting: "No Audio Detected"

When the spectrum shows nothing:

```
1. Is audio playing? Check with: pactl list sinks
2. Is the right source selected? Check with: pactl list sources short
3. PipeWire running? Check with: systemctl --user status pipewire
4. Monitor source available? Check with: pactl list sources | grep -A2 monitor
5. Permissions: Is the user in the 'audio' group?
```

The web UI surfaces these checks as an interactive diagnostic wizard.

---

## 8. Multi-Source Audio

### Scenarios and Solutions

#### Scenario 1: Bliss cranks Perturbator

**Setup:** Spotify playing synthwave. Whole room should pulse.

**Solution:** Default configuration. System monitor captures Spotify output. Bass-heavy synthwave drives strong beat detection. All effects receive the same audio data. Every LED in the room is in sync.

**Effect recommendation:** "Pulse" preset with Cyberpunk color scheme. Bass → room brightness. Treble → sparkle. Beat → flash. Harmonic hue → palette shift.

#### Scenario 2: Jake's gaming

**Setup:** Game audio (explosions, ambient music) + voice chat (Discord).

**Solution A (simple):** System monitor captures everything. Explosions trigger bass events. Ambient music shifts colors. Discord voice chat is filtered by beat detection threshold (speech energy is mid-range and not percussive enough to trigger onsets).

**Solution B (clean):** PipeWire routing. Route game audio to a dedicated virtual sink. Route Discord to the default sink. Point Hypercolor at the game sink's monitor. Zero voice chat bleed.

```bash
# Create virtual sink for game audio
pw-cli create-node adapter '{
  factory.name=support.null-audio-sink
  node.name=game-audio
  media.class=Audio/Sink
  audio.position=[FL FR]
}'

# In Hypercolor config:
# source = "game-audio.monitor"
```

#### Scenario 3: Luna streams

**Setup:** Mic active for stream, Spotify playing, wants lighting to react to Spotify only.

**Solution:** Create a virtual sink for music, route Spotify to it, capture its monitor. Her microphone goes to OBS/streaming software through the default audio path. Hypercolor never sees mic input.

#### Scenario 4: Alex's ambient living room

**Setup:** Soft music, ceiling aurora effect, minimal reactivity.

**Solution:** Default monitor capture with high smoothing, low sensitivity, heavy temporal filtering. The "Ambient" mapping preset. Beat detection essentially disabled (high threshold). Color drift follows spectral centroid slowly over minutes.

**Audio settings:**
```toml
[audio]
gain = 0.5
sensitivity = 20
smoothing_attack = 0.1
smoothing_decay = 0.02
beat_threshold = 3.0
noise_gate_db = -40
```

### Zone-Based Audio Routing

Future capability: different audio sources for different device zones.

```toml
[[zones]]
name = "desk"
audio_source = "game-audio.monitor"
effect = "Cyber Rain"

[[zones]]
name = "ceiling"
audio_source = "music.monitor"
effect = "Aurora"

[[zones]]
name = "case"
audio_source = "system-monitor"  # default
effect = "Audio Pulse"
```

This requires running multiple capture streams simultaneously, which PipeWire handles natively.

---

## 9. AudioData Struct — The Contract

This is the complete data structure injected into effects every frame. It matches and extends the Lightscript API.

```rust
/// Complete audio analysis data, computed once per frame.
/// Injected into Servo as `window.engine.audio` and
/// into wgpu shaders as a uniform buffer.
pub struct AudioData {
    // ─── Standard (LightScript compatible) ────────────────────
    /// Overall audio level (dBFS, typically -100 to 0)
    pub level: f32,
    /// Stereo width (0 = mono, 1 = full stereo)
    pub width: f32,
    /// Audio density (spectral flatness, 0-1)
    pub density: f32,
    /// 200-bin frequency spectrum (log-spaced, normalized 0-1)
    pub freq: [f32; 200],
    /// Simple beat flag (true on beat onset)
    pub beat: bool,
    /// Beat pulse envelope (1.0 on beat, decays to 0)
    pub beat_pulse: f32,

    // ─── Band Energy ───────────────────────────────────────
    /// Bass energy (20-250 Hz, normalized 0-1)
    pub bass: f32,
    /// Mid energy (250-4000 Hz, normalized 0-1)
    pub mid: f32,
    /// Treble energy (4000-20000 Hz, normalized 0-1)
    pub treble: f32,

    // ─── Mel Scale ─────────────────────────────────────────
    /// 24 mel-spaced frequency bands (raw energy)
    pub mel_bands: [f32; 24],
    /// 24 mel bands auto-normalized to 0-1
    pub mel_bands_normalized: [f32; 24],

    // ─── Chromagram ────────────────────────────────────────
    /// 12-bin pitch class energy (C, C#, D, ..., B)
    pub chromagram: [f32; 12],
    /// Dominant pitch class (0-11)
    pub dominant_pitch: u8,
    /// Confidence of dominant pitch (0-1)
    pub dominant_pitch_confidence: f32,

    // ─── Spectral Features ─────────────────────────────────
    /// Spectral flux (overall rate of spectral change)
    pub spectral_flux: f32,
    /// Per-band spectral flux [bass, mid, treble]
    pub spectral_flux_bands: [f32; 3],
    /// Spectral centroid (brightness, Hz normalized to 0-1)
    pub brightness: f32,
    /// Spectral spread (bandwidth around centroid, 0-1)
    pub spread: f32,
    /// Spectral rolloff frequency (85% energy point, 0-1)
    pub rolloff: f32,

    // ─── Harmonic Analysis ─────────────────────────────────
    /// Harmonic hue (0-1, maps to 360-degree color wheel)
    pub harmonic_hue: f32,
    /// Chord mood (-1 = minor, 0 = ambiguous, +1 = major)
    pub chord_mood: f32,

    // ─── Beat Detection (Extended) ─────────────────────────
    /// Beat phase (0-1 continuous ramp, 0 = on beat)
    pub beat_phase: f32,
    /// Beat confidence (0-1, how sure we are about the tempo)
    pub beat_confidence: f32,
    /// Beat anticipation (0-1, ramps up before predicted beat)
    pub beat_anticipation: f32,
    /// Raw onset detection (true on any transient)
    pub onset: bool,
    /// Onset pulse envelope (like beatPulse but for all onsets)
    pub onset_pulse: f32,
    /// Estimated BPM
    pub tempo: f32,
}
```

### Servo Injection

For the Servo (HTML/Canvas) rendering path, `AudioData` is serialized to JavaScript and injected into `window.engine.audio`:

```rust
impl AudioData {
    pub fn to_js_injection(&self) -> String {
        format!(
            r#"window.engine = window.engine || {{}};
            window.engine.audio = {{
                level: {level},
                width: {width},
                density: {density},
                freq: new Int8Array([{freq}]),
                beat: {beat},
                beatPulse: {beat_pulse},
                bass: {bass},
                mid: {mid},
                treble: {treble},
                melBands: new Float32Array([{mel}]),
                melBandsNormalized: new Float32Array([{mel_norm}]),
                chromagram: new Float32Array([{chroma}]),
                dominantPitch: {dominant_pitch},
                dominantPitchConfidence: {dominant_pitch_conf},
                spectralFlux: {spectral_flux},
                spectralFluxBands: new Float32Array([{flux_bands}]),
                brightness: {brightness},
                spread: {spread},
                rolloff: {rolloff},
                harmonicHue: {harmonic_hue},
                chordMood: {chord_mood},
                beatPhase: {beat_phase},
                beatConfidence: {beat_confidence},
                beatAnticipation: {beat_anticipation},
                onset: {onset},
                onsetPulse: {onset_pulse},
                tempo: {tempo}
            }};"#,
            level = self.level,
            width = self.width,
            density = self.density,
            freq = self.freq_to_int8_string(),
            beat = self.beat as u8,
            beat_pulse = self.beat_pulse,
            bass = self.bass,
            mid = self.mid,
            treble = self.treble,
            mel = float_array_str(&self.mel_bands),
            mel_norm = float_array_str(&self.mel_bands_normalized),
            chroma = float_array_str(&self.chromagram),
            dominant_pitch = self.dominant_pitch,
            dominant_pitch_conf = self.dominant_pitch_confidence,
            spectral_flux = self.spectral_flux,
            flux_bands = float_array_str(&self.spectral_flux_bands),
            brightness = self.brightness,
            spread = self.spread,
            rolloff = self.rolloff,
            harmonic_hue = self.harmonic_hue,
            chord_mood = self.chord_mood,
            beat_phase = self.beat_phase,
            beat_confidence = self.beat_confidence,
            beat_anticipation = self.beat_anticipation,
            onset = self.onset as u8,
            onset_pulse = self.onset_pulse,
            tempo = self.tempo,
        )
    }

    /// Convert freq[200] to Int8Array format (matching LightScript convention)
    /// LightScript stores freq as signed 8-bit: -128 to 127
    fn freq_to_int8_string(&self) -> String {
        self.freq.iter()
            .map(|&f| ((f * 255.0 - 128.0).clamp(-128.0, 127.0)) as i8)
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}
```

### wgpu Uniform Buffer

For native shader effects, audio data is packed into a GPU uniform buffer:

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AudioUniforms {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat_pulse: f32,
    pub beat_phase: f32,
    pub tempo: f32,
    pub harmonic_hue: f32,
    pub spectral_flux: f32,
    pub brightness: f32,    // spectral centroid
    pub chord_mood: f32,
    pub beat_anticipation: f32,
    // Pad to 16-byte alignment
    pub _padding: [f32; 4],
}
```

Plus a storage buffer or texture for the full 200-bin spectrum, 24-band mel, and 12-bin chromagram (for effects that want per-bin access).

```wgsl
// audio.wgsl — shared audio uniform interface

struct AudioData {
    level: f32,
    bass: f32,
    mid: f32,
    treble: f32,
    beat_pulse: f32,
    beat_phase: f32,
    tempo: f32,
    harmonic_hue: f32,
    spectral_flux: f32,
    brightness: f32,
    chord_mood: f32,
    beat_anticipation: f32,
}

@group(1) @binding(0) var<uniform> audio: AudioData;
@group(1) @binding(1) var audio_spectrum: texture_1d<f32>; // 200 bins
@group(1) @binding(2) var audio_mel: texture_1d<f32>;      // 24 bands
@group(1) @binding(3) var audio_chroma: texture_1d<f32>;   // 12 bins
```

---

## 10. Performance

### CPU Budget Analysis

Target: < 5% of a single core on i7-14700K (equivalent: ~5% of one P-core at 5.5 GHz).

| Operation | Per-Frame Cost | At 60 FPS | Notes |
|---|---|---|---|
| **Ring buffer management** | ~1 us | 60 us | Lock-free SPSC ring buffer |
| **Hann window (1024)** | ~2 us | 120 us | Pre-computed coefficients |
| **FFT 1024-point** | ~15 us | 900 us | realfft crate, in-place r2c |
| **Magnitude + log scale** | ~5 us | 300 us | 512 complex → 512 magnitude |
| **200-bin mapping** | ~3 us | 180 us | Log interpolation |
| **Smoothing (200 bins)** | ~1 us | 60 us | Asymmetric EMA |
| **Band energy (3 bands)** | ~1 us | 60 us | Summation |
| **Mel filterbank (24)** | ~10 us | 600 us | Sparse matrix multiply |
| **Chromagram (12)** | ~8 us | 480 us | From 4096-point FFT, at 15 Hz |
| **Spectral features (4)** | ~5 us | 300 us | Centroid, spread, rolloff, flux |
| **Beat detection** | ~10 us | 600 us | Energy onset + spectral flux + tempo |
| **Harmonic analysis** | ~5 us | 300 us | Dominant pitch, chord mood, hue |
| **JS injection string** | ~20 us | 1200 us | String formatting for Servo path |
| **Total per frame** | **~86 us** | **~5.2 ms** | |

At ~86 microseconds per frame, the DSP pipeline consumes ~0.5% of one core at 60 FPS. Well within budget, even with generous margins.

### Threading Architecture

```
┌────────────────────────────────────────────────────────────────┐
│  Main Thread (tokio async runtime)                             │
│  ├ Render loop (60fps)                                         │
│  ├ Device output                                               │
│  ├ API/WebSocket serving                                       │
│  └ Reads AudioData via Arc<AtomicCell<AudioData>>              │
│                                                                │
│  Audio Capture Thread (cpal callback — OS-managed)             │
│  └ Pushes samples to lock-free ring buffer                     │
│                                                                │
│  DSP Thread (dedicated std::thread, pinned)                    │
│  ├ Reads from ring buffer every ~5ms (hop size)                │
│  ├ Runs full DSP pipeline                                      │
│  └ Writes AudioData to Arc<AtomicCell<AudioData>>              │
└────────────────────────────────────────────────────────────────┘
```

**Why a dedicated thread instead of tokio?**

The DSP pipeline is pure computation -- no I/O, no async. Running it on a dedicated thread with a tight spin loop (or condvar wake on new samples) gives us deterministic timing without competing with the tokio task scheduler. Pin it to a specific core if latency profiling shows jitter.

**Lock-free data flow:**

```rust
use crossbeam::queue::ArrayQueue;
use crossbeam::utils::CachePadded;

/// Audio capture → DSP thread (lock-free SPSC ring buffer)
type SampleBuffer = ArrayQueue<f32>;

/// DSP thread → render thread (atomic swap of latest analysis)
type AudioDataSlot = CachePadded<AtomicCell<AudioData>>;
```

No mutexes in the hot path. The render thread always reads the latest `AudioData` without blocking.

### Downsampling for Low-Power Systems

For systems where even 5ms of DSP per frame is too much (embedded, Raspberry Pi, old hardware):

| Strategy | Savings | Impact |
|---|---|---|
| Run DSP at 30 Hz instead of 60 | 50% | Slightly delayed response, beats still detected |
| Reduce FFT to 512-point | ~40% | Coarser bass resolution. Fine for most effects. |
| Skip chromagram/harmonic | ~30% | Lose pitch-to-color mapping. Spectrum + beat still work. |
| Skip mel filterbank | ~15% | Lose per-band visualization. Use raw 200-bin output. |

Configure via quality tier:

```rust
pub enum AudioQuality {
    /// Full pipeline: FFT 1024 + mel + chroma + harmonics @ 60Hz
    Full,
    /// Balanced: FFT 1024 + mel + beat @ 30Hz
    Balanced,
    /// Minimal: FFT 512 + beat @ 30Hz
    Minimal,
}
```

### The 4096-Point FFT Budget

The secondary 4096-point FFT (for chromagram and pitch detection) runs at 15 Hz, not 60. Cost: ~60 us every 4th frame. Negligible when amortized.

If even this is too much, compute it on demand only when an effect declares `audio_reactive: true` with harmonic features in its metadata. Static color effects don't need chromagram computation.

---

## Crate Dependencies

| Crate | Purpose | License |
|---|---|---|
| `cpal` | Cross-platform audio capture | Apache-2.0 |
| `realfft` | FFT (faster than spectrum-analyzer for our use case) | Apache-2.0/MIT |
| `libpulse-binding` | PulseAudio API for monitor source discovery | MIT/Apache |
| `crossbeam` | Lock-free ring buffer and atomic utilities | MIT/Apache |
| `bytemuck` | Zero-copy GPU buffer marshaling | MIT/Apache/Zlib |

The `spectrum-analyzer` crate (listed in ARCHITECTURE.md) wraps `realfft` with windowing and frequency analysis. Either works; `realfft` gives us more control over the pipeline, while `spectrum-analyzer` gives us a higher-level API. Recommendation: start with `spectrum-analyzer` for rapid prototyping, drop to `realfft` if we need finer control over the DSP chain.

---

## Open Questions

1. **PipeWire native vs. PulseAudio compatibility layer?** Using `libpulse` for source enumeration works on both PipeWire and PulseAudio. But `pipewire-rs` gives us direct access to the PipeWire graph for advanced routing (per-app capture). Worth the additional dependency?

2. **Chromagram accuracy.** The constant-Q transform (CQT) produces better chromagrams than mapping FFT bins to pitch classes. Is the quality improvement worth the added complexity? CQT is more expensive to compute.

3. **JACK host backend.** Should we support cpal's JACK host directly (feature-gated), or rely entirely on PipeWire's JACK compatibility? Pro audio users on legacy JACK might care.

4. **Audio data versioning.** The `AudioData` struct will grow over time. Should we version the JS injection contract (`window.engine.audio.v2`) to avoid breaking existing effects when we add fields?

5. **GPU-accelerated FFT.** For extreme scenarios (multiple simultaneous audio sources, 4096+ point FFTs at 60Hz), could we offload FFT to the GPU via wgpu compute shaders? Probably overkill, but architecturally elegant.
