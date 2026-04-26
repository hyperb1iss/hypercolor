# Spec 43 — Face SDK

> TypeScript SDK for authoring LCD display faces — reusable gauge
> components, sensor formatters, layout primitives, and a `face()`
> declarative API that mirrors the existing `effect()` / `canvas()`
> pattern. Faces build to standalone HTML via the same esbuild pipeline.

**Status:** Draft (v1)
**Author:** Nova
**Date:** 2026-04-12
**Packages:** `@hypercolor/sdk` (extends existing)
**Depends on:** Display Faces (42), LightScript Runtime
**Related:** Effect SDK (`sdk/packages/core/src/effects/`)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [face() Declarative API](#4-face-declarative-api)
5. [Sensor Control Type](#5-sensor-control-type)
6. [Drawing Primitives](#6-drawing-primitives)
7. [Layout System](#7-layout-system)
8. [Typography](#8-typography)
9. [Design Tokens](#9-design-tokens)
10. [Build Pipeline](#10-build-pipeline)
11. [Starter Faces](#11-starter-faces)
12. [Delivery Waves](#12-delivery-waves)
13. [Known Constraints](#13-known-constraints)
14. [Verification Strategy](#14-verification-strategy)

---

## 1. Overview

Effects and faces share the same LightScript runtime, the same Servo
rendering path, and the same property/control system. But authoring them
is different. Effects paint pixels to a canvas via WebGL shaders or
Canvas 2D draw calls. Faces compose information-rich layouts — gauges,
text, sparklines, icons — that need DOM rendering, CSS layout, web fonts,
and smooth transitions.

The existing `effect()` and `canvas()` APIs produce effects that fill a
canvas with procedural visuals. They don't provide:

- Sensor data access in the render callback
- DOM container management (faces need HTML elements, not just canvas)
- Reusable gauge/chart/text components
- Display-aware layout (480×480 Corsair, 320×320 Kraken, circular masks)
- Typography controls (font family, size, weight as user-configurable
  properties)
- A `sensor` control type for meter picker UI

This spec adds a `face()` API and supporting primitives to the existing
`@hypercolor/sdk` package. Faces are authored in `sdk/src/faces/`, built
to `effects/hypercolor/` via the existing esbuild pipeline, and discovered
by the daemon alongside regular effects. The `EffectCategory::Display`
metadata distinguishes them.

---

## 2. Problem Statement

### 2.1 Duplicated Boilerplate

Every face needs: sensor polling, value formatting, gauge arc math,
text layout, color interpolation, animation easing, circular display
masking. Without shared components, each face author reimplements these
from scratch. SignalRGB's faces show this problem — each is a self-
contained HTML file with duplicated gauge drawing code.

### 2.2 No Sensor Control Type

The SDK provides `num()`, `combo()`, `color()`, `toggle()`, `text()`,
and `hue()` control factories. There is no `sensor()` factory. The
daemon supports `type="sensor"` in `<meta>` tags (the meta parser
handles it), but the TypeScript SDK has no way to declare one. Face
authors would have to use `text()` and hope users type a valid sensor
label.

### 2.3 Canvas-Only Rendering

`canvas()` provides a `CanvasRenderingContext2D` callback. Faces need
DOM elements for proper text rendering (fonts, CSS, subpixel
antialiasing), layout (flexbox/grid), and transitions (CSS animations).
A face that's purely canvas-drawn looks worse than one using real DOM
text with canvas for gauge graphics.

### 2.4 No Display Awareness

Effects render to a daemon-configured canvas (640×480 default). Faces
render to a specific LCD at its native resolution (480×480, 320×320,
etc.). The SDK has no concept of display dimensions, circular masking,
or per-device layout adaptation.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **`face()` declarative API** — one function call to define a face
  with controls, presets, and a DOM-aware render function
- **`sensor()` control factory** — first-class sensor picker control
  that produces `type="sensor"` meta tags
- **Reusable drawing primitives** — arc gauge, ring gauge, bar, sparkline,
  radial progress, all as composable functions
- **Typography system** — `font()` control factory for user-configurable
  fonts, with bundled web font loading helpers
- **Layout primitives** — responsive grid, circular mask, anchor
  positioning, display-aware scaling
- **Design tokens** — SilkCircuit palette, spacing scale, shadow/glow
  utilities as importable constants
- **Build pipeline** — `just faces-build` scans `sdk/src/faces/`,
  outputs HTML with `<meta category="display"/>` tag
- **Starter faces** — 4+ polished faces shipping with Hypercolor

### 3.2 Non-Goals

- **Visual face editor** — authoring is code (TypeScript + CSS)
- **Face marketplace** — community sharing is out of scope
- **Non-Servo rendering** — faces require Servo; no native fallback
- **3D rendering** — WebGL/Three.js faces are theoretically possible
  but not a target for the SDK primitives

---

## 4. face() Declarative API

### 4.1 Signature

```typescript
import { face, sensor, color, num, combo, toggle, font } from "@hypercolor/sdk";

export default face(
  "System Monitor",
  {
    // Controls — same factories as effects, plus sensor() and font()
    cpuSensor: sensor("CPU Sensor", "cpu_temp"),
    gpuSensor: sensor("GPU Sensor", "gpu_temp"),
    accent: color("Accent", "#80ffea"),
    showDate: toggle("Show Date", true),
    layout: combo("Layout", ["Full", "Compact", "Minimal"]),
    clockFont: font("Clock Font", "JetBrains Mono"),
    gaugeStyle: combo("Gauge Style", ["Arc", "Ring", "Bar"]),
  },
  {
    description: "Animated system dashboard with arc gauges",
    author: "Hypercolor",
    // Design basis — face is authored against this resolution.
    // Automatically scales to actual display dimensions.
    designBasis: { width: 480, height: 480 },
    circular: false,
    presets: [
      {
        name: "SilkCircuit Dark",
        description: "Neon cyan accents on dark background",
        controls: { accent: "#80ffea", layout: "Full" },
      },
    ],
  },
  (ctx) => {
    // ctx.container — DOM element (div) covering the full display
    // ctx.canvas    — Canvas2D overlay for custom drawing
    // ctx.width     — display width in pixels
    // ctx.height    — display height in pixels
    // ctx.circular  — whether the display is circular
    // ctx.scale     — scale factor from designBasis to actual display

    // Setup: create DOM elements, initialize state
    const gauge = document.createElement("div");
    ctx.container.appendChild(gauge);

    // Return update function — called every frame
    return (time, controls, sensors) => {
      // controls — resolved control values (same as effect controls)
      // sensors  — live sensor readings from engine.getSensorValue()
      const cpu = sensors.read(controls.cpuSensor);
      const gpu = sensors.read(controls.gpuSensor);
      // Update DOM and canvas...
    };
  },
);
```

### 4.2 FaceContext

The setup function receives a `FaceContext`:

```typescript
interface FaceContext {
  /** Full-display DOM container. Append child elements here. */
  container: HTMLDivElement;
  /** Canvas overlay — same size as container, layered on top.
   *  Use for custom drawing (gauges, sparklines, etc.) */
  canvas: HTMLCanvasElement;
  /** Canvas 2D rendering context. */
  ctx: CanvasRenderingContext2D;
  /** Display width in CSS pixels. */
  width: number;
  /** Display height in CSS pixels. */
  height: number;
  /** Whether the display is circular (e.g., some AIO LCDs). */
  circular: boolean;
  /** Scale factor from designBasis to actual display dimensions.
   *  1.0 when display matches designBasis exactly. */
  scale: number;
  /** Device pixel ratio for high-DPI rendering. */
  dpr: number;
}
```

### 4.3 Sensors Object

The update function receives a `SensorAccessor`:

```typescript
interface SensorReading {
  value: number;
  min: number;
  max: number;
  unit: string;
}

interface SensorAccessor {
  /** Read a sensor by label. Returns null if not available. */
  read(label: string): SensorReading | null;
  /** All available sensor labels. */
  list(): string[];
  /** Read and normalize to [0, 1] based on min/max. */
  normalized(label: string): number;
  /** Formatted display string (e.g., "65°C", "78%", "4.2 GB"). */
  formatted(label: string): string;
}
```

The `SensorAccessor` wraps `window.engine.getSensorValue()` and
`window.engine.sensors` with convenience methods. `read()` is the
raw reading. `normalized()` returns `(value - min) / (max - min)`.
`formatted()` appends the unit with appropriate precision.

### 4.4 Lifecycle

```
face() called
  ↓
Metadata extraction (build-time, __HYPERCOLOR_METADATA_ONLY__)
  ↓
Runtime initialization:
  1. Create container div + canvas overlay
  2. Apply circular mask CSS if applicable
  3. Load web fonts (if font() controls are used)
  4. Call setup function → returns update function
  5. Start requestAnimationFrame loop:
     - Read controls from engine globals
     - Build SensorAccessor from engine.sensors
     - Call update(time, controls, sensors)
```

### 4.5 Differences from canvas()

| Aspect          | `canvas()`                      | `face()`                        |
| --------------- | ------------------------------- | ------------------------------- |
| Render target   | Canvas 2D only                  | DOM container + canvas overlay  |
| Category        | Inferred (ambient, audio, etc.) | Always `display`                |
| Sensor access   | Manual via `window.engine`      | `sensors` param in update       |
| Design basis    | Optional                        | Required (defaults to 480×480)  |
| Font controls   | Not available                   | `font()` factory                |
| Sensor controls | Not available                   | `sensor()` factory              |
| Circular mask   | Not available                   | Automatic via `circular` option |
| HTML template   | Canvas-only body                | DOM container + canvas layers   |

---

## 5. Sensor Control Type

### 5.1 Factory

```typescript
interface SensorOptions {
  tooltip?: string;
  group?: string;
}

/** Sensor picker — user selects from available system sensors. */
function sensor(
  label: string,
  defaultValue: string,
  opts?: SensorOptions,
): ControlSpec<"sensor">;
```

### 5.2 Build Output

```html
<meta
  property="cpuSensor"
  label="CPU Sensor"
  type="sensor"
  default="cpu_temp"
  group="Sensors"
/>
```

The daemon's meta parser already handles `type="sensor"` — it creates
a text input with a sensor browser in the UI. The SDK just needs to
produce the right meta tag.

### 5.3 Runtime Value

The sensor control value is a string (sensor label like `"cpu_temp"`,
`"gpu_load"`, `"ram_used"`). At runtime, pass it to
`sensors.read(controls.cpuSensor)` to get the live reading.

---

## 6. Drawing Primitives

Composable canvas drawing functions. All accept a `CanvasRenderingContext2D`
and draw within caller-specified bounds. No global state, no side effects
beyond the canvas they draw on.

### 6.1 Arc Gauge

```typescript
interface ArcGaugeOptions {
  /** Center X, Y in canvas coordinates. */
  cx: number;
  cy: number;
  /** Outer radius. */
  radius: number;
  /** Gauge thickness (stroke width). */
  thickness: number;
  /** Start angle in radians (default: 0.75π — bottom-left). */
  startAngle?: number;
  /** Sweep angle in radians (default: 1.5π — 270°). */
  sweep?: number;
  /** Background track color. */
  trackColor?: string;
  /** Fill color or gradient stops. */
  fillColor: string | [string, string];
  /** Value 0–1. */
  value: number;
  /** Animated value approach speed (0–1, lower = smoother). */
  smooth?: number;
  /** Glow intensity (0 = none, 1 = full). */
  glow?: number;
  /** End cap style. */
  cap?: "butt" | "round";
}

function arcGauge(ctx: CanvasRenderingContext2D, opts: ArcGaugeOptions): void;
```

### 6.2 Ring Gauge

Circular progress ring — thinner, no background track, centered value text.

```typescript
interface RingGaugeOptions {
  cx: number;
  cy: number;
  radius: number;
  thickness: number;
  color: string | [string, string];
  value: number;
  label?: string;
  labelFont?: string;
  labelColor?: string;
  valueFont?: string;
  valueColor?: string;
  glow?: number;
}

function ringGauge(ctx: CanvasRenderingContext2D, opts: RingGaugeOptions): void;
```

### 6.3 Bar Gauge

Horizontal or vertical bar with gradient fill.

```typescript
interface BarGaugeOptions {
  x: number;
  y: number;
  width: number;
  height: number;
  value: number;
  fillColor: string | [string, string];
  trackColor?: string;
  borderRadius?: number;
  direction?: "horizontal" | "vertical";
  glow?: number;
}

function barGauge(ctx: CanvasRenderingContext2D, opts: BarGaugeOptions): void;
```

### 6.4 Sparkline

Mini line chart from a rolling value buffer.

```typescript
interface SparklineOptions {
  x: number;
  y: number;
  width: number;
  height: number;
  /** Rolling value buffer (newest last). */
  values: number[];
  /** Value range [min, max]. */
  range: [number, number];
  color: string;
  lineWidth?: number;
  fill?: boolean;
  fillOpacity?: number;
}

function sparkline(ctx: CanvasRenderingContext2D, opts: SparklineOptions): void;
```

### 6.5 Value History Buffer

Helper for sparkline data collection.

```typescript
class ValueHistory {
  constructor(capacity: number);
  push(value: number): void;
  values(): number[];
  latest(): number;
}
```

### 6.6 Color Utilities

```typescript
/** Interpolate between two hex colors by ratio [0–1]. */
function lerpColor(a: string, b: string, t: number): string;

/** Create a canvas gradient from color stops. */
function gradientArc(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  radius: number,
  startAngle: number,
  endAngle: number,
  colors: [string, string],
): CanvasGradient;

/** Parse hex color to [r, g, b, a] floats. */
function parseHex(hex: string): [number, number, number, number];

/** Apply glow effect (shadow blur) around subsequent draws. */
function withGlow(
  ctx: CanvasRenderingContext2D,
  color: string,
  intensity: number,
  fn: () => void,
): void;
```

---

## 7. Layout System

### 7.1 Circular Display Mask

Applied automatically when `face()` options include `circular: true` or
the daemon reports a circular display. Uses CSS `clip-path: circle(50%)`.

The face author can also access `ctx.circular` to adapt layout — e.g.,
avoid corners that would be clipped on a circular LCD.

### 7.2 Responsive Grid

```typescript
interface GridOptions {
  /** Number of columns. */
  cols: number;
  /** Number of rows. */
  rows: number;
  /** Gap between cells in design-basis pixels. */
  gap?: number;
  /** Padding from display edges in design-basis pixels. */
  padding?: number;
}

/** Returns cell bounds for layout. */
function grid(
  width: number,
  height: number,
  opts: GridOptions,
): Array<{ x: number; y: number; w: number; h: number }>;
```

### 7.3 Scale Context

Reuses the existing `scaleContext()` from `@hypercolor/sdk/math`:

```typescript
import { scaleContext } from "@hypercolor/sdk";

const s = scaleContext(canvas, { width: 480, height: 480 });
// s.dx(100) → scaled x position
// s.dw(200) → scaled width
```

Face authors design against `designBasis` and scale adapts to actual
display resolution.

---

## 8. Typography

### 8.1 Font Control Factory

```typescript
interface FontOptions {
  tooltip?: string;
  group?: string;
  /** Available font families the user can pick from. */
  families?: string[];
}

/** Font family picker. Default families if none specified:
 *  JetBrains Mono, Inter, Orbitron, Roboto Condensed, Space Grotesk */
function font(
  label: string,
  defaultFamily: string,
  opts?: FontOptions,
): ControlSpec<"combobox">;
```

The `font()` factory is syntactic sugar over `combo()` — it produces a
combobox control with font family names as values. The face runtime loads
the selected font via CSS `@font-face` or Google Fonts import before
first render.

### 8.2 Font Loading

```typescript
/** Load a web font and return a promise that resolves when ready. */
async function loadFont(family: string): Promise<void>;

/** Preload a set of fonts used by the face. */
async function preloadFonts(families: string[]): Promise<void>;
```

Font loading happens during face initialization (after setup, before
first update call). The face framework calls `preloadFonts()` with all
font control default values.

### 8.3 Built-in Font Set

Faces ship with access to these fonts (loaded from local daemon assets
or Google Fonts CDN when available):

| Font             | Use Case                           | Style      |
| ---------------- | ---------------------------------- | ---------- |
| JetBrains Mono   | Code, data readouts, sensor values | Monospace  |
| Inter            | UI labels, descriptions            | Sans-serif |
| Orbitron         | Futuristic displays, clocks        | Display    |
| Roboto Condensed | Compact layouts, small text        | Condensed  |
| Space Grotesk    | Modern geometric, headings         | Sans-serif |

---

## 9. Design Tokens

Importable constants matching the SilkCircuit design language.

```typescript
// sdk/packages/core/src/faces/tokens.ts

export const palette = {
  electricPurple: "#e135ff",
  neonCyan: "#80ffea",
  coral: "#ff6ac1",
  electricYellow: "#f1fa8c",
  successGreen: "#50fa7b",
  errorRed: "#ff6363",

  bg: {
    deep: "#0a0a12",
    surface: "#12121f",
    overlay: "#1a1a2e",
    raised: "#242440",
  },

  fg: {
    primary: "#e8e6f0",
    secondary: "#9d9bb0",
    tertiary: "#6b6980",
  },
} as const;

export const spacing = {
  xs: 4,
  sm: 8,
  md: 16,
  lg: 24,
  xl: 32,
  xxl: 48,
} as const;

export const radius = {
  sm: 4,
  md: 8,
  lg: 16,
  full: 9999,
} as const;
```

### 9.1 Themed Gauge Colors

Pre-built color schemes for common sensor visualizations:

```typescript
export const sensorColors = {
  temperature: {
    cool: "#80ffea", // < 40°C
    warm: "#f1fa8c", // 40–70°C
    hot: "#ff6363", // > 70°C
    gradient: ["#80ffea", "#f1fa8c", "#ff6363"] as const,
  },
  load: {
    low: "#50fa7b", // < 30%
    mid: "#f1fa8c", // 30–70%
    high: "#ff6ac1", // > 70%
    gradient: ["#50fa7b", "#f1fa8c", "#ff6ac1"] as const,
  },
  memory: {
    free: "#80ffea",
    used: "#e135ff",
    gradient: ["#80ffea", "#e135ff"] as const,
  },
} as const;
```

### 9.2 Color by Value

```typescript
/** Pick a color from a gradient based on a 0–1 value. */
function colorByValue(value: number, stops: readonly string[]): string;
```

---

## 10. Build Pipeline

### 10.1 Source Layout

```
sdk/src/faces/
    silkcircuit-hud/
        main.ts
        styles.css       (optional — inlined by esbuild)
    neon-clock/
        main.ts
    pulse-temp/
        main.ts
    sensor-dashboard/
        main.ts
```

### 10.2 Build Script Changes

Extend `sdk/scripts/build-effect.ts` (or create `build-face.ts`) to:

1. Scan `sdk/src/faces/` for directories with `main.ts`
2. Extract metadata using the same `__HYPERCOLOR_METADATA_ONLY__` path
3. Detect `face()` definitions (new `__hypercolorFaceDefs__` global)
4. Generate HTML with:
   - `<meta category="display"/>` tag (distinguishes from effects)
   - DOM container div instead of bare canvas
   - Canvas overlay element layered on top
   - Circular mask CSS when face declares `circular: true`
   - Face-specific CSS reset (no scrollbars, no selection, etc.)
5. Output to `effects/hypercolor/` alongside effects

### 10.3 HTML Template (Faces)

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>System Monitor</title>
    <meta description="Animated system dashboard" />
    <meta publisher="Hypercolor" />
    <meta category="display" />
    <meta
      property="cpuSensor"
      label="CPU Sensor"
      type="sensor"
      default="cpu_temp"
      group="Sensors"
    />
    <!-- ... more controls ... -->
  </head>
  <body style="margin:0;overflow:hidden;background:#0a0a12">
    <div
      id="faceContainer"
      style="position:relative;width:100vw;height:100vh;overflow:hidden"
    >
      <canvas
        id="faceCanvas"
        style="position:absolute;top:0;left:0;width:100%;height:100%;
                   pointer-events:none"
      ></canvas>
    </div>
    <script>
      // bundled face JS
    </script>
  </body>
</html>
```

### 10.4 Just Recipes

```makefile
faces-build:     # Build all faces -> effects/hypercolor/*.html
    cd sdk && bun scripts/build-face.ts --all

face-build NAME: # Build single face
    cd sdk && bun scripts/build-face.ts src/faces/{{NAME}}/main.ts
```

---

## 11. Starter Faces

### 11.1 SilkCircuit HUD

The flagship face. Full system dashboard with animated arc gauges.

**Layout (480×480):**

```
┌────────────────────────────┐
│       12:45 PM             │  ← Clock (Orbitron, large)
│     April 12, 2026         │  ← Date (Inter, muted)
│                            │
│   ╭──CPU──╮   ╭──GPU──╮   │  ← Two arc gauges
│   │  65°  │   │  58°  │   │     with temp in center
│   ╰───────╯   ╰───────╯   │
│                            │
│  ▓▓▓▓▓▓▓▓▓▓▓░░░  RAM 62%  │  ← Memory bar
│  ████████░░░░░░░  CPU 42%  │  ← CPU load bar
│                            │
│  ┈┈┈╲╱┈┈╱╲┈┈  temp trail  │  ← Sparkline
└────────────────────────────┘
```

**Controls:** CPU/GPU sensor pickers, accent color, clock font, 12/24hr,
show date toggle, gauge style (arc/ring/bar).

**Presets:**

- _SilkCircuit Dark_ — neon cyan + electric purple on deep black
- _Forge_ — amber + red on dark charcoal
- _Arctic_ — ice blue + white on navy

### 11.2 Neon Clock

Minimal animated clock. Large time display with subtle pulsing glow,
optional date, configurable font and colors. Smooth CSS transitions on
digit changes.

**Controls:** Font family, accent color, 12/24hr, show seconds, show
date, glow intensity.

### 11.3 Pulse Temp

Single-sensor focus with dramatic visual treatment. Large animated ring
gauge with temperature/load in the center, trailing sparkline below,
configurable thresholds for color shifts (cool → warm → hot).

**Controls:** Sensor picker, color scheme (temp/load/memory), threshold
low/high, glow intensity, show sparkline.

### 11.4 Sensor Grid

Multi-sensor dashboard — 2×2 or 3×2 grid of compact gauges, each with
configurable sensor binding. Adapts layout to display resolution. Good
for "see everything at a glance."

**Controls:** 4–6 sensor pickers, gauge style, compact/detailed toggle,
accent color.

---

## 12. Delivery Waves

### Wave 0 — SDK Foundation

Core face API and drawing primitives.

**Files:**

```
sdk/packages/core/src/
    faces/
        index.ts          — face() declarative API
        context.ts        — FaceContext, SensorAccessor
        lifecycle.ts      — initialization, RAF loop, font loading
    gauges/
        index.ts          — re-exports
        arc.ts            — arcGauge()
        ring.ts           — ringGauge()
        bar.ts            — barGauge()
        sparkline.ts      — sparkline(), ValueHistory
    layout/
        index.ts          — grid(), circularMask()
    tokens.ts             — SilkCircuit palette, spacing, sensorColors
    controls/
        specs.ts          — add sensor() factory (extend existing)
```

**Deliverable:** `face()` builds to standalone HTML. Sensor controls work.
Arc gauge renders on canvas. One test face confirms the pipeline.

### Wave 1 — Starter Faces

**Files:**

```
sdk/src/faces/
    silkcircuit-hud/main.ts
    neon-clock/main.ts
    pulse-temp/main.ts
    sensor-grid/main.ts
```

**Deliverable:** 4 polished faces. `just faces-build` produces HTML.
Daemon discovers and lists them with `category: "display"`.

### Wave 2 — Build Pipeline + Typography

**Files:**

```
sdk/scripts/build-face.ts     — face build script (or extend build-effect.ts)
justfile                       — faces-build, face-build recipes
sdk/packages/core/src/
    faces/fonts.ts             — loadFont(), preloadFonts()
    controls/specs.ts          — font() factory
```

**Deliverable:** `just faces-build` works. Font controls render in the UI.
Faces load selected fonts before first render.

### Wave 3 — Polish and Extras

- Additional drawing primitives (radial text, icon helpers, animated
  transitions)
- More starter faces based on community requests
- Face hot-reload in the daemon (watcher already supports it)
- Documentation and face authoring guide

---

## 13. Known Constraints

### 13.1 CSS Inlining

The current build pipeline (`build-effect.ts`) only handles `.glsl`
loaders and injects JS into a `<script>` tag. It does not process CSS
files. Face CSS must be handled via one of:

- **CSS-in-JS** — face styles written as JavaScript string templates
  (simplest, no build changes needed, esbuild bundles it with the JS)
- **esbuild CSS loader** — add `loader: { '.css': 'text' }` to the
  esbuild config and import CSS as a string, injected into a `<style>`
  tag at runtime
- **Inline styles** — face components use `element.style` directly

Recommendation: CSS-in-JS for v1 (faces import style constants from
tokens). Add CSS loader support in Wave 2 for external stylesheets.

### 13.2 Metadata Key Path

Existing effect metadata uses `__hypercolorEffectDefs__`. Faces use the
**same key** with a `type: 'face'` discriminant. The build script's
`NewApiDef.type` union includes `'face'` and `extractMetadata` propagates
the type field through to the returned metadata object. Cross-directory
validation throws if a `face()` is placed in `src/effects/` or vice versa.

### 13.3 DOM Element IDs

Effects use `exStage` / `exCanvas` element IDs (hardcoded in
`BaseEffect`). Faces should use different IDs (`faceContainer` /
`faceCanvas`) and NOT extend `BaseEffect` — faces have a fundamentally
different lifecycle (DOM + canvas layering vs. canvas-only). The `face()`
implementation is standalone, sharing only control resolution utilities
with the effect path.

### 13.4 Runtime Type Surface

`sdk/packages/core/src/runtime.ts` only declares `audio` and `zone` on
`HypercolorEngine`. The daemon injects `sensors`, `sensorList`,
`getSensorValue()`, `width`, and `height` at runtime, but these have
no TypeScript declarations. The face SDK must extend the runtime types:

```typescript
interface HypercolorEngine {
  audio: HypercolorAudio;
  zone: HypercolorZone;
  sensors: Record<string, SensorReading>;
  sensorList: string[];
  getSensorValue(name: string): SensorReading | null;
  width: number;
  height: number;
}
```

### 13.5 Sensor Control UI

The spec states that `type="sensor"` produces a sensor browser in the
UI. The daemon's meta parser handles the type, but the UI control panel
currently renders sensor controls as plain text inputs (not a sensor
picker). A proper sensor browser widget is a follow-up UI task, not
blocked by the SDK work.

### 13.6 Font Loading

The `face()` lifecycle loads Google Fonts for all font-picker controls
before starting the render loop. This uses `document.fonts.load()` to
ensure the default font family is ready before first paint, avoiding
fallback font flashes. In Servo environments without network access,
fonts fall back to system defaults gracefully (the `Promise.allSettled`
call never throws).

### 13.7 Gradient Type Safety

Gauge `fillColor` options accept `string | readonly [string, string]`.
The `sensorColors` gradient constants use `as const` to infer exact
tuple types, so they're directly assignable without casts. Multi-stop
gradients (3+ colors) should be passed through `colorByValue()` first
to resolve to a single color string.

---

## 14. Verification Strategy

### 14.1 SDK Tests

- `sdk/packages/core/tests/face.test.ts` — `face()` metadata extraction,
  sensor control serialization, lifecycle callbacks
- `sdk/packages/core/tests/gauges.test.ts` — arc/ring/bar gauge math
  (angles, positions, clamping)
- `sdk/packages/core/tests/sensors.test.ts` — SensorAccessor formatting,
  normalization, missing sensor handling

### 14.2 Build Tests

- `just faces-build` succeeds with zero errors
- Built HTML contains `<meta category="display"/>` tag
- Built HTML contains sensor control meta tags with `type="sensor"`
- Biome lint and TypeScript typecheck pass on face sources

### 14.3 Integration Tests

- Load built face HTML in the daemon (with Servo)
- Verify sensor data flows through `engine.getSensorValue()`
- Verify controls update live via the API
- Verify arc gauge renders correctly at 480×480 and 320×320
- Preview JPEG shows expected face content

### 14.4 Visual Verification

- Assign each starter face to a display (physical or simulated)
- Confirm animations are smooth (target 30fps minimum)
- Confirm fonts load and render correctly
- Confirm sensor values update in real-time
- Confirm presets switch cleanly without flicker
