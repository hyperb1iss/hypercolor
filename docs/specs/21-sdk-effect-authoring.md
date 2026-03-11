# Spec 21 -- SDK Effect Authoring API

> Delightful effect authoring for `@hypercolor/sdk` -- write a shader or a draw function, declare controls once, ship.

**Status:** Draft
**Package:** `@hypercolor/sdk` (`sdk/packages/core/`)
**Supersedes:** Current decorator-based pattern (reflect-metadata + manual 5-method override)

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Design Philosophy](#2-design-philosophy)
3. [Progressive Tiers](#3-progressive-tiers)
4. [Shader Effects: `effect()`](#4-shader-effects-effect)
5. [Canvas Effects: `canvas()`](#5-canvas-effects-canvas)
6. [GLSL Pragma Effects (Tier 0)](#6-glsl-pragma-effects-tier-0)
7. [Controls System](#7-controls-system)
8. [Palette as a First-Class Concept](#8-palette-as-a-first-class-concept)
9. [Audio Integration](#9-audio-integration)
10. [Uniform Mapping](#10-uniform-mapping)
11. [Build-Time Metadata Extraction](#11-build-time-metadata-extraction)
12. [Runtime Control Flow](#12-runtime-control-flow)
13. [Type System](#13-type-system)
14. [HTML Output Contract](#14-html-output-contract)
15. [Migration Guide](#15-migration-guide)
16. [Implementation Plan](#16-implementation-plan)
17. [Open Questions](#17-open-questions)

---

## 1. Problem Statement

The current SDK requires every control to be declared in **five separate locations**, plus a sixth from the decorator:

```typescript
// 1. Decorator — metadata for build-time extraction
@NumberControl({ label: 'Speed', min: 1, max: 10, default: 5 })
speed!: number

// 2. initializeControls() — reads initial value from window
this.speed = getControlValue('speed', 5)

// 3. getControlValues() — normalizes for the render loop
speed: normalizeSpeed(getControlValue('speed', 5))

// 4. createUniforms() — registers the GLSL uniform
this.registerUniform('iSpeed', 1.0)

// 5. updateUniforms() — pushes control value to GPU
this.setUniform('iSpeed', c.speed)
```

This affects all 15 existing effects identically — none has any custom JS frame logic. Every effect follows the same 5-method boilerplate with only the control names and shader source varying. The boilerplate is 100% derivable from the controls and shader.

| Issue | Impact |
|-------|--------|
| **5-place duplication** | Adding a control requires editing 5 methods. 86 lines for a simple effect. |
| **Manual name mapping** | `speed` (JS) → `iSpeed` (GLSL) is convention, not enforced. Typos cause silent failures. |
| **Ad-hoc normalization** | `normalizeSpeed()`, `comboboxValueToIndex()` called manually. Easy to forget. |
| **`reflect-metadata`** | 15KB runtime for build-time-only metadata. Dead-end TC39 path. |
| **Build-time execution hack** | Must execute effect module with fake DOM to extract metadata. |
| **`!` assertions everywhere** | `speed!: number` because TypeScript can't verify decorator init. |
| **Separate controls interface** | `MeteorControls` mirrors class properties with different types. |

---

## 2. Design Philosophy

Learned from the best creative coding environments:

| Insight | Source | Application |
|---------|--------|-------------|
| Parameters live in the file | ISF | `#pragma control` in GLSL, or inline in the `effect()` call |
| One function, everything works | Shadertoy | `effect(name, shader, controls)` — that's the whole API |
| Value shape = widget type | Leva / dat.GUI | `[1, 10, 5]` is a slider. `string[]` is a combobox. |
| Same API, no ceiling | p5.js | Tier 0 → Tier 3 is additive, never a rewrite |
| Magic names that just work | Shadertoy | `speed` auto-normalizes. `palette` auto-indexes. |
| The draw function IS the effect | p5.js | `(ctx, time, controls) => {}` for canvas. No class. |

### Core Principles

1. **Single declaration.** Each control defined exactly once. Name, type, range, default, uniform, normalization — all in one place.
2. **Shape is type.** `[1, 10, 5]` is a number slider. `['Fire', 'Ice']` is a combobox. `'#ff6ac1'` is a color picker. No `type:` field needed.
3. **Canvas stays canvas.** The `CanvasRenderingContext2D` is passed directly. No wrapper, no helpers bolted on, no "p5.js at home."
4. **Palette is a function.** In canvas: `palette(0.5)` returns a CSS color string. In shaders: auto-mapped to `int` index. Same control, surface-appropriate output.
5. **Audio is a function call.** `audio()` returns data or null. No config flag for the common case.
6. **Static extraction.** Build tooling reads metadata without executing effect code.
7. **Same HTML output.** The compiled artifact is identical regardless of which tier authored it.

---

## 3. Progressive Tiers

```
CANVAS EFFECTS                          SHADER EFFECTS
──────────────────                      ──────────────────

Tier 0: Plain HTML                      Tier 0: Single .glsl file
  <canvas> + <script>                     #pragma hypercolor controls
  Zero toolchain                          Self-contained shader
  ↓                                       ↓
Tier 1: canvas()                        Tier 1: effect()
  One function call                       One function call
  Draw callback                           Shader + controls object
  ↓                                       ↓
Tier 2: canvas() + factory              Tier 2: effect() + hooks
  Setup + draw with closure               setup() + frame() for
  State between frames                    computed uniforms
  ↓                                       ↓
Tier 3: CanvasEffect class              Tier 3: WebGLEffect class
  Full OOP escape hatch                   Full OOP escape hatch
  Multi-canvas, custom loop               Multi-pass, custom pipeline
        ↓                                       ↓
        └──────────┬───────────────────────────┘
                   ▼
          HTML file with <meta> tags
                   ↓
          Servo renderer → LEDs
```

Both paths converge to the same HTML output. Both paths have the same progressive disclosure. Each speaks its own language — canvas effects feel like canvas, shader effects feel like shaders.

---

## 4. Shader Effects: `effect()`

### 4.1 Tier 1 — Minimal (Covers 95% of Effects)

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

const PALETTES = ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk']

export default effect('Meteor Storm', shader, {
    speed:       [1, 10, 5],        // [min, max, default] → slider
    density:     [10, 100, 50],
    trailLength: [10, 100, 60],
    glow:        [10, 100, 65],
    palette:     PALETTES,          // string[] → combobox
})
```

9 lines. No boilerplate methods. No `reflect-metadata`. No `!` assertions.

**How it works:**

The value shape determines the control type:

| Shape | Control | Example |
|-------|---------|---------|
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `string[]` | Combobox | `palette: ['Fire', 'Ice']` |
| `boolean` | Toggle | `invert: false` |
| `'#rrggbb'` | Color picker | `accent: '#ff6ac1'` |

Labels are auto-derived from keys: `trailLength` → `"Trail Length"`.

The `speed` key is magic — it auto-applies `normalizeSpeed()`. The control named `palette` auto-applies `comboboxValueToIndex()`.

### 4.2 Tier 2 — Explicit Factories (When You Need Custom Labels)

```typescript
import { effect, num, combo, toggle } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Voronoi Glass', shader, {
    speed:    num('Speed', [1, 10], 5),
    scale:    num('Cell Size', [10, 100], 50),
    edgeGlow: num('Edge Glow', [10, 100], 70, { tooltip: 'Brightness of cell edges' }),
    mode:     combo('Distance', ['Euclidean', 'Manhattan', 'Chebyshev']),
    palette:  combo('Palette', PALETTES),
    invert:   toggle('Invert', false),
})
```

Explicit factories when you need: custom labels, tooltips, non-default normalization, or custom uniform names.

### 4.3 Tier 2.5 — With Hooks (Computed Uniforms)

```typescript
export default effect('Complex Thing', shader, {
    speed:   [1, 10, 5],
    palette: PALETTES,
}, {
    audio: true,

    setup(ctx) {
        ctx.registerUniform('iPhase', 0.0)
        ctx.registerUniform('iComplexity', [1.0, 0.5, 0.25])
    },

    frame(ctx, time) {
        const phase = Math.sin(time * ctx.controls.speed) * 0.5 + 0.5
        ctx.setUniform('iPhase', phase)
    },
})
```

The 4th argument is the "advanced bag" — `setup`, `frame`, `audio`, custom vertex shader. 95% of effects never touch it.

### 4.4 Function Signature

```typescript
function effect(
    name: string,
    shader: string,
    controls: ControlMap,
    options?: {
        description?: string          // default: auto-generated from name
        author?: string               // default: 'Hypercolor'
        audio?: boolean               // default: false
        vertexShader?: string         // default: fullscreen quad
        setup?: (ctx: ShaderContext) => void | Promise<void>
        frame?: (ctx: ShaderContext, time: number) => void
    }
): Effect
```

---

## 5. Canvas Effects: `canvas()`

### 5.1 Tier 1 — Stateless Draw (Most Canvas Effects)

```typescript
import { canvas } from '@hypercolor/sdk'

export default canvas('Glow Particles', {
    speed: [1, 10, 5],
    count: [10, 500, 100],
    glow:  [10, 100, 60],
    palette: ['SilkCircuit', 'Fire', 'Aurora'],

}, (ctx, time, { speed, count, glow, palette }) => {
    ctx.clearRect(0, 0, 320, 200)

    for (let i = 0; i < count; i++) {
        const x = Math.sin(time * speed + i * 0.7) * 140 + 160
        const y = Math.cos(time * speed * 0.8 + i * 1.1) * 80 + 100
        const r = 2 + Math.sin(time + i) * glow * 0.02

        ctx.fillStyle = palette(i / count)
        ctx.beginPath()
        ctx.arc(x, y, r, 0, Math.PI * 2)
        ctx.fill()
    }
})
```

The draw function receives:
- `ctx` — the raw `CanvasRenderingContext2D`. Unmodified. Unwrapped. The real thing.
- `time` — seconds elapsed (same as `iTime` in shaders)
- `controls` — resolved values, already normalized, ready to destructure

`palette` is a **function** in canvas context — `palette(0.5)` returns a CSS color string. Because in canvas-land you need colors for `fillStyle`, not integer indices for GLSL.

### 5.2 Tier 2 — Stateful (Factory Pattern)

For effects that maintain state between frames (particles, physics, trails):

```typescript
export default canvas('Firefly Meadow', {
    count: [10, 200, 80],
    speed: [1, 10, 5],

}, () => {
    // Setup — runs once. This outer function creates the closure.
    const flies = Array.from({ length: 200 }, () => ({
        x: Math.random() * 320,
        y: Math.random() * 200,
        phase: Math.random() * Math.PI * 2,
    }))

    // Return the draw function — runs every frame
    return (ctx, time, { count, speed }) => {
        ctx.fillStyle = 'rgba(0, 0, 0, 0.1)'
        ctx.fillRect(0, 0, 320, 200)

        for (let i = 0; i < count; i++) {
            const f = flies[i]
            f.x += Math.sin(f.phase + time * speed) * 0.5
            f.y += Math.cos(f.phase * 1.3 + time * speed) * 0.3

            const glow = Math.sin(time * 2 + f.phase) * 0.5 + 0.5
            ctx.globalAlpha = glow
            ctx.fillStyle = '#50fa7b'
            ctx.fillRect(f.x, f.y, 2, 2)
        }
        ctx.globalAlpha = 1
    }
})
```

**Detection:** The runtime checks `fn.length` (number of declared parameters):
- `fn.length >= 1` → **stateless** draw function (called every frame)
- `fn.length === 0` → **stateful** factory (invoked once, must return a draw function)

This works for the overwhelmingly common case. However, `Function.length` can be 0 for functions using rest params (`...args`) or default values. For edge cases where the runtime can't distinguish, use `canvas.stateful()`:

```typescript
// Explicit factory — no ambiguity
export default canvas.stateful('Fireflies', controls, () => {
    const state = initState()
    return (ctx, time, { count, speed }) => { /* draw */ }
})
```

This is the p5.js `setup/draw` split — but it's just functions. No classes, no `this`, no lifecycle methods. State lives in closures.

### 5.3 Audio-Reactive Canvas

```typescript
import { canvas, audio } from '@hypercolor/sdk'

export default canvas('Waveform', {
    palette:   ['SilkCircuit', 'Cyberpunk', 'Aurora'],
    thickness: [1, 20, 4],

}, (ctx, time, { palette, thickness }) => {
    const a = audio()
    ctx.clearRect(0, 0, 320, 200)

    if (a) {
        ctx.strokeStyle = palette(a.bass)
        ctx.lineWidth = thickness * a.level
        ctx.beginPath()
        ctx.moveTo(0, 100)
        for (let x = 0; x < 320; x++) {
            const bin = Math.floor(x / 320 * a.spectrum.length)
            ctx.lineTo(x, 100 - a.spectrum[bin] * 80)
        }
        ctx.stroke()
    }
})
```

No `audioReactive: true` flag. No uniform registration. Call `audio()` when you want data. Returns `AudioData` or `null`. Pull model — the effect decides when it needs audio.

### 5.4 Function Signature

```typescript
type DrawFn = (ctx: CanvasRenderingContext2D, time: number, controls: ResolvedControls) => void
type FactoryFn = () => DrawFn

// Stateless: draw function called every frame
function canvas(
    name: string,
    controls: ControlMap,
    draw: DrawFn,
    options?: CanvasOptions
): Effect

// Stateful: factory returns draw function (detected via fn.length === 0)
function canvas(
    name: string,
    controls: ControlMap,
    factory: FactoryFn,
    options?: CanvasOptions
): Effect

// Explicit stateful: bypasses arity detection entirely
canvas.stateful(
    name: string,
    controls: ControlMap,
    factory: FactoryFn,
    options?: CanvasOptions
): Effect

interface CanvasOptions {
    description?: string
    author?: string
    width?: number       // default: 320
    height?: number      // default: 200
}
```

---

## 6. GLSL Pragma Effects (Tier 0)

The shader IS the effect. One file. Self-contained.

```glsl
#version 300 es
#pragma hypercolor "Meteor Storm" by "Hypercolor"
#pragma hypercolor audio

#pragma control speed       "Speed"     float(1, 10) = 5
#pragma control density     "Density"   float(10, 100) = 50
#pragma control trailLength "Trail"     float(10, 100) = 60
#pragma control glow        "Glow"      float(10, 100) = 65
#pragma control palette     "Palette"   enum("SilkCircuit", "Fire", "Ice", "Aurora", "Cyberpunk")

precision highp float;
out vec4 fragColor;

// These are auto-declared from #pragma control:
//   uniform float iSpeed;        (normalized via speed curve)
//   uniform float iDensity;
//   uniform float iTrailLength;
//   uniform float iGlow;
//   uniform int iPalette;
//
// These are always available:
//   uniform float iTime;
//   uniform vec2 iResolution;
//   uniform vec2 iMouse;
//
// These are available because of `#pragma hypercolor audio`:
//   uniform float iAudioBass, iAudioMid, iAudioTreble, ...

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    // ... your shader ...
    fragColor = vec4(col, 1.0);
}
```

### 6.1 Pragma Syntax

```
#pragma hypercolor "Effect Name" by "Author"
#pragma hypercolor "Effect Name" by "Author" : "Description text"
#pragma hypercolor audio

#pragma control <key> "<label>" float(<min>, <max>) = <default>
#pragma control <key> "<label>" int(<min>, <max>) = <default>
#pragma control <key> "<label>" bool = true|false
#pragma control <key> "<label>" enum("<val1>", "<val2>", ...)
#pragma control <key> "<label>" color = #rrggbb
```

### 6.2 Build Processing

The build tool:
1. Tokenizes lines, extracting `#pragma hypercolor` and `#pragma control` directives (line-by-line tokenizer, not raw regex — handles quoted strings with spaces, escaped characters, and multi-value enum lists correctly)
2. Strips pragma lines from the GLSL source
3. Injects auto-generated `uniform` declarations **after** the `#version` and `precision` lines (critical: `#version` must remain the absolute first line in GLSL ES 3.0 — injecting before it causes a compile error)
4. Wraps in a TypeScript shim that calls `effect()` internally
5. Bundles and emits HTML

**Injection point example:**

```glsl
#version 300 es                    ← stays first (required by spec)
precision highp float;             ← stays second
                                   ← uniforms injected HERE
uniform float iSpeed;              ← auto-generated from #pragma control
uniform int iPalette;              ← auto-generated from #pragma control
uniform float iTime;               ← always present
uniform vec2 iResolution;          ← always present
out vec4 fragColor;
// ... rest of shader ...
```

This means a `.glsl` file can be a complete, shippable effect with zero TypeScript. Inspired by ISF's JSON-in-comments pattern, but using `#pragma` because it's valid GLSL (preprocessor ignores unknown pragmas).

### 6.3 Auto-Declared Uniforms

The `#pragma control` line generates a corresponding `uniform` declaration:

| Pragma Type | GLSL Uniform | Notes |
|-------------|-------------|-------|
| `float(min, max) = default` | `uniform float iKey;` | |
| `int(min, max) = default` | `uniform int iKey;` | |
| `bool = true` | `uniform int iKey;` | GLSL supports `uniform bool` but int is more portable — uses 0/1 |
| `enum(...)` | `uniform int iKey;` | Index into values array |
| `color = #hex` | `uniform vec3 iKey;` | RGB floats 0.0-1.0 |

The uniform name is derived from the control key: `speed` → `iSpeed`, `trailLength` → `iTrailLength` (same `'i' + PascalCase` convention used everywhere).

---

## 7. Controls System

### 7.1 Shape-Based Type Inference

The core insight from Leva: the **shape of the value** determines the widget type. No `type:` field needed.

```typescript
// Shorthand — shape IS type
{
    speed:       [1, 10, 5],                    // 3-number array → slider
    palette:     ['SilkCircuit', 'Fire', 'Ice'], // string array → combobox
    invert:      false,                          // boolean → toggle
    accent:      '#ff6ac1',                      // hex string → color picker
}

// Explicit — factory functions for custom labels/options
{
    speed:       num('Speed', [1, 10], 5),
    palette:     combo('Palette', PALETTES),
    invert:      toggle('Invert', false),
    accent:      color('Accent Color', '#ff6ac1'),
}
```

Both forms can be mixed in the same controls object.

### 7.2 Shorthand Inference Rules

| Value Shape | Inferred Type | Widget | Notes |
|------------|---------------|--------|-------|
| `[number, number, number]` | `number` | Slider | `[min, max, default]` |
| `[number, number, number, number]` | `number` | Slider with step | `[min, max, default, step]` |
| `string[]` | `combobox` | Dropdown | First value is default (use `combo()` for non-first default) |
| `boolean` | `boolean` | Toggle | |
| `'#rrggbb'` or `'#rrggbbaa'` | `color` | Color picker | Hex string starting with `#` |
| `string` | `text` | Text field | Non-hex string value is default |
| `number` | `number` | Slider | Range 0-100, value is default. Escape hatch for simple cases. |

**TypeScript type narrowing:** Shorthand tuples use `as const` to preserve literal tuple types, ensuring the inference engine distinguishes `[1, 10, 5]` (3-tuple → slider) from `number[]`:

```typescript
export default effect('Aurora', shader, {
    speed:   [1, 10, 5] as const,       // readonly [1, 10, 5] — unambiguous 3-tuple
    palette: ['Fire', 'Ice'] as const,   // readonly ["Fire", "Ice"] — unambiguous string tuple
} as const)
```

In practice, `as const` is optional — the `effect()` function signature uses overloads and conditional types to infer correctly from plain array literals in most cases. But `as const` is the escape hatch when inference fails.

**Combobox default behavior:** The first string in the array is always the default. If your default isn't the first item, either reorder the array or use the explicit `combo()` factory:

```typescript
// Default is 'Synthwave' (2nd in the list) — use combo()
palette: combo('Palette', ['SilkCircuit', 'Synthwave', 'Fire'], { default: 'Synthwave' })
```

### 7.3 Explicit Factory Functions

```typescript
num(label: string, range: [number, number], defaultValue: number, opts?: {
    step?: number
    tooltip?: string
    normalize?: 'speed' | 'percentage' | 'none'
    uniform?: string
}): ControlSpec

combo(label: string, values: readonly string[], opts?: {
    default?: string       // defaults to first value
    tooltip?: string
    uniform?: string
}): ControlSpec

toggle(label: string, defaultValue: boolean, opts?: {
    tooltip?: string
    uniform?: string
}): ControlSpec

color(label: string, defaultValue: string, opts?: {
    tooltip?: string
    uniform?: string
}): ControlSpec

hue(label: string, range: [number, number], defaultValue: number, opts?: {
    tooltip?: string
    uniform?: string
}): ControlSpec

text(label: string, defaultValue: string, opts?: {
    tooltip?: string
    uniform?: string            // note: text controls map to a uniform only if explicitly set
}): ControlSpec
```

### 7.4 Magic Names

Certain control key names trigger automatic behavior:

| Key Name | Auto-Behavior |
|----------|---------------|
| `speed` | Applies `normalizeSpeed()` — exponential curve, 1-10 → 0.2-2.83 |
| `palette` | Shader: `comboboxValueToIndex()`. Canvas: wraps value as `palette(t)` function |

Magic names are a convention, not a requirement. Override with explicit factories:

```typescript
{
    speed: num('Animation Rate', [0, 20], 10),  // NOT normalized as speed — custom label + range
}
```

### 7.5 Label Derivation

When using shorthand (no explicit factory), labels are derived from camelCase keys:

```
speed        → "Speed"
trailLength  → "Trail Length"
edgeGlow     → "Edge Glow"
gridDensity  → "Grid Density"
rainIntensity → "Rain Intensity"
```

Algorithm: split on uppercase boundaries, capitalize first word, join with spaces.

---

## 8. Palette as a First-Class Concept

Palette behaves differently in shader vs. canvas contexts because the rendering surfaces need different things.

### 8.1 In Shader Effects

Palette resolves to an integer index for the `iPalette` uniform:

```typescript
export default effect('Aurora', shader, {
    palette: ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk'],
})

// At runtime:
// User selects "Fire" → window.palette = "Fire"
// getControlValues() → comboboxValueToIndex("Fire", PALETTES) → 1
// setUniform('iPalette', 1) → gl.uniform1i(loc, 1)
```

The GLSL shader uses `iPalette` as an index into its IQ palette functions (existing pattern, unchanged).

### 8.2 In Canvas Effects

Palette resolves to a **callable function** that returns CSS color strings:

```typescript
export default canvas('Particles', {
    palette: ['SilkCircuit', 'Fire', 'Ice'],
}, (ctx, time, { palette }) => {
    ctx.fillStyle = palette(0.0)       // → 'rgb(225, 53, 255)'  (start)
    ctx.fillStyle = palette(0.5)       // → 'rgb(128, 255, 234)'  (middle)
    ctx.fillStyle = palette(1.0)       // → 'rgb(80, 250, 123)'   (end)
    ctx.fillStyle = palette(0.3, 0.7)  // → 'rgba(255, 106, 193, 0.7)' (with alpha)
})
```

The `palette` function:
- Takes `t` in range [0, 1] — position along the gradient
- Optional second argument: alpha (0-1)
- Returns a CSS-compatible color string (`rgb(...)` or `rgba(...)`)
- When the user switches palette in the control panel, the function returns colors from the new palette
- Uses Oklab interpolation internally for perceptually uniform gradients

### 8.3 Palette Registry

Each palette name maps to a set of color stops defined in the shared palette registry (`sdk/shared/palettes.json`):

```typescript
// Available to both canvas and shader effects
import { palettes } from '@hypercolor/sdk'

palettes.names()                    // ['SilkCircuit', 'Fire', 'Ice', ...]
palettes.get('SilkCircuit')         // { stops: [...], iq: {...}, accent: '#e135ff' }
palettes.sample('Fire', 0.5)        // [r, g, b] as 0-1 floats
palettes.css('Fire', 0.5)           // 'rgb(255, 128, 0)'
palettes.css('Fire', 0.5, 0.7)     // 'rgba(255, 128, 0, 0.7)'
```

Effects can also define inline palettes for effect-specific color schemes that shouldn't be in the global registry:

```typescript
export default effect('Cyber Descent', shader, {
    palette: combo('Palette', ['BladeRunner', 'Neon District', ...]),
})
```

---

## 9. Audio Integration

### 9.1 Pull Model for Canvas Effects

Audio data is accessed by calling `audio()` — no configuration needed:

```typescript
import { canvas, audio } from '@hypercolor/sdk'

export default canvas('Spectrum', {
    sensitivity: [0, 100, 50],
}, (ctx, time, { sensitivity }) => {
    const a = audio()
    if (!a) return  // no audio source — graceful degradation

    // a.bass, a.mid, a.treble, a.beat, a.level, a.spectrum, ...
})
```

`audio()` returns `AudioData | null`. When null, the effect should degrade to a pleasant ambient state (quality gate requirement). No flag, no registration — just call when you need it.

### 9.2 Declarative for Shader Effects

Shader effects declare `audio: true` because the 18 audio uniforms need to be registered and pushed every frame:

```typescript
// Shorthand in controls — just include it
export default effect('Shockwave', shader, {
    speed:     [1, 10, 5],
    intensity: [0, 100, 75],
    palette:   PALETTES,
}, { audio: true })

// The shader receives all 18 audio uniforms automatically:
// iAudioBass, iAudioMid, iAudioTreble, iAudioBeat,
// iAudioBeatPulse, iAudioLevel, iAudioSwell, ...
```

### 9.3 Shader Pragma

```glsl
#pragma hypercolor audio
```

Equivalent to `{ audio: true }` — registers all audio uniforms.

---

## 10. Uniform Mapping

### 10.1 Name Derivation

Uniform names are auto-derived from control keys using `'i' + PascalCase`:

```
speed         → iSpeed
density       → iDensity
trailLength   → iTrailLength
edgeGlow      → iEdgeGlow
palette       → iPalette
```

Override with the `uniform` option in explicit factories:

```typescript
{
    ringCount: num('Rings', [0, 100], 50, { uniform: 'iNumRings' }),
}
```

### 10.2 Auto-Registration

`effect()` iterates the controls and calls `registerUniform()` for each:

```typescript
// Internal pseudocode
for (const [key, spec] of Object.entries(controls)) {
    const name = spec.uniform ?? deriveUniformName(key)
    const initial = resolveInitialValue(spec)
    this.registerUniform(name, initial)
}
```

### 10.3 Type-Appropriate GL Calls

The int/float distinction is handled automatically by `WebGLEffect.pushUniform()`, which queries `gl.getActiveUniform()` to detect integer-typed uniforms. The control layer doesn't need to know about GLSL types.

| Control Type | Default Value | Uniform Initial | GL Call |
|-------------|---------------|-----------------|---------|
| `num` | `5` | `5.0` | `uniform1f` |
| `num` + `normalize: 'speed'` | `5` | `1.0` | `uniform1f` |
| `combo` | `'SilkCircuit'` | `0` | `uniform1i` (auto-detected) |
| `toggle` | `true` | `1` | `uniform1i` (auto-detected) |
| `color` | `'#ff6ac1'` | `[1.0, 0.416, 0.757]` | `uniform3fv` |

---

## 11. Build-Time Metadata Extraction

### 11.1 Current Problem

The build script currently fakes a DOM, executes the effect module, and reads `reflect-metadata` from a `globalThis` hack. Fragile.

### 11.2 New Approach

`effect()` and `canvas()` detect metadata-only mode and short-circuit:

```typescript
function effect(name, shader, controls, options?) {
    if (globalThis.__HYPERCOLOR_METADATA_ONLY__) {
        // Append to registry — supports multi-effect files (rare but valid)
        globalThis.__hypercolorEffectDefs__ ??= []
        globalThis.__hypercolorEffectDefs__.push({ name, shader, controls, ...options })
        return
    }
    // Runtime: construct and wire the real effect
    return new GeneratedWebGLEffect(name, shader, controls, options)
}
```

The build script reads `__hypercolorEffectDefs__` — an array of plain objects with `name`, `controls` (inert `ControlSpec` objects), and metadata. Uses the last entry for single-effect files (the common case), supports multi-effect files as a forward-compatible escape hatch. No `reflect-metadata`, no class instantiation, no DOM.

### 11.3 Pragma Effects

For Tier 0 `.glsl` files, the build script:
1. Tokenizes `#pragma` lines (line-by-line tokenizer — handles quoted strings with spaces, enum value lists, and `=` default assignments)
2. Extracts effect name, author, description, controls, audio flag
3. Strips pragma lines from shader source
4. Injects `uniform` declarations after `#version`/`precision` block (see 6.2)
5. Wraps in a TypeScript shim calling `effect()` internally
6. Bundles and emits HTML

### 11.4 Meta Tag Generation

Both paths generate identical `<meta>` tags:

```html
<meta property="speed" label="Speed" type="number" min="1" max="10" default="5"
      tooltip="Meteor animation speed"/>
<meta property="palette" label="Palette" type="combobox" default="SilkCircuit"
      values="SilkCircuit,Fire,Ice,Aurora,Cyberpunk" tooltip="Color palette"/>
```

The `property` attribute comes from the control key. All other attributes from the `ControlSpec`. The `tooltip` attribute is optional and maps to the `tooltip` field in factory functions. When using shorthand (no factory), tooltips are omitted from meta tags — use explicit factories to add them.

---

## 12. Runtime Control Flow

### 12.1 Shader Effects

```
Daemon sets window.speed = 7
Daemon calls window.update()
    → effect.update()
    → for each control in controls:
        raw = window[key]                    // 7
        normalized = applyNormalization(spec, raw)  // normalizeSpeed(7) → 1.65
    → for each control in controls:
        setUniform(deriveUniformName(key), normalized)
            // setUniform('iSpeed', 1.65)    → gl.uniform1f
            // setUniform('iPalette', 0)     → gl.uniform1i (auto-detected)
```

### 12.2 Canvas Effects

```
Daemon sets window.speed = 7
Daemon calls window.update()
    → resolves all controls (same normalization)
    → for palette controls: wraps value in palette(t) function
    → stores resolved controls for next draw() call

requestAnimationFrame
    → draw(ctx, time, resolvedControls)
```

### 12.3 ShaderContext / CanvasContext

The `setup` and `frame` hooks receive a context object:

```typescript
interface ShaderContext {
    readonly controls: ResolvedControls      // current values, normalized
    readonly audio: AudioData | null         // current frame audio
    readonly gl: WebGL2RenderingContext
    readonly program: WebGLProgram
    readonly width: number
    readonly height: number
    registerUniform(name: string, value: UniformValue): void
    setUniform(name: string, value: UniformValue): void
    debug(level: string, msg: string): void
}
```

Canvas effects don't need a context — the draw function args (`ctx`, `time`, `controls`) carry everything. For the factory pattern, closured state replaces context.

---

## 13. Type System

### 13.1 Control Type Inference

TypeScript infers control types from the shape:

```typescript
const eff = effect('Test', shader, {
    speed: [1, 10, 5],
    palette: ['Fire', 'Ice', 'Aurora'],
    invert: false,
    accent: '#ff6ac1',
})

// TypeScript infers controls as:
// {
//     speed: number
//     palette: number              (index in shader context)
//     invert: number               (0 | 1 for GLSL)
//     accent: [number, number, number]  (RGB floats for vec3)
// }
```

For canvas effects, the resolved types differ:

```typescript
const eff = canvas('Test', {
    speed: [1, 10, 5],
    palette: ['Fire', 'Ice', 'Aurora'],
    invert: false,
    accent: '#ff6ac1',
}, (ctx, time, controls) => {
    // TypeScript infers controls as:
    // {
    //     speed: number
    //     palette: (t: number, alpha?: number) => string  (function!)
    //     invert: boolean
    //     accent: string              (CSS hex string)
    // }
})
```

### 13.2 ControlSpec Types

```typescript
// What the user provides
type ControlShorthand =
    | readonly [number, number, number]      // num slider (strict 3-tuple)
    | readonly [number, number, number, number]  // num slider with step (strict 4-tuple)
    | readonly string[]                      // combobox (2+ strings)
    | boolean                                // toggle
    | `#${string}`                           // color (hex string)
    | string                                 // text (non-hex string)

// What the factory functions return
interface ControlSpec<T extends ControlTypeName = ControlTypeName> {
    readonly __type: T
    readonly key: string                     // set during defineEffect processing
    readonly label: string
    readonly default: unknown
    readonly tooltip?: string
    readonly uniform?: string                // override derived name
    readonly meta: Record<string, unknown>   // min, max, values, etc.
    readonly normalize?: NormalizeHint
}

type ControlMap = Record<string, ControlShorthand | ControlSpec>
```

---

## 14. HTML Output Contract

The compiled HTML is identical regardless of which API or tier authored the effect:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Meteor Storm</title>
  <meta description="Streaking meteors with physics trails and atmospheric glow"/>
  <meta publisher="Hypercolor"/>
  <meta property="speed" label="Speed" type="number" min="1" max="10" default="5"
        tooltip="Meteor speed"/>
  <meta property="density" label="Density" type="number" min="10" max="100" default="50"
        tooltip="Meteor count"/>
  <meta property="trailLength" label="Trail" type="number" min="10" max="100" default="60"
        tooltip="Trail length"/>
  <meta property="glow" label="Glow" type="number" min="10" max="100" default="65"
        tooltip="Glow intensity"/>
  <meta property="palette" label="Palette" type="combobox" default="SilkCircuit"
        values="SilkCircuit,Fire,Ice,Aurora,Cyberpunk" tooltip="Color palette"/>
</head>
<body style="margin:0;overflow:hidden;background:#000">
  <canvas id="exCanvas" width="320" height="200"></canvas>
  <script>/* IIFE bundle */</script>
</body>
</html>
```

Runtime contract with Servo (unchanged):
- Canvas: `id="exCanvas"`, 320x200
- Controls: `window[propertyName]`
- Update: `window.update()`
- Audio: `window.engine.audio.*`

---

## 15. Migration Guide

### 15.1 Meteor Storm — Before (86 Lines)

```typescript
import 'reflect-metadata'
import {
    ComboboxControl, Effect, NumberControl, WebGLEffect,
    comboboxValueToIndex, getControlValue, initializeEffect, normalizeSpeed,
} from '@hypercolor/sdk'
import fragmentShader from './fragment.glsl'

interface MeteorControls {
    speed: number
    density: number
    trailLength: number
    glow: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk']

@Effect({
    name: 'Meteor Storm',
    description: 'Streaking meteors with physics trails and atmospheric glow',
    author: 'Hypercolor',
})
class MeteorStorm extends WebGLEffect<MeteorControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Meteor speed' })
    speed!: number
    @NumberControl({ label: 'Density', min: 10, max: 100, default: 50, tooltip: 'Meteor count' })
    density!: number
    @NumberControl({ label: 'Trail', min: 10, max: 100, default: 60, tooltip: 'Trail length' })
    trailLength!: number
    @NumberControl({ label: 'Glow', min: 10, max: 100, default: 65, tooltip: 'Glow intensity' })
    glow!: number
    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'SilkCircuit', tooltip: 'Color palette' })
    palette!: string

    constructor() { super({ fragmentShader }) }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.density = getControlValue('density', 50)
        this.trailLength = getControlValue('trailLength', 60)
        this.glow = getControlValue('glow', 65)
        this.palette = getControlValue('palette', 'SilkCircuit')
    }
    protected getControlValues(): MeteorControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            density: getControlValue('density', 50),
            trailLength: getControlValue('trailLength', 60),
            glow: getControlValue('glow', 65),
            palette: comboboxValueToIndex(getControlValue('palette', 'SilkCircuit'), PALETTES, 0),
        }
    }
    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDensity', 50)
        this.registerUniform('iTrailLength', 60)
        this.registerUniform('iGlow', 65)
        this.registerUniform('iPalette', 0)
    }
    protected updateUniforms(c: MeteorControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iDensity', c.density)
        this.setUniform('iTrailLength', c.trailLength)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new MeteorStorm()
initializeEffect(() => effect.initialize(), { instance: effect })
```

### 15.2 Meteor Storm — After (11 Lines)

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Meteor Storm', shader, {
    speed:       [1, 10, 5],
    density:     [10, 100, 50],
    trailLength: [10, 100, 60],
    glow:        [10, 100, 65],
    palette:     ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk'],
})
```

**87% reduction.** Same HTML output. Same runtime behavior.

### 15.3 Codemod

A migration codemod can automate 90%+ of conversions:

1. Parse class for `@Effect({...})` → extract name, description, author
2. Parse `@NumberControl`/`@ComboboxControl` decorators → control specs
3. Read `getControlValues()` for normalization patterns (`normalizeSpeed` → magic name or `normalize: 'speed'`)
4. Verify uniform names match `i` + PascalCase convention
5. Emit `effect(name, shader, controls)`

Edge cases requiring manual intervention:
- Custom `render()` overrides → `frame` hook
- Class properties used across frames → factory closure
- Non-standard uniform naming → explicit `uniform:` option

---

## 16. Implementation Plan

### Phase A: Core API (`sdk/packages/core/`)

| Step | File | Description |
|------|------|-------------|
| A1 | `src/controls/specs.ts` | `num()`, `combo()`, `toggle()`, `color()`, `hue()` factory functions |
| A2 | `src/controls/infer.ts` | Shape-to-type inference (`[1,10,5]` → NumSpec, etc.) |
| A3 | `src/controls/names.ts` | `deriveLabel()`, `deriveUniformName()`, magic name detection |
| A4 | `src/effects/effect.ts` | `effect()` implementation — generates `WebGLEffect` from config |
| A5 | `src/effects/canvas-fn.ts` | `canvas()` implementation — generates `CanvasEffect` from draw fn |
| A6 | `src/palette/runtime.ts` | Palette-as-function for canvas effects |
| A7 | `src/index.ts` | Export new API alongside existing (both coexist) |

### Phase B: Build Pipeline

| Step | File | Description |
|------|------|-------------|
| B1 | `scripts/build-effect.ts` | Support `effect()`/`canvas()` metadata via `__hypercolorEffectDef__` |
| B2 | `scripts/pragma-parser.ts` | Line tokenizer for `#pragma hypercolor` / `#pragma control` (handles quoted strings, commas in enum lists) |
| B3 | `scripts/build-effect.ts` | Support `.glsl` as direct effect entry (Tier 0) |
| B4 | `scripts/build-effect.ts` | Backward compat with decorator-based extraction |

### Phase C: Migration

| Step | Scope | Description |
|------|-------|-------------|
| C1 | All 15 Batch 1 effects | Migrate to `effect()` |
| C2 | Batch 2 effects | Author directly with `effect()` / `canvas()` |
| C3 | Remove `reflect-metadata` | Once all effects migrated |

### Phase D: Polish

| Step | Description |
|------|-------------|
| D1 | Deprecation warnings on decorator API |
| D2 | Update `create-effect` scaffolding templates |
| D3 | Error messages with suggestions ("Did you mean iSpeed?") |
| D4 | Update design doc 17 |

---

## 17. Open Questions

1. **Should Tier 0 (pragma) produce a `.glsl` → `.html` pipeline directly?** Or should it generate an intermediate TypeScript file that then goes through the existing esbuild pipeline? Direct is simpler; intermediate reuses existing tooling.

2. **Canvas effect dimensions.** Currently hardcoded 320x200. Should `canvas()` accept custom dimensions, or is 320x200 the universal Hypercolor canvas contract?

3. **Palette function caching.** `palette(t)` does Oklab interpolation on every call. Should it cache results? For 60fps with hundreds of particles, this could matter. Possibly pre-compute a 256-entry LUT on palette change.

4. **Should `audio()` auto-set `audioReactive` in metadata?** If the build script sees `import { audio }` in the source, it could auto-flag the effect as audio-reactive without explicit config. Clever but potentially surprising.

5. **Shader validation at build time.** Should the build tool compile the shader (via headless WebGL or ANGLE) and report errors with control-aware diagnostics? e.g., "Uniform `iSpped` declared in shader but no control named `spped` — did you mean `speed`?"

6. **Should we ship a `noise()` function for canvas effects?** Canvas effects don't have GPU noise. A JS Simplex implementation would make canvas effects much more expressive. But it's scope creep — `@hypercolor/sdk/noise` as an opt-in import?
