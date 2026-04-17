+++
title = "SDK Reference"
description = "Complete API reference for the @hypercolor/sdk TypeScript package"
weight = 3
template = "page.html"
+++

The `@hypercolor/sdk` package gives you the runtime API for effect code and the Bun-powered authoring CLI that builds, validates, previews, and installs those effects.

## Architecture Overview

```
my-effect-pack/
  effects/
    aurora/
      main.ts
      fragment.glsl   # optional
  dist/
  package.json
  bunfig.toml
```

Effects are authored as TypeScript modules that export a single default value created by one of the SDK entry points (`effect`, `canvas`, or `canvas.stateful`). The authoring CLI bundles each effect into a standalone HTML artifact with all metadata, script code, and shader sources inlined.

## Authoring CLI

Inside a scaffolded workspace, the SDK ships a Bun-first `hypercolor` CLI:

```bash
bunx hypercolor dev
bunx hypercolor build --all
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
bunx hypercolor add ember --template canvas
```

Scaffolded workspaces expose the same flow through package scripts:

```bash
bun run dev
bun run build
bun run validate
bun run ship
bun run ship:daemon
```

`bun run dev` launches the preview studio with:

- effect switching across the whole workspace
- generated controls and preset switching
- audio simulation with beat triggering
- LED preview sampling for strip, matrix, and ring layouts
- canvas presets for daemon, strip, matrix, and ring aspect ratios

## Entry Points

### `effect(name, shader, controls?, options?)`

Creates a WebGL fragment shader effect.

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed: [1, 10, 5],
    palette: ['Northern Lights', 'SilkCircuit', 'Cyberpunk'],
})
```

**Parameters:**
- `name` — Display name for the effect
- `shader` — GLSL fragment shader source (imported as string)
- `controls` — Optional control definitions (see [Controls API](#controls-api))
- `options` — Optional metadata (description, author, audio)

### `canvas(name, controls?, renderFn, options?)`

Creates a Canvas 2D effect with a stateless render function.

```typescript
import { canvas } from '@hypercolor/sdk'

export default canvas('Gradient Sweep', {
    speed: [1, 10, 3],
}, (ctx, time, controls) => {
    const gradient = ctx.createLinearGradient(0, 0, 320, 0)
    const offset = (time * (controls.speed as number) * 0.1) % 1
    gradient.addColorStop(offset, '#e135ff')
    gradient.addColorStop((offset + 0.5) % 1, '#80ffea')
    ctx.fillStyle = gradient
    ctx.fillRect(0, 0, 320, 200)
})
```

**Parameters:**
- `name` — Display name
- `controls` — Optional control definitions
- `renderFn` — `(ctx: CanvasRenderingContext2D, time: number, controls: Record<string, ControlValue>) => void`
- `options` — Optional metadata

### `canvas.stateful(name, controls?, initFn, options?)`

Creates a Canvas 2D effect with persistent state.

```typescript
import { canvas } from '@hypercolor/sdk'

export default canvas.stateful('Particles', {
    count: [10, 500, 100],
}, () => {
    const particles = createParticles(100)
    return (ctx, time, controls) => {
        updateAndRender(ctx, particles, controls)
    }
})
```

The `initFn` runs once and returns the render function. State created in `initFn` persists across frames via closure.

## Controls API

Controls define the user-configurable parameters that appear in the UI. They can be declared with shorthand (shape inference) or explicit factory functions.

### Shorthand (Shape Inference)

| Value | Type | Example |
|---|---|---|
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `string[]` | Dropdown (first is default) | `palette: ['Fire', 'Ice']` |
| `'#rrggbb'` | Color picker | `color: '#ff0066'` |
| `boolean` | Toggle | `invert: false` |

### Factory Functions

For full control over labels, tooltips, defaults, and step values:

```typescript
import { num, combo, color, toggle, hue, text } from '@hypercolor/sdk'
```

**`num(label, [min, max], default, options?)`** — Number slider

```typescript
density: num('Particle Density', [10, 1000], 200, {
    step: 10,
    tooltip: 'Number of particles in the field',
})
```

**`combo(label, values, options?)`** — Dropdown selector

```typescript
palette: combo('Color Theme', ['SilkCircuit', 'Ice', 'Aurora'], {
    default: 'Ice',  // Override first-item default
})
```

**`color(label, default)`** — Color picker

```typescript
bgColor: color('Background', '#0d0221')
```

**`toggle(label, default, options?)`** — Boolean toggle

```typescript
mirror: toggle('Mirror Mode', false, {
    tooltip: 'Reflect the effect horizontally',
})
```

**`hue(label, [min, max], default)`** — Hue angle slider (0-360)

```typescript
baseHue: hue('Base Hue', [0, 360], 270)
```

**`text(label, default)`** — Text input

```typescript
message: text('Display Text', 'HYPERCOLOR')
```

### Controls in Shaders

For `effect()` (WebGL) effects, numeric controls are automatically mapped to GLSL uniforms:

```typescript
// TypeScript
export default effect('My Shader', shader, {
    speed: [1, 10, 5],
    intensity: [0, 100, 82],
})
```

```glsl
// GLSL — these uniforms are auto-injected with i + PascalCase naming
uniform float iSpeed;      // from control key "speed"
uniform float iIntensity;  // from control key "intensity"
```

## Audio Input Data

Effects can consume real-time audio analysis data. The SDK exposes audio through the `engine.audio` global and shader uniforms.

### Canvas Effects

```typescript
// Access via the global engine object
const audio = (window as any).engine?.audio

if (audio) {
    audio.level        // Overall RMS level (0-1)
    audio.bass         // Low-frequency energy (0-1)
    audio.mid          // Mid-frequency energy (0-1)
    audio.treble       // High-frequency energy (0-1)
    audio.beat         // True on detected beat
    audio.beatPulse    // Decaying pulse on beat (1 → 0)
    audio.freq         // Float32Array[200] — FFT frequency bins

    // Mel scale
    audio.melBands              // Float32Array[24]
    audio.melBandsNormalized    // Float32Array[24] (0-1)

    // Spectral features
    audio.spectralFlux          // Rate of spectral change
    audio.brightness            // Spectral centroid
    audio.spread                // Spectral width

    // Harmonic analysis
    audio.harmonicHue           // Dominant pitch mapped to hue (0-360)
    audio.chordMood             // Minor (-1) to Major (+1)

    // Beat tracking
    audio.beatPhase             // Phase within current beat (0-1)
    audio.beatConfidence        // Beat detection confidence (0-1)
    audio.onset                 // True on transient onset
}
```

### Shader Effects

Audio data is available through built-in uniforms:

```glsl
uniform float iTime;              // Elapsed seconds
uniform vec2 iResolution;         // Canvas size (320, 200)
uniform float iAudioLevel;        // Overall audio level (0-1)
uniform float iAudioBass;         // Bass band (0-1)
uniform float iAudioMid;          // Mid band (0-1)
uniform float iAudioTreble;       // Treble band (0-1)
uniform sampler2D iAudioSpectrum; // 200-bin FFT as a 1D texture
```

## Canvas Rendering Pipeline

The render pipeline for Canvas effects:

1. Your render function receives a `CanvasRenderingContext2D` whose size matches the daemon's
   configured render canvas (640x480 by default, user-tunable in Settings → Rendering)
2. You draw whatever you want — shapes, gradients, images, text
3. The SDK reads the canvas pixels after your function returns
4. Pixel data is sent to the daemon's spatial sampler
5. The sampler maps canvas positions to physical LED positions
6. Colors are pushed to hardware

Always read `canvas.width` and `canvas.height` on every frame — don't hardcode. Historical effects
were authored against a 320x200 grid and scale via normalized coordinates; writing new effects the
same way keeps them resolution-independent. Readback cost scales with the configured canvas:
~256 KB/frame at 320x200, ~1.17 MB/frame at 640x480, both trivially fast on modern hardware.

## Building and Distribution

### Build a Single Effect

```bash
bunx hypercolor build effects/my-effect/main.ts
```

### Build All Effects

```bash
bunx hypercolor build --all
```

### Validate

```bash
bunx hypercolor validate dist/my-effect.html
```

### Install

```bash
bunx hypercolor install dist/my-effect.html
bunx hypercolor install dist/my-effect.html --daemon
```

### Output

Built effects land in `dist/` as self-contained HTML files. Each file includes all JavaScript, CSS, shader code, and metadata inlined. Local install copies them into the user effects directory. Daemon install uploads them through `POST /api/v1/effects/install` and registers them immediately.

### Effect Discovery

Effects declare their metadata through the SDK entry points. The build pipeline resolves those definitions into HTML meta tags, including controls, presets, author, description, and `hypercolor-version`. The daemon reads that metadata directly from the HTML artifact. No separate manifest file is needed.
