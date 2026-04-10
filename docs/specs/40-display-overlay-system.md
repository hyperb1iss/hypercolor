# Spec 40 — Display Overlay System

> Compositable widget overlays for AIO and LCD displays — clock faces, system
> monitors, images, and HTML content layered on top of the active effect.

**Status:** Draft
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

AIO liquid cooler LCDs and other display devices (Corsair iCUE LINK, XC7 Elite,
XD6 Elite, Ableton Push 2, future NZXT Kraken) currently show the effect canvas
sampled through a viewport, period. Users cannot overlay a clock, CPU
temperature, custom image, or animated GIF on top of the running effect without
replacing the entire display content.

This spec adds a **per-display overlay compositor** that layers user-configured
widgets on top of the viewport-sampled effect canvas before JPEG encoding. The
overlay system is display-specific — LED strips and LED zones are unaffected.

The approach:

- Overlays are rendered by lightweight **overlay renderers** (native Rust
  widgets for clocks, sensors, images, text) that produce transparent RGBA
  canvases at the display's native resolution
- A **display overlay compositor** blends overlay layers on top of the
  viewport-sampled effect canvas using SparkleFlinger's existing blend math
- A **system sensor pipeline** polls CPU/GPU temperatures, load, memory, and
  fan speeds via `sysinfo` (with optional `nvml-wrapper` for NVIDIA GPUs)
  and publishes snapshots for overlay renderers and effects to consume
- **HTML overlays via Servo** come in a later phase, reusing the LightScript
  runtime with SignalRGB-compatible `<meta>` properties and
  `engine.getSensorValue()` for sensor data
- The architecture anticipates optional GPU composition (wgpu texture layers,
  shader-based blending) once the render pipeline modernization (design doc
  28, Wave 7) lands

---

## 2. Problem Statement

### 2.1 What Users Want

Every competing RGB platform (SignalRGB, NZXT CAM, iCUE, Lian Li L-Connect)
supports overlaying widgets on AIO LCD screens. The common request is: "I want
to see my CPU temperature on top of whatever effect is running." Today
Hypercolor forces a binary choice: effect OR custom content, never both.

### 2.2 What the Ecosystem Lacks

Research across 15+ open-source AIO LCD projects reveals a clear gap. Nobody
has a general-purpose compositor that takes an effect-rendered canvas as input,
composites widget overlays with alpha blending and z-ordering on top, and
outputs to arbitrary LCD hardware:

| Project | Stack | Compositing | Limitation |
|---------|-------|-------------|------------|
| SignalRGB | Qt WebEngine | HTML overlay on effect | Windows-only, closed source |
| NZXT-ESC | React/TS | 20-element overlay editor | Windows-only, depends on CAM |
| TRCC Linux | Python/Qt | Runtime overlay composition | Text/sensor overlays only, no effects |
| trlcd_libusb | C | Alpha-blended framebuffer | Single device, no effect engine |
| AIOLCDUnchained | Python | Temperature on SignalRGB canvas | Rudimentary, not general-purpose |
| OpenLinkHub | Go | None | Static image/GIF push only |
| lian-li-linux | Rust/Slint | None | Direct media push only |
| CoolerControl | Rust/Tauri | None (via CoolerDash plugin) | Fixed display modes |

Hypercolor has the compositor (SparkleFlinger), the effect engine (Servo +
native renderers), and the display output pipeline. The missing piece is the
overlay layer between them.

### 2.3 Architectural Fit

Overlays are **per-display, not per-effect.** A user might want a clock on
their AIO pump and a temperature gauge on their Corsair LCD, but no overlay on
LED strips. This means overlay composition belongs in the display output path,
not in the main render pipeline. The main render loop remains unchanged.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- Per-display overlay configuration: each LCD device can have independent
  overlay stacks
- Built-in overlay renderers: clock, system sensor gauge, static/animated
  image, styled text
- System sensor pipeline that feeds overlay renderers, effects, and the API
- Alpha-composited overlay layers using SparkleFlinger's existing blend math
- Overlay caching: renderers only re-render when their content changes (clock
  every second, sensor every 2 seconds, static image never)
- REST API for CRUD on per-display overlay configurations
- SVG-based overlay templates so artists can design faces without Rust
- Circular display awareness (mask overlays to match circular AIO LCDs)
- No performance regression on the main render loop or LED output path

### 3.2 Non-Goals

- Replacing the main render pipeline or effect engine
- GPU-native overlay rendering in the first pass (future Wave)
- User-facing overlay editor UI (the API enables third-party or future UI work)
- Audio-reactive overlays (possible via FrameInput later, not in scope)
- Video playback overlays (ffmpeg integration is a separate concern)
- Overlay support for LED-only devices (no pixel surface to composite onto)

---

## 4. Landscape

### 4.1 Rendering Crates

| Crate | Version | Role | Notes |
|-------|---------|------|-------|
| **tiny-skia** | 0.12.0 | 2D rasterizer + compositor | Pure Rust, SIMD-optimized (SSE2/AVX2/NEON). Pixmap operates on premultiplied RGBA — same layout as Canvas. Actively maintained by Linebender. |
| **cosmic-text** | 0.18.2 | Text layout + shaping | Full Unicode via rustybuzz, variable fonts, color emoji, subpixel positioning. Maintained by System76. |
| **resvg** | 0.47.0 | SVG rendering | Full SVG 1.1 via tiny-skia backend. Enables artist-editable clock face and gauge templates as SVG assets. |
| **image** | 0.25.10 | Image loading | PNG, JPEG, WebP decode. RGBA buffer compatible with tiny-skia Pixmap. |
| **gif** | — | Animated GIF decode | Streaming frame-by-frame decode with disposal method handling. Part of image-rs ecosystem. |
| **sysinfo** | 0.38.3 | System monitoring | CPU temps/load, memory, components (hwmon/sysfs). Cross-platform. |
| **nvml-wrapper** | latest | NVIDIA GPU monitoring | Optional. Wraps NVML for GPU temp, load, VRAM, fan speed. Requires libnvidia-ml.so at runtime. |

### 4.2 Key Design Inspiration

- **trlcd_libusb**: Oversized framebuffer with viewport crop, INI-driven layout
  config, alpha-blended layer compositing. The only open-source project doing
  real per-pixel alpha blending for LCD overlays.
- **NZXT-ESC**: Per-element transforms (position, rotation, scale, opacity) with
  z-ordering, 20-element cap. The UX target for what a rich overlay editor
  should feel like.
- **TRCC Linux**: Hexagonal architecture with interchangeable UI adapters over
  shared services. Runtime overlay composition with base layer + N overlay
  elements. 77+ sensor types.
- **SignalRGB**: HTML `<meta>` property declarations, `engine.getSensorValue()`
  for sensor data, transparent background compositing. Hypercolor's LightScript
  runtime already implements the same contract.

---

## 5. Architecture

### 5.1 Composition Point

Overlay composition happens **inside the display output worker**, between
viewport sampling and JPEG encoding. This keeps the main render pipeline
completely unchanged and makes overlays naturally per-device.

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
                ┌────────┴────────┐         ┌────────┴────────┐                  │
                │ OverlayComposer │         │ OverlayComposer │           (no overlays)
                │  [clock, temp]  │         │  [sensor gauge] │                  │
                └────────┬────────┘         └────────┬────────┘                  │
                         │                           │                           │
                  brightness + mask           brightness + mask           brightness + mask
                         │                           │                           │
                    JPEG encode                 JPEG encode                 JPEG encode
                         │                           │                           │
                      USB bulk                    USB bulk                    USB bulk
```

### 5.2 Overlay Renderer Model

Each overlay type implements the `OverlayRenderer` trait, which produces a
transparent RGBA canvas on demand:

```
                    OverlayRegistry
                    ┌─────────────────────────────────────────┐
                    │  OverlayInstance                        │
                    │  ├── renderer: Box<dyn OverlayRenderer> │
                    │  ├── config: OverlaySlot                │
                    │  ├── cached_canvas: Option<Canvas>      │
                    │  └── last_render_at: Instant             │
                    │                                          │
                    │  OverlayInstance                        │
                    │  ├── renderer: ...                       │
                    │  └── ...                                 │
                    └─────────────────────────────────────────┘
```

Renderers cache aggressively. A clock overlay re-renders once per second. A
temperature gauge every 2 seconds. A static image never. Between renders, the
compositor blends the cached canvas — a single alpha composite pass at 480x480
costs microseconds.

### 5.3 Sensor Data Flow

```
SensorPoller (background thread, 1-2s interval)
    ├── sysinfo::System (CPU, memory, components)
    └── nvml::Device (optional, NVIDIA GPU)
            │
            ▼
    Arc<SystemSnapshot> via watch::channel
            │
            ├──► OverlayRenderers (sensor gauge, temp display)
            ├──► LightScript runtime (engine.getSensorValue())
            ├──► Effect controls (ControlType::Sensor binding)
            └──► REST API (GET /api/v1/system/sensors)
```

---

## 6. Overlay Rendering Stack

### 6.1 Why tiny-skia

The overlay renderer needs to draw arcs (gauge sweeps), filled shapes (clock
faces), stroked paths (clock hands), text (sensor readouts), and composite
images (PNG icons) onto a pixel buffer at 480x480. Options considered:

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **tiny-skia** | Pure Rust, SIMD, sub-ms at 480x480, Pixmap is RGBA like Canvas | 20-100% slower than native Skia | **Use.** Speed is irrelevant at 480x480. |
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

### 6.3 Pixmap ↔ Canvas Bridge

tiny-skia's `Pixmap` stores premultiplied RGBA. Hypercolor's `Canvas` stores
straight (non-premultiplied) sRGB RGBA. The bridge must convert between them.

At overlay render time: render into Pixmap (premultiplied), then un-premultiply
into a Canvas for SparkleFlinger. SparkleFlinger's blend math converts back to
linear premultiplied internally, so the round-trip is: premul → straight →
linear premul. This matches the existing blend path and preserves correctness.

For the GPU path (Phase 4), overlays stay premultiplied all the way through.

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
    /// Timestamp of this snapshot.
    pub polled_at: Instant,
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

### 7.4 Consumers

1. **Overlay renderers** receive `Arc<SystemSnapshot>` via `OverlayInput`.
2. **LightScript runtime** exposes `engine.getSensorValue(label)` by searching
   `components` by label. Falls back to well-known keys: `"cpu_temp"`,
   `"gpu_temp"`, `"cpu_load"`, `"gpu_load"`, `"ram_used"`.
3. **Effect controls** can bind a `ControlType::Sensor` to a sensor label,
   mapping the sensor's range to the control's `[0.0, 1.0]` range. This is
   the "Link to CPU Temperature" use case from design doc 02.
4. **REST API** exposes the latest snapshot at `GET /api/v1/system/sensors`.

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
    /// Path to HTML file (Phase 3).
    pub path: String,
    /// Property overrides (SignalRGB-compatible meta properties).
    pub properties: HashMap<String, serde_json::Value>,
    /// Render cadence in milliseconds (default 1000).
    pub render_interval_ms: u32,
}
```

### 8.3 Overlay Renderer Trait

```rust
/// Produces overlay canvases on demand. Implementations are Send but NOT Sync
/// (same constraint as EffectRenderer — Servo is single-threaded).
pub trait OverlayRenderer: Send {
    /// Render the overlay content to a transparent RGBA canvas at the given
    /// display dimensions. Called only when `needs_redraw` returns true.
    fn render(&mut self, input: &OverlayInput) -> Result<Canvas>;

    /// How often this overlay should be re-rendered. The compositor checks
    /// elapsed time against this interval and only calls `render` when due.
    fn update_interval(&self) -> Duration;

    /// Whether the overlay content has changed since the last render,
    /// independent of the time interval. For example, a sensor overlay
    /// returns true when the sensor value has changed meaningfully.
    fn needs_redraw(&self, input: &OverlayInput) -> bool;

    /// Update configuration. Called when the user modifies overlay settings
    /// via the API. Returns true if the overlay needs an immediate re-render.
    fn update_config(&mut self, source: &OverlaySource) -> Result<bool>;

    /// Release resources (font caches, image buffers, SVG trees).
    fn destroy(&mut self);
}

/// Input provided to overlay renderers each time they are asked to render.
pub struct OverlayInput {
    /// Wall-clock time for clock overlays.
    pub now: SystemTime,
    /// Display width in pixels.
    pub display_width: u32,
    /// Display height in pixels.
    pub display_height: u32,
    /// Whether the display is circular (for masking hints).
    pub circular: bool,
    /// Latest system sensor snapshot.
    pub sensors: Arc<SystemSnapshot>,
    /// Elapsed seconds since overlay was created (for animations).
    pub elapsed_secs: f32,
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
CanvasFrame → viewport sample → overlay compose → brightness → circular mask → JPEG → USB
                                      │
                        OverlayComposer::compose()
                         ├── base: viewport-sampled RGB → Canvas
                         ├── overlay[0]: cached/fresh Canvas (alpha)
                         ├── overlay[1]: cached/fresh Canvas (alpha)
                         └── result: composed Canvas
```

### 9.3 OverlayComposer

The `OverlayComposer` lives inside each `DisplayWorkerHandle` and owns the
overlay instances for that display. It is created when the display worker
spawns and updated when overlay config changes via a `watch` channel.

```rust
pub(crate) struct OverlayComposer {
    /// Per-slot overlay instances, ordered bottom-to-top.
    instances: Vec<OverlayInstance>,
    /// Shared sensor snapshot receiver.
    sensor_rx: watch::Receiver<Arc<SystemSnapshot>>,
    /// Cached composed result (reused when no overlay needs re-rendering).
    cached_composed: Option<Canvas>,
    /// Whether any overlay has been modified since last compose.
    dirty: bool,
    /// Display geometry for overlay input.
    display_width: u32,
    display_height: u32,
    circular: bool,
    /// When the overlay stack was created (for elapsed_secs).
    created_at: Instant,
}

struct OverlayInstance {
    slot: OverlaySlot,
    renderer: Box<dyn OverlayRenderer>,
    cached_canvas: Option<Canvas>,
    last_rendered_at: Option<Instant>,
}
```

### 9.4 Compose Algorithm

```
fn compose(&mut self, base_canvas: &Canvas) -> &Canvas:
    if no overlays enabled:
        return base_canvas   // zero-cost passthrough

    let input = build_overlay_input()
    let any_updated = false

    for instance in &mut self.instances:
        if !instance.slot.enabled:
            continue
        let interval_elapsed = instance.last_rendered_at
            .is_none_or(|t| t.elapsed() >= instance.renderer.update_interval())
        if interval_elapsed || instance.renderer.needs_redraw(&input):
            instance.cached_canvas = Some(instance.renderer.render(&input)?)
            instance.last_rendered_at = Some(Instant::now())
            any_updated = true

    if !any_updated && self.cached_composed.is_some():
        return self.cached_composed  // reuse cached composite

    // Compose layers using SparkleFlinger blend math
    let mut composed = base_canvas.clone()  // base layer (effect viewport)
    for instance in &self.instances:
        if !instance.slot.enabled:
            continue
        if let Some(overlay_canvas) = &instance.cached_canvas:
            let positioned = position_overlay(overlay_canvas, &instance.slot.position,
                                              self.display_width, self.display_height)
            blend_layer(&mut composed, &positioned,
                        instance.slot.blend_mode, instance.slot.opacity)

    self.cached_composed = Some(composed.clone())
    return self.cached_composed
```

### 9.5 Integration with DisplayWorkItem

The `DisplayWorkItem` gains an optional overlay config reference. The display
output thread resolves overlay configs from `AppState` when building work items,
and passes a `watch::Receiver<DisplayOverlayConfig>` to each worker at spawn.

### 9.6 Performance Budget

At 480x480 with 2 overlay layers:

| Step | Estimated Cost | Notes |
|------|---------------|-------|
| Overlay render (cached) | 0 µs | Skipped most frames |
| Overlay render (fresh, tiny-skia) | 200–500 µs | Clock face with text, once/sec |
| Position + blend per layer | 50–100 µs | Alpha composite 480x480 |
| Total overlay overhead | 100–200 µs typical | Dominated by blend, not render |
| Current JPEG encode | 1–3 ms | Unchanged |

The overlay pass adds roughly 10% overhead to the display output pipeline,
well within the 15 FPS budget (66 ms per frame). The JPEG encode remains the
bottleneck.

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

Phase 3 adds HTML overlay support, reusing the existing Servo worker and
LightScript runtime.

### 11.1 Challenge: Single Servo Instance

Servo is process-global (OnceLock). The active effect already occupies it at
full frame rate. HTML overlays must not compete for frames.

### 11.2 Approach: Low-Cadence Timeshare

HTML overlays typically need much lower cadence than effects:

| Overlay Content | Required Cadence |
|----------------|-----------------|
| Clock | 1 FPS |
| Sensor readout | 0.5 FPS |
| Animated widget | 5–10 FPS |
| Static content | Render once |

The Servo worker gains an **overlay render queue** alongside the existing
effect render pipeline. Between effect frames, when budget headroom exists, the
worker services one pending overlay render request. The overlay result is cached
and reused until the next render is scheduled.

Workflow:
1. `HtmlOverlayRenderer` submits a render request to the Servo overlay queue
2. The Servo worker services overlay requests during idle budget slices
3. Completed overlay renders are sent back via a oneshot channel
4. The `HtmlOverlayRenderer` caches the result as its overlay canvas
5. If budget is tight, overlay renders are deferred — the cached canvas is
   used until the next successful render

This approach has bounded impact on the effect pipeline. Overlay renders are
strictly lower priority than effect frames.

### 11.3 LightScript Sensor Integration

The LightScript runtime gains `engine.getSensorValue(label)`:

```javascript
// In the overlay HTML
function updateDisplay() {
    const cpuTemp = engine.getSensorValue("cpu_temp");
    const gpuTemp = engine.getSensorValue("gpu_temp");
    const cpuLoad = engine.getSensorValue("cpu_load");
    // Render to canvas...
}
setInterval(updateDisplay, 2000);
```

The runtime injects sensor values from the latest `SystemSnapshot` as a JS
object on `window.engine.sensors`, enabling both polling via
`getSensorValue()` and reactive access.

### 11.4 SignalRGB Compatibility

Overlay HTML files use the same `<meta>` property declarations as SignalRGB
LCD faces:

```html
<meta property="textColor" label="Text Color" type="color" default="#ffffff" />
<meta property="fontSize" label="Font Size" type="number" min="20" max="200" default="80" />
<meta property="targetSensor" label="Sensor" type="sensor" default="CPU Temperature" />
<meta background="true" />
```

This means SignalRGB LCD face HTML files should work in Hypercolor with
minimal or no modification.

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

### Wave 0 — Sensor Pipeline Foundation

**Goal:** System sensor data flowing through the daemon, accessible to
overlays, effects, and the API.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 0.1 | Add `sysinfo` dependency, `SystemSnapshot` and `SensorReading` types | types | `just check` |
| 0.2 | Implement `SensorPoller` background thread with watch channel | daemon | Unit test: poller produces valid snapshots |
| 0.3 | Add optional `nvml-wrapper` behind `nvidia` feature flag | daemon | Feature-gated compile, graceful fallback |
| 0.4 | Wire `SensorPoller` into `AppState`, start on daemon boot | daemon | `just daemon`, verify sensor data in logs |
| 0.5 | Add `GET /api/v1/system/sensors` endpoint | daemon | curl test, verify JSON response |
| 0.6 | Add `get_sensor_data` MCP tool | daemon | MCP tool test |
| 0.7 | Wire sensor snapshot into LightScript: `engine.getSensorValue()` | core | Unit test: JS can read sensor values |

**Exit criteria:** `GET /api/v1/system/sensors` returns valid CPU temp, load,
and memory data. NVIDIA GPU data available when feature is enabled.

---

### Wave 1 — Overlay Infrastructure

**Goal:** The overlay compositor exists and can composite transparent canvases
on top of the display viewport. No renderers yet — test with solid-color
canvases.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 1.1 | Add overlay config types (`DisplayOverlayConfig`, `OverlaySlot`, `OverlaySource`, etc.) | types | Serde round-trip tests |
| 1.2 | Add `OverlayRenderer` trait | core | Compiles, trait is object-safe |
| 1.3 | Add `OverlayComposer` in display output module | daemon | Unit test: compose with mock overlays |
| 1.4 | Integrate `OverlayComposer` into `DisplayWorkerHandle` | daemon | Integration test: overlay composites onto viewport canvas |
| 1.5 | Add overlay config watch channel (config changes propagate to workers) | daemon | Config update triggers worker refresh |
| 1.6 | Add display overlay API endpoints (CRUD) | daemon | API integration tests |
| 1.7 | Benchmark: display output with 0, 1, 2 overlay layers | daemon | No regression on 0-overlay path, <200 µs for 2 layers |

**Exit criteria:** A mock overlay (solid color with alpha) composites visibly
on a Corsair LCD. API can add/remove overlays. Zero-overlay path has no
measurable regression.

---

### Wave 2 — Native Overlay Renderers

**Goal:** Built-in renderers for the four core overlay types. Users can
configure clock faces, sensor gauges, images, and text overlays via the API.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 2.1 | Add `tiny-skia`, `cosmic-text`, `resvg` dependencies | core | `just check` |
| 2.2 | Implement Pixmap ↔ Canvas bridge (premul ↔ straight conversion) | core | Round-trip pixel accuracy tests |
| 2.3 | Implement `ClockRenderer` (digital + analog) | core | Visual snapshot tests at 480x480 |
| 2.4 | Implement `SensorRenderer` (numeric + gauge + bar) | core | Snapshot tests with mock sensor data |
| 2.5 | Implement `ImageRenderer` (PNG/JPEG + animated GIF) | core | Load test images, GIF frame cycling test |
| 2.6 | Implement `TextRenderer` (styled + scrolling + sensor interpolation) | core | Text layout tests, interpolation tests |
| 2.7 | Ship default SVG templates (2 clock faces, 2 gauge styles) | assets | Templates render correctly via resvg |
| 2.8 | Wire renderer factory: `OverlaySource` → `Box<dyn OverlayRenderer>` | core | Factory dispatches correctly for all types |
| 2.9 | End-to-end test: clock + sensor gauge on Corsair LCD | daemon | Manual visual verification on hardware |

**Exit criteria:** A user can configure a clock and CPU temperature gauge on
their AIO LCD via the API, both render correctly with transparency over the
active effect.

---

### Wave 3 — HTML Overlays via Servo

**Goal:** User-provided HTML overlay files render through Servo and composite
on top of the display output. SignalRGB LCD face compatibility.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 3.1 | Add overlay render queue to Servo worker | core | Unit test: overlay request serviced between effect frames |
| 3.2 | Implement `HtmlOverlayRenderer` (submits to Servo queue, caches result) | core | Render test with simple HTML overlay |
| 3.3 | Add `engine.getSensorValue()` to LightScript runtime | core | JS test: sensor values accessible |
| 3.4 | Add `engine.sensors` object with full snapshot access | core | JS test: iterate sensor readings |
| 3.5 | Test SignalRGB LCD face compatibility (Clock.html, Simple Sensor.html) | daemon | SignalRGB faces render without modification |
| 3.6 | Document HTML overlay authoring (meta properties, sensor API, examples) | docs | Written, examples work |

**Exit criteria:** SignalRGB's Clock.html and Simple Sensor.html render
correctly as Hypercolor overlays. Custom HTML overlays can access sensor data
via `engine.getSensorValue()`.

---

### Wave 4 — Effect Sensor Binding

**Goal:** Effects can bind controls to system sensor values, completing the
`ControlType::Sensor` path from design doc 02.

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 4.1 | Add `ControlType::Sensor` variant with label and range mapping | types | Serde tests |
| 4.2 | Wire sensor snapshot into `EffectEngine::tick` for sensor-bound controls | core | Unit test: sensor value maps to control range |
| 4.3 | Add sensor binding API (`PATCH /api/v1/effects/current/controls/{name}/bind`) | daemon | API test: bind CPU temp to a color control |
| 4.4 | Update LightScript preamble to inject sensor-bound control values | core | JS test: control value reflects sensor reading |

**Exit criteria:** An effect's color control can be bound to CPU temperature,
transitioning from blue (30°C) to red (100°C) automatically.

---

### Wave 5 — GPU Composition (Future, Post-Modernization)

**Goal:** Optional wgpu-based overlay composition for display output.

**Blocked by:** Render pipeline modernization Wave 7 (design doc 28).

**Tasks:**

| # | Task | Crates | Verify |
|---|------|--------|--------|
| 5.1 | GPU overlay texture cache (upload on render, cache until next update) | daemon | Texture reuse across frames |
| 5.2 | Fullscreen-quad alpha blend shader for overlay composition | daemon | Visual parity with CPU path |
| 5.3 | GPU → CPU readback for JPEG encode | daemon | Correct JPEG output from GPU-composed frame |
| 5.4 | Parity tests: CPU vs GPU overlay composition | daemon | Pixel-level comparison within tolerance |
| 5.5 | Benchmark: GPU vs CPU overlay composition at 480x480 | daemon | Measured improvement |

**Exit criteria:** GPU overlay composition produces visually identical output
to the CPU path with measurable performance improvement.

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

### 16.4 Compatibility Testing

Wave 3 includes explicit testing of SignalRGB LCD face HTML files to verify
the LightScript runtime correctly handles their property declarations, sensor
queries, and rendering output.

---

## 17. Recommendation

Build this in four deliberate stages:

1. **Wave 0 (sensor pipeline)** delivers value immediately — effects and the
   API gain system sensor data even before overlays exist. The
   `ControlType::Sensor` binding from design doc 02 becomes possible.

2. **Waves 1–2 (overlay infrastructure + native renderers)** are the core
   feature. tiny-skia, cosmic-text, and resvg are mature, lightweight, pure
   Rust crates that add minimal binary size (~2 MiB total) and compile
   quickly. SVG templates make overlay design accessible to artists.

3. **Wave 3 (HTML overlays)** adds the flexibility ceiling. Users can build
   anything they can express in HTML/CSS/JS, with SignalRGB face
   compatibility as a migration path.

4. **Waves 4–5 (sensor binding + GPU)** complete the vision. Effects react to
   hardware state. GPU composition eliminates the CPU round-trip for overlay
   blending.

The rendering stack (tiny-skia + cosmic-text + resvg) is the right choice.
It is pure Rust, actively maintained, SIMD-optimized, and collectively used by
the COSMIC desktop, Linebender, and resvg ecosystems. No other combination
offers the same quality-to-weight ratio for CPU rendering at 480x480.

Nobody in the open-source ecosystem has solved this problem properly. The
closest projects (TRCC Linux, trlcd_libusb, NZXT-ESC) each solve a fragment.
Hypercolor can be the first to unify effect rendering, overlay compositing,
system sensor data, and hardware LCD output into a single coherent pipeline on
Linux.
