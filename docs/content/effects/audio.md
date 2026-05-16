+++
title = "Audio"
description = "The AudioData surface and when to reach for each field"
weight = 7
template = "page.html"
+++

Hypercolor's audio pipeline runs FFT, beat detection, spectral analysis, mel-band binning, chromagram estimation, and harmonic mood inference every frame. Effects get all of that as a single `AudioData` struct, pulled per frame.

## Getting audio data

Canvas effects pull with `audio()`:

```typescript
import { audio, canvas } from "@hypercolor/sdk";

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

Shader effects get audio through auto-registered uniforms when `audio: true` is set:

```glsl
uniform float iAudioBass;
uniform float iAudioBeatPulse;
uniform float iAudioHarmonicHue;
// ...
```

The shader uniform surface is a subset of the canvas surface. See [GLSL Effects](@/effects/glsl-effects.md#audio-uniforms) for the complete uniform list. This page covers the full canvas `AudioData` surface, which is what to reach for when.

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
  frequencyRaw: Int8Array; // 200 raw dB values
  frequencyWeighted: Float32Array; // A-weighted

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

That's a lot. The guidance below pairs mood and motion to the right field.

## When to reach for what

### For impact

`beatPulse` is the go-to for "kick hit feels like a drum." It decays over a handful of frames, so multiplying by a size or brightness gives a nice punch without flickering.

```typescript
const punch = a.beatPulse;
const radius = base + punch * minDim * 0.2;
```

`onsetPulse` catches transients that aren't metrical beats: snare rolls, fills, non-drum onsets. Use it when you want a reaction that isn't tied to a metronome.

`bass` is a raw band level; use it when you want a continuous response to low-end energy, not an impulse.

### For structure

`levelShort` is the short-window RMS envelope. Perfect for "overall loudness right now" without the jitter of `level`. Multiply it into line widths, glow radii, or alpha.

`levelLong` is the long-window version. Use it for drift, breathing, or slow trend adaptation.

`momentum` is the derivative of level. Positive means things are getting louder; negative means quieter. `swell` is the positive half, which is what you usually want for "rising."

### For mood

`chordMood` is the minor-to-major axis. Negative leans minor; positive leans major. Shift warm/cool palettes, curl petals inward, tilt angles, any gesture that reads as "sad" vs "bright."

```typescript
const warmth = Math.max(a.chordMood, 0);
const minorness = Math.max(-a.chordMood, 0);
```

`harmonicHue` is the Circle of Fifths mapped to a hue wheel. Rotate palette sampling by this value so harmonic movement in the music becomes color movement on the hardware.

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

`dominantPitch` is the pitch class with the most energy. `dominantPitchConfidence` tells you how clean the winner is. Combine them to only punch up the dominant pitch when there's an obvious one:

```typescript
if (a.dominantPitchConfidence > 0.6) {
  highlightPetal(a.dominantPitch);
}
```

### For spectral detail

`melBands` is 24 perceptually-spaced frequency bands (0-1). `melBandsNormalized` applies rolling AGC to keep bands hot even in quiet passages. These are the right tool for bar visualizers, spectrograms, and anything that shows the spectrum as a shape.

`spectralFluxBands` is per-band rate of change as a 3-vector `[bass, mid, treble]`. Use it to kick specific zones when their band gets busy.

`brightness`, `spread`, `rolloff`, `roughness` are scalar timbre descriptors. `brightness` correlates with "brighter sound" (lots of high-frequency content), `spread` with "how wide the spectrum is," `rolloff` with "where the energy falls off," `roughness` with "how dissonant it is." Use them for fine-grained mood modulation that goes beyond the major/minor axis.

### Raw FFT

`frequency` is the 200-bin FFT, normalized 0-1. `frequencyWeighted` is A-weighted to match perceived loudness. Drop down to raw FFT only when you want custom band shaping that doesn't fit the mel bands.

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

**Using raw `beat` as a threshold.** `beat` is a single-frame signal. Effects that gate on `beat > 0.5` flicker unpleasantly. Use `beatPulse` for smooth decay instead.

**Depending on tempo.** `tempo` is an estimate and swings during intros and bridges. Don't use it as a frame budget; use it for display or for long-window averaging.

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
} from "@hypercolor/sdk";
```

`getFrequencyRange(audio, lowHz, highHz)` averages FFT bins across a custom band if the default three bands aren't what you need. `smoothValue(current, target, rate, deltaTime)` is a useful exponential smoother when you want to damp a jittery field.

`getPitchClassIndex(audio)` returns the integer index (0–11) of the dominant pitch class. `getPitchEnergy(audio, pitchClass)` returns the chromagram energy at a specific pitch class. These are lower-level companions to `dominantPitch` for effects that need to react to individual notes.

`getHarmonicColor(audio)` maps the current harmonic analysis to a CSS color string ready for `fillStyle`. `getMoodColor(audio)` derives a mood-based color from `chordMood` and `harmonicHue`.

`getBeatAnticipation(audio)` returns a value that rises before the predicted next beat, useful for pre-fire effects that should start moving slightly ahead of the kick.

`getScreenZoneData(audio, zone)` returns screen zone sampling data for screen-reactive effects. Use it when your effect needs to respond to a specific region of the captured screen rather than the full-frame audio stream.

## Designing audio reactivity

Two rules that separate good audio-reactive effects from frantic ones:

1. **Give the effect a life of its own.** Idle motion that's not driven by audio means the effect reads as alive when music is playing and still alive when it isn't. Prism Choir breathes via `Math.sin(time * 0.6)` and that baseline is what makes it look intentional in silence.

2. **Use harmony, not just energy.** Bass and beat get used in every beginner effect. `chromagram`, `harmonicHue`, and `chordMood` are what make an effect feel musically literate. Reach for them.
