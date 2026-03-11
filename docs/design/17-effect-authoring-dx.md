# Design Document 17: Effect Authoring Developer Experience

> From "I want to make an effect" to "my effect is running on thousands of setups."

---

## Overview

The effect authoring experience is the lifeblood of Hypercolor. Without a compelling, low-friction path from idea to running LEDs, the ecosystem never reaches critical mass. This document designs the complete developer journey across three effect formats (HTML/Canvas, WGSL shaders, GLSL shaders), four authoring skill levels (consumer, artist, web developer, shader wizard), and the full lifecycle from scaffolding to publishing.

### Design Principles

1. **Zero to LEDs in 5 minutes.** A web developer with no RGB experience should see their first custom effect on hardware within a single `hypercolor new` + `hypercolor dev` cycle.
2. **The web is the foundation.** HTML/Canvas effects are the on-ramp. The 320x200 canvas, the `<meta>` tag controls, the `requestAnimationFrame` loop -- this is deliberately simple. Don't abstract it away.
3. **Progressive complexity.** Canvas 2D -> WebGL/Three.js -> WGSL compute shaders. Each step unlocks more power without invalidating what came before.
4. **Hot-reload everything.** Save a file, see the change on LEDs. No restart, no rebuild, no manual refresh. This is non-negotiable.
5. **Hardware is optional for development.** The dev server provides a complete simulation environment. You should be able to author effects on a laptop with zero RGB hardware connected.

---

## 1. Effect Development Server

The `hypercolor dev` command is the centerpiece of the authoring experience. It launches a local development environment that combines browser preview, hardware output, and debugging tools into a single coordinated workflow.

### 1.1 Command Interface

```bash
# Launch dev mode for an effect
hypercolor dev effects/custom/my-effect.html

# Launch dev mode for a WGSL shader
hypercolor dev effects/native/aurora.wgsl

# Launch with hardware output enabled (requires running daemon)
hypercolor dev my-effect.html --hardware

# Launch on a specific port
hypercolor dev my-effect.html --port 9421

# Launch with a specific layout preset
hypercolor dev my-effect.html --layout full-pc-case

# Launch with audio simulator pre-loaded
hypercolor dev my-effect.html --audio-sim
```

### 1.2 Architecture

The dev server is a lightweight Axum instance separate from the main Hypercolor daemon. It coordinates three subsystems:

```
hypercolor dev my-effect.html
       │
       ├── Browser Preview (localhost:9421)
       │   ├── Live effect rendering (320x200 canvas)
       │   ├── Virtual LED layout viewer
       │   ├── Control panel (auto-generated from meta tags)
       │   ├── Audio input simulator
       │   ├── Performance profiler
       │   └── Error overlay
       │
       ├── File Watcher (notify crate)
       │   ├── HTML/JS/TS file changes → hot reload
       │   ├── WGSL/GLSL file changes → shader recompile + swap
       │   └── Asset changes (images, fonts) → cache bust
       │
       └── Hardware Bridge (optional, via daemon IPC)
           ├── Connects to running hypercolor daemon
           ├── Pushes dev effect frames to hardware
           └── Overrides current effect on connected devices
```

### 1.3 Hot Reload Pipeline

For HTML/Canvas effects:

```
File save detected (notify)
    → Compute file hash (skip if unchanged)
    → For HTML effects:
        → WebSocket message to browser: { type: "reload", path: "my-effect.html" }
        → Browser reloads iframe containing the effect
        → Effect state optionally preserved (time, control values)
    → For Vite-built effects (TypeScript/Lightscript):
        → Vite HMR handles module replacement
        → Effect class re-instantiated with preserved state
    → Frame appears on browser preview within ~50ms of save
    → If --hardware: frame relayed to daemon → LEDs update simultaneously
```

For WGSL/GLSL shaders:

```
File save detected (notify)
    → Parse shader source
    → If parse error:
        → Send error to browser overlay (line number, message, source context)
        → Keep previous working shader running (no flicker)
        → Terminal output with colored error diagnostics
    → If parse OK:
        → Compile via wgpu (naga validation for WGSL, glslang for GLSL)
        → If compile error:
            → Same error overlay treatment
        → If compile OK:
            → Hot-swap pipeline (new render/compute pipeline, same uniform bindings)
            → Seamless transition, no frame drop
            → Browser preview updates instantly
```

The key insight: **errors never kill the preview.** The last working version keeps running while errors are displayed. This is critical for the shader editing flow where typos are constant.

### 1.4 Browser Preview UI

The dev server serves a purpose-built development interface at `localhost:9421`:

```
┌────────────────────────────────────────────────────────────┐
│  Hypercolor Dev  │  my-effect.html  │  60.0 fps  │  0.8ms │
├────────────────────────────────────────────────────────────┤
│                                                            │
│  ┌──────────────────────────────────┐  ┌────────────────┐ │
│  │                                  │  │  Controls       │ │
│  │     320x200 Effect Canvas        │  │                 │ │
│  │     (actual rendered output)     │  │  Speed: ████░ 7 │ │
│  │                                  │  │  Color: [____]  │ │
│  │                                  │  │  Style: [combo] │ │
│  └──────────────────────────────────┘  │  Glow:  ████░ 8 │ │
│                                        │                 │ │
│  ┌──────────────────────────────────┐  │  ── Audio ──    │ │
│  │     Virtual LED Layout           │  │  Source: [Sim]  │ │
│  │     (devices rendered from       │  │  BPM: 128       │ │
│  │      canvas sampling)            │  │  ▂▃█▅▂▁▃▅█▃▂▁  │ │
│  │                                  │  │                 │ │
│  │  ╭──────╮   ╭──╮  ╭──╮          │  └────────────────┘ │
│  │  │ Case │   │F1│  │F2│ ← fans   │                     │
│  │  │ strip│   ╰──╯  ╰──╯          │  ┌────────────────┐ │
│  │  ╰──────╯          ▬▬▬ ← RAM    │  │  Performance   │ │
│  │                                  │  │  Frame: 0.8ms  │ │
│  └──────────────────────────────────┘  │  GPU:   12%    │ │
│                                        │  Mem:   45MB   │ │
│  ┌──────────────────────────────────┐  │  Dropped: 0    │ │
│  │  Console / Errors                │  └────────────────┘ │
│  │  > Effect loaded (0.3ms)         │                     │
│  │  > Audio sim: 128 BPM, Bass      │                     │
│  └──────────────────────────────────┘                     │
└────────────────────────────────────────────────────────────┘
```

**Key panels:**

- **Effect Canvas** -- The actual 320x200 output, scaled up for visibility. Pixel-perfect rendering, no interpolation artifacts. Click to inspect pixel color values.
- **Virtual LED Layout** -- Shows how the effect maps to physical devices. Uses the spatial layout engine to sample the canvas at LED positions and render colored dots/strips/rings. Switchable between layout presets.
- **Controls** -- Auto-generated from `<meta>` tags (HTML effects) or uniform declarations (shaders). Changes are injected live without reload. Drag a slider, see it instantly.
- **Audio Panel** -- Source selector (system audio, simulator, file), BPM display, real-time spectrum visualization, individual band controls for the simulator.
- **Performance** -- Frame time histogram, GPU utilization, memory usage, dropped frame counter. Frame budget bar showing how much of the 16.6ms budget is consumed.
- **Console** -- Effect `console.log` output, error messages with source mapping, warnings about deprecated API usage.

### 1.5 Virtual LED Layout Viewer

The layout viewer renders a spatial representation of how the effect appears on actual hardware configurations. It uses the same `SpatialSampler` as the production daemon -- what you see in dev is what you get on hardware.

**Built-in layout presets:**

| Preset | Description | LED Count |
|---|---|---|
| `single-strip` | One horizontal LED strip | 60 |
| `dual-strip` | Two parallel strips (desk underglow) | 120 |
| `pc-case-basic` | 2 fans + 1 strip | ~80 |
| `full-pc-case` | 4 fans + 2 strips + RAM + GPU + Strimers | ~500 |
| `keyboard-60` | 60% keyboard matrix (14x5) | 70 |
| `keyboard-full` | Full-size keyboard matrix (22x6) | 132 |
| `monitor-ambient` | LED strip behind monitor (3 sides) | 90 |
| `room` | Multiple strips around a room perimeter | 300 |
| `desk-setup` | Monitor + keyboard + mouse + desk strips | ~250 |

Authors can also load custom layout JSON files exported from the main Hypercolor spatial editor.

**Viewport features:**
- Pan and zoom with mouse/trackpad
- Toggle between 2D overhead and 3D perspective views
- Show/hide sampling grid overlay (see which canvas pixels map to which LEDs)
- LED brightness simulation (gamma-correct preview of actual LED output)
- Device labels and zone names

### 1.6 Audio Input Simulator

Most compelling effects are audio-reactive, but testing requires music playing. The audio simulator provides deterministic, reproducible audio input for development.

**Simulator modes:**

| Mode | Description | Use Case |
|---|---|---|
| **Metronome** | Clean, periodic beats at configurable BPM | Test beat detection, pulse effects |
| **Sweep** | Frequency sweep from bass to treble over N seconds | Verify full-spectrum response |
| **Bass Pulse** | Isolated bass hits with configurable intensity | Test bass-reactive effects |
| **Full Spectrum** | Pre-recorded spectrum data playing in a loop | Realistic testing without live audio |
| **Random** | Randomized spectrum with configurable energy distribution | Stress testing, edge cases |
| **File** | Load an audio file, extract real FFT data | Test against specific songs |

**Simulator controls:**
- BPM slider (60-200)
- Per-band energy sliders (bass, low-mid, mid, high-mid, treble)
- Beat confidence slider (how "clean" the beats are)
- Global level / intensity
- Pause / step-frame for debugging

The simulator outputs the complete Lightscript audio API surface -- `level`, `bass`, `mid`, `treble`, `freq[200]`, `beat`, `beatPulse`, `melBands[24]`, `chromagram[12]`, `spectralFlux`, `harmonicHue`, `chordMood`, `beatPhase`, `onset`, and all the rest. This ensures effects developed against the simulator behave identically with real audio input.

### 1.7 Performance Profiler

The profiler runs continuously during development and flags potential issues before they reach production.

**Metrics tracked:**

```
Frame Timing
├── Total frame time (target: <16.6ms for 60fps)
├── Effect render time (Canvas 2D draw calls / shader execution)
├── Spatial sampling time
├── Browser compositing overhead
└── Frame time variance (jitter detection)

Resource Usage
├── GPU memory allocation
├── JS heap size (for HTML effects)
├── Canvas draw call count per frame
├── Texture/buffer allocation count
└── WebGL state changes per frame

Alerts (shown inline)
├── "Frame budget exceeded: 18.2ms avg (target: 16.6ms)"
├── "High GC pressure: 12 collections/sec (reduce allocations)"
├── "Draw call count: 847/frame (consider batching)"
└── "Memory growing: +2MB/min (possible leak)"
```

**Profiler flame chart:**
A timeline view showing per-frame breakdown. Hovering over frames shows exactly where time is spent. Critical for optimizing complex effects before publishing.

---

## 2. HTML/Canvas Effect Authoring

The HTML path is the primary on-ramp. It leverages the most widely-known programming environment on the planet -- web development -- and connects it directly to RGB hardware.

### 2.1 The Simplest Possible Effect

This is the minimum viable effect. If someone can write this, they can make LEDs do things:

```html
<head>
  <title>My First Effect</title>
  <meta description="A simple color wave" />
  <meta property="speed" label="Speed" type="number" min="1" max="10" default="5" />
</head>
<body>
  <canvas id="exCanvas" width="320" height="200"></canvas>
</body>
<script>
  const canvas = document.getElementById("exCanvas");
  const ctx = canvas.getContext("2d");
  let t = 0;

  function render() {
    for (let x = 0; x < 320; x++) {
      const hue = (x + t) % 360;
      ctx.fillStyle = `hsl(${hue}, 100%, 50%)`;
      ctx.fillRect(x, 0, 1, 200);
    }
    t += speed; // 'speed' is auto-injected from the <meta> tag
    requestAnimationFrame(render);
  }
  requestAnimationFrame(render);
</script>
```

That's it. No build step, no imports, no classes, no decorators. Just HTML and the Canvas 2D API. The `speed` variable is injected as a window global from the `<meta>` tag, exactly like the LightScript format does today.

This compatibility with the existing HTML effect format is deliberate. The 210+ community effects already in `effects/community/` run unmodified on this path. The Rainbow effect is 66 lines. The Borealis effect uses inline simplex noise. These are the patterns we preserve.

### 2.2 The Lightscript Path (TypeScript + Vite)

For authors who want more structure -- TypeScript types, class-based organization, decorator controls, npm packages, hot module replacement -- the Lightscript SDK provides a professional development environment built on Vite.

**Project structure (generated by `hypercolor new`):**

```
my-effect/
├── src/
│   ├── effect.ts          # Main effect class
│   └── utils.ts           # Helper functions (optional)
├── package.json
├── tsconfig.json
├── vite.config.ts
├── index.html             # Entry point (auto-generated, minimal)
└── hypercolor.toml         # Effect metadata
```

**Effect class with Lightscript decorators:**

```typescript
import { CanvasEffect, NumberControl, ComboboxControl, ColorControl } from "@hypercolor/lightscript";

export class NeonRain extends CanvasEffect {
  @NumberControl({ label: "Speed", min: 1, max: 20, default: 8 })
  speed!: number;

  @NumberControl({ label: "Density", min: 10, max: 200, default: 80 })
  density!: number;

  @ComboboxControl({
    label: "Palette",
    values: ["Cyberpunk", "Vaporwave", "Toxic", "Ocean"],
    default: "Cyberpunk",
  })
  palette!: string;

  @ColorControl({ label: "Accent", default: "#ff00ff" })
  accent!: string;

  private drops: Drop[] = [];

  onInit(): void {
    this.drops = Array.from({ length: this.density }, () => this.spawnDrop());
  }

  onRender(ctx: CanvasRenderingContext2D, dt: number): void {
    ctx.fillStyle = "rgba(0, 0, 0, 0.15)";
    ctx.fillRect(0, 0, 320, 200);

    const audio = this.getAudioData();

    for (const drop of this.drops) {
      drop.y += this.speed * dt * (1 + audio.bass * 2);
      if (drop.y > 200) this.resetDrop(drop);

      const color = this.paletteColor(drop.hue);
      ctx.fillStyle = color;
      ctx.fillRect(drop.x, drop.y, 2, drop.length);
    }
  }

  private paletteColor(hue: number): string {
    // Palette-aware color generation
  }
}
```

**Key Lightscript SDK features:**

- `CanvasEffect` base class: manages canvas lifecycle, control injection, frame loop
- `WebGLEffect` base class: wraps Three.js scene/renderer, provides shader uniform bindings
- Decorator-based controls: `@NumberControl`, `@ComboboxControl`, `@ColorControl`, `@BooleanControl`, `@HueControl`
- `getAudioData()`: returns the full audio API surface with TypeScript types
- `onInit()`, `onRender(ctx, dt)`, `onResize()`, `onControlChange(name, value)` lifecycle hooks
- Full TypeScript intellisense for the entire API surface

**Vite integration:**

The Lightscript SDK includes a Vite plugin (`@hypercolor/vite-plugin-lightscript`) that handles:

- TypeScript compilation with decorator support
- Bundling the effect + Lightscript runtime into a single HTML file for distribution
- Generating `<meta>` tags from decorator metadata (backwards-compatible output)
- Source maps for debugging in the dev server
- npm dependency bundling (simplex-noise, chroma-js, etc. are inlined)
- Hot module replacement during development -- change a control default, see it update without full reload

**npm package support:**

```typescript
import { createNoise2D } from "simplex-noise";
import chroma from "chroma-js";

export class NoiseField extends CanvasEffect {
  private noise = createNoise2D();

  onRender(ctx: CanvasRenderingContext2D, dt: number): void {
    const imageData = ctx.createImageData(320, 200);
    for (let y = 0; y < 200; y++) {
      for (let x = 0; x < 320; x++) {
        const n = this.noise(x * 0.02 + this.time, y * 0.02);
        const color = chroma.scale(["#000", "#e135ff", "#80ffea"])(n * 0.5 + 0.5);
        const [r, g, b] = color.rgb();
        const i = (y * 320 + x) * 4;
        imageData.data[i] = r;
        imageData.data[i + 1] = g;
        imageData.data[i + 2] = b;
        imageData.data[i + 3] = 255;
      }
    }
    ctx.putImageData(imageData, 0, 0);
  }
}
```

### 2.3 WebGL / Three.js Effects

For GPU-accelerated effects using GLSL shaders, the Lightscript SDK provides a `WebGLEffect` base class that wraps Three.js:

```typescript
import { WebGLEffect, NumberControl } from "@hypercolor/lightscript";

export class PlasmaShader extends WebGLEffect {
  @NumberControl({ label: "Complexity", min: 1, max: 10, default: 5 })
  complexity!: number;

  @NumberControl({ label: "Speed", min: 1, max: 20, default: 8 })
  speed!: number;

  fragmentShader = /* glsl */ `
    uniform float iTime;
    uniform vec2 iResolution;
    uniform float complexity;
    uniform float speed;
    uniform float iAudioBass;

    varying vec2 vUv;

    void main() {
      vec2 uv = vUv * complexity;
      float t = iTime * speed * 0.1;

      float v = sin(uv.x * 3.0 + t);
      v += sin(uv.y * 4.0 - t * 0.7);
      v += sin((uv.x + uv.y) * 2.0 + t * 1.3);
      v += sin(length(uv - 0.5) * 5.0 - t) * (1.0 + iAudioBass);

      vec3 color = vec3(
        sin(v * 3.14159) * 0.5 + 0.5,
        sin(v * 3.14159 + 2.094) * 0.5 + 0.5,
        sin(v * 3.14159 + 4.189) * 0.5 + 0.5
      );

      gl_FragColor = vec4(color, 1.0);
    }
  `;

  // Standard uniforms (iTime, iResolution, iMouse, iAudio*) are
  // bound automatically. Custom controls are bound by name.
}
```

**Standard uniform bindings (auto-injected):**

| Uniform | Type | Description |
|---|---|---|
| `iTime` | `float` | Elapsed seconds since effect start |
| `iResolution` | `vec2` | Canvas size (320.0, 200.0) |
| `iMouse` | `vec2` | Mouse position (normalized 0-1, rarely used for LED effects) |
| `iAudioLevel` | `float` | Overall audio level (0-1) |
| `iAudioBass` | `float` | Bass band energy (0-1) |
| `iAudioMid` | `float` | Mid band energy (0-1) |
| `iAudioTreble` | `float` | Treble band energy (0-1) |
| `iAudioSpectrum` | `sampler2D` | 200-bin FFT as a 200x1 texture |
| `iAudioBeat` | `float` | Beat pulse (0-1, spikes on beats) |
| `iAudioBeatPhase` | `float` | Phase within current beat (0-1) |

These are the same uniforms used by lightscript-workshop today. Effect portability is preserved.

### 2.4 LightScript Compatibility Layer

Hypercolor must run the existing corpus of 210+ community effects and 5 built-in effects without modification. The compatibility layer handles the differences between the original Ultralight/Qt WebEngine environment and Hypercolor's Servo renderer.

**What the shim provides:**

1. **Control injection** -- Parse `<meta property="...">` tags, inject values as `window[propertyName]` globals
2. **`window.update()` callback** -- Called when control values change (some effects rely on this)
3. **Audio data** -- Populate `window.engine.audio` with the full Lightscript audio API
4. **Console redirection** -- Capture `console.log/warn/error` and forward to dev server
5. **Missing API polyfills** -- Any browser APIs that Servo doesn't support but effects rely on

**Compatibility test suite:**

```
effects/community/
├── borealis.html          ✓ Canvas 2D, simplex noise inline
├── cyberpunk-2077.html    ✓ Canvas 2D, gradient effects
├── fire.html              ✓ Canvas 2D, pixel manipulation
├── nebula.html            ✓ Canvas 2D, particle system
├── ...
└── (210 effects total)

Test matrix:
├── Renders without JS errors
├── Controls from <meta> tags are parsed correctly
├── Animation loop runs at target framerate
├── Audio data injection works (if audio-reactive)
├── Visual output matches reference screenshots (±5% tolerance)
└── No memory leaks over 60 seconds of runtime
```

The compatibility test suite runs as part of CI. Any Servo update that breaks existing effects is caught before merge.

---

## 3. WGSL Shader Authoring

The native GPU path. Maximum performance, direct hardware access, no web engine overhead. For authors who want to push the limits or who come from a shader programming background.

### 3.1 WGSL Effect Structure

A WGSL effect is a single `.wgsl` file with a companion `.toml` metadata file:

```
effects/native/
├── aurora.wgsl
├── aurora.toml
├── plasma-ocean.wgsl
├── plasma-ocean.toml
└── lib/                     # Shared shader libraries
    ├── noise.wgsl
    ├── color.wgsl
    ├── audio.wgsl
    └── math.wgsl
```

**Minimal WGSL effect (`aurora.wgsl`):**

```wgsl
// Hypercolor standard uniform block -- always available
struct Uniforms {
    time: f32,
    resolution: vec2<f32>,
    mouse: vec2<f32>,
    audio_level: f32,
    audio_bass: f32,
    audio_mid: f32,
    audio_treble: f32,
    audio_beat: f32,
    audio_beat_phase: f32,
    // Custom controls follow
    speed: f32,
    color_shift: f32,
    wave_count: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var audio_spectrum: texture_2d<f32>;  // 200x1 FFT
@group(0) @binding(2) var spectrum_sampler: sampler;

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / u.resolution;

    // Aurora effect logic
    var color = vec3<f32>(0.0);
    let t = u.time * u.speed * 0.1;

    for (var i = 0u; i < u32(u.wave_count); i++) {
        let fi = f32(i);
        let wave = sin(uv.x * 6.0 + t + fi * 0.7) * 0.5 + 0.5;
        let band = smoothstep(0.3, 0.7, 1.0 - abs(uv.y - wave));
        let hue = fract(fi * 0.13 + u.color_shift * 0.01 + t * 0.05);

        color += hsv_to_rgb(hue, 0.8, band * (0.5 + u.audio_bass * 0.5));
    }

    return vec4<f32>(color, 1.0);
}
```

**Companion metadata (`aurora.toml`):**

```toml
[effect]
name = "Aurora"
description = "Northern lights simulation with audio-reactive wave intensity"
author = "hyperb1iss"
version = "1.0.0"
tags = ["ambient", "audio-reactive", "nature"]
audio_reactive = true

[controls.speed]
label = "Speed"
type = "number"
min = 1.0
max = 20.0
default = 5.0
tooltip = "Animation speed"

[controls.color_shift]
label = "Color Shift"
type = "number"
min = 0.0
max = 360.0
default = 0.0
tooltip = "Rotate the color palette"

[controls.wave_count]
label = "Waves"
type = "number"
min = 3.0
max = 15.0
default = 7.0
step = 1.0
tooltip = "Number of aurora bands"
```

### 3.2 Shader Include System

Reusable shader libraries eliminate boilerplate and provide battle-tested implementations of common operations.

**Include syntax (preprocessor directive, resolved at compile time):**

```wgsl
// #include "lib/noise.wgsl"
// #include "lib/color.wgsl"

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / u.resolution;
    let n = fbm_3d(vec3(uv * 4.0, u.time * 0.3), 6);  // from noise.wgsl
    let color = palette(n, PALETTE_SUNSET);              // from color.wgsl
    return vec4(color, 1.0);
}
```

**Standard library modules:**

| Module | Contents |
|---|---|
| `lib/noise.wgsl` | Simplex 2D/3D/4D, value noise, Worley/cellular, FBM (fractional Brownian motion), curl noise, domain warping |
| `lib/color.wgsl` | HSV/HSL/Oklab conversion, palette interpolation, gamma correction, color blending modes, named palettes |
| `lib/audio.wgsl` | Spectrum sampling helpers, beat-reactive easing, frequency band extraction, spectrum smoothing |
| `lib/math.wgsl` | Rotation matrices, SDF primitives (circle, box, hexagon), smooth min/max, polar coordinates, remapping |
| `lib/pattern.wgsl` | Voronoi, checkerboard, hexagonal grid, truchet tiles, reaction-diffusion |

The include system is a simple text preprocessor -- no module system, no namespacing, just source concatenation with duplicate-include guards. This keeps WGSL effects fully self-contained when distributed (includes are inlined at build time).

### 3.3 Compute Shader Effects

For effects that need per-pixel logic with shared memory (e.g., cellular automata, fluid simulation, particle systems with spatial hashing):

```wgsl
struct Uniforms {
    time: f32,
    resolution: vec2<f32>,
    audio_bass: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<storage, read_write> state: array<f32>;  // persistent state buffer

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= 320u || id.y >= 200u) { return; }

    let idx = id.y * 320u + id.x;
    let uv = vec2<f32>(id.xy) / u.resolution;

    // Read previous state
    let prev = state[idx];

    // Compute new state (Game of Life, fluid sim, etc.)
    let next = compute_cell(id.xy, prev);

    // Write state for next frame
    state[idx] = next;

    // Write pixel output
    let color = state_to_color(next);
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(color, 1.0));
}
```

Compute shaders get a persistent `state` buffer that survives across frames -- essential for stateful effects like fluid simulations, Game of Life variants, and trail-based particle systems.

### 3.4 Shadertoy Compatibility

Shadertoy is the world's largest shader gallery with thousands of effects. Making it easy to port Shadertoy shaders to Hypercolor dramatically expands the available effect library.

**Approach: GLSL translation layer, not runtime emulation.**

Hypercolor accepts GLSL fragment shaders with Shadertoy conventions and transpiles them to WGSL at compile time via naga:

```bash
# Port a Shadertoy shader
hypercolor new effect --template shadertoy my-shader

# The template includes the Shadertoy compatibility header
```

**Shadertoy compatibility header (auto-included):**

```glsl
// Shadertoy → Hypercolor bridge
// These uniforms match Shadertoy's built-in variables
uniform float iTime;
uniform vec2  iResolution;
uniform vec2  iMouse;
uniform int   iFrame;

// Hypercolor audio extensions (not in Shadertoy)
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform sampler2D iChannel0;  // maps to audio spectrum texture

// Paste your Shadertoy mainImage() function below
// void mainImage(out vec4 fragColor, in vec2 fragCoord) { ... }

void main() {
    mainImage(gl_FragColor, gl_FragCoord.xy);
}
```

**Translation pipeline:**

```
Shadertoy GLSL (mainImage)
    → Prepend compatibility header
    → GLSL validation (glslang)
    → GLSL → SPIR-V (glslang)
    → SPIR-V → WGSL (naga)
    → Standard wgpu render pipeline
```

**What works automatically:**
- All GLSL math functions, texture sampling, swizzling
- `iTime`, `iResolution`, `iMouse`, `iFrame` uniforms
- Single-pass shaders (the vast majority of Shadertoy effects)
- Most GLSL extensions used by Shadertoy

**What requires manual adaptation:**
- Multi-pass shaders (Buffer A/B/C/D) -- need to be restructured as compute passes
- `iChannel0-3` when used for texture input (need to provide textures)
- Shaders using `texelFetch` on buffer channels for feedback loops
- Shaders that exceed WGSL's limitations (recursion depth, certain loop patterns)

**Practical expectation:** ~70% of Shadertoy's single-pass shaders should port with copy-paste + the compatibility header. Multi-pass shaders require manual work but are architecturally possible with compute shader state buffers.

### 3.5 Shader Error Reporting

Shader compilation errors are the number one friction point in shader development. The error experience must be exceptional.

**Error display in browser preview:**

```
┌──────────────────────────────────────────────────────────────┐
│  SHADER ERROR                                                │
│                                                              │
│  aurora.wgsl:24:15  error: type mismatch                     │
│                                                              │
│    22 │     let wave = sin(uv.x * 6.0 + t + fi * 0.7);      │
│    23 │     let band = smoothstep(0.3, 0.7, 1.0 - wave);    │
│  > 24 │     color += hsv_to_rgb(hue, 0.8, band);            │
│       │               ^^^^^^^^^^                              │
│    25 │   }                                                   │
│                                                              │
│  Expected: fn(f32, f32, f32) -> vec3<f32>                    │
│  Got:      fn(f32, f32, vec2<f32>) -> vec3<f32>              │
│                                                              │
│  Hint: 'band' is vec2<f32> because 'wave' is vec2<f32>      │
│  from sin() on line 22. Did you mean uv.x instead of uv?    │
│                                                              │
│  Previous working version still running.                     │
└──────────────────────────────────────────────────────────────┘
```

**Error reporting features:**

1. **Source context** -- Show surrounding lines with the error line highlighted
2. **Precise location** -- Column-level caret pointing at the exact token
3. **Type information** -- Show expected vs. actual types
4. **Hints** -- Trace the error back to its root cause when possible (e.g., type propagation from an earlier expression)
5. **Non-destructive** -- Previous working shader keeps running, error overlay floats on top
6. **Terminal output** -- Same information with ANSI colors for terminal-based editing workflows
7. **Editor integration** -- LSP-compatible diagnostics for editors that support WGSL language servers

**Terminal error output example:**

```
  error[E0001]: type mismatch in aurora.wgsl
    --> aurora.wgsl:24:15
     |
  22 |     let wave = sin(uv.x * 6.0 + t + fi * 0.7);
  23 |     let band = smoothstep(0.3, 0.7, 1.0 - wave);
  24 |     color += hsv_to_rgb(hue, 0.8, band);
     |               ^^^^^^^^^^ expected vec3<f32>, found vec3<f32> -> error in argument 3
     |
     = note: 'band' has type vec2<f32>
     = hint: did you mean `uv.x` instead of `uv` on line 22?
```

---

## 4. Visual Effect Builder

For creators who think in colors and layers, not code. The visual builder is a browser-based tool that generates effect code without requiring programming knowledge.

### 4.1 Feasibility Assessment

**v1: Ship the layer compositor.** It's the highest impact, lowest complexity visual tool. Think "Photoshop layers for LED effects" -- stack gradient layers, blend them, animate properties, wire audio to parameters.

**v2: Add the node-based shader editor.** This is significantly more complex (think Blender shader nodes) and requires a custom node graph runtime. It's a 6-month project on its own.

**v3: AI-assisted generation.** Natural language to effect code, with the visual builder as an interactive refinement tool.

### 4.2 Layer-Based Compositor (v1 Target)

The compositor stacks visual layers, each with its own generator, blend mode, and animation. The output is a 320x200 canvas that feeds into the standard spatial mapping pipeline.

**Layer types:**

| Layer Type | Description | Parameters |
|---|---|---|
| **Gradient** | Linear, radial, conic, or multi-stop gradient | Colors, angle, position, scale |
| **Solid** | Single color fill | Color |
| **Noise** | Simplex/Perlin/Worley noise field | Scale, octaves, speed, color mapping |
| **Pattern** | Repeating geometric patterns | Type (stripe, checker, hex, dot), size, rotation |
| **Wave** | Sine/triangle/sawtooth wave patterns | Frequency, amplitude, direction, phase |
| **Particles** | Simple particle emitter | Count, size, speed, lifetime, color |
| **Image** | Static image or GIF | Source, position, scale, tiling |

**Layer properties:**

```
Layer: "Bass Pulse"
├── Type: Radial Gradient
├── Colors: #e135ff → #000000
├── Position: center
├── Scale: 0.5
├── Blend Mode: Screen
├── Opacity: 80%
├── Animation:
│   ├── Scale: oscillate 0.3 → 0.8, period 2s
│   └── Opacity: linked to Audio → Bass (gain: 1.5)
└── Audio Wiring:
    └── Scale → audio.bass (min: 0.2, max: 1.0, smoothing: 0.7)
```

**Audio parameter wiring:**

The visual builder lets you drag audio sources to any layer parameter. This is the killer feature -- no code required to make an effect pulse with the bass.

```
Audio Sources             Layer Parameters
─────────────             ────────────────
[level]        ──────────→ Layer.opacity
[bass]         ──────────→ Layer.scale
[mid]                      Layer.rotation
[treble]       ──────────→ Layer.color_shift
[beat]         ──────────→ Layer.position.y
[beatPhase]                Layer.blur
[spectralFlux]
[harmonicHue]  ──────────→ Layer.hue_offset
```

Each wiring has:
- **Input range** mapping (e.g., bass 0.0-1.0 → scale 0.3-1.5)
- **Smoothing** factor (0 = raw, 1 = heavily smoothed)
- **Response curve** (linear, exponential, logarithmic, step)
- **Invert** toggle

**Code generation:**

The visual builder generates a standard HTML/Canvas effect that can be exported, edited manually, and published. The generated code is clean and readable -- not a black box.

```typescript
// Auto-generated by Hypercolor Visual Builder
// Feel free to edit! This is standard Lightscript code.

import { CanvasEffect } from "@hypercolor/lightscript";

export class MyVisualEffect extends CanvasEffect {
  layers = [
    { type: "gradient", colors: ["#e135ff", "#000000"], mode: "radial", blend: "screen" },
    { type: "noise", scale: 4, speed: 0.3, colors: ["#000", "#80ffea"], blend: "add" },
  ];

  onRender(ctx: CanvasRenderingContext2D, dt: number): void {
    const audio = this.getAudioData();
    // Layer 0: Bass-reactive radial gradient
    this.drawRadialGradient(ctx, {
      scale: 0.3 + audio.bass * 0.7,
      opacity: 0.8,
    });
    // Layer 1: Flowing noise field
    this.drawNoiseField(ctx, {
      offset: this.time * 0.3,
      blend: "lighter",
    });
  }
}
```

### 4.3 Preset Generators

Quick-start presets for common effect patterns. Each preset generates a starting point in the layer compositor that the user can customize.

**Preset categories:**

| Category | Presets |
|---|---|
| **Ambient** | Slow gradient cycle, breathing pulse, aurora, lava lamp, ocean waves |
| **Audio-Reactive** | Spectrum bars, bass pulse, beat flash, waveform, VU meter |
| **Gaming** | Health bar, cooldown indicator, team colors, ambient from screen |
| **Seasonal** | Holiday lights, fireplace, thunderstorm, snowfall, spring bloom |
| **Aesthetic** | Vaporwave, cyberpunk, retrowave, pastel dream, dark forest |

### 4.4 Node-Based Shader Editor (v2/v3)

A more powerful visual programming environment modeled on Blender's Shader Editor and Unreal Engine's Material Editor. Nodes represent operations (math, color, noise, audio input) and edges represent data flow.

**Why defer this:**

1. Building a performant, intuitive node graph editor is a major engineering effort (custom WebGL renderer for the graph, serialization format, undo/redo, copy/paste, grouping, presets)
2. The code it generates (WGSL compute shaders) is harder to hand-edit than the layer compositor's output
3. The target audience (visual thinkers who also want shader-level control) is small in v1
4. Existing tools (Shadertoy, ISF, TouchDesigner) prove the concept but also show the complexity

**Architecture notes for when we build it:**

- Graph editor: Canvas 2D or WebGL rendering of nodes and connections (avoid DOM-based node editors -- they get sluggish at 100+ nodes)
- Graph evaluation: compile node graph → WGSL shader source → standard wgpu pipeline
- Live preview: re-evaluate graph on any node parameter change, hot-swap shader
- Node library: Math (add, multiply, remap, clamp), Color (HSV, blend, gradient), Noise (simplex, worley, FBM), Audio (spectrum, bands, beat), Texture (sample, UV transform), Output (color, alpha)

---

## 5. Effect Templates & Starters

### 5.1 `hypercolor new` Command

```bash
# Interactive mode -- prompts for template and name
hypercolor new effect

# Direct mode
hypercolor new effect my-neon-rain
hypercolor new effect --template canvas-2d my-neon-rain
hypercolor new effect --template webgl-shader cool-plasma
hypercolor new effect --template wgsl-compute cellular-life
hypercolor new effect --template shadertoy ported-shader

# Create in a specific directory
hypercolor new effect --output ./my-effects/ aurora-dream
```

### 5.2 Template Catalog

Each template provides a working, visually interesting starting point -- not an empty skeleton. The generated effect should look good immediately so the author has something to modify rather than starting from zero.

#### `canvas-2d-basic`

**For:** Web developers new to RGB effects
**Output:** Single HTML file (no build step)
**Includes:** Canvas 2D rendering loop, 3 example controls (speed, color, size), tutorial comments explaining every line
**Visual:** Animated color gradient with interactive parameters

```
File generated: effects/custom/my-effect.html (standalone, no deps)
```

#### `canvas-2d-particles`

**For:** Intermediate web developers
**Output:** Single HTML file with inline particle system
**Includes:** Particle class, spatial hashing for performance, gravity/wind forces, audio-reactive spawn rate, 5 controls
**Visual:** Glowing particle fountain that pulses with audio

#### `canvas-2d-audio`

**For:** Developers building audio-reactive effects
**Output:** Single HTML file with full audio API usage
**Includes:** All audio API properties demonstrated, spectrum visualization, beat detection response, mel band display
**Visual:** Spectrum analyzer with beat-reactive background

#### `lightscript-ts`

**For:** TypeScript developers wanting the full Lightscript SDK
**Output:** Vite project with `package.json`, `tsconfig.json`, `vite.config.ts`
**Includes:** `CanvasEffect` subclass with decorators, typed audio API, npm dependency example, HMR setup
**Visual:** Noise-based color field with audio reactivity

#### `webgl-shader`

**For:** Developers with GLSL/shader experience
**Output:** Vite project with Three.js and custom fragment shader
**Includes:** `WebGLEffect` subclass, custom fragment shader with standard uniforms, audio spectrum texture, 4 controls
**Visual:** Plasma effect with audio-reactive distortion

#### `wgsl-fragment`

**For:** Shader programmers targeting the native path
**Output:** `.wgsl` + `.toml` files
**Includes:** Fragment shader with standard uniform block, noise library include, 3 controls
**Visual:** Animated simplex noise with palette cycling

#### `wgsl-compute`

**For:** Advanced shader programmers needing persistent state
**Output:** `.wgsl` + `.toml` files
**Includes:** Compute shader with state buffer, workgroup sizing, cellular automata example
**Visual:** Conway's Game of Life with audio-reactive mutation rate

#### `shadertoy`

**For:** Shadertoy users porting existing shaders
**Output:** `.glsl` + `.toml` files with Shadertoy compatibility header
**Includes:** Compatibility header, example mainImage function, audio extension uniforms, porting guide in comments
**Visual:** Raymarched sphere with iridescent material (demonstrates common Shadertoy patterns)

### 5.3 Template Content Quality

Every template includes:

1. **Tutorial comments** -- Not just "what" but "why." Explain the rendering model, the audio API, the spatial mapping. Comments are written for someone who's never built an LED effect.

   ```typescript
   // The canvas is 320x200 pixels. This is the "virtual screen" that
   // Hypercolor samples to determine LED colors. Your effect draws to
   // this canvas, and the spatial layout engine maps regions of the
   // canvas to physical LED positions on your hardware.
   //
   // Think of it like painting a picture that gets projected onto
   // your LED strips, fans, and RAM sticks.
   ```

2. **Working audio reactivity** -- Every template responds to audio out of the box. The author can see the audio response immediately and understand how to customize it.

3. **Multiple controls** -- Each template demonstrates 3-5 controls of different types (number, combobox, color, boolean) so the author has working examples to copy.

4. **Performance best practices** -- Templates use efficient patterns (ImageData for pixel manipulation, requestAnimationFrame for timing, object pooling for particles). No anti-patterns in starter code.

5. **`hypercolor.toml` metadata** -- Pre-filled with sensible defaults including a generated description, tags, and version number.

---

## 6. Testing & Debugging

### 6.1 Device Simulator

The device simulator renders your effect as it would appear on specific hardware configurations, without requiring that hardware to be connected.

```bash
# Run effect with a specific device simulation
hypercolor dev my-effect.html --simulate "corsair-h150i"
hypercolor dev my-effect.html --simulate "lian-li-strimer-plus"

# List available device simulations
hypercolor simulate --list
```

**Device simulation library:**

| Device | LED Count | Topology | Notes |
|---|---|---|---|
| WLED strip (1m, 60/m) | 60 | Linear strip | Most common WLED config |
| WLED strip (5m, 60/m) | 300 | Linear strip | Long strip, tests color continuity |
| Corsair LL120 fan | 16 | Ring | Dual-ring fan, inner + outer |
| Corsair H150i LCD | 48 | 3x ring | AIO pump head with LCD |
| Lian Li Strimer Plus ATX | 120 | 20x6 matrix | Cable with grid layout |
| Lian Li Strimer Plus GPU | 108 | 27x4 matrix | GPU power cable |
| Razer Huntsman V2 | 132 | 22x6 matrix | Per-key keyboard |
| G.Skill Trident Z5 (2x) | 10 | 2x linear (5 each) | RAM stick LEDs |
| ASUS ROG Strix Z790 | 8 | Irregular | Motherboard accent LEDs |
| Full PC setup | ~500 | Mixed | All above devices combined |

Each simulation includes:
- Accurate LED positions (measured from real hardware)
- Correct color format (RGB vs GRB)
- Brightness characteristics (gamma curve, max brightness)
- Physical dimensions for scale reference

### 6.2 Layout Presets

Pre-configured spatial layouts for common setups. These go beyond single-device simulation to show how effects map across an entire multi-device setup.

```bash
# Switch layout in the dev server
# (also available via the UI dropdown)
hypercolor dev my-effect.html --layout gaming-desk
```

| Layout | Devices | Description |
|---|---|---|
| `minimal` | 1 strip (60 LEDs) | Fastest preview, single-strip testing |
| `gaming-desk` | Monitor backlight + desk strips + keyboard | Typical desk setup |
| `full-tower` | 4 fans + 2 strips + RAM + GPU Strimer + ATX Strimer | Full PC case |
| `studio` | Desk strips + monitor + shelving strips | Content creator setup |
| `room-ambient` | Ceiling strip + wall strips + bias lighting | Room-scale lighting |
| `streamers` | Strimers only (ATX + GPU) | For testing Strimer-specific layouts |

### 6.3 Performance Benchmarking

```bash
# Run performance benchmark on an effect
hypercolor bench effects/custom/my-effect.html

# Output:
# ╭──────────────────────────────────────────────╮
# │  Performance Benchmark: my-effect.html       │
# ├──────────────────────────────────────────────┤
# │  Renderer:    Servo (HTML/Canvas)            │
# │  Resolution:  320x200                        │
# │  Duration:    10 seconds (600 frames)        │
# │                                              │
# │  Frame Time:                                 │
# │    Average:   2.3ms   ████░░░░░░░░  (14%)   │
# │    P95:       3.8ms   ██████░░░░░░  (23%)   │
# │    P99:       8.1ms   █████████░░░  (49%)   │
# │    Max:       12.4ms  ██████████░░  (75%)   │
# │    Budget:    16.6ms  (60fps target)         │
# │                                              │
# │  Memory:                                     │
# │    Start:     12MB                           │
# │    End:       14MB                           │
# │    Peak:      15MB                           │
# │    Growth:    +0.2MB/min (OK)                │
# │                                              │
# │  Draw Calls:  24/frame avg                   │
# │  GC Events:   3 over 10s (low)              │
# │                                              │
# │  Verdict:     PASS (comfortably under budget)│
# ╰──────────────────────────────────────────────╯
```

**Benchmark modes:**

- `--quick`: 3-second benchmark, basic metrics
- `--full`: 30-second benchmark with memory leak detection, GC analysis, and frame time distribution histogram
- `--stress`: Run with all audio bands maxed, rapid control changes, tests worst case
- `--compare <other-effect>`: Side-by-side performance comparison between two effects

### 6.4 Compatibility Testing

Ensuring effects render identically across the Servo path and a reference browser (Chromium).

```bash
# Run compatibility test
hypercolor test-compat effects/community/borealis.html

# What it does:
# 1. Render 60 frames in Servo (headless)
# 2. Render 60 frames in Chromium via Playwright (headless)
# 3. Compare output frames pixel-by-pixel
# 4. Report differences with heatmap visualization
#
# Output:
# borealis.html: 98.7% match (frame 0-59)
#   Frame 12: 96.2% match (worst) -- diff in gradient interpolation
#   Diff heatmap saved to: test-output/borealis-diff-f12.png
```

**Automated compatibility CI:**

```yaml
# Run against the full community effect corpus
hypercolor test-compat effects/community/ --threshold 95
# 210 effects tested
# 203 pass (>95% match)
# 5 minor differences (90-95% match) -- gradient interpolation, font rendering
# 2 failures (<90% match) -- investigate
```

### 6.5 Screenshot Comparison Testing

For regression testing during engine development. Captures golden screenshots and compares against them on every change.

```bash
# Generate golden screenshots for all effects
hypercolor screenshot-golden effects/

# Run regression test
hypercolor test-screenshots effects/
# 16 effects tested, 16 match golden screenshots
# 0 regressions detected
```

**How it works:**

1. Render effect for N frames (configurable, default: frame 0, 30, 60, 120)
2. Capture canvas as PNG
3. Compare against stored golden screenshot using perceptual difference (SSIM, not pixel-exact)
4. Fail if SSIM drops below threshold (default: 0.98)
5. Generate diff visualization showing where changes occurred

### 6.6 Color Accuracy Verification

LEDs don't reproduce colors perfectly. Gamma curves, color space limitations, and brightness caps all affect the final output. The color verification tool helps authors understand how their effect will actually look.

```bash
hypercolor verify-color my-effect.html --device "wled-ws2812b"
```

**What it checks:**

- **Gamma correction** -- Shows how canvas colors map through the device's gamma curve
- **Color gamut** -- Flags colors outside the LED's reproducible gamut (e.g., very saturated blues on WS2812B)
- **Brightness clipping** -- Warns when effect uses near-white colors that will clip on current-limited strips
- **Dark detail** -- Flags low-value colors that may disappear entirely on LEDs with high minimum brightness
- **Color uniformity** -- Shows how adjacent LED colors will look with the device's specific spacing

Output includes a side-by-side preview: "what you designed" vs "what the LEDs will actually show."

---

## 7. Documentation & Learning

### 7.1 Documentation Architecture

```
docs.hypercolor.dev
├── Getting Started
│   ├── Installation
│   ├── Your First Effect (5-minute tutorial)
│   ├── Understanding the Canvas
│   └── Connecting Hardware
│
├── Tutorials (progressive difficulty)
│   ├── 01 - Color Gradient (Canvas 2D basics)
│   ├── 02 - Animated Waves (requestAnimationFrame)
│   ├── 03 - Adding Controls (meta tags)
│   ├── 04 - Audio Reactivity (beat detection)
│   ├── 05 - Particles (physics, object pooling)
│   ├── 06 - WebGL Shaders (Three.js + GLSL)
│   ├── 07 - WGSL Native Shaders (maximum performance)
│   ├── 08 - Compute Shaders (cellular automata)
│   ├── 09 - Publishing Your Effect
│   └── 10 - Advanced Audio (mel bands, chromagram, spectral analysis)
│
├── API Reference
│   ├── Lightscript SDK (auto-generated from TypeScript types)
│   │   ├── CanvasEffect
│   │   ├── WebGLEffect
│   │   ├── Audio API
│   │   ├── Control Decorators
│   │   └── Utility Functions
│   ├── WGSL Standard Library
│   │   ├── noise.wgsl
│   │   ├── color.wgsl
│   │   ├── audio.wgsl
│   │   └── math.wgsl
│   ├── Meta Tag Reference (LightScript-compatible)
│   └── Standard Uniforms
│
├── Cookbook
│   ├── Gradients & Color Cycling
│   ├── Particle Systems
│   ├── Audio-Reactive Patterns
│   ├── Noise & Procedural Generation
│   ├── Porting Shadertoy Effects
│   ├── Performance Optimization
│   ├── Multi-Zone Awareness
│   └── Seasonal / Themed Effects
│
├── Concepts
│   ├── The 320x200 Canvas
│   ├── Spatial Mapping (canvas → LEDs)
│   ├── The Render Loop
│   ├── Audio Pipeline (capture → FFT → bands → effects)
│   ├── Effect Formats (HTML vs WGSL vs GLSL)
│   └── Device Topologies (strips, rings, matrices)
│
└── Examples Gallery
    ├── (every built-in + custom effect with annotated source)
    └── (filterable by: technique, difficulty, audio-reactive, etc.)
```

### 7.2 Interactive Tutorials

The tutorials are not just documentation pages -- they're interactive playgrounds embedded in the browser. Built as a custom component in the docs site.

**Tutorial playground features:**

- **Live code editor** -- Edit effect code on the left, see the result on the right. Changes apply instantly.
- **Step-by-step progression** -- Each tutorial has ~10 steps. Each step highlights specific lines of code and explains what they do.
- **"Try this" challenges** -- After each explanation, a small challenge: "Change the speed to 15 and see what happens" or "Add a second gradient layer."
- **Reset button** -- Revert to the step's starting code if the author gets lost.
- **Fork to dev** -- One-click to export the tutorial code as a full effect project and open it in `hypercolor dev`.

**Example tutorial flow (Tutorial 04: Audio Reactivity):**

```
Step 1: "Here's a simple pulsing circle. It's pretty, but static."
        [Live preview of a breathing circle]

Step 2: "Let's make it respond to music. First, access the audio data."
        [Highlight: const audio = this.getAudioData();]
        [Explain what getAudioData() returns]

Step 3: "Now use the bass energy to control the circle's radius."
        [Highlight: const radius = 50 + audio.bass * 100;]
        [Audio simulator starts playing -- circle pulses]

Step 4: "Try it! Turn on the metronome simulator and watch."
        [Metronome button, BPM slider]

Step 5: "Let's add color response. Use harmonicHue for musically-
         meaningful color changes."
        [Highlight: const hue = audio.harmonicHue * 360;]

...and so on through beat detection, spectral flux, mel bands.
```

### 7.3 API Reference Generation

The API reference is auto-generated from source code to ensure it's always current.

**Lightscript SDK (TypeScript):**
- Generated from TypeScript declaration files using TypeDoc
- Every class, method, property, and decorator is documented
- Includes inline examples for every public API
- Cross-linked (e.g., clicking `getAudioData()` takes you to the `AudioData` type definition)

**WGSL Standard Library:**
- Generated from structured comments in `.wgsl` files:
  ```wgsl
  /// Fractional Brownian motion using simplex noise.
  ///
  /// Stacks multiple octaves of noise for natural-looking terrain,
  /// clouds, and organic textures.
  ///
  /// @param p - 3D sample position
  /// @param octaves - Number of noise layers (1-8, higher = more detail)
  /// @returns Noise value in range [-1, 1]
  ///
  /// @example
  /// let height = fbm_3d(vec3(uv * 4.0, time * 0.1), 6);
  fn fbm_3d(p: vec3<f32>, octaves: i32) -> f32 { ... }
  ```
- Custom doc generator parses these comments and renders them as API pages

### 7.4 Cookbook

The cookbook is a curated collection of copy-pasteable patterns for common effect techniques. Each entry includes:

1. **The pattern** -- A self-contained code snippet that solves one specific problem
2. **When to use it** -- Clear description of the use case
3. **Live preview** -- Embedded interactive preview
4. **Variations** -- 2-3 modifications that show how to customize the pattern
5. **Performance notes** -- Any perf considerations (e.g., "this pattern is O(n^2), limit particle count to 200")

**Example cookbook entries:**

| Pattern | Description |
|---|---|
| Smooth gradient cycle | HSL-based gradient that cycles smoothly without color banding |
| Particle fountain | Gravity-based particles with configurable emission, pooling for perf |
| Bass-reactive pulse | Radial pulse that expands on beat, contracts between beats |
| Trail effect | Partial canvas clear (alpha overlay) for motion trails |
| Spectrum bars | FFT visualization with configurable bar count, peak hold |
| Noise-based aurora | Multi-octave simplex noise with color palette mapping |
| Voronoi cells | Animated Voronoi diagram (both Canvas 2D and WGSL versions) |
| Reaction-diffusion | Gray-Scott model for organic pattern generation |
| Color palette interpolation | Iq-style cosine palettes with HSL fallback |
| Per-zone awareness | Effects that adapt to zone topology (different on strips vs fans) |

### 7.5 Example Gallery

Every built-in and custom effect has a gallery entry with:

- **Live preview** (embedded canvas running the actual effect)
- **Full annotated source code** (with syntax highlighting and inline comments)
- **Control panel** (interactive controls to explore the effect's parameter space)
- **Technical notes** (techniques used, performance characteristics, audio features)
- **"Use as template" button** (fork the effect into a new project)

The gallery is filterable by:
- Rendering path (Canvas 2D, WebGL, WGSL)
- Audio-reactive (yes/no)
- Difficulty (beginner, intermediate, advanced)
- Visual style (ambient, energetic, gaming, nature, abstract)
- Technique (particles, noise, shader, pattern, gradient)

### 7.6 Video Content

Video tutorials supplement the written docs for visual learners:

| Series | Episodes | Format |
|---|---|---|
| **Quick Start** | 3 episodes (5 min each) | Install → first effect → hardware |
| **Effect Masterclass** | 10 episodes (15-20 min each) | Deep dive into each effect technique |
| **Shader Wizardry** | 5 episodes (20 min each) | WGSL shader programming from scratch |
| **Live Builds** | Monthly | Build an effect from scratch on stream, explain decisions |

Videos are embedded in the relevant docs pages (not siloed in a separate video section). The Quick Start series is linked from the README and installation guide.

---

## 8. Effect Publishing

### 8.1 Publishing Pipeline

```bash
# Validate effect metadata and structure
hypercolor validate my-effect.html

# Generate preview images and package for distribution
hypercolor package my-effect.html

# Publish to the Hypercolor effect registry
hypercolor publish my-effect.html
```

### 8.2 `hypercolor validate`

Pre-flight checks before publishing:

```
Validating my-effect.html...

  ✓ Title present: "Neon Rain"
  ✓ Description present (48 chars)
  ✓ Publisher: hyperb1iss
  ✓ Controls valid: 4 controls, all typed correctly
  ✓ Canvas dimensions: 320x200
  ✓ No console errors during 5-second render
  ✓ Frame rate: 60fps (2.1ms avg frame time)
  ✓ Memory stable: no growth over 30 seconds
  ✗ Missing: tags (add at least 2 tags for discoverability)
  ✗ Missing: preview image (run `hypercolor package` to generate)

2 issues found. Fix before publishing.
```

**Validation rules:**

| Check | Required | Description |
|---|---|---|
| Title | Yes | Non-empty, max 50 characters |
| Description | Yes | Non-empty, max 200 characters |
| Author/Publisher | Yes | Non-empty |
| Tags | Yes (2+) | From a controlled vocabulary + freeform |
| Canvas size | Yes | Must be 320x200 (or compatible) |
| No JS errors | Yes | Effect runs without throwing for 5 seconds |
| Frame rate | Warn | Warning if below 30fps, fail if below 15fps |
| Memory stability | Warn | Warning if memory grows >1MB/min |
| Preview image | Required for publish | Auto-generated or manually provided |
| No external requests | Warn | Effects should be self-contained (CDN deps get vendored) |
| License | Recommended | Default: MIT if not specified |
| Version | Yes for updates | Semantic versioning (1.0.0 format) |

### 8.3 `hypercolor package`

Packages an effect for distribution:

```bash
hypercolor package my-effect.html
# Or for a Vite project:
hypercolor package my-effect/

# Output:
# Packaging neon-rain v1.0.0...
#   Building (Vite)... done (1.2s)
#   Generating preview images... done
#   Creating package... done
#
# Output: neon-rain-1.0.0.hyper
#   ├── neon-rain.html         (bundled effect, 48KB)
#   ├── neon-rain-preview.png  (320x200 static preview)
#   ├── neon-rain-preview.webm (3-second animated preview, 128KB)
#   ├── neon-rain-thumb.png    (80x50 thumbnail)
#   └── manifest.toml          (metadata, checksums)
```

**Preview image generation:**

1. Render the effect for 3 seconds
2. Select the "most interesting" frame using a heuristic:
   - Highest color variance (avoid capturing a frame that's mostly black)
   - Prefer frames with audio reactivity visible (if audio-reactive)
   - Avoid transition frames (partial fades, etc.)
3. Generate:
   - Static preview: 320x200 PNG of the selected frame
   - Animated preview: 3-second WebM at 30fps (compressed, typically 50-200KB)
   - Thumbnail: 80x50 PNG for list views

Authors can override the auto-generated preview with a custom image:
```bash
hypercolor package my-effect.html --preview my-custom-preview.png
```

### 8.4 Package Format (`.hyper`)

The `.hyper` format is a gzip-compressed tar archive with a standardized structure:

```
neon-rain-1.0.0.hyper (tar.gz)
├── manifest.toml
│   ├── [effect]
│   │   ├── name = "Neon Rain"
│   │   ├── version = "1.0.0"
│   │   ├── author = "hyperb1iss"
│   │   ├── description = "..."
│   │   ├── tags = ["cyberpunk", "particles", "audio-reactive"]
│   │   ├── format = "html"  # or "wgsl" or "glsl"
│   │   ├── audio_reactive = true
│   │   ├── license = "MIT"
│   │   └── min_hypercolor_version = "0.1.0"
│   ├── [checksums]
│   │   ├── effect = "sha256:..."
│   │   ├── preview = "sha256:..."
│   │   └── animation = "sha256:..."
│   └── [controls]
│       └── (redundant listing for registry indexing without unpacking)
│
├── neon-rain.html           # The effect file (or .wgsl / .glsl)
├── preview.png              # 320x200 static preview
├── preview.webm             # 3-second animated preview
└── thumb.png                # 80x50 thumbnail
```

### 8.5 `hypercolor publish`

Publishing pushes the packaged effect to the Hypercolor Effect Registry.

```bash
hypercolor publish neon-rain-1.0.0.hyper

# First-time setup:
# You need a Hypercolor account. Run `hypercolor auth login` first.
#
# Publishing neon-rain v1.0.0...
#   Authenticating... OK (hyperb1iss)
#   Uploading package (52KB)... done
#   Registry validation... passed
#   Published! https://effects.hypercolor.dev/hyperb1iss/neon-rain
```

**Registry features:**

- **Namespaced packages** -- `author/effect-name` prevents name collisions
- **Semantic versioning** -- Publish 1.0.0, then 1.0.1, then 1.1.0. Users can pin versions or follow latest.
- **Update notifications** -- The Hypercolor daemon checks for effect updates periodically (opt-in). Users see "2 effect updates available" in the UI.
- **Download counts** -- Public download stats for discoverability ranking
- **User ratings** -- 5-star ratings with optional text reviews
- **Dependency tracking** -- If an effect requires a specific Hypercolor version, the registry enforces compatibility
- **Flagging/moderation** -- Community flagging for effects that don't work or contain inappropriate content

### 8.6 Version Management

```bash
# Bump version and publish
hypercolor publish --bump patch    # 1.0.0 → 1.0.1
hypercolor publish --bump minor    # 1.0.1 → 1.1.0
hypercolor publish --bump major    # 1.1.0 → 2.0.0

# Unpublish a specific version (within 72-hour window)
hypercolor unpublish neon-rain@1.0.1

# Deprecate (soft) -- shows warning but still installable
hypercolor deprecate neon-rain@1.0.0 --message "Use v2.0.0, major rewrite"
```

### 8.7 Effect Installation (User Side)

```bash
# Install an effect from the registry
hypercolor install hyperb1iss/neon-rain

# Install a specific version
hypercolor install hyperb1iss/neon-rain@1.2.0

# Update all installed effects
hypercolor update effects

# Browse effects from the CLI
hypercolor search "audio reactive cyberpunk"

# List installed effects
hypercolor list effects --installed
```

Effects install to `effects/installed/<author>/<name>/` and appear immediately in the effect browser (web UI, TUI, CLI).

---

## 9. AI-Assisted Effect Creation

### 9.1 The Vision

Natural language to running LEDs. "Generate a calm ocean aurora effect" produces a working effect that you can immediately preview, tweak, and publish. AI doesn't replace the authoring tools -- it accelerates them.

### 9.2 MCP Integration

Hypercolor exposes an MCP server that lets AI assistants interact with the effect engine directly:

**MCP tools:**

| Tool | Description |
|---|---|
| `create_effect` | Generate a new effect from a natural language description |
| `modify_effect` | Modify an existing effect's code or parameters |
| `preview_effect` | Capture a screenshot of the current effect output |
| `set_control` | Change an effect's control value |
| `get_audio_state` | Read current audio analysis data |
| `list_effects` | List available effects with metadata |
| `get_effect_source` | Read an effect's source code |
| `run_benchmark` | Run performance benchmark on an effect |
| `get_layout` | Get current spatial layout configuration |

**Example MCP interaction:**

```
User: "Make me an effect that looks like rain on a window at night"

AI (via MCP):
  1. create_effect({
       description: "rain on window at night",
       format: "canvas-2d",
       template: "canvas-2d-particles"
     })
  2. [Generates effect code with:
       - Dark blue-black background
       - Transparent rain drop particles falling with slight randomization
       - Occasional "splashes" when drops hit the bottom
       - Subtle lightning flashes (random full-canvas brightening)
       - Audio-reactive: lightning frequency increases with treble energy]
  3. preview_effect() → captures screenshot
  4. "Here's your rain effect. I've made the drops fall at varying
      speeds with slight wind drift. Lightning flashes respond to
      treble frequencies in your audio. Would you like me to adjust
      the drop density or add puddle reflections?"
```

### 9.3 Prompt-to-Shader Pipeline

For WGSL effects, the AI workflow looks like:

```
Natural language prompt
    → LLM generates WGSL shader + TOML metadata
    → Shader validation (naga)
    → If valid: hot-load into dev server, show preview
    → If invalid: feed errors back to LLM for correction
    → User sees result, provides feedback
    → LLM modifies shader based on feedback
    → Iterate until satisfied
```

**Key design decisions:**

1. **Always show the code.** AI-generated effects are not opaque -- the source is right there, fully editable. The AI is a starting point, not a black box.
2. **Leverage existing templates.** The AI doesn't generate from scratch every time -- it starts from the closest template and modifies it. This ensures structural correctness and best practices.
3. **Iterative refinement.** "Make it more purple." "Slow down the waves." "Add a bass-reactive pulse." Each instruction modifies the existing effect, preserving what works.
4. **Full context.** The MCP server provides the AI with: the current effect source, the audio API reference, the control system reference, available shader library functions, and performance constraints. The AI has everything it needs to generate correct code.

### 9.4 AI-Powered Parameter Tuning

Beyond code generation, AI can help tune existing effects:

```
User: "This effect looks too harsh on my keyboard. Soften it."

AI analyzes:
  1. get_effect_source() → reads the current effect code
  2. Identifies brightness/contrast parameters
  3. set_control("glowIntensity", 40)  # was 80
  4. set_control("colorSpeed", 15)      # was 30
  5. preview_effect() → captures result
  6. "I've reduced the glow intensity and slowed the color cycling.
      The effect should feel calmer on your keyboard now. Want me
      to also soften the color palette?"
```

### 9.5 AI Effect Co-Pilot (Editor Integration)

For developers writing effects in their editor (VS Code, Neovim), the AI acts as a co-pilot:

- **Inline suggestions** -- As you type shader code, the AI suggests completions based on the Hypercolor API, WGSL standard library, and common patterns
- **Error explanations** -- When a shader fails to compile, the AI explains the error in plain language and suggests fixes
- **Documentation lookups** -- Ask "what does `fbm_3d` do?" and get the answer inline
- **Performance advice** -- "This loop will be slow on integrated GPUs. Consider using a lookup texture instead."

This integrates through the existing MCP protocol -- any AI assistant with MCP support (Claude, etc.) can serve as the co-pilot.

---

## 10. Persona Scenarios

### 10.1 Bliss -- The Expert

**Profile:** Principal engineer. Writes WGSL shaders in Neovim. Has a full PC case with 500+ LEDs across PrismRGB, WLED, and OpenRGB devices. Wants maximum control and minimum friction.

**Workflow:**

```
1. $ hypercolor new effect --template wgsl-fragment aurora-v2
   → Scaffolds aurora-v2.wgsl + aurora-v2.toml in effects/native/

2. Opens aurora-v2.wgsl in Neovim (wgsl.vim syntax, naga LSP for completions)

3. In a terminal split:
   $ hypercolor dev effects/native/aurora-v2.wgsl --hardware --layout full-tower
   → Dev server starts on :9421
   → Hardware bridge connects to running daemon
   → All 500 LEDs now show the shader output

4. Edits shader in Neovim. Saves.
   → File watcher triggers (< 10ms)
   → Shader recompiled via naga (< 50ms)
   → Pipeline hot-swapped
   → Browser preview updates (< 50ms)
   → Hardware LEDs update on next frame (< 16ms)
   → Total save-to-LEDs: ~100ms

5. Hits a type error. Terminal shows:
   error[E0001]: type mismatch in aurora-v2.wgsl:42:18
   → Neovim LSP shows the same error inline with red underline

6. Fixes error. Saves. Shader is back instantly. No restart.

7. Opens browser preview briefly to check the virtual layout viewer.
   Adjusts Strimer zone position in the spatial editor.

8. Runs performance benchmark:
   $ hypercolor bench effects/native/aurora-v2.wgsl
   → 0.3ms/frame. 3000+ fps theoretical. Pass.

9. Packages and publishes:
   $ hypercolor package effects/native/aurora-v2.wgsl
   $ hypercolor publish aurora-v2-1.0.0.hyper
```

**What makes this work for Bliss:**
- Terminal-native workflow (no mandatory browser)
- Sub-100ms save-to-LEDs latency
- LSP integration for editor of choice
- Direct hardware output during development
- No unnecessary abstraction layers

### 10.2 Yuki -- The Artist

**Profile:** Digital artist and illustrator. Creates custom color palettes for their art. Wants LED lighting that matches their aesthetic. No programming experience, comfortable with Photoshop-like tools.

**Workflow:**

```
1. Opens Hypercolor web UI → Effect Builder tab

2. Starts from the "Ambient" preset → "Slow Gradient Cycle"
   → Sees a gentle gradient cycling through default colors

3. Replaces colors with their custom palette:
   → #2D1B69 (deep purple)
   → #E8B4B8 (dusty rose)
   → #4ECDC4 (teal)
   → #FFE66D (warm yellow)
   → Uses the gradient editor (drag stops, adjust positions)

4. Adds a second layer: "Noise" type
   → Sets noise scale, speed, and maps it to the teal color
   → Blend mode: Overlay
   → Opacity: 30%
   → The effect now has organic movement over the gradient

5. Wires audio:
   → Drags [bass] → Layer 1 opacity (range: 50-100%)
   → Drags [harmonicHue] → Layer 1 hue offset (range: -30 to +30)
   → Turns on the audio simulator (Metronome, 120 BPM)
   → Watches the effect pulse subtly with the beat

6. Checks the virtual LED viewer:
   → Selects "gaming-desk" layout
   → Sees how the gradient maps across monitor backlight + desk strips
   → Satisfied with the color distribution

7. Clicks "Export as Effect"
   → The builder generates clean Lightscript code
   → Fills in name: "Yuki's Studio Palette"
   → Fills in description: "Dusty rose and teal gradient with organic noise"
   → Auto-generates preview images

8. Clicks "Publish"
   → Effect goes live on the registry
```

**What makes this work for Yuki:**
- Zero code required
- Visual palette tools (color pickers, gradient editor)
- Drag-and-drop audio wiring
- Immediate visual feedback
- One-click export and publish

### 10.3 Jake -- The Consumer

**Profile:** Gamer. Bought RGB hardware because it looks cool. Has no interest in creating effects. Wants to browse, install, and switch between effects with minimal effort.

**Workflow:**

```
1. Installs Hypercolor, runs the daemon, opens the web UI

2. Goes to the Effect Browser
   → Sees curated "Featured" section with popular effects
   → Browses "Gaming" category
   → Finds "Cyberpunk Pack" (5 themed effects by a community creator)
   → Animated preview shows each effect in the pack

3. Clicks "Install" → pack downloads instantly

4. Selects "Cyberpunk Neon Rain" from the pack
   → Effect starts on all connected devices immediately
   → Controls panel shows: Speed, Density, Color Theme

5. Adjusts "Color Theme" from "Pink/Blue" to "Green/Purple"
   → LEDs update in real time

6. Saves as a profile: "Gaming Mode"

7. Later, creates a "Chill Mode" profile with "Ocean Waves" effect
   → Sets up schedule: Gaming Mode 6pm-12am, Chill Mode 12am-6am

8. Gets a notification: "Cyberpunk Pack v1.2 available — adds new
   'Glitch' effect"
   → One-click update
```

**What makes this work for Jake:**
- Curated marketplace with animated previews
- One-click install
- Simple control panel (no code visible)
- Profile system with scheduling
- Update notifications

### 10.4 Sam -- The Musician

**Profile:** Electronic music producer. Wants audio-reactive lighting synced to specific frequency bands for their studio during production sessions and live performances.

**Workflow:**

```
1. $ hypercolor new effect --template canvas-2d-audio studio-viz
   → Creates a new audio-reactive effect project

2. Opens the dev server:
   $ hypercolor dev studio-viz.html --audio-source system

3. Starts playing a track in their DAW. The dev server captures system audio.
   → Default template shows a spectrum visualizer

4. Customizes the audio response:
   → Assigns kick drum frequency range (40-80 Hz) to a radial pulse
   → Assigns snare range (200-400 Hz) to a strobe flash
   → Assigns hi-hats (8-12 kHz) to sparkle particles
   → Uses melBands[] for fine-grained frequency control
   → Uses beatPhase for synchronized animation timing

5. Tests with different tracks to ensure the response is genre-appropriate
   → Uses the audio simulator's "File" mode to load their own tracks
   → Adjusts sensitivity per band

6. Checks latency:
   → Audio capture → FFT → effect response should be < 20ms
   → Uses the performance profiler's audio latency measurement

7. For live performance:
   → Configures a minimal "performance mode" layout (no browser needed)
   → Maps MIDI controller to effect parameters (future feature)
   → Effect runs via daemon, controlled via the TUI on a secondary display

8. Publishes to the registry with tags: ["audio", "studio", "performance"]
```

**What makes this work for Sam:**
- Full audio API access with per-band control
- System audio capture (capture from DAW output)
- Audio file testing mode for reproducible development
- Low-latency audio pipeline
- Performance-mode operation (headless daemon + TUI)

### 10.5 Alex -- The Web Developer (30-Minute Challenge)

**Profile:** Frontend developer (React, TypeScript). Has a WLED strip behind their monitor. Has never built an RGB effect. Can they create and deploy a custom effect in 30 minutes?

**Timeline:**

```
Minute 0-2: Install and Setup
  $ cargo install hypercolor-cli  # or download binary
  $ hypercolor daemon &           # start the daemon
  # WLED strip auto-discovered via mDNS

Minute 2-5: Scaffold
  $ hypercolor new effect --template canvas-2d-basic my-first-effect
  $ hypercolor dev my-first-effect.html
  # Browser opens at localhost:9421
  # Sees a working color gradient on screen AND on their WLED strip

Minute 5-10: Understand the Canvas
  # Reads the tutorial comments in the generated code:
  # "The canvas is 320x200 pixels... effects draw to this canvas...
  #  the spatial layout engine maps regions to LED positions..."
  #
  # Modifies the gradient colors. Saves. Sees it update instantly.
  # "Oh, this is just a normal HTML canvas. I know this."

Minute 10-15: Add Controls
  # Copies the example <meta> tag, adds their own:
  <meta property="waveFreq" label="Wave Frequency" type="number"
        min="1" max="20" default="5" />
  # Uses `waveFreq` variable in the render function
  # Slider appears in the control panel. Adjusting it changes the effect.

Minute 15-20: Make it Audio-Reactive
  # Reads Tutorial 04 summary in the template comments
  # Adds: if (window.engine?.audio) { bass = window.engine.audio.bass; }
  # Turns on the audio simulator in the dev UI
  # Effect now pulses with the simulated beats

Minute 20-25: Polish
  # Adjusts colors to match their desk aesthetic
  # Tweaks the audio sensitivity
  # Adds a second visual element (particles or trails)
  # Checks the virtual LED viewer to see how it maps to their strip

Minute 25-28: Package
  $ hypercolor validate my-first-effect.html
  # Adds missing description and tags
  $ hypercolor package my-first-effect.html
  # Preview images generated automatically

Minute 28-30: Ship
  $ hypercolor publish my-first-effect-1.0.0.hyper
  # Published!
  # "That was way easier than I expected."
```

**What makes the 30-minute timeline achievable:**

1. Auto-discovery of WLED device (no manual configuration)
2. Template includes working code with tutorial comments (not empty)
3. The HTML/Canvas format is immediately familiar to web developers
4. Hot-reload means every change is visible instantly -- no "did my change work?" uncertainty
5. Audio simulator means no dependency on having music playing
6. Validation tells you exactly what's missing before publishing
7. Preview images are auto-generated (no design tools needed)

**What could slow it down:**
- Cargo build time on first install (mitigated by providing pre-built binaries)
- WLED not being auto-discovered (firewall, mDNS issues)
- Unfamiliar with Canvas 2D API (mitigated by tutorial comments and working examples)

---

## Implementation Priorities

### Phase 1: Core Dev Experience (Ship with v0.1)

The minimum viable authoring experience. Everything needed for an expert to create and test effects locally.

| Feature | Priority | Complexity |
|---|---|---|
| `hypercolor dev` command (file watcher + browser preview) | P0 | Medium |
| HTML/Canvas hot-reload (iframe reload on save) | P0 | Low |
| WGSL shader hot-swap (naga compile + pipeline swap) | P0 | Medium |
| Auto-generated control panel from `<meta>` tags | P0 | Low |
| Virtual LED layout viewer (basic -- single strip) | P0 | Medium |
| `hypercolor new effect` with 3 templates (canvas-2d-basic, wgsl-fragment, lightscript-ts) | P0 | Low |
| Shader error overlay (browser) | P0 | Low |
| Performance metrics (frame time display) | P1 | Low |

### Phase 2: Rich Development (v0.2)

The experience that makes development genuinely pleasant.

| Feature | Priority | Complexity |
|---|---|---|
| Hardware bridge (dev server → daemon → LEDs) | P0 | Medium |
| Audio simulator (metronome, sweep, file modes) | P0 | Medium |
| Layout presets (5+ presets) | P1 | Low |
| Full Lightscript SDK with Vite plugin | P1 | High |
| `hypercolor bench` command | P1 | Medium |
| Device simulator library (10+ devices) | P1 | Medium |
| WGSL include system (`lib/noise.wgsl`, etc.) | P1 | Low |
| Terminal error output (ANSI colors) | P1 | Low |
| All 8 templates | P2 | Medium |

### Phase 3: Publishing & Community (v0.3)

The ecosystem layer.

| Feature | Priority | Complexity |
|---|---|---|
| `hypercolor validate` | P0 | Low |
| `hypercolor package` (with preview generation) | P0 | Medium |
| `.hyper` package format | P0 | Low |
| Effect registry (API + storage) | P1 | High |
| `hypercolor publish` / `hypercolor install` | P1 | Medium |
| Compatibility test suite (210 community effects) | P1 | High |
| Screenshot comparison testing | P2 | Medium |
| Color accuracy verification | P2 | Medium |

### Phase 4: Accessibility & AI (v0.4+)

Broadening the creator audience.

| Feature | Priority | Complexity |
|---|---|---|
| Visual layer compositor | P1 | High |
| Preset generators (ambient, audio, gaming) | P1 | Medium |
| Interactive tutorial system | P1 | High |
| Docs site (auto-generated API reference + cookbook) | P1 | Medium |
| MCP server for AI-assisted creation | P2 | Medium |
| AI prompt-to-shader pipeline | P2 | High |
| Node-based shader editor | P3 | Very High |
| Shadertoy compatibility layer | P2 | Medium |

---

## Open Questions

1. **Registry hosting.** Self-hosted? GitHub-based (effect repos as repos)? A dedicated service? The registry is a long-term commitment -- it needs to be reliable and funded. Consider starting with a GitHub-based approach (effects as repos in a GitHub org, metadata indexed by a static site) and migrating to a dedicated service when the community outgrows it.

2. **Effect sandboxing.** HTML effects run arbitrary JavaScript. In the Servo renderer, this is somewhat sandboxed, but effects could still do things like infinite loops or excessive memory allocation. Do we need a resource limiter? (Probably yes -- per-effect memory cap, frame time watchdog that kills effects that consistently exceed budget.)

3. **Multi-pass shader effects.** Shadertoy's Buffer A/B/C/D system enables feedback loops and multi-pass rendering (fluid sim, blur passes, etc.). Supporting this in the WGSL path requires a multi-pass compute pipeline with ping-pong buffers. Worth the complexity for v1? (Probably defer to v2, but design the state buffer system to be extensible to multiple buffers.)

4. **Effect dependencies.** Can effects depend on shared libraries? (e.g., a popular noise library, a physics engine.) Or should every effect be fully self-contained? Self-contained is simpler for distribution but leads to duplicated code. Consider a "vendor" approach -- effects can declare dependencies that are bundled at package time.

5. **Versioned API contract.** The Lightscript audio API surface is large (30+ properties). How do we handle API evolution without breaking existing effects? Semantic versioning of the API itself, with effects declaring which API version they target. The runtime provides backwards-compatible shims for older versions.

6. **Visual builder code quality.** Auto-generated code from the visual builder needs to be clean enough that developers can take over and hand-edit. This is hard -- generated code tends to be verbose and structural. Invest in a code generation layer that produces idiomatic, well-commented output.
