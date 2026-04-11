# Spec 40 — Display Overlay System

> Compositable widget overlays for any pixel-addressable display device —
> clock faces, system monitors, images, and HTML content layered on top of the
> active effect. Works for AIO liquid cooler LCDs today; extends to any
> future display transport (HDMI, DisplayPort, e-paper, DSI panels) without
> changes to the compositor.

**Status:** Draft (v2, revised per cross-model review)
**Author:** Nova
**Date:** 2026-04-10
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Depends on:** Display Output (10 §display), Spatial Layout (06), SparkleFlinger (design/30),
Render Surface Queue (36)
**Related:** `docs/design/28-render-pipeline-modernization-plan.md`,
`docs/design/02-effect-system.md` (sensor control type)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Landscape](#4-landscape)
5. [Architecture](#5-architecture)
6. [Overlay Rendering Stack](#6-overlay-rendering-stack)
7. [System Sensor Pipeline](#7-system-sensor-pipeline)
8. [Type Definitions](#8-type-definitions)
9. [Display Output Pipeline Changes](#9-display-output-pipeline-changes)
10. [Overlay Renderers](#10-overlay-renderers)
11. [HTML Overlay via Servo](#11-html-overlay-via-servo)
12. [API Surface](#12-api-surface)
13. [Configuration](#13-configuration)
14. [GPU Acceleration Path](#14-gpu-acceleration-path)
15. [Delivery Waves](#15-delivery-waves)
16. [Verification Strategy](#16-verification-strategy)
17. [Recommendation](#17-recommendation)

---

## 1. Overview

Any pixel-addressable display attached to Hypercolor — Corsair iCUE LINK, XC7
Elite, XD6 Elite, Ableton Push 2, future NZXT Kraken, future Lian Li
HydroShift, and whatever hardware the ecosystem adds next — currently shows
the effect canvas sampled through a viewport, period. Users cannot overlay a
clock, CPU temperature, custom image, or animated GIF on top of the running
effect without replacing the entire display content.

This spec adds a **per-display overlay compositor** that layers user-configured
widgets on top of the viewport-sampled effect canvas before the final transport
encode. The overlay system is **display-specific** — LED strips and LED zones
are unaffected, and LCD devices can each carry independent overlay stacks.

The approach is transport-agnostic: the compositor operates on the post-
viewport RGBA staging buffer that every display worker already owns. As long
as a device's HAL driver exposes itself through `DeviceTopologyHint::Display`
with a width, height, and optional circular flag, it can host overlays. That
includes hypothetical future transports like USB-attached HDMI capture
devices, e-paper panels, or directly framebuffered DRM outputs.

The approach:

- Overlays are rendered by lightweight **overlay renderers** (native Rust
  widgets for clocks, sensors, images, text) that write **into** a caller-
  owned RGBA target buffer, matching the `render_into` contract that
  `EffectRenderer` already uses
- A **worker-local overlay compositor** in each display worker blends
  positioned/clipped overlay layers onto a freshly sampled base frame every
  tick, with explicit handling for anchored widgets that cover only a portion
  of the display
- A **system sensor pipeline** polls CPU/GPU temperatures, load, memory, and
  fan speeds via `sysinfo` (with optional `nvml-wrapper` for NVIDIA GPUs) and
  publishes snapshots through the existing `FrameInput` path so effects and
  overlays see identical data
- **HTML overlays via Servo** are explicitly gated on multi-session Servo
  support. Until then, HTML overlays are mutually exclusive with HTML effects
  on the same daemon and the spec treats this as a known constraint, not a
  workaround
- The architecture anticipates optional GPU composition (wgpu texture layers,
  shader-based blending) once the render pipeline modernization (design doc
  28, Wave 7) lands

---

## 2. Problem Statement

### 2.1 What Users Want

Every other RGB and AIO control platform worth comparing to supports
overlaying widgets on LCD screens. The common request is: "I want to see
my CPU temperature on top of whatever effect is running." Today
Hypercolor forces a binary choice: effect OR custom content, never both.

### 2.2 What the Ecosystem Lacks

Research across 15+ open-source AIO LCD projects reveals a clear gap.
Nobody has a general-purpose compositor that takes an effect-rendered
canvas as input, composites widget overlays with alpha blending and
z-ordering on top, and outputs to arbitrary LCD hardware on Linux. The
landscape breaks down into three tiers:

| Tier | What exists | Limitation |
|------|-------------|------------|
| Full overlay systems | Python/Qt hexagonal compositors with runtime text/sensor overlays (Thermalright ecosystem), C framebuffer compositors with real alpha blending (one-device tools), React/TS overlay editors with per-element transforms | Either Windows-only, tied to proprietary control software, single-device, or limited to text/sensor overlays without effect compositing |
| Basic LCD rendering | Go/Rust/Python daemons pushing static images, GIFs, or ffmpeg frames to supported coolers | No compositor, no overlay system — direct media push |
| Single-purpose tools | Python scripts rendering specific widgets to specific devices via shell integration | Fixed display modes, narrow device support |

The closest any project comes to "effect + overlay" composition is a
Python tool that receives a canvas from an external effect engine and
composites a temperature string on top. It is a hack, not an architecture.

Hypercolor has the compositor (SparkleFlinger), the effect engine (Servo +
native renderers), and the display output pipeline. The missing piece is
the overlay layer between them.

### 2.3 Architectural Fit

Overlays are **per-display, not per-effect.** A user might want a clock
on their AIO pump LCD, a temperature gauge on their reservoir LCD, and
nothing at all on their LED strips. Someday they might want a full
dashboard on an HDMI-attached desk panel without touching the other two
configurations. This means overlay composition belongs in the display
output path, not in the main render pipeline. The main render loop
remains unchanged, which is the critical invariant: effects never have
to know that overlays exist, and LED rendering is unaffected.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- Per-display overlay configuration: each display device can carry its
  own independent overlay stack
- Transport-agnostic design: any display device that exposes
  `DeviceTopologyHint::Display` is automatically eligible for overlays,
  including future HDMI/DSI/e-paper transports
- Built-in overlay renderers: clock, system sensor gauge, static/animated
  image, styled text
- System sensor pipeline that feeds effects, overlays, and the API
  through a single `FrameInput::sensors` carrier
- Alpha-composited overlay layers using shared blend math (extracted
  from SparkleFlinger into a common module)
- Overlay output caching: per-overlay buffers are cached and re-rendered
  only on cadence or content change; the composite itself is rebuilt
  every tick so the base effect stays live
- REST API for CRUD on per-display overlay configurations with explicit
  failure state surfaced per slot
- SVG-based overlay templates so designers can create custom clock faces
  and gauge artwork without writing Rust
- Circular display awareness (mask overlays to match circular displays)
- Effect control bindings to sensor values (completing `ControlKind::Sensor`)
- No performance regression on the main render loop or LED output path
- Zero canvas clones on the overlay path — the composer operates on
  worker-local staging only

### 3.2 Non-Goals

- Replacing the main render pipeline or effect engine
- Mutating any render-thread-owned canvas or published surface
- GPU-native overlay rendering in the first pass (future wave, blocked
  on render modernization Wave 7)
- HTML overlays running in parallel with HTML effects (blocked on
  upstream Servo multi-session support — see §11)
- User-facing overlay editor UI (the API enables third-party or future UI work)
- Audio-reactive overlays (possible via `FrameInput::audio` later, not in scope)
- Video playback overlays (ffmpeg integration is a separate concern)
- Overlay support for LED-only devices (no pixel surface to composite onto)
- Display-output-direct overlay rendering for non-display devices

---

## 4. Landscape

### 4.1 Rendering Crates

| Crate | Version | Role | Notes |
|-------|---------|------|-------|
| **tiny-skia** | 0.12.0 | 2D rasterizer + compositor | Pure Rust, SIMD-optimized (SSE2/AVX2/NEON). Pixmap operates on **premultiplied** sRGB RGBA, which is different from Canvas (straight sRGB) — see §6.3 for the conversion rules. Actively maintained by Linebender. |
| **cosmic-text** | 0.18.2 | Text layout + shaping | Full Unicode via rustybuzz, variable fonts, color emoji, subpixel positioning. Maintained by System76. |
| **resvg** | 0.47.0 | SVG rendering | Full SVG 1.1 via tiny-skia backend. Enables artist-editable clock face and gauge templates as SVG assets. |
| **image** | 0.25.10 | Image loading | PNG, JPEG, WebP decode. RGBA buffer compatible with tiny-skia Pixmap. |
| **gif** | — | Animated GIF decode | Streaming frame-by-frame decode with disposal method handling. Part of image-rs ecosystem. |
| **sysinfo** | 0.38.3 | System monitoring | CPU temps/load, memory, components (hwmon/sysfs). Cross-platform. |
| **nvml-wrapper** | latest | NVIDIA GPU monitoring | Optional. Wraps NVML for GPU temp, load, VRAM, fan speed. Requires libnvidia-ml.so at runtime. |

### 4.2 Key Design Inspiration

- **Oversized framebuffer with viewport crop** (from a C libusb tool for
  one cooler family): allows easy clipping and animated pan backgrounds.
  Worth studying for the one tool in the ecosystem doing real per-pixel
  alpha blending for LCD overlays.
- **Per-element transforms with z-order and opacity** (from a React
  overlay editor for NZXT coolers on Windows): the UX target for what a
  rich overlay editor should feel like. 20-element caps with independent
  transforms are the right ceiling for this feature.
- **Hexagonal architecture with shared services and interchangeable UI
  adapters** (from a Python daemon targeting Thermalright hardware on
  Linux): the cleanest reference for an overlay system where runtime
  compositing and a REST-addressable compositor state can coexist without
  coupling to the UI layer. Also ships 77+ sensor types via a pluggable
  source abstraction.
- **HTML `<meta>` property declaration + `engine.getSensorValue()` JS
  API for LCD faces** (from a long-standing Windows RGB control suite):
  the contract Hypercolor's existing LightScript runtime already
  implements for effects. The same contract applies to HTML overlays
  once Wave 3 unblocks.

---

## 5. Architecture

### 5.1 Composition Point

Overlay composition happens **inside each display output worker**, between
viewport sampling and the existing brightness/mask/encode pipeline. This
keeps the main render pipeline completely unchanged, makes overlays
naturally per-device, and avoids any mutation of canonical published
surfaces — the composer operates on worker-local staging buffers only.

Each worker owns:

- a reusable **premultiplied RGBA staging buffer** sized to its display
- an **OverlayComposer** holding per-slot renderer instances and their
  cached output buffers
- the existing JPEG encoder state, brightness factor, and circular mask

The main render pipeline hands the worker a freshly viewport-sampled
straight-RGBA frame every tick through the existing
`DisplayWorkItem::source` path. The worker copies that frame into its
premultiplied staging buffer, blends cached overlays on top, unpremultiplies
once at the end, and feeds the existing brightness/mask/JPEG pipeline.
No `Canvas` clone, no mutation of any surface owned by the render thread.

```
Main Render Pipeline (unchanged)
    EffectRenderer → Canvas → SparkleFlinger → CanvasFrame
                                                     │
                                          DisplayOutputThread
                                                     │
                         ┌───────────────────────────┼───────────────────────────┐
                         │                           │                           │
                   DisplayWorker              DisplayWorker              DisplayWorker
                   (AIO LCD)                  (Corsair LCD)             (Push 2)
                         │                           │                           │
                  viewport sample             viewport sample            viewport sample
                         │                           │                           │
                ┌────────┴────────────┐    ┌─────────┴────────────┐              │
                │ premul staging      │    │ premul staging        │      (no overlays)
                │ ↓                    │    │ ↓                      │              │
                │ OverlayComposer     │    │ OverlayComposer        │              │
                │ ├─ ClockRenderer    │    │ └─ SensorGaugeRenderer │              │
                │ └─ SensorRenderer   │    │                        │              │
                │ ↓                    │    │ ↓                      │              │
                │ premul → straight   │    │ premul → straight      │              │
                └─────────┬───────────┘    └─────────┬──────────────┘              │
                          │                          │                             │
                  brightness + mask           brightness + mask           brightness + mask
                          │                          │                             │
                    JPEG encode                JPEG encode                   JPEG encode
                          │                          │                             │
                       USB bulk                   USB bulk                      USB bulk
```

**Why not extend SparkleFlinger?** SparkleFlinger is the render-thread
compositor and operates on full-frame, same-size `ProducerFrame` layers
with an ownership model tuned for the main pipeline's surface pool. The
display overlay composer needs positioned/clipped layers at arbitrary
sizes, operates on a different lifecycle (per-display, not per-frame
globally), and sits behind the existing per-worker FPS throttle. These
are genuinely different compositors. Wave 1 extracts the shared sRGB LUT
and per-pixel blend math from SparkleFlinger into a common module so both
compositors share correctness without sharing structure.

### 5.2 Overlay Renderer Model

Each overlay type implements the `OverlayRenderer` trait, which writes pixels
**into** a caller-owned target buffer rather than returning an owned canvas.
This matches the `render_into` contract already used by `EffectRenderer`
after Spec 36 migration, so both rendering layers share the same ownership
discipline.

```
                    OverlayComposer (per display worker)
                    ┌─────────────────────────────────────────────┐
                    │  PremulStaging (reusable, display-sized)    │
                    │                                              │
                    │  OverlayInstance[0]                          │
                    │  ├── renderer: Box<dyn OverlayRenderer>     │
                    │  ├── slot:     OverlaySlot (config)         │
                    │  ├── cached_buffer: OverlayBuffer (reused)  │
                    │  ├── last_rendered_at: Option<Instant>      │
                    │  └── failure state (see §9.7)               │
                    │                                              │
                    │  OverlayInstance[1]                          │
                    │  ├── ...                                     │
                    └─────────────────────────────────────────────┘
```

**Caching rule:** the composer caches per-overlay render outputs, not the
final composite. Every display tick the composer freshly copies the base
frame into its premultiplied staging buffer, then re-blends the cached
overlay buffers on top. Overlay renders themselves are the cadence-gated
work — clock once per second, sensor gauge once per two seconds, static
image never. Between renders, the positioned blits of the cached buffers
cost microseconds per layer.

This avoids the obvious trap of caching the composite: the base frame
changes every tick (the effect is running), so any cached composite would
freeze the effect behind static overlays.

### 5.3 Sensor Data Flow

```
SensorPoller (background thread, 1-2 s interval)
    ├── sysinfo::System       (CPU load, memory, components, fans)
    ├── sysinfo::Components   (hwmon temperatures incl. AMD GPU)
    └── nvml::Device          (optional, NVIDIA GPU telemetry)
            │
            ▼
    Arc<SystemSnapshot> via tokio::sync::watch
            │
            │   (single source of truth, stable snapshot per frame)
            │
            ├──► InputManager → FrameInputs.sensors
            │        │
            │        ▼
            │   FrameInput::sensors  (passed to every EffectRenderer tick)
            │        │
            │        ├──► LightscriptRuntime (feeds window.engine.sensors)
            │        └──► Native effects with ControlKind::Sensor bindings
            │
            ├──► OverlayComposer per display worker (via OverlayInput)
            │
            └──► REST API: GET /api/v1/system/sensors
```

**Key design rule:** there is exactly **one** sensor carrier, and it lives
on `FrameInput`. Overlays and effects both receive sensor data through the
same `Arc<SystemSnapshot>` that the render thread already carries per
frame. This avoids the asymmetry where overlays could see sensor data that
effects couldn't — and it means Wave 0 can wire sensors into effects
immediately, before any overlay work lands.

The LightScript runtime (`hypercolor-core/src/effect/lightscript.rs`)
already injects `window.engine.sensors`, `window.engine.sensorList`,
`window.engine.getSensorValue()`, `window.engine.setSensorValue()`, and
`window.engine.resetSensors()` into the Servo JavaScript context. Those
stubs exist but have no real data source today. Wave 0's job is to feed
them from `FrameInput::sensors`, not to reinvent the API.

---

## 6. Overlay Rendering Stack

### 6.1 Why tiny-skia

The overlay renderer needs to draw arcs (gauge sweeps), filled shapes (clock
faces), stroked paths (clock hands), text (sensor readouts), and composite
images (PNG icons) onto a pixel buffer at 480x480. Options considered:

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **tiny-skia** | Pure Rust, SIMD, sub-ms at 480×480, Pixmap stores premul sRGB RGBA (converts cleanly to/from Canvas) | 20-100% slower than native Skia | **Use.** Speed is irrelevant at 480×480. |
| cosmic-text alone | Great text layout | No path/shape rendering | **Pair with tiny-skia.** |
| resvg alone | Full SVG rendering | Overkill for dynamic content, no text layout API | **Use for templates only.** |
| embedded-graphics | No-alloc, widget ecosystem | Bitmap fonts, no anti-aliasing, too low quality for 480x480 | Skip. |
| femtovg | NanoVG API, nice | Requires OpenGL ES 3.0+, no CPU path | Skip. |
| vello_cpu | Promising SIMD perf | Unstable API (v0.0.7), preliminary x86 SIMD | Monitor for future. |
| vl-convert-canvas2d | Canvas 2D API over tiny-skia | RC stage, adds abstraction | Optional convenience layer. |

### 6.2 Rendering Pipeline Per Overlay

```
OverlayRenderer::render(input)
    │
    ├── tiny-skia::Pixmap::new(width, height)     // Transparent RGBA
    │
    ├── [SVG template] resvg::render(svg_tree, &mut pixmap)
    │   └── Clock face dial, gauge background, decorative elements
    │
    ├── [Dynamic content] tiny-skia path operations
    │   ├── Arc paths for gauge needles / clock hands
    │   ├── Filled shapes for indicators
    │   └── Gradient fills for temperature ranges
    │
    ├── [Text] cosmic-text → glyph rasterization → tiny-skia blit
    │   ├── Sensor values ("72°C", "45%")
    │   ├── Clock digits ("14:37")
    │   └── Custom labels
    │
    └── Result: Canvas (from Pixmap RGBA bytes, with transparency)
```

### 6.3 Pixmap and Canvas Are Semantically Incompatible

This is a correctness boundary, not a drop-in cast. tiny-skia's `Pixmap`
stores **premultiplied** sRGB RGBA. Hypercolor's `Canvas` stores **straight**
(non-premultiplied) sRGB RGBA. Both the sampler and SparkleFlinger assume
straight storage before linearization. Copying Pixmap bytes directly into a
Canvas would darken every translucent edge pixel by an alpha factor.

Two conversion paths are acceptable, and the implementation must pick one
explicitly:

**Path A — Unpremultiply at the renderer boundary.** Overlay renderers own
a Pixmap, draw into it, then unpremultiply into a `Canvas` (or directly into
the display worker's staging buffer) at the end of `render_into`. The
unpremultiply loop must special-case `alpha == 0` to produce `(0, 0, 0, 0)`
instead of dividing by zero, and must clamp after the divide to avoid values
drifting above 1.0 due to rounding. Tests cover fully-transparent pixels,
fully-opaque pixels, and 50% alpha edges against hand-computed reference
values.

**Path B — Keep overlays premultiplied all the way to final staging.** The
overlay composer carries a private "premultiplied staging buffer" parallel
to the straight-alpha base frame, and only unpremultiplies once at the end,
after all overlay layers have been composited together. This is strictly
cheaper (one conversion per frame instead of one per overlay) and is the
default path for Wave 1.

**The spec commits to Path B.** Path A is noted only for reference. The
display worker's staging layer holds premultiplied linear-light RGBA for
the overlay pass, converts to straight sRGB once at the end, then hands off
to the brightness/circular-mask/JPEG pipeline that already exists.

For the GPU path (Wave 5), overlays stay premultiplied linear the entire
way through; the only conversion happens at the readback-and-encode point.

---

## 7. System Sensor Pipeline

### 7.1 Polling Architecture

A dedicated `SensorPoller` runs on a background OS thread (not a tokio task —
`sysinfo` performs blocking syscalls). It publishes `Arc<SystemSnapshot>`
through a `tokio::sync::watch` channel at a configurable interval (default 2
seconds).

### 7.2 Data Sources

| Data | Linux Source | Crate | Feature Flag |
|------|-------------|-------|-------------|
| CPU temperature | hwmon / coretemp / k10temp | `sysinfo` | always |
| CPU load (per-core + aggregate) | /proc/stat | `sysinfo` | always |
| Memory (RAM + swap) | /proc/meminfo | `sysinfo` | always |
| Component temps (chipset, NVMe, etc.) | hwmon | `sysinfo` | always |
| Fan speeds | hwmon | `sysinfo` | always |
| NVIDIA GPU temp, load, VRAM, fan | NVML (libnvidia-ml.so) | `nvml-wrapper` | `nvidia` feature |
| AMD GPU temp, load | hwmon/amdgpu | `sysinfo` | always (via components) |

The `nvidia` feature flag is optional. Systems without NVIDIA GPUs or without
`libnvidia-ml.so` skip GPU polling gracefully. AMD GPU monitoring works through
the standard hwmon path that `sysinfo` already reads.

### 7.3 Snapshot Types

```rust
/// Published at 1-2 second intervals via watch channel.
pub struct SystemSnapshot {
    /// Aggregate CPU load across all cores (0.0–100.0).
    pub cpu_load_percent: f32,
    /// Per-core CPU load.
    pub cpu_loads: Vec<f32>,
    /// CPU package temperature if available.
    pub cpu_temp_celsius: Option<f32>,
    /// GPU temperature if available (NVIDIA via NVML, AMD via hwmon).
    pub gpu_temp_celsius: Option<f32>,
    /// GPU load percentage if available.
    pub gpu_load_percent: Option<f32>,
    /// GPU VRAM used in MB if available.
    pub gpu_vram_used_mb: Option<f32>,
    /// RAM usage as a percentage (0.0–100.0).
    pub ram_used_percent: f32,
    /// RAM used in MB.
    pub ram_used_mb: f64,
    /// RAM total in MB.
    pub ram_total_mb: f64,
    /// All raw component readings from sysinfo.
    pub components: Vec<SensorReading>,
    /// Unix timestamp in milliseconds for serde-safe transport.
    pub polled_at_ms: u64,
}

pub struct SensorReading {
    pub label: String,
    pub value: f32,
    pub unit: SensorUnit,
    pub max: Option<f32>,
    pub critical: Option<f32>,
}

#[derive(Clone, Copy)]
pub enum SensorUnit {
    Celsius,
    Percent,
    Megabytes,
    Rpm,
    Watts,
    Mhz,
}
```

### 7.4 Integration with InputManager and FrameInput

The `SensorPoller` runs on its own OS thread (sysinfo performs blocking
syscalls, so a tokio task is the wrong fit) and publishes snapshots through
a `tokio::sync::watch::Sender<Arc<SystemSnapshot>>`. The `InputManager`
holds the corresponding receiver alongside its other sources.

`InputData` gains a new variant so the existing `InputSource`/`InputManager`
architecture carries sensor data uniformly with audio, screen, and
interaction:

```rust
pub enum InputData {
    Audio(AudioData),
    Interaction(InteractionData),
    Screen(ScreenData),
    Sensors(Arc<SystemSnapshot>),  // NEW — carried every frame
    None,
}
```

`FrameInputs` (the render-thread-owned aggregate of all input sources
sampled for a single frame) gains a `sensors: Arc<SystemSnapshot>` field.
`sample_inputs` pulls the latest snapshot from the watch receiver at the
same point it samples audio and screen data — this is a cheap `borrow()`
with no allocation.

`FrameInput` (the per-frame reference passed to `EffectRenderer::render_into`
and `OverlayRenderer::render_into`) gains a matching reference:

```rust
pub struct FrameInput<'a> {
    pub time_secs: f32,
    pub delta_secs: f32,
    pub frame_number: u64,
    pub audio: &'a AudioData,
    pub interaction: &'a InteractionData,
    pub screen: Option<&'a ScreenData>,
    pub sensors: &'a SystemSnapshot,  // NEW
    pub canvas_width: u32,
    pub canvas_height: u32,
}
```

The watch channel always holds at least an empty-but-valid snapshot
(zero loads, no temperatures, empty components vector) so the reference
is always dereferenceable even before the poller has produced real data.

### 7.5 Consumers

1. **Effects (native and HTML)** read sensors via `FrameInput::sensors`.
   Native effects access it directly. HTML effects access it through the
   existing LightScript runtime, which reads `FrameInput::sensors` when
   building the per-frame script preamble and writes the values into
   `window.engine.sensors` — the API already exists, Wave 0 just connects
   the pipe.
2. **Overlay renderers** read sensors via `OverlayInput::sensors`, which
   is populated from the same `Arc<SystemSnapshot>` that feeds
   `FrameInput::sensors`. Overlays and effects see identical values in
   any given tick.
3. **Effect controls** with `ControlKind::Sensor` (already in the type
   system at `hypercolor-types/src/effect.rs:197`) can bind to a sensor
   label. Wave 4 completes the binding — a small mapper reads the bound
   sensor value from `FrameInputs::sensors` at frame-prepare time and
   writes it into the effect's control store, mapped from the sensor's
   range to the control's `[0.0, 1.0]`.
4. **REST API** exposes the latest snapshot at `GET /api/v1/system/sensors`.
5. **MCP tools** expose the snapshot via `get_sensor_data` for AI access.

---

## 8. Type Definitions

### 8.1 Overlay Configuration

```rust
/// Per-display overlay stack. Stored in user config, editable via API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DisplayOverlayConfig {
    /// Ordered bottom-to-top. First overlay is closest to the effect canvas.
    pub overlays: Vec<OverlaySlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OverlaySlot {
    /// Unique identifier within this display's overlay stack.
    pub id: OverlaySlotId,
    /// Human-readable name.
    pub name: String,
    /// What to render.
    pub source: OverlaySource,
    /// Where to place the overlay on the display.
    pub position: OverlayPosition,
    /// Blend mode for compositing over the layer below.
    pub blend_mode: OverlayBlendMode,
    /// Opacity of this overlay layer (0.0–1.0).
    pub opacity: f32,
    /// Whether this overlay is active.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverlaySource {
    Clock(ClockConfig),
    Sensor(SensorOverlayConfig),
    Image(ImageOverlayConfig),
    Text(TextOverlayConfig),
    Html(HtmlOverlayConfig),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayBlendMode {
    /// Standard alpha compositing (source-over). Default.
    Normal,
    /// Additive blending — bright overlays glow.
    Add,
    /// Screen blend — brightens without blowing out.
    Screen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    /// Overlay covers the entire display surface.
    FullScreen,
    /// Positioned widget with anchor, offset, and size.
    Anchored {
        anchor: Anchor,
        offset_x: i32,
        offset_y: i32,
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}
```

### 8.2 Overlay Source Configs

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClockConfig {
    /// "digital" or "analog".
    pub style: ClockStyle,
    /// 12 or 24 hour format (digital only).
    pub hour_format: HourFormat,
    /// Show seconds hand/digits.
    pub show_seconds: bool,
    /// Show date below clock.
    pub show_date: bool,
    /// Date format string (e.g., "%Y-%m-%d", "%b %d").
    pub date_format: Option<String>,
    /// Font family for digital clock digits.
    pub font_family: Option<String>,
    /// Primary color (digits, hands).
    pub color: String,
    /// Secondary color (dial marks, date text).
    pub secondary_color: Option<String>,
    /// Optional SVG template path for custom face design.
    pub template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SensorOverlayConfig {
    /// Which sensor to display. Well-known: "cpu_temp", "gpu_temp",
    /// "cpu_load", "gpu_load", "ram_used". Or a raw sysinfo label.
    pub sensor: String,
    /// Display style.
    pub style: SensorDisplayStyle,
    /// Unit label to show (e.g., "°C", "%", "MB").
    pub unit_label: Option<String>,
    /// Min value for gauge range.
    pub range_min: f32,
    /// Max value for gauge range.
    pub range_max: f32,
    /// Color at min value (cool).
    pub color_min: String,
    /// Color at max value (hot).
    pub color_max: String,
    /// Font family for value text.
    pub font_family: Option<String>,
    /// Optional SVG template for custom gauge face.
    pub template: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorDisplayStyle {
    /// Numeric value with unit (e.g., "72°C").
    Numeric,
    /// Radial gauge arc.
    Gauge,
    /// Horizontal or vertical bar.
    Bar,
    /// Minimal: just the number, no decoration.
    Minimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ImageOverlayConfig {
    /// Path to image file (PNG, JPEG, WebP, GIF).
    pub path: String,
    /// For animated GIFs: playback speed multiplier (1.0 = normal).
    pub speed: f32,
    /// How to fit the image within the overlay bounds.
    pub fit: ImageFit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Scale to fill, crop excess.
    Cover,
    /// Scale to fit entirely, letterbox if needed.
    Contain,
    /// Stretch to fill exactly.
    Stretch,
    /// No scaling, center at original size.
    Original,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TextOverlayConfig {
    /// The text to display. Supports {sensor:label} interpolation.
    pub text: String,
    /// Font family.
    pub font_family: Option<String>,
    /// Font size in pixels.
    pub font_size: f32,
    /// Text color.
    pub color: String,
    /// Text alignment within the overlay bounds.
    pub align: TextAlign,
    /// Scroll horizontally if text exceeds bounds.
    pub scroll: bool,
    /// Scroll speed in pixels per second.
    pub scroll_speed: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HtmlOverlayConfig {
    /// Path to HTML file (Wave 3, gated on multi-session Servo).
    pub path: String,
    /// Property overrides declared via `<meta property>` tags in the HTML.
    pub properties: HashMap<String, serde_json::Value>,
    /// Render cadence in milliseconds (default 1000).
    pub render_interval_ms: u32,
}
```

### 8.3 Overlay Renderer Trait

The trait mirrors `EffectRenderer::render_into` so overlay renderers never
allocate canvases on the hot path. Cadence and cache policy live in the
composer, not the trait — keeping the trait focused on "turn state into
pixels" and the composer focused on "decide when to redraw."

```rust
/// Renders overlay content into a caller-owned target buffer. Send but not
/// Sync (same constraint as EffectRenderer — Servo and tiny-skia state are
/// single-threaded).
pub trait OverlayRenderer: Send {
    /// Called once when the renderer is created, before the first render.
    /// `target_size` is the natural size of the overlay (may differ from
    /// the display size for anchored widgets).
    fn init(&mut self, target_size: OverlaySize) -> Result<()>;

    /// Called when the target size changes (display reconnected with a new
    /// resolution, user repositioned the widget). The renderer should
    /// resize internal caches (glyph cache, SVG tree, image scale).
    fn resize(&mut self, target_size: OverlaySize) -> Result<()>;

    /// Write overlay pixels into `target`. The target is owned by the
    /// compositor, is the correct size for this overlay, and carries
    /// premultiplied sRGB RGBA (zeroed before the call). Returns Ok(()) on
    /// success or an error classified via `OverlayError` (see §9.7).
    fn render_into(
        &mut self,
        input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> Result<(), OverlayError>;

    /// Content-change hint. The composer only invokes `render_into` when
    /// either the cadence timer fires or `content_changed` returns true.
    /// For clocks this is "the wall-clock minute changed"; for sensors it
    /// is "the bound sensor value moved outside a dead-band." Defaults to
    /// true to match conservative behavior.
    fn content_changed(&self, _input: &OverlayInput<'_>) -> bool {
        true
    }

    /// Release resources (font caches, image buffers, SVG trees).
    /// Called when the overlay slot is removed or the worker shuts down.
    fn destroy(&mut self) {}
}

/// Premultiplied sRGB RGBA target buffer owned by the compositor.
/// A thin wrapper around a reusable byte buffer plus dimensions.
pub struct OverlayBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,  // 4 bytes per pixel, premultiplied sRGB
}

#[derive(Debug, Clone, Copy)]
pub struct OverlaySize {
    pub width: u32,
    pub height: u32,
}

/// Input provided to overlay renderers each time they are asked to render.
/// Note the explicit lifetime — the compositor borrows the latest frame
/// snapshot rather than cloning an Arc every call.
pub struct OverlayInput<'a> {
    /// Wall-clock time for clock overlays.
    pub now: SystemTime,
    /// Display width in pixels (the full display, not the widget).
    pub display_width: u32,
    /// Display height in pixels.
    pub display_height: u32,
    /// Whether the display is circular (for masking hints).
    pub circular: bool,
    /// Latest system sensor snapshot — same Arc the effect engine sees.
    pub sensors: &'a SystemSnapshot,
    /// Elapsed seconds since overlay was created (for animations).
    pub elapsed_secs: f32,
    /// Frame number (monotonic, from the display worker's tick counter).
    pub frame_number: u64,
}

/// Renderer-specific failure mode.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    /// Asset is malformed (bad SVG, unsupported image format, HTML syntax
    /// error). The composer disables the slot and surfaces the error via
    /// the API but continues rendering the rest of the display.
    #[error("asset error: {0}")]
    Asset(#[from] anyhow::Error),

    /// Transient failure (IO error reading a file, temporary Servo
    /// deadline miss). The composer keeps the last successful render and
    /// retries on the next cadence tick. Rate-limited per §9.7.
    #[error("transient: {0}")]
    Transient(String),

    /// Fatal error that should tear down this slot entirely.
    #[error("fatal: {0}")]
    Fatal(String),
}
```

---

## 9. Display Output Pipeline Changes

### 9.1 Current Flow

```
CanvasFrame → viewport sample → brightness → circular mask → JPEG → USB
```

### 9.2 New Flow

```
CanvasFrame → viewport sample → premul staging → overlay compose → unpremul →
    brightness → circular mask → JPEG → USB
                       │
         OverlayComposer::compose_into(staging)
          ├── overlay_buffers[0] (cached OverlayBuffer, re-rendered on cadence)
          ├── overlay_buffers[1] (cached OverlayBuffer, re-rendered on cadence)
          └── each buffer blitted at its Anchored position with the slot's
              blend_mode + opacity, directly into the premul staging buffer
```

### 9.3 OverlayComposer

The `OverlayComposer` lives inside each `DisplayWorkerHandle` and owns the
overlay instances for that display. It is created when the display worker
spawns and is updated via a `watch::Receiver<Arc<DisplayOverlayConfig>>`
when the user modifies overlay configuration. The composer owns its own
staging buffers; the display worker does not hold any canonical `Canvas`
values on the overlay path.

```rust
pub(crate) struct OverlayComposer {
    /// Per-slot overlay instances, ordered bottom-to-top.
    instances: Vec<OverlayInstance>,
    /// Reusable premultiplied RGBA staging buffer sized to the display.
    /// The display worker hands a freshly viewport-sampled frame here every
    /// tick, the composer blends overlays on top in place, and the final
    /// unpremultiply happens before the downstream brightness/mask/JPEG
    /// stages. Zero full-frame canvas clones on the happy path.
    staging: PremulStaging,
    /// Display geometry for overlay input.
    display_width: u32,
    display_height: u32,
    circular: bool,
    /// When the overlay stack was created (monotonic clock, for
    /// OverlayInput::elapsed_secs).
    created_at: Instant,
}

struct OverlayInstance {
    slot: OverlaySlot,
    renderer: Box<dyn OverlayRenderer>,
    /// Reusable render target owned by the composer. The renderer writes
    /// into it via `render_into`; the composer never allocates a new
    /// buffer per frame.
    cached_buffer: OverlayBuffer,
    /// True once `render_into` has succeeded at least once and the
    /// buffer contains valid pixels. Start-of-day renders skip blend.
    has_valid_render: bool,
    /// Monotonic timestamp of the last successful render.
    last_rendered_at: Option<Instant>,
    /// Consecutive failure counter for rate-limiting retries (see §9.7).
    consecutive_failures: u32,
    /// Earliest time the composer will attempt to render this slot again
    /// after a failure, if any.
    backoff_until: Option<Instant>,
    /// Error state for API surfacing.
    last_error: Option<OverlayError>,
}

pub(crate) struct PremulStaging {
    /// 4 bytes per pixel, premultiplied sRGB RGBA.
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}
```

### 9.4 Compose Algorithm

**Critical invariant:** the composer does **not** cache the final composed
frame. The base viewport canvas changes every tick because the active effect
is running; caching the composite would freeze the effect behind static
overlays. The composer caches the **per-overlay render outputs** (cheap —
rendered only on cadence) and re-blends them over a fresh base every frame
(also cheap — a few positioned alpha blits at 480×480).

```
fn compose_into(&mut self, base_frame: &RgbaStraight) -> &PremulStaging:
    // 1. Copy base frame into staging with straight→premul conversion.
    //    This costs one full-frame pass but is strictly cheaper than any
    //    canvas clone because it fuses the conversion with the copy.
    self.staging.write_from_straight(base_frame)

    // 2. If no overlays enabled, return staging as-is. Downstream unpremul
    //    step will handle conversion back to straight.
    if self.instances.iter().all(|i| !i.slot.enabled):
        return &self.staging

    // 3. For each overlay slot, decide whether to re-render, then blend.
    let input = self.build_overlay_input()
    for instance in &mut self.instances:
        if !instance.slot.enabled:
            continue
        if instance.is_in_backoff(now):
            // Skip render, but still blend last-known-good if available.
            self.blend_slot_if_valid(instance)
            continue

        let cadence_due = instance.cadence_due(now)
        let content_dirty = instance.renderer.content_changed(&input)
        if cadence_due || content_dirty || !instance.has_valid_render:
            match instance.renderer.render_into(&input, &mut instance.cached_buffer):
                Ok(()) =>
                    instance.has_valid_render = true
                    instance.last_rendered_at = Some(now)
                    instance.consecutive_failures = 0
                    instance.last_error = None
                Err(err) =>
                    self.handle_render_error(instance, err, now)
                    // Fall through — blend last-good buffer if we have one.

        // 4. Blend this overlay's cached buffer into the staging buffer
        //    at the slot's anchored position, with clipping to staging
        //    bounds and respecting blend_mode + opacity. Only blends if
        //    has_valid_render is true.
        self.blend_slot_if_valid(instance)

    &self.staging
```

The blend step takes the overlay's cached `OverlayBuffer` and blits it into
the premultiplied staging buffer at the slot's `Anchored` position. Blend
modes (`Normal`, `Add`, `Screen`) are applied per-pixel within the overlay's
bounding rectangle only — the staging buffer outside the bounding rectangle
is untouched. This is strictly O(overlay_area) per layer, not
O(display_area), so multiple small anchored widgets remain cheap.

**Why not SparkleFlinger directly?** SparkleFlinger's `compose` only blends
full-frame, same-size layers — it has no concept of positioned or clipped
layers, and its API takes `CompositionLayer` values that own `ProducerFrame`
references to upstream canvases. The overlay compositor's needs are
fundamentally different: many small widgets at arbitrary positions, reused
staging, no canvas ownership. The overlay compositor borrows SparkleFlinger's
**blend math** (the per-pixel premultiplied linear-light math and sRGB LUTs)
as a shared module, but operates its own positioned-blit loop.

Wave 1 extracts the sRGB LUTs and per-pixel blend functions from
`sparkleflinger.rs` into a shared `blend_math` module under
`hypercolor-core` so both the render-thread SparkleFlinger and the per-
display OverlayComposer can depend on it without cross-crate coupling.

### 9.5 Integration with DisplayWorkItem

`DisplayWorkItem` is unchanged — the composer is spawned lazily inside the
worker based on a `watch::Receiver<Arc<DisplayOverlayConfig>>` passed at
spawn time. The display output thread resolves overlay configs from
`AppState` (keyed by `DeviceId`) and pushes updates via the watch channel
when the user edits overlays. Workers never touch `AppState` directly.

When the watch channel yields a new config, the composer reconciles:

- **Added slots:** instantiate the renderer via the factory, call `init`
  with the slot's target size. If instantiation fails with `OverlayError::Fatal`,
  record the error and leave the slot disabled (surfaced via the API).
- **Removed slots:** call `destroy()` on the renderer and drop it.
- **Modified slots:** if the source type is unchanged and the renderer
  supports in-place reconfig (via a separate `reconfigure` trait method),
  apply the change; otherwise tear down and recreate. Position changes
  never require recreation — only source or size changes do.

Reconciliation happens at the top of the compose loop, before rendering,
so config edits take effect on the next display tick.

### 9.6 Performance Budget

At 480×480 with 2 overlay layers (base sized full display, overlays
sized 120×120 anchored):

| Step | Estimated Cost | Notes |
|------|---------------|-------|
| Base frame straight→premul copy | ~400 µs | One full-frame fused copy+convert |
| Overlay render (cached) | 0 µs | Skipped most frames |
| Overlay render (fresh, tiny-skia) | 200–500 µs | Clock face with text, once/sec |
| Positioned alpha blit per layer | 5–15 µs | 120×120 = 14.4K pixels, LUT-accelerated |
| Staging premul→straight copy | ~400 µs | Fused convert+copy back to straight |
| Total overlay overhead typical | ~850 µs | Dominated by the two conversion passes |
| Current JPEG encode | 1–3 ms | Unchanged |

The overlay pass adds ~850 µs per frame at 15 FPS (~13 ms/s of CPU), well
within budget. The JPEG encode remains the dominant cost. When overlays are
disabled, the composer bypasses both the staging conversion and the blend
loop — the display worker takes the existing straight-RGBA path unchanged.

### 9.7 Failure Policy and Slot Lifecycle

Overlay renderers can fail in several ways and the composer must handle each
deterministically. The policy mirrors Servo's existing fatal/non-fatal
classification (see `servo/circuit_breaker.rs`).

**`OverlayError::Asset`** — malformed input (bad SVG, corrupt image, HTML
parse error, missing font glyph). Disable the slot immediately. Record the
error in `last_error` so the API can surface it. The slot remains in the
config (so the user sees it and can fix it) but produces no pixels. On next
config update for the slot, retry once.

**`OverlayError::Transient`** — temporary failure (IO error, resource
exhaustion, Servo render timeout). Keep the last successful `cached_buffer`
and continue blending it. Increment `consecutive_failures` and set
`backoff_until` to `now + backoff_duration(consecutive_failures)`. Backoff
is exponential starting at 500 ms, doubling per failure, capped at 30 s.
After 5 consecutive failures, escalate to `Asset` (disable the slot). A
successful render resets the counter.

**`OverlayError::Fatal`** — unrecoverable (Servo crashed, renderer state
corrupted). Tear down the slot: call `destroy()` and drop the renderer.
The next config update recreates it from scratch.

**Startup failure** — if `init` fails when a slot is first created, log
once, mark the slot disabled, surface the error via the API. Subsequent
config updates retry.

**Per-slot telemetry** — the composer tracks per-slot `last_rendered_at`,
`consecutive_failures`, and `last_error`, exposed via
`GET /api/v1/displays/{device_id}/overlays/{slot_id}` so users can
diagnose broken overlays without reading daemon logs.

**Shutdown** — when the display worker stops, the composer calls
`destroy()` on every instance before dropping itself.

---

## 10. Overlay Renderers

### 10.1 ClockRenderer

Renders digital or analog clock faces.

**Digital mode:**
- Render time string via cosmic-text with configurable font, size, color
- Optional date string below
- Optional SVG background template via resvg

**Analog mode:**
- SVG template defines the dial face (tick marks, numerals, decorative elements)
- tiny-skia path strokes for hour, minute, second hands (rotated from center)
- Anti-aliased rendering at native display resolution

**Update interval:** 1 second (500 ms if showing seconds with smooth sweep).

### 10.2 SensorRenderer

Displays system sensor values as numeric readouts or visual gauges.

**Numeric mode:** Value text with unit label, optional min/max indicators.

**Gauge mode:**
- SVG template for gauge background/dial
- tiny-skia arc path for the value sweep, interpolating color between
  `color_min` and `color_max` based on `(value - range_min) / (range_max - range_min)`
- cosmic-text for the value label centered in the gauge

**Bar mode:** Horizontal or vertical fill bar with gradient.

**Update interval:** 2 seconds (configurable).

### 10.3 ImageRenderer

Displays static images or animated GIFs with transparency.

**Static images:** Load once via `image` crate, resize/fit according to
`ImageFit`, cache forever.

**Animated GIFs:** Decode frames lazily via `gif` crate, cycle through frames
respecting per-frame delay timings. Cache decoded frames in memory (at display
resolution, not source resolution).

**Update interval:** Static: `Duration::MAX`. GIF: per-frame delay.

### 10.4 TextRenderer

Displays styled text with optional `{sensor:label}` interpolation.

- Layout via cosmic-text with word wrapping within overlay bounds
- Optional horizontal scroll for text that exceeds bounds
- Sensor interpolation evaluated on each render from the latest
  `SystemSnapshot`

**Update interval:** 2 seconds (sensor interpolation cadence), or 33 ms if
scrolling.

---

## 11. HTML Overlay via Servo

Wave 3 is conditional and honest. HTML overlays fit Hypercolor's LightScript
runtime cleanly at the JS API level, but the Servo worker as it stands
today cannot host them in parallel with HTML effects without a redesign.
This section specifies what's possible now and what's blocked.

### 11.1 The Real Constraint: Single Session, Not Just Single Instance

Section 5 of the spec mentioned "single OnceLock instance" as the blocker,
but that's underselling it. The current Servo worker
(`crates/hypercolor-core/src/effect/servo/worker.rs`) is built around:

- one process-global `Servo` engine (initialized once, reinitialization
  panics inside libservo)
- one active `WebView` at any given time
- one loaded HTML document — loading a new URL tears down the current
  document's JS context
- a serial command loop — `Load`, `Render`, `Unload` commands are handled
  one at a time

This is not a lock contention problem that a mutex can solve. An HTML
overlay render request cannot interleave with an effect render without
unloading the effect's document, because there is only one WebView and
one JS context. "Render an overlay between effect frames" means "unload
the effect, load the overlay, render, unload the overlay, reload the
effect" — which takes hundreds of milliseconds per cycle and destroys the
effect's JS state (animation timers, RAF callbacks, variable bindings).

### 11.2 Compatibility Policy

Until Servo supports multiple concurrent WebView sessions in a single
process (tracked upstream but not available today), HTML overlays are
**mutually exclusive with HTML effects** on the same daemon instance.
The daemon enforces this:

- When an HTML effect is active and the user tries to enable an HTML
  overlay on any display, the API returns `409 Conflict` with a
  diagnostic pointing at the active HTML effect.
- When an HTML overlay is configured and the user activates an HTML
  effect, the HTML overlay is automatically disabled (its slot remains
  in config but `enabled` flips to false) and the API response includes
  a warning listing the overlays that were disabled.
- Native overlays (Clock, Sensor, Image, Text) have no such restriction
  and work alongside any effect, HTML or native.
- Native effects have no Servo footprint and can coexist with HTML
  overlays without conflict.

This is a known limitation, surfaced clearly in the API and docs, not a
workaround. The exclusion rule is deterministic and cheap to check: the
`AppState` already knows which effect is active and whether it's HTML.

### 11.3 When Servo Gains Multi-Session Support

If and when Servo exposes multi-session rendering (multiple independent
WebViews sharing one process, each with its own document and JS context),
Wave 3 revisits the design:

1. Each `HtmlOverlayRenderer` instance gets its own long-lived WebView,
   loaded with its overlay HTML document, never unloaded until the slot
   is removed.
2. Renders are dispatched per-WebView via a multi-session command queue
   inside the worker thread.
3. The per-WebView JS context persists, so `setInterval` callbacks,
   animation timers, and variable state all work normally.
4. HTML overlays coexist with HTML effects because each has its own
   session. Resource contention is managed by a per-session budget and
   a work-stealing scheduler inside the worker.

This design is already specified in Servo's long-term roadmap (see
offscreen render context PRs in `servo/servo`), but it's not something
Hypercolor can ship today. Wave 3 is explicitly blocked on upstream
progress.

### 11.4 Interim Workaround: Native Renderers and Separate Daemons

Users who want HTML-defined LCD faces today have two options:

1. **Use native overlay renderers.** The Clock, Sensor, Image, and Text
   renderers cover the common cases (clock faces, sensor gauges, temp
   displays, custom text). SVG templates via resvg let designers create
   custom clock faces and gauge artwork without writing Rust. For most
   users, this is enough.
2. **Wait for Wave 3.** Users with hard requirements on HTML face
   compatibility (existing HTML asset libraries, or very custom
   widgets not covered by the native renderers) need to wait for
   upstream Servo multi-session support. The strictly-interim
   alternative is running HTML effects and HTML overlays in separate
   daemon instances — one daemon per display — which is architecturally
   ugly but technically possible.

### 11.5 LightScript Sensor Injection (Already Partially Done)

Even though HTML overlays are gated on Wave 3, the LightScript runtime
already has every sensor API stub baked in. `window.engine.sensors`,
`window.engine.sensorList`, `window.engine.getSensorValue()`,
`window.engine.setSensorValue()`, and `window.engine.resetSensors()`
are all injected by the per-frame preamble today
(`crates/hypercolor-core/src/effect/lightscript.rs:161` onward). They
just report empty values because no real data source is connected.

Wave 0 closes that gap for HTML **effects** by wiring
`FrameInput::sensors` into the preamble generator. The same wiring
will trivially apply to HTML overlays once Wave 3 unblocks — there is
no per-consumer adapter to write, the preamble generator feeds the
same `window.engine.sensors` object to every active WebView.

```javascript
// In any LightScript-compatible HTML document
function updateDisplay() {
    const cpuTemp = engine.getSensorValue("cpu_temp");
    const gpuTemp = engine.getSensorValue("gpu_temp");
    const cpuLoad = engine.getSensorValue("cpu_load");
    // Render to canvas...
}
setInterval(updateDisplay, 2000);
```

### 11.6 HTML Overlay Property Declaration (Future, Gated on 11.3)

When Wave 3 unblocks, HTML overlay files will declare properties via
`<meta>` tags using the same syntax Hypercolor's effect loader already
parses:

```html
<meta property="textColor" label="Text Color" type="color" default="#ffffff" />
<meta property="fontSize" label="Font Size" type="number" min="20" max="200" default="80" />
<meta property="targetSensor" label="Sensor" type="sensor" default="CPU Temperature" />
<meta background="true" />
```

The parser already handles all of these control types — see
`HtmlControlKind::Sensor` at `effect/meta_parser.rs:18` and the full
set of kinds in `effect/loader.rs`. The overlay side just reuses the
existing meta parser once Wave 3's multi-session infrastructure lands.

---

## 12. API Surface

### 12.1 Display Overlay Endpoints

```
GET    /api/v1/displays
    List display devices with overlay support.
    Response: { data: [DisplayInfo], meta: {...} }

GET    /api/v1/displays/{device_id}/overlays
    Get overlay config for a display.
    Response: { data: DisplayOverlayConfig, meta: {...} }

PUT    /api/v1/displays/{device_id}/overlays
    Replace the entire overlay stack for a display.
    Body: DisplayOverlayConfig
    Response: { data: DisplayOverlayConfig, meta: {...} }

POST   /api/v1/displays/{device_id}/overlays
    Add an overlay slot to a display (appended to top of stack).
    Body: OverlaySlot (without id — server assigns)
    Response: { data: OverlaySlot, meta: {...} }

PATCH  /api/v1/displays/{device_id}/overlays/{slot_id}
    Update an existing overlay slot.
    Body: Partial<OverlaySlot>
    Response: { data: OverlaySlot, meta: {...} }

DELETE /api/v1/displays/{device_id}/overlays/{slot_id}
    Remove an overlay slot.
    Response: 204 No Content

POST   /api/v1/displays/{device_id}/overlays/reorder
    Reorder overlay stack (change z-order).
    Body: { slot_ids: [OverlaySlotId] }
    Response: { data: DisplayOverlayConfig, meta: {...} }
```

**HTML overlay conflict behavior (Wave 3 gate, see §11.2):** Until
upstream Servo supports multi-session rendering, HTML overlays are
mutually exclusive with HTML effects on the same daemon. The API
enforces this with explicit, diagnosable errors:

- `POST`/`PUT`/`PATCH` that enables an HTML overlay while an HTML
  effect is active returns `409 Conflict` with an error body identifying
  the active HTML effect and suggesting native overlay alternatives.
- Activating an HTML effect while HTML overlays are enabled auto-
  disables those overlays (flips `enabled: false`) and returns a
  `warnings` array in the effect activation response listing each
  auto-disabled overlay.
- Native overlays (Clock, Sensor, Image, Text) are never affected by
  this rule and always coexist with any effect type.

Per-slot runtime state is surfaced via the single-slot GET:

```
GET    /api/v1/displays/{device_id}/overlays/{slot_id}
    Single slot with runtime diagnostics.
    Response: {
        data: {
            slot: OverlaySlot,
            runtime: {
                last_rendered_at: Option<Timestamp>,
                consecutive_failures: u32,
                last_error: Option<String>,
                status: "active" | "disabled" | "failed" | "html_gated",
            }
        },
        meta: {...}
    }
```

### 12.2 System Sensor Endpoints

```
GET    /api/v1/system/sensors
    Latest system sensor snapshot.
    Response: { data: SystemSnapshot, meta: {...} }

GET    /api/v1/system/sensors/{label}
    Single sensor reading by label.
    Response: { data: SensorReading, meta: {...} }
```

### 12.3 Overlay Catalog Endpoint

```
GET    /api/v1/overlays/catalog
    Available overlay types with their config schemas.
    Response: { data: [OverlayCatalogEntry], meta: {...} }
```

### 12.4 MCP Tools

```
get_sensor_data        — Current system sensor snapshot
set_display_overlay    — Configure overlay for a display (AI-friendly)
list_display_overlays  — List overlays on all displays
```

---

## 13. Configuration

### 13.1 Config File

Overlay configs are stored per-device in the user config file alongside device
settings:

```toml
[displays.overlay."corsair-lcd-001"]
[[displays.overlay."corsair-lcd-001".overlays]]
id = "clock-1"
name = "Clock"
source = { type = "clock", style = "digital", hour_format = "24h",
           show_seconds = true, color = "#80ffea" }
position = { type = "anchored", anchor = "bottom_center",
             offset_x = 0, offset_y = -20, width = 200, height = 60 }
blend_mode = "normal"
opacity = 0.85
enabled = true

[[displays.overlay."corsair-lcd-001".overlays]]
id = "temp-1"
name = "CPU Temperature"
source = { type = "sensor", sensor = "cpu_temp", style = "gauge",
           range_min = 30.0, range_max = 100.0,
           color_min = "#80ffea", color_max = "#ff6363" }
position = { type = "anchored", anchor = "top_right",
             offset_x = -10, offset_y = 10, width = 120, height = 120 }
blend_mode = "normal"
opacity = 0.9
enabled = true
```

### 13.2 Sensor Polling Config

```toml
[sensors]
enabled = true
poll_interval_secs = 2
nvidia = true  # Enable NVIDIA GPU monitoring via NVML
```

### 13.3 SVG Template Directory

Built-in SVG templates ship in `assets/overlay-templates/`:

```
assets/overlay-templates/
├── clocks/
│   ├── digital-default.svg
│   ├── analog-minimal.svg
│   └── analog-classic.svg
├── gauges/
│   ├── radial-default.svg
│   ├── radial-thin.svg
│   └── bar-horizontal.svg
└── frames/
    ├── circle-border.svg
    └── rounded-rect.svg
```

Users can add custom SVG templates to `~/.config/hypercolor/templates/`.

---

## 14. GPU Acceleration Path

When the render pipeline modernization (design doc 28, Wave 7) delivers a wgpu
compositor, overlays gain an optional GPU path.

### 14.1 GPU Overlay Composition

```
effect_canvas → GPU texture (upload or GPU-resident if wgpu renderer)
overlay[0] → GPU texture (cached, updated rarely)
overlay[1] → GPU texture (cached, updated rarely)
    │
    ▼
GPU compositor: fullscreen quad per layer, alpha blend shader
    │
    ▼
GPU readback → CPU JPEG encode → USB
```

Overlay textures are uploaded once on render and cached until the next overlay
update. Since overlays update at 0.5–1 Hz and display output runs at 15 Hz,
the GPU texture cache hit rate is 93–97%.

### 14.2 GPU-Native Overlay Renderers (Future)

For maximum GPU residency, overlay renderers could also be wgpu shaders:
- **SDF text rendering** for clock digits and sensor values
- **Texture sampling** for image overlays
- **Parametric arc shaders** for gauge sweeps

This eliminates the CPU render → upload → GPU compose round-trip entirely.
The overlay never leaves the GPU until the final JPEG encode readback.

This is explicitly future work and not required for the initial delivery.

---

## 15. Delivery Waves

Waves are ordered by dependency, not by feature glamour. Wave 0 delivers
value on its own (sensor data for effects), Wave 1 lands the compositor
infrastructure, Wave 2 ships the native renderers that users actually see,
Wave 3 is explicitly blocked on upstream Servo, and Waves 4–5 are small
follow-ons that extend the base.

Every wave must produce benchmark evidence before its exit criteria are
considered met. See §16 for the benchmark policy.

### Wave 0 — Sensor Pipeline End-to-End

**Goal:** System sensor data flowing from hardware through `FrameInput`
to native effects, HTML effects, the LightScript JS stubs, and the REST
API. This is the foundation everything else builds on and it delivers
user-visible value on its own — effects can react to CPU temperature
before any overlay work lands.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 0.1 | Add `sysinfo` dependency, `SystemSnapshot` and `SensorReading` types, `SensorUnit` enum | types | Serde round-trip tests, empty-snapshot constructor |
| 0.2 | Add optional `nvml-wrapper` behind `nvidia` feature flag with graceful fallback when `libnvidia-ml.so` is missing | core | Feature-gated compile, runtime probe test |
| 0.3 | Implement `SensorPoller` as a background OS thread with `tokio::sync::watch::Sender<Arc<SystemSnapshot>>` | core | Poller produces monotonic snapshots, watch receiver updates |
| 0.4 | Extend `InputData` enum with `Sensors(Arc<SystemSnapshot>)` variant | core | Serde tests |
| 0.5 | Extend `FrameInputs` with `sensors: Arc<SystemSnapshot>` field | daemon | Frame scheduler tests |
| 0.6 | Extend `FrameInput<'a>` with `sensors: &'a SystemSnapshot` reference | core | EffectRenderer tick tests read the reference |
| 0.7 | Wire `SensorPoller` watch receiver into `InputManager` and pull latest snapshot during `sample_inputs` | daemon | Integration test: snapshot propagates to frame inputs |
| 0.8 | Update LightScript per-frame preamble generator to populate `window.engine.sensors`, `sensorList`, and related stubs from `FrameInput::sensors` | core | JS test: existing `engine.getSensorValue()` returns real data |
| 0.9 | Add `GET /api/v1/system/sensors` endpoint + `get_sensor_data` MCP tool | daemon | curl test + MCP test |
| 0.10 | Benchmark: `sample_inputs` cost with sensors wired, `FrameInput` build cost | daemon | No measurable regression on render pipeline |

**Exit criteria:** `GET /api/v1/system/sensors` returns live CPU temperature,
load, and memory data. HTML effects using `engine.getSensorValue("cpu_temp")`
see real values. Native effects receive `FrameInput::sensors`. Render loop
timing unchanged (<1% overhead from sensor sampling). NVIDIA GPU telemetry
available when `nvidia` feature is enabled and hardware is present.

---

### Wave 1 — Overlay Infrastructure

**Goal:** The per-display overlay compositor exists, accepts a config via
watch channel, and can blit positioned buffers onto the staging frame.
Tests with a `MockRenderer` that produces solid-color buffers — no real
tiny-skia work yet.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 1.1 | Add overlay config types (`DisplayOverlayConfig`, `OverlaySlot`, `OverlaySlotId`, `OverlaySource`, `OverlayPosition`, `Anchor`, `OverlayBlendMode`) | types | Serde round-trip tests |
| 1.2 | Add `OverlayRenderer` trait, `OverlayBuffer`, `OverlayInput`, `OverlayError`, `OverlaySize` | core | Trait is object-safe, compiles |
| 1.3 | Extract sRGB LUTs and per-pixel blend math from `sparkleflinger.rs` into a shared `blend_math` module under `hypercolor-core` | core | SparkleFlinger tests still pass, new module has unit tests |
| 1.4 | Implement `PremulStaging` (worker-local premul RGBA staging buffer with straight↔premul copy helpers) | daemon | Round-trip pixel tests, including edge alpha values |
| 1.5 | Implement `OverlayComposer` with config reconciliation, cadence-gated render, positioned alpha blit, and the full failure policy from §9.7 | daemon | Unit tests for reconciliation, blit, backoff, error classification |
| 1.6 | Wire `OverlayComposer` into the display worker between viewport sampling and brightness/mask/JPEG | daemon | Integration test: mock overlay composites onto real viewport frame |
| 1.7 | Add `watch::Receiver<Arc<DisplayOverlayConfig>>` to `DisplayWorkerHandle::spawn`, resolved from `AppState` by `DeviceId` | daemon | Config update triggers worker refresh within one tick |
| 1.8 | Add display overlay REST endpoints (list, get, replace, add, patch, delete, reorder) per §12.1 | daemon | API integration tests, 409 on invalid slot ordering |
| 1.9 | `MockRenderer` test double that produces solid-color buffers for harness tests | daemon | Used by §1.5 and §1.6 tests |
| 1.10 | Benchmarks: display output with 0, 1, 2, 4 overlay layers using `MockRenderer`; zero-overlay bypass path vs overlay-disabled baseline | daemon | 0-overlay bypass matches baseline within noise; 4-layer overhead ≤1 ms at 480×480 |

**Exit criteria:** A `MockRenderer` overlay (positioned 120×120 solid red at
50% alpha) composites visibly on a Corsair LCD. API can add, move, remove,
and reorder overlays. Zero-overlay path has zero measurable regression.
Failure policy is tested end-to-end with an intentionally-failing mock.

---

### Wave 2 — Native Overlay Renderers

**Goal:** Ship the four built-in renderers so users can configure clock
faces, sensor gauges, images, and text overlays via the API and see them
on real hardware.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 2.1 | Add `tiny-skia`, `cosmic-text`, `resvg`, `image`, `gif` dependencies | core | `just check`, license compatibility check via `cargo deny` |
| 2.2 | Implement tiny-skia `Pixmap` → `OverlayBuffer` bridge (premul→premul byte copy, width/height match, tests for alpha edge cases per §6.3) | core | Round-trip pixel accuracy tests with hand-computed reference values |
| 2.3 | Add `ClockConfig`, `SensorOverlayConfig`, `ImageOverlayConfig`, `TextOverlayConfig` types | types | Serde round-trip tests |
| 2.4 | Implement `ClockRenderer` (digital + analog, SVG template support, cosmic-text digits) | core | Visual snapshot tests at 480×480, 240×240, 120×120 |
| 2.5 | Implement `SensorRenderer` (numeric + gauge + bar, SVG template, color interpolation) | core | Snapshot tests with mock sensor data at various values |
| 2.6 | Implement `ImageRenderer` (PNG/JPEG/WebP static + animated GIF with frame cycling, `ImageFit` modes) | core | Load test images, GIF frame timing test |
| 2.7 | Implement `TextRenderer` (cosmic-text layout, horizontal scroll, `{sensor:label}` interpolation from `OverlayInput::sensors`) | core | Text layout tests, scroll animation test, interpolation tests |
| 2.8 | Ship default SVG templates (2 clock faces, 2 gauge styles, 1 frame border) under `assets/overlay-templates/` | assets | Templates render correctly via resvg, visual regression snapshots |
| 2.9 | Wire renderer factory: `OverlaySource` → `Box<dyn OverlayRenderer>` | core | Factory dispatches correctly for all types |
| 2.10 | End-to-end hardware test: clock + sensor gauge on Corsair LCD alongside a running effect | daemon | Manual visual verification |
| 2.11 | Benchmarks: per-renderer render cost at 480×480 and 120×120, full-pipeline cost with 2 overlays vs 0 | daemon | Clock ≤500 µs, sensor gauge ≤500 µs, full pipeline ≤1.5 ms extra at 2 overlays |

**Exit criteria:** A user can configure a clock and CPU temperature gauge
on their AIO LCD via the API, both render correctly with transparency over
the running effect. Animated GIF overlays cycle frames smoothly. Text
overlays interpolate live sensor values.

---

### Wave 3 — HTML Overlays via Servo (Blocked on Upstream)

**Goal:** HTML overlay files render through Servo alongside HTML effects.

**Status:** **BLOCKED.** The Servo worker is built around a single active
WebView and a single JS context per page. HTML overlays cannot share the
worker with HTML effects without tearing down the effect's JS state per
render cycle, which is architecturally broken (see §11.1). This wave
waits on upstream Servo multi-session rendering support, which is on the
Servo roadmap but not shippable today.

**Tasks (when unblocked):**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 3.1 | Upgrade Servo integration to multi-session WebView API | core | Multiple independent WebViews share one Servo process |
| 3.2 | Extend `ServoWorker` with per-session command routing, session lifecycle, and per-session budget | core | Sessions isolated, crashes don't leak between sessions |
| 3.3 | Implement `HtmlOverlayRenderer` with a long-lived per-slot WebView session | core | Render test with simple HTML overlay |
| 3.4 | Remove the HTML-effect vs HTML-overlay mutual-exclusion check in the API (§11.2) | daemon | API integration test: both can be active |
| 3.5 | End-to-end HTML overlay test: clock, sensor readout, custom text HTML files render and composite correctly | daemon | Reference HTML overlays render without modification |
| 3.6 | Document HTML overlay authoring (meta properties, sensor API, examples) | docs | Written, examples work |

**Interim behavior (until Wave 3 unblocks):** The API accepts HTML overlay
config but returns `409 Conflict` when an HTML effect is active on the
same daemon. The error message points users at Wave 3's upstream gate and
suggests native overlay renderers as the alternative.

**Exit criteria (future):** Reference HTML overlays (clock face, sensor
readout, custom text) render correctly as Hypercolor overlays alongside
HTML effects with no per-frame Servo contention.

---

### Wave 4 — Effect Sensor Control Binding

**Goal:** Effects can bind individual controls to system sensor values,
completing the `ControlKind::Sensor` path that already exists in the
type system.

**Note:** `ControlKind::Sensor` already exists at
`hypercolor-types/src/effect.rs:197`. The LightScript runtime already
plumbs `engine.getSensorValue()` (see Wave 0.8). What's missing is the
**binding layer** that maps sensor values into effect control values
for native effects and HTML effects that want declarative bindings
rather than JS-level polling.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 4.1 | Add `ControlBinding` type with source (sensor label), target range, dead-band, and smoothing | types | Serde tests |
| 4.2 | Add `effect.controls[name].binding = ControlBinding` serialization | types | Round-trip tests |
| 4.3 | Wire binding evaluation into `EffectEngine::prepare_frame` (reads sensor from `FrameInputs::sensors`, maps to control range, writes to control store) | core | Unit test: bound control value reflects sensor reading |
| 4.4 | Add binding API: `PUT /api/v1/effects/current/controls/{name}/binding` | daemon | API test: bind CPU temp to a slider control |
| 4.5 | Benchmarks: effect frame cost with 0, 1, 5 sensor bindings | core | ≤20 µs overhead per binding |

**Exit criteria:** An effect's slider or color control can be bound to
CPU temperature, transitioning from 0.0 at 30°C to 1.0 at 100°C
automatically. Both native and HTML effects honor the binding.

---

### Wave 5 — GPU Composition (Blocked on Render Modernization)

**Goal:** Optional wgpu-based overlay composition for display output,
eliminating the premul staging conversion round-trip.

**Blocked by:** Render pipeline modernization Wave 7 (design doc 28).

**Tasks (when unblocked):**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 5.1 | GPU overlay texture cache (upload on render, cache until next update) | daemon | Texture reuse across frames |
| 5.2 | Fullscreen-quad alpha blend shader for positioned overlay composition | daemon | Visual parity with CPU path |
| 5.3 | GPU → CPU readback for JPEG encode | daemon | Correct JPEG output from GPU-composed frame |
| 5.4 | Parity tests: CPU vs GPU overlay composition | daemon | Pixel-level comparison within tolerance (sRGB ±2/channel) |
| 5.5 | Benchmark: GPU vs CPU overlay composition at 480×480 with 0, 2, 8 overlays | daemon | Measurable improvement at 2+ overlays; no regression at 0 overlays |

**Exit criteria:** GPU overlay composition produces visually identical
output to the CPU path with measurable performance improvement at 2+
overlay layers.

---

## 16. Verification Strategy

### 16.1 Per-Wave Checks

- `just verify` (fmt + lint + test) after every wave
- Targeted crate tests for all new types and traits
- Integration tests for API endpoints
- Manual hardware verification on Corsair LCD for visual correctness

### 16.2 Performance Benchmarks

Every wave must produce benchmark evidence:

| Benchmark | Measured | Baseline |
|-----------|----------|----------|
| Display output: 0 overlays | Total encode+send time | Wave 0 (no regression) |
| Display output: 2 overlays | Overlay compose time | <200 µs at 480x480 |
| Overlay render: clock (tiny-skia) | Render time | <500 µs at 480x480 |
| Overlay render: sensor gauge | Render time | <500 µs at 480x480 |
| Sensor poll cycle | Poll duration | <50 ms |
| Display output FPS | Sustained throughput | 15 FPS maintained |

### 16.3 Visual Parity

Snapshot tests compare overlay renders against reference images with a
per-pixel tolerance (sRGB ±2 per channel) to catch rendering regressions
without being brittle to anti-aliasing differences.

### 16.4 Failure Policy Testing

Wave 1 exit includes end-to-end tests of the failure policy from §9.7:

- `Asset` errors from an intentionally-malformed SVG disable the slot and
  surface the error via the API
- `Transient` errors from a flaky mock renderer trigger exponential
  backoff and preserve the last good buffer
- `Fatal` errors tear down the renderer and trigger recreation on the
  next config update
- Repeated failures escalate `Transient` to `Asset` after 5 consecutive
  misses as specified

### 16.5 Hardware Matrix

Wave 2 exit requires visual verification on at least:

- One 480×480 circular display (Corsair iCUE LINK, XC7 Elite, or Elite
  Capellix)
- One 480×480 square display (XD6 Elite)
- One non-square display (Push 2, 960×160)

The matrix exists to catch aspect-ratio and circular-mask bugs early.
Future displays with different resolutions or aspect ratios are
automatically covered by the same tests with different viewport
parameters.

---

## 17. Recommendation

Build this in five stages, gated on dependencies rather than feature
glamour:

1. **Wave 0 (sensor pipeline end-to-end)** delivers standalone value
   immediately. Effects — native and HTML — gain live system sensor data
   through `FrameInput::sensors`, the REST API exposes the snapshot, and
   the LightScript JS stubs (`engine.getSensorValue`, `engine.sensors`)
   start returning real data. No overlays yet, but users can already
   build CPU-temperature-reactive effects. This is worth shipping on its
   own.

2. **Wave 1 (overlay infrastructure)** lands the per-display compositor,
   the shared `blend_math` module, the premultiplied staging pipeline,
   config reconciliation, failure policy, and the REST API. Tested
   with a `MockRenderer` — no tiny-skia yet. This is the foundation
   that Wave 2 builds on.

3. **Wave 2 (native renderers)** is the feature users see. Clock, sensor
   gauge, image/GIF, and text overlays with SVG template support. The
   rendering stack (tiny-skia + cosmic-text + resvg) is pure Rust,
   actively maintained by Linebender and System76, SIMD-optimized, and
   adds ~2 MiB to the binary. No other combination offers the same
   quality-to-weight ratio for CPU rendering at 480×480.

4. **Wave 4 (effect sensor binding)** completes the `ControlKind::Sensor`
   path that already exists in the type system. Small, focused, unlocks
   declarative "bind this slider to CPU temp" use cases without needing
   users to write JavaScript.

5. **Waves 3 and 5 are blocked and explicitly scoped as such.** Wave 3
   (HTML overlays) requires upstream Servo multi-session rendering
   support. Wave 5 (GPU composition) requires render pipeline
   modernization Wave 7. Both waves have clear entry conditions and are
   not on the critical path for shipping the feature.

The research phase surveyed 15+ open-source projects working in this
space and found a clear gap: nobody has a general-purpose compositor
that takes an effect-rendered canvas as input, composites widget
overlays with alpha blending and z-ordering on top, and outputs to
arbitrary display hardware on Linux. Hypercolor can be the first to
unify effect rendering, overlay compositing, system sensor data, and
hardware display output into a single coherent pipeline.

And because the composition point is the display worker — not the
display transport — the architecture holds for any future pixel-
addressable output. An HDMI capture card, an e-paper panel, a DSI-
connected small display, or a framebuffered DRM output all plug into
the same overlay compositor the moment they expose themselves through
`DeviceTopologyHint::Display`. The spec delivers AIO LCD overlays
today and absorbs "any screen" as hardware support expands.
