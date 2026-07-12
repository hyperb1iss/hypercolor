+++
title = "SDK API reference"
description = "Flat export-by-export reference for hypercolor: effects, controls, audio, palettes, math, layout, motion, gauges, faces."
weight = 130
template = "page.html"
+++

This is the complete export surface of `hypercolor`, generated directly from
`sdk/packages/core/src/index.ts`. Every name below is exported from the package
root, so a single import line reaches all of it:

```typescript
import { canvas, effect, face, num, paletteControl, audio, lerp } from 'hypercolor'
```

The package is `hypercolor` version `0.1.0`. It is pre-release and **not yet
published to npm** — scaffolded workspaces resolve it through a local `file:` spec
pointing at this checkout. See [Setup](@/effects/setup.md) for how that wires up.

{% callout(type="info") %}
The narrative guides cover the common surface in depth:
[TypeScript canvas effects](@/effects/typescript-effects.md),
[Controls](@/effects/controls.md), [Palettes](@/effects/palettes.md),
[Audio](@/effects/audio.md), [GLSL effects](@/effects/glsl-effects.md), and
[Display faces](@/effects/display-faces.md). This page is the flat index — reach
for it when you want every signature in one place, including the math, layout,
motion, and gauge helper families the guides do not enumerate.
{% end %}

## Declarative API

The three entry points. Each one **registers** the effect as a side effect of the
call and returns `void`. You `export default` the call, but the runtime value of
that default export is `undefined` — registration happens through a global the
build harness reads.

### canvas

```typescript
function canvas(
    name: string,
    controls: ControlMap,
    renderFn: DrawFn | FactoryFn,
    options?: CanvasFnOptions,
): void

canvas.stateful(
    name: string,
    controls: ControlMap,
    factory: FactoryFn,
    options?: CanvasFnOptions,
): void
```

Canvas2D effects. `renderFn` is either a stateless draw function or a stateful
factory. Detection is purely by **arity**: `renderFn.length === 0` is treated as a
factory (runs once, returns the per-frame draw function); arity of one or more is a
stateless draw function called every frame. Use `canvas.stateful()` to force the
factory path when the heuristic fights you.

```typescript
type DrawFn = (
    ctx: CanvasRenderingContext2D,
    time: number,
    controls: Record<string, unknown>,
) => void

type FactoryFn = () => DrawFn
```

`time` is in **seconds**. The canvas never auto-clears — your draw function owns
clearing (opaque `fillRect` for clean frames, semi-transparent for trails). Read
`ctx.canvas.width` / `ctx.canvas.height` every frame; the daemon renders at
640×480 by default but the size is user-configurable.

```typescript
interface CanvasFnOptions {
    description?: string
    author?: string
    audio?: boolean      // required when you read audio — enforced at build time
    screen?: boolean     // opt into screen-zone sampling
    category?: string
    builtinId?: string
    designBasis?: DesignBasis  // author against a fixed grid, scale automatically
    presets?: PresetDef[]
}
```

{% callout(type="warning") %}
`audio: true` is not cosmetic. If your source touches `audio(`, `ctx.audio`,
`getAudioData(`, or `engine.audio` without it, the build **fails** with an audio
reactivity validation error. Same contract for `effect()` shaders.
{% end %}

### effect

```typescript
function effect(
    name: string,
    shader: string,          // fragment shader GLSL source
    controls: ControlMap,
    options?: EffectFnOptions,
): void
```

GLSL fragment-shader effects. These run as **WebGL2 inside Servo**, not as a native
wgpu lane — the SDK bundles the GLSL into an HTML/WebGL artifact. There is no
runnable GPU shader path in the engine today; treat wgpu as future work. See
[GLSL effects](@/effects/glsl-effects.md) for the uniform contract.

Each control maps to a uniform named `i` + PascalCase of the key (`trailLength` →
`iTrailLength`), overridable per control. Every control except `asset` must have a
matching uniform in the shader or the build fails.

```typescript
interface EffectFnOptions {
    description?: string
    author?: string
    audio?: boolean
    screen?: boolean
    category?: string
    builtinId?: string
    presets?: PresetDef[]
    vertexShader?: string
    preserveDrawingBuffer?: boolean
    setup?: (ctx: ShaderContext) => void | Promise<void>
    frame?: (ctx: ShaderContext, time: number) => void
}

interface ShaderContext {
    readonly controls: Record<string, unknown>
    readonly audio: AudioData | null   // getter — pulls fresh data each access
    readonly gl: WebGL2RenderingContext
    readonly program: WebGLProgram
    readonly width: number
    readonly height: number
    registerUniform(name: string, value: UniformValue): void
    setUniform(name: string, value: UniformValue): void
}
```

### face

```typescript
function face(
    name: string,
    controls: ControlMap,
    options: FaceOptions,
    setupFn: (ctx: FaceContext) => FaceUpdateFn,
): void
```

Full-screen HTML faces for device LCDs (pump caps, strips, panels). `setupFn` runs
once and **returns** the per-frame update function. Data sources are opt-in through
`FaceOptions` (`audio`, `media`, `net`, `lighting`; `sensors` is always present),
and per-shape variants can be supplied via `variants`. See
[Display faces](@/effects/display-faces.md) for the full contract, the Servo CSS
matrix (flexbox yes, CSS grid no), and the two canonical displays every face must
handle.

### Shared types

```typescript
type ControlMap        // record of control key → shorthand or ControlSpec
type ControlShorthand  // [min,max,default] | [min,max,default,step] | string[] | bool | "#hex" | string | number
type ControlSpec       // the resolved spec a factory produces
type PresetDef = { name: string; description?: string; controls: Record<string, unknown> }
```

## Control factories

Every factory returns a `ControlSpec`. You can pass shorthand instead and the SDK
infers the type — see [Controls](@/effects/controls.md) for the inference rules.

```typescript
num(label, range: readonly [number, number], defaultValue: number, opts?: NumOptions): ControlSpec<'number'>
combo(label, values: readonly string[], opts?: ComboOptions): ControlSpec<'combobox'>
paletteControl(label, values: readonly string[], opts?: PaletteControlOptions): ControlSpec<'combobox'>
toggle(label, defaultValue: boolean, opts?: ToggleOptions): ControlSpec<'boolean'>
color(label, defaultValue: string, opts?: ColorOptions): ControlSpec<'color'>
hue(label, range: readonly [number, number], defaultValue: number, opts?: HueOptions): ControlSpec<'hue'>
text(label, defaultValue: string, opts?: TextOptions): ControlSpec<'textfield'>
asset(label, mediaKind?: MediaKind, opts?: AssetOptions): ControlSpec<'asset'>
sensor(label, defaultValue: string, opts?: SensorOptions): ControlSpec<'sensor'>
rect(label, defaultValue: RectValue, opts?: RectOptions): ControlSpec<'rect'>
font(label, defaultFamily: string, opts?: FontOptions): ControlSpec<'combobox'>
```

Notes that bite if you miss them:

- `paletteControl` is a distinct factory from `combo`. It sets `meta.palette = true`,
  which triggers the palette magic. In canvas effects the control value becomes a
  `PaletteFn`; in shaders it becomes an integer index uniform (`iPalette`). A plain
  `combo` does **not** get this treatment.
- `font` is sugar over `combo` whose values are font-family names. The default
  family is auto-prepended if it is not already in the list. The bundled default
  set is `JetBrains Mono`, `Inter`, `Orbitron`, `Audiowide`, `Bebas Neue`,
  `DM Sans`, `Exo 2`, `Roboto Condensed`, `Rajdhani`, `Space Mono`,
  `Space Grotesk`, `Sora`.
- `sensor` returns a sensor-label string (`"cpu_temp"`); read the live value with
  `engine.getSensorValue(label)`. Mostly used in faces.
- `rect` returns a `RectValue` (`{ x, y, width, height }`, normalized `[0,1]`).
  `MediaKind` is `'any' | 'image' | 'video' | 'lottie'`.

```typescript
interface RectValue { x: number; y: number; width: number; height: number }
type MediaKind = 'any' | 'image' | 'video' | 'lottie'
```

### Control value helpers

```typescript
getControlValue<T>(propertyName: string, defaultValue: T): T
getAllControls<T extends Record<string, unknown>>(controls: T): T
normalizeSpeed(speed: number): number        // max(0.2, (speed/5) ** 1.5)
normalizePercentage(value: number, defaultValue?: number, minValue?: number): number  // value / 100
comboboxValueToIndex(value: string | number, options: string[], defaultIndex?: number): number
boolToInt(value: boolean | number): number
```

`speed` is the **only** magic-normalized control name. A slider value of `5` maps
to `1.0` through `normalizeSpeed`. These helpers are exposed for when you author a
control manually and need to reproduce the same normalization.

Exported control-definition types (the resolved shapes the runtime consumes):
`BaseControls`, `ControlValues`, `ControlDefinition`, `ControlDefinitionType`,
`NumberControlDefinition`, `ComboboxControlDefinition`, `BooleanControlDefinition`,
`ColorControlDefinition`, `HueControlDefinition`, `TextFieldControlDefinition`,
`AssetControlDefinition`, `RectControlDefinition`, plus the option interfaces
(`AssetOptions`, `FontOptions`, `PaletteControlOptions`, `RectOptions`,
`SensorOptions`).

## Audio

```typescript
import { audio } from 'hypercolor'
// `audio` is getAudioData re-exported. Both names work.

function getAudioData(): AudioData          // pull model — call inside draw, every frame
function getScreenZoneData(): ScreenZoneData // 28×20 = 560-point screen grid

const FFT_SIZE = 200       // frequency / frequencyRaw / frequencyWeighted length
const MEL_BANDS = 24       // melBands / melBandsNormalized length
const PITCH_CLASSES = 12   // chromagram length (C..B)
```

`AudioData` is a wide per-frame struct. Key fields (all `0–1` unless noted):
`level`, `levelRaw` (dB), `bass`, `mid`, `treble`, `beat`, `beatPulse`
(decaying — prefer this over raw `beat`), `beatPhase`, `beatConfidence`, `tempo`
(BPM), `frequency` (200, `Float32Array`), `frequencyRaw` (200, `Int8Array`),
`frequencyWeighted` (200), `melBands` / `melBandsNormalized` (24), `chromagram`
(12), `dominantPitch` (0–11), `harmonicHue` (0–360), `chordMood` (−1 minor → +1
major), `brightness` (spectral centroid), `spectralFlux`, `onset`, `onsetPulse`,
`bassEnv` / `midEnv` / `trebleEnv`, `swell`, `momentum`. See
[Audio](@/effects/audio.md) for the full table and idioms.

{% callout(type="info") %}
The TypeScript field names here (camelCase, `tempo`, `frequency`) differ from the
**Rust** `AudioData` used by native effects (snake_case, `bpm`, `spectrum`).
Shaders also see only a subset — no `chromagram`, `melBands`, or `dominantPitch`
uniforms. See [Native Rust effects](@/effects/native-rust-effects.md) for the
Rust-side names.
{% end %}

### Audio helpers

```typescript
getBassLevel(frequency: Float32Array): number
getMidLevel(frequency: Float32Array): number
getTrebleLevel(frequency: Float32Array): number
getFrequencyRange(frequency: Float32Array, start: number, end: number): number
getMelRange(audio: AudioData, startBand: number, endBand: number): number
getPitchEnergy(audio: AudioData, pitchClass: number | string): number
getPitchClassName(pitchClass: number): string
getPitchClassIndex(name: string): number
pitchClassToHue(pitchClass: number): number              // Circle of Fifths → hue
getHarmonicColor(audio: AudioData, saturation?: number, lightness?: number): [number, number, number]
getMoodColor(audio: AudioData, ...): [number, number, number]
getBeatAnticipation(audio: AudioData, anticipation?: number): number
isOnBeat(audio: AudioData, division?: number, tolerance?: number): boolean
normalizeAudioLevel(level: number): number
normalizeFrequencyBin(value: number, max?: number): number
smoothValue(currentValue: number, previousValue: number, smoothing?: number): number
hslToRgb(h: number, s: number, l: number): [number, number, number]
```

## Palettes

```typescript
paletteNames(): string[]                                       // every registered palette name
getPalette(name: string): PaletteEntry | undefined             // stops + metadata, undefined if unknown
createPaletteFn(name: string): PaletteFn                       // t in [0,1] → CSS color
samplePalette(name: string, t: number): [number, number, number]
samplePaletteCSS(name: string, t: number, alpha?: number): string

type PaletteFn = (t: number) => string
type PaletteEntry  // { stops, background, ... } — the registry shape
```

Palette interpolation is **Oklab** (256-entry LUT, cached per name). An unknown
palette name samples to magenta so the mistake is visible on the hardware. The
registry lives in `sdk/shared/palettes.json`; [Palettes](@/effects/palettes.md)
lists the named entries.

## Base classes

For authors who want the class form instead of `canvas()` / `effect()`:

```typescript
class BaseEffect      // shared lifecycle + control re-poll
class CanvasEffect    // Canvas2D base (designBasis, scaleContext())
class WebGLEffect     // WebGL2 shader base

type EffectConfig
type CanvasEffectConfig
type WebGLEffectConfig
type UniformValue
```

The declarative functions generate subclasses of these; reach for the classes only
when you need lifecycle control the functions do not expose.

## Math

```typescript
clamp(value, min, max): number
saturate(value): number                  // clamp to [0,1]
lerp(a, b, t): number
mix(a, b, t): number                     // alias of lerp
inverseLerp(a, b, value): number
step(edge, value): number
smoothstep(edge0, edge1, value): number
smoothApproach(current, target, lambda, dt): number   // frame-rate-independent
smoothAsymmetric(...): number

// Easings (number → number, domain [0,1])
easeInQuad, easeOutQuad, easeInOutQuad
easeInCubic, easeOutCubic, easeInOutCubic
```

### Scale context

```typescript
scaleContext(source: CanvasSize, designBasis?: DesignBasis): ScaleContext

interface DesignBasis { width: number; height: number }
interface ScaleContext {
    width: number; height: number
    sx: number; sy: number; scale: number
    dx(x): number; dy(y): number        // design coord → live pixels
    dw(w): number; dh(h): number        // design size → live pixels
    ds(value): number                   // uniform scale (radii, strokes, fonts)
    nx(t): number; ny(t): number        // normalized [0,1] → live pixels
}
```

Build one per frame inside your draw function. With no `designBasis` the scale is
the identity, so `dx/dy/dw/dh` echo their inputs. This is how effects stay
pixel-identical when the daemon's canvas size changes.

## Layout

Geometry helpers, primarily for faces but usable anywhere. All operate on plain
`Rect` / `Point` objects.

```typescript
grid(area: Rect, cols: number, rows: number, gap?: number): Rect[]
rail(area: Rect, n: number, gap?: number): Rect[]          // single-axis split
ring(area: Rect, n: number, options?: RingOptions): Point[]
polar(center: Point, radius: number, angle: number): Point
center(area: Rect): Point
inset(area: Rect, amount: number): Rect
anchor(area: Rect, position: AnchorPosition, size: AnchorSize, margin?: number): Rect
fitText(ctx: CanvasRenderingContext2D, text: string, rect: Rect, options?: FitTextOptions): number

interface Rect { x: number; y: number; width: number; height: number }
interface Point { x: number; y: number }
type AnchorPosition
type AnchorSize
type RingOptions
type FitTextOptions
```

## Motion

Stateful animation primitives. Each holds internal state and is advanced by your
update call with a delta or current time. Construct once (in a stateful factory or
face setup), then drive it each frame.

```typescript
class Spring     // critically-dampable spring; update(target, dt) → number
spring(initial: number, options?: SpringOptions): Spring

class Tween      // timed interpolation; done(elapsed) → boolean
tween(from: number, to: number, duration: number, easing?: EasingFn): Tween

class Smoothed   // exponential smoothing by half-life; update(target, dt) → number
smoothed(initial: number, halflife: number): Smoothed

class Transition // eased transition that retargets on change; update(target, now) → number
transitionOnChange(duration: number, easing?: EasingFn): Transition

class Timeline   // named, sequenced keyframes; add(name, start, duration, easing)
timeline(): Timeline

// Easings (EasingFn = (t: number) => number)
linear, easeInQuad, easeOutQuad, easeInOutQuad
easeInCubic, easeOutCubic, easeInOutCubic
easeOutBack, easeOutElastic

type EasingFn
type SpringOptions
```

{% callout(type="tip") %}
FPS is adaptive across five tiers. Always drive motion off a time or delta
(`time_secs`, `performance.now()` deltas, `dt`), never off frame counts, so the
animation is correct at 15, 30, and 60 FPS alike.
{% end %}

## Gauges

Drawing helpers for sensor dashboards and faces. The plain functions render once;
the `create*` variants return animated objects that ease toward new values across
frames.

```typescript
arcGauge(ctx: CanvasRenderingContext2D, opts: ArcGaugeOptions): void
barGauge(ctx: CanvasRenderingContext2D, opts: BarGaugeOptions): void
ringGauge(ctx: CanvasRenderingContext2D, opts: RingGaugeOptions): void
sparkline(ctx: CanvasRenderingContext2D, opts: SparklineOptions): void

createArcGauge(base: Omit<ArcGaugeOptions, 'value'>, animate?: GaugeAnimateOptions): AnimatedArcGauge
createBarGauge(base: Omit<BarGaugeOptions, 'value'>, animate?: GaugeAnimateOptions): AnimatedBarGauge
createRingGauge(base, animate?: GaugeAnimateOptions): AnimatedRingGauge

class ValueHistory   // rolling buffer for sparklines; push(value), values(), min, max

type ArcGaugeOptions; type BarGaugeOptions; type RingGaugeOptions
type SparklineOptions; type SparklineBand; type GaugeAnimateOptions
type AnimatedArcGauge; type AnimatedBarGauge; type AnimatedRingGauge
```

## Faces

Face-specific exports. The `face()` entry point lives under [Declarative
API](#declarative-api); these are the supporting types and a token kit.

```typescript
// Types
type FaceContext; type FaceUpdateFn; type FaceOptions; type FaceVariants
type FaceDisplayInfo; type FaceDisplayShape; type FaceDisplayClass
type InjectedDisplayDescriptor
type AudioAccessor; type MediaAccessor; type NetAccessor
type LightingAccessor; type SensorAccessor
type MediaInfo; type NetInfo; type LightingInfo; type SensorReading
type FaceDataSources

// Token kit (from hypercolor, sourced from faces/tokens)
const palette        // named face color ramps
const spacing
const radius
const sensorColors
parseHex(hex: string): [number, number, number, number]
lerpColor(a: string, b: string, t: number): string
colorByValue(value: number, stops: readonly string[]): string
withAlpha(color: string, alpha: number): string                 // hex input only
withGlow(ctx, color: string, intensity: number, fn: () => void): void
```

## Utilities and initialization

```typescript
// Debug
createDebugLogger(namespace: string, enabled?: boolean)
debug(...args: unknown[]): void
printStartupBanner(): void
type HSLColor; type RGBColor; type UpdateFunction

// Initialization (called for you by canvas/effect/face — rarely needed directly)
initializeEffect(initFunction: () => void, options?: InitOptions): void
type InitializationMode = 'immediate' | 'deferred' | 'metadata-only'
interface InitOptions { mode?: InitializationMode; instance?: unknown }
```

`metadata-only` mode is what the build harness sets to extract control and preset
metadata without running the effect. You will not call `initializeEffect` by hand
in normal authoring.

## Where to go next

- Build and ship the artifact: [Dev workflow](@/effects/dev-workflow.md).
- The authoring CLI flags (`build` / `validate` / `install` / `add`) are covered in
  the dev-workflow and setup pages; it is distinct from the system `hypercolor` CLI
  documented under [the API section](@/api/cli.md).
- Drive a running daemon from an agent over the CLI or MCP — see the Agents & MCP
  section once it lands, or the MCP server notes under the API section.
