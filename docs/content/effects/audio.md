+++
title = "Audio API"
description = "The full AudioData surface for canvas effects, the Rust snake_case vs TS camelCase split, and the shader-uniform cross-reference."
weight = 60
template = "page.html"
+++

Hypercolor's audio pipeline runs FFT, beat detection, spectral analysis, mel-band binning, chromagram estimation, and harmonic mood inference every frame. Effects get all of that as a single `AudioData` value, pulled per frame.

There are two `AudioData` shapes, and they do not share field names. TypeScript canvas and shader effects see the camelCase SDK surface documented on this page. Native Rust effects see a smaller snake_case struct. The split is the most common thing docs get wrong, so it has its own section near the bottom: [Rust vs TypeScript field names](#rust-vs-typescript-field-names).

![Audio-pulse effect reacting to music](/img/effects/audio-pulse.webp)

## Getting audio data

Canvas effects pull with `audio()` (a re-exported alias of `getAudioData()`, declared as `export { getAudioData as audio }`):

```typescript
import { audio, canvas } from "hypercolor";

export default canvas(
  "Pulse",
  controls,
  (ctx, time, controls) => {
    const a = audio();
    const intensity = Math.max(a.beatPulse, a.bassEnv * 0.6);
    // ...
  },
  { audio: true },
);
```

The pull is per frame: call `audio()` inside the draw function every frame, not once at module load. Outside the daemon (a bare browser, or the build's metadata pass) it returns a silent default rather than throwing, so effects render in preview without audio wired up.

{% callout(type="warning") %}
`{ audio: true }` is not cosmetic. The build scans your source for `audio(`, `ctx.audio`, `getAudioData(`, or `engine.audio`, and if it finds any of them without `audio: true` in the options it fails the build with `Audio reactivity validation failed for <entry>: effect uses audio helpers but is missing audio: true`. Set the flag whenever you read audio.
{% end %}

Shader effects get audio through auto-registered uniforms when `audio: true` is set:

```glsl
uniform float iAudioBass;
uniform float iAudioBeatPulse;
uniform float iAudioHarmonicHue;
// ...
```

The shader uniform surface is a strict subset of the canvas surface. Array fields (`chromagram`, `melBands`, raw `frequency`) are canvas-only; if a shader needs pitch-class data it has to drop to a canvas effect. See [GLSL effects](@/effects/glsl-effects.md#audio-uniforms) for the complete uniform list. This page covers the full canvas `AudioData` surface, which is what to reach for when you need more than the shader subset.

## The `AudioData` surface

```typescript
interface AudioData {
  // Levels
  level: number; // 0-1 overall RMS
  levelRaw: number; // raw dB, -100 to 0
  levelShort: number; // short-window envelope
  levelLong: number; // long-window envelope
  density: number; // spectral flatness (0-1)
  width: number; // stereo width (0-1)

  // Bands
  bass: number; // 0-1
  mid: number; // 0-1
  treble: number; // 0-1
  bassEnv: number; // attack envelope
  midEnv: number;
  trebleEnv: number;

  // Beats
  beat: number; // 1 on beat frame
  beatPulse: number; // decaying pulse after beat (1 → 0)
  beatPhase: number; // phase within current beat (0-1)
  beatConfidence: number; // 0-1
  tempo: number; // estimated BPM
  momentum: number; // level derivative (-1..1)
  swell: number; // positive momentum (0-1)

  // Raw FFT
  frequency: Float32Array; // 200 bins, 0-1
  frequencyRaw: Int8Array; // 200 raw signed values
  frequencyWeighted: Float32Array; // 200 bins, A-weighted

  // Perceptual frequency
  melBands: Float32Array; // 24 bands
  melBandsNormalized: Float32Array; // 24 bands with rolling AGC

  // Onsets
  spectralFlux: number; // overall flux
  spectralFluxBands: Float32Array; // per-band [bass, mid, treble]
  onset: number; // 1 on onset frame
  onsetPulse: number; // decaying onset

  // Harmony
  chromagram: Float32Array; // 12 pitch classes (C..B)
  dominantPitch: number; // 0-11
  dominantPitchConfidence: number; // 0-1
  harmonicHue: number; // 0-360, Circle of Fifths
  chordMood: number; // -1 minor..+1 major

  // Timbre
  brightness: number; // spectral centroid
  spread: number; // spectral spread
  rolloff: number; // high-frequency rolloff
  roughness: number; // dissonance
}
```

The fixed sizes are worth memorizing: `frequency`, `frequencyRaw`, and `frequencyWeighted` are **200** elements; `melBands` and `melBandsNormalized` are **24**; `chromagram` is **12** (one per chromatic pitch class). They are exported as constants too: `FFT_SIZE` (200), `MEL_BANDS` (24), `PITCH_CLASSES` (12).

That's a lot of surface. The guidance below pairs mood and motion to the right field.

## When to reach for what

### For impact

`beatPulse` is the go-to for "kick hit feels like a drum." It decays over a handful of frames, so multiplying it into a size or brightness gives a nice punch without flickering.

```typescript
const punch = a.beatPulse;
const radius = base + punch * minDim * 0.2;
```

`onsetPulse` catches transients that aren't metrical beats: snare rolls, fills, non-drum onsets. Use it when you want a reaction that isn't tied to a metronome.

`bass` is a raw band level; use it when you want a continuous response to low-end energy, not an impulse.

### For structure

`levelShort` is the short-window RMS envelope. Good for "overall loudness right now" without the jitter of `level`. Multiply it into line widths, glow radii, or alpha.

`levelLong` is the long-window version. Use it for drift, breathing, or slow trend adaptation.

`momentum` is the derivative of level. Positive means things are getting louder; negative means quieter. `swell` is the positive half, which is what you usually want for "rising."

### For mood

`chordMood` is the minor-to-major axis. Negative leans minor; positive leans major. Shift warm/cool palettes, curl petals inward, tilt angles, any gesture that reads as "sad" versus "bright."

```typescript
const warmth = Math.max(a.chordMood, 0);
const minorness = Math.max(-a.chordMood, 0);
```

`harmonicHue` is the Circle of Fifths mapped to a hue wheel (0-360). Rotate palette sampling by this value so harmonic movement in the music becomes color movement on the hardware.

```typescript
const hueOffset = a.harmonicHue / 360;
ctx.fillStyle = pal((t + hueOffset) % 1);
```

### For pitch-class detail

`chromagram` is the most underused audio surface. It's 12 floats, one per chromatic pitch class, each representing how much energy is at that note across all octaves. Map chroma indices onto spatial positions for structures that literally track the music's harmony.

```typescript
for (let i = 0; i < 12; i++) {
  const energy = a.chromagram[i];
  drawPetalAtAngle((i / 12) * Math.PI * 2, energy);
}
```

`dominantPitch` is the pitch class with the most energy (0-11, C through B). `dominantPitchConfidence` tells you how clean the winner is. Combine them to only punch up the dominant pitch when there's an obvious one:

```typescript
if (a.dominantPitchConfidence > 0.6) {
  highlightPetal(a.dominantPitch);
}
```

### For spectral detail

`melBands` is 24 perceptually-spaced frequency bands (0-1). `melBandsNormalized` applies rolling AGC to keep bands hot even in quiet passages. These are the right tool for bar visualizers, spectrograms, and anything that shows the spectrum as a shape.

`spectralFluxBands` is per-band rate of change as a 3-vector `[bass, mid, treble]`. Use it to kick specific zones when their band gets busy.

`brightness`, `spread`, `rolloff`, and `roughness` are scalar timbre descriptors. `brightness` correlates with "brighter sound" (lots of high-frequency content), `spread` with "how wide the spectrum is," `rolloff` with "where the energy falls off," `roughness` with "how dissonant it is." Use them for fine-grained mood modulation that goes beyond the major/minor axis.

### Raw FFT

`frequency` is the 200-bin FFT, normalized 0-1. `frequencyWeighted` is A-weighted to match perceived loudness. `frequencyRaw` is the same 200 bins as a signed 8-bit array (`Int8Array`), kept for compatibility with LightScript-era effects. Drop down to raw FFT only when you want custom band shaping that doesn't fit the mel bands.

## Idle behavior

With no audio, every audio field is zero or a sensible idle value. Don't gate behavior on strict equality with zero; clamp to a floor so the effect stays alive in a quiet room:

```typescript
const level = Math.max(a.levelShort, 0.04);
```

A breathing baseline derived from `time` is a good companion for idle:

```typescript
const breath = 0.5 + 0.5 * Math.sin(time * 0.6);
const alive = level + breath * 0.3;
```

## Anti-patterns

**Maxing every band.** Shoving `bass`, `mid`, and `treble` into three separate visual channels tends to flatten everything to white noise. Pick one band for structure and one for sparkle; ignore the third or use it sparingly.

**Using raw `beat` as a threshold.** `beat` is a single-frame signal. Effects that gate on `beat > 0.5` flicker unpleasantly. Use `beatPulse` for smooth decay instead, and prefer routing beat energy into motion rather than brightness.

**Depending on tempo.** `tempo` is an estimate and swings during intros and bridges (its silent default is `120`). Don't use it as a frame budget; use it for display or for long-window averaging.

**Ignoring `beatConfidence`.** On non-rhythmic music, the beat detector still fires but with low confidence. Multiply your beat response by `beatConfidence` to stay graceful:

```typescript
const beatGate = a.beatPulse * a.beatConfidence;
```

## Audio helpers

The SDK ships a handful of utilities for common audio transforms:

```typescript
import {
  getBassLevel,
  getMidLevel,
  getTrebleLevel,
  getFrequencyRange,
  getMelRange,
  getPitchClassName,
  getPitchClassIndex,
  getPitchEnergy,
  pitchClassToHue,
  getHarmonicColor,
  getMoodColor,
  getBeatAnticipation,
  getScreenZoneData,
  isOnBeat,
  normalizeAudioLevel,
  normalizeFrequencyBin,
  smoothValue,
} from "hypercolor";
```

`getBassLevel`, `getMidLevel`, and `getTrebleLevel` take the `frequency` array and average fixed bin ranges (0-10, 10-80, 80-200). `getFrequencyRange(frequency, start, end)` averages an arbitrary bin range when the default three bands aren't what you need. `getMelRange(audio, startBand, endBand)` does the same against `melBandsNormalized`. `smoothValue(current, previous, smoothing)` is an exponential smoother for damping a jittery field (default smoothing `0.5`). `normalizeFrequencyBin(value, max)` clamps a single raw bin to 0-1 (default `max` of 128).

`getPitchClassName(index)` and `getPitchClassIndex(name)` convert between pitch-class integers (0-11) and note names (`"C"`, `"C#"`, …). `getPitchEnergy(audio, pitchClass)` returns the chromagram energy at a specific pitch class, accepting either an index or a note name. `pitchClassToHue(pitchClass)` maps a pitch class to a hue via the Circle of Fifths.

`getHarmonicColor(audio, saturation?, lightness?)` maps the current `harmonicHue` to an RGB triple (defaults `0.7`/`0.5`). `getMoodColor(majorColor, minorColor, audio)` blends between two RGB colors along `chordMood`. `getBeatAnticipation(audio, anticipation?)` returns a value that rises just before the predicted next beat, useful for pre-fire effects. `isOnBeat(audio, division?, tolerance?)` is a boolean phase test against `beatPhase`.

`getScreenZoneData()` returns the 28×20 screen color grid (560 sample points of hue, saturation, and lightness) for screen-reactive effects that respond to a region of the captured display rather than the audio stream.

## Rust vs TypeScript field names

The audio pipeline runs in the Rust daemon and produces one analysis snapshot per DSP frame. That snapshot reaches effects two different ways, and the field names differ on each side.

For TypeScript effects, the daemon injects a JavaScript object onto `engine.audio` every frame. The SDK's `getAudioData()` reads that injected object and assembles the rich camelCase `AudioData` documented above, filling any field the daemon did not provide with a silent default (a zero-filled `FFT_SIZE`/`MEL_BANDS`/`PITCH_CLASSES` array for the array fields, sensible scalars like `tempo: 120` and `brightness: 0.5` for the rest).

For native Rust effects, the daemon hands a `&AudioData` directly inside `FrameInput`. That struct (`hypercolor_types::audio::AudioData`) is a smaller snake_case surface, and several names differ from the TypeScript side. Documenting one set of names for both paths is the most common mistake.

| Rust field (`hypercolor-types`) | TypeScript field (SDK) | Notes |
| ------------------------------- | ---------------------- | ----- |
| `spectrum` (`Vec<f32>`, 200) | `frequency` (`Float32Array`, 200) | Same 200 log-spaced bins, renamed |
| `mel_bands` (24) | `melBands` (24) | TS adds `melBandsNormalized` (rolling AGC) |
| `chromagram` (12) | `chromagram` (12) | 12 pitch classes, C..B |
| `beat_detected` (`bool`) | `beat` (`number`, 0/1) | TS exposes it as a number |
| `beat_confidence` | `beatConfidence` | |
| `beat_phase` | `beatPhase` | |
| `beat_pulse` | `beatPulse` | Decaying envelope on both sides |
| `bpm` | `tempo` | Different name for the BPM estimate |
| `rms_level` | `level` | Overall loudness |
| `peak_level` | _(none)_ | Native-only; no TS field |
| `spectral_centroid` | `brightness` | Renamed |
| `spectral_flux` | `spectralFlux` | |
| `onset_detected` (`bool`) | `onset` (`number`, 0/1) | |
| `onset_pulse` | `onsetPulse` | |

The Rust struct also exposes `bass()`, `mid()`, and `treble()` as methods that average bin ranges of `spectrum` (bins 0-39, 40-129, 130-199), and `AudioData::silence()` for the zero-filled no-audio case. The constants live on the Rust side too: `SPECTRUM_BINS` (200), `MEL_BANDS` (24), `CHROMA_BINS` (12).

The TypeScript surface is much larger: `bassEnv`, `harmonicHue`, `chordMood`, `dominantPitch`, `spread`, `rolloff`, `roughness`, `momentum`, `swell`, and the envelope fields are canvas-only conveniences derived on top of the same analysis. Native Rust effects compiled into `core/src/effect/builtin/` consume the smaller snake_case struct directly through `FrameInput.audio` and register via `core/src/effect/builtin/mod.rs`. The `audio_pulse` builtin is the canonical reference for reading the native surface.

{% callout(type="info") %}
Pitch-class index `0` is C and runs chromatically to `11` (B) on both sides. The chromagram is normalized so the loudest bin is `1.0`.
{% end %}

## Designing audio reactivity

Two rules that separate good audio-reactive effects from frantic ones:

1. **Give the effect a life of its own.** Idle motion that's not driven by audio means the effect reads as alive when music is playing and still alive when it isn't. A `Math.sin(time * 0.6)` breathing baseline is what makes an effect look intentional in silence.

2. **Use harmony, not just energy.** Bass and beat get used in every beginner effect. `chromagram`, `harmonicHue`, and `chordMood` are what make an effect feel musically literate. Reach for them.

To hear it on real hardware, set up a capture source in [audio setup](@/guide/audio-setup.md), then apply an audio-reactive effect such as `audio-pulse`, `cymatics`, or `spectral-fire` and watch the spectrum react. For the shader-side surface, see [GLSL effects](@/effects/glsl-effects.md#audio-uniforms). The compiled-in native path reads the snake_case struct described above through `core/src/effect/builtin/`.
