# Spec 42 — Display Faces via Multi-Session Servo

> Full-fidelity HTML display content for LCD devices — sensor dashboards,
> animated clocks, custom artwork, and interactive visualizers — rendered by
> Servo at native display resolution and up to 60 fps. A face is just an
> effect assigned to a display device's RenderGroup: same LightScript SDK,
> same `engine.getSensorValue()` meters, same CSS/canvas/animation power,
> same `<meta>` property system. No new rendering framework, no separate
> browser process — just more WebViews in the same Servo instance.

**Status:** Draft (v1)
**Author:** Nova
**Date:** 2026-04-12
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Depends on:** Render Groups (27), Display Overlay System (40), Display
Output (10 §display), SparkleFlinger (design/30)
**Related:** `docs/design/28-render-pipeline-modernization-plan.md`,
Virtual Display Simulator (41)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Landscape](#4-landscape)
5. [Architecture](#5-architecture)
6. [Multi-Session Servo Worker](#6-multi-session-servo-worker)
7. [Face Effect Category and Discovery](#7-face-effect-category-and-discovery)
8. [Per-Group Canvas Routing](#8-per-group-canvas-routing)
9. [Face Assignment via RenderGroups](#9-face-assignment-via-rendergroups)
10. [LightScript Meter API for Faces](#10-lightscript-meter-api-for-faces)
11. [API Surface](#11-api-surface)
12. [UI Integration](#12-ui-integration)
13. [Native Overlay Deprecation Path](#13-native-overlay-deprecation-path)
14. [Delivery Waves](#14-delivery-waves)
15. [Verification Strategy](#15-verification-strategy)
16. [Recommendation](#16-recommendation)

---

## 1. Overview

Hypercolor's display overlay system (Spec 40) shipped native Rust overlay
renderers — clocks, sensor gauges, text, and images — composited onto the
viewport-sampled effect canvas in each display worker. These native
renderers produce functional but visually basic output: no CSS animations,
no rich typography, no canvas-based procedural visuals, no gradients or
glow effects. They are a fraction of what HTML/CSS/JS can deliver.

SignalRGB's LCD face system demonstrates the right model: each face is a
standalone HTML file that uses the same property/meter API as effects,
renders with full browser capabilities, and achieves rich, animated display
content with minimal effort. Their `LCDFaces/` directory contains self-
contained HTML files like `Clock.html`, `Simple Sensor.html`, and
`PulsingLogo.html` that use `engine.getSensorValue()` for meter data and
CSS animations for visual richness.

Hypercolor already has every ingredient except concurrent Servo rendering:

- **LightScript SDK** with `engine.getSensorValue()`, `engine.sensors`,
  `engine.sensorList` — all injected into Servo every frame
- **`<meta property=...>` system** for declarative control binding,
  including `type="sensor"` for meter pickers
- **RenderGroup multi-effect infrastructure** — `EffectPool` managing
  concurrent `Box<dyn EffectRenderer>` per group, each with independent
  canvas, spatial layout, and controls
- **Servo HTML rendering** — `ServoRenderer` driving WebView via the shared
  worker, with LightScript runtime injection

The single constraint is that the Servo worker manages **one WebView at a
time**. This spec removes that constraint, enabling N concurrent Servo
sessions — one per RenderGroup that uses an HTML effect. A display face
becomes simply an effect in a RenderGroup that targets a display device.

---

## 2. Problem Statement

### 2.1 Visual Quality Gap

The native overlay renderers produce utilitarian output:

| Renderer | Output | Limitation |
|----------|--------|------------|
| ClockRenderer | Digital text, analog circle+hands | No gradients, glow, animation, or custom fonts in UI |
| SensorRenderer | Numeric text, arc gauge, bar fill | Single lerped color, basic arc segments, no transitions |
| TextRenderer | cosmic_text layout, horizontal scroll | Single color, no outlines/shadows/gradients |
| ImageRenderer | Static + GIF, 4 fit modes | No blend effects, no tinting |

Meanwhile, a 30-line HTML face achieves richer visuals through CSS
animations, canvas 2D drawing, SVG, web fonts, and the full browser
rendering pipeline.

### 2.2 Unnecessary Complexity

The native overlay system adds substantial code that Servo already handles:

- `OverlayComposer` — per-display compositor with blend math
- `PremulStaging` — premultiplied alpha staging buffers
- `blend_math.rs` — sRGB LUT pixel blending
- Four native renderers (clock, sensor, text, image) totaling ~1500 lines
- `OverlayRendererFactory` — construction dispatch
- Per-slot render cadence tracking, error handling, exponential backoff

All of this exists because HTML overlays were gated behind Servo multi-
session support. Solving multi-session makes the native overlay path
unnecessary for display devices.

### 2.3 Architectural Misfit

The overlay system was designed as "widgets composited on top of the
effect." But for LCD displays, the face **is** the content — users don't
typically want Rainbow Wave on their Corsair LCD with a clock hovering
over it. They want a sensor dashboard, or an animated clock, or custom
artwork. The face is a first-class effect, not a secondary overlay.

The RenderGroup system (Spec 27) already models this correctly: each group
runs its own effect with its own canvas. A display face is just a
RenderGroup whose effect happens to be a sensor dashboard, assigned to a
display device. The multi-effect infrastructure handles concurrency,
canvas isolation, and device routing.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **Multiple concurrent Servo WebViews** within one process — LED effects
  and display faces render simultaneously, each at their own resolution
  and frame rate
- **Faces are effects** — same LightScript SDK, same `<meta>` property
  declarations, same `engine.*` API, same effect registry discovery and
  hot-reload
- **Native display resolution** — faces render at the LCD's pixel
  dimensions (480×480 Corsair, 320×320 Kraken, etc.), not at the global
  effect canvas size
- **Unlocked frame rate** — faces bypass the 15 fps overlay cap; target
  30 fps by default, up to 60 fps for simple faces when budget allows
- **Sensor data via LightScript meters** — `engine.getSensorValue()` and
  the `type="sensor"` meta property, already wired
- **Per-group canvas routing** — display workers receive their
  RenderGroup's canvas directly, bypassing viewport remapping
- **Backwards compatible** — native overlay renderers remain as fallback
  for non-Servo builds or lightweight widget use cases

### 3.2 Non-Goals

- **GPU-accelerated face rendering** — Servo uses software rendering
  (SoftwareRenderingContext). GPU composition (design doc 28, Wave 7)
  is orthogonal and not required for this spec.
- **In-browser face editor** — users author face HTML externally (or
  select from bundled faces). A visual editor is future work.
- **Face marketplace / sharing** — discovery of community faces is out
  of scope.
- **Per-pixel overlay compositing on top of faces** — the native overlay
  compositor (Spec 40 Waves 0–2) still works, but this spec doesn't
  extend it. Faces handle their own visual composition via HTML/CSS.
- **Replacing native effects** — native Rust effects (solid color, pulse,
  perlin noise, etc.) are unaffected. Only display rendering changes.

---

## 4. Landscape

### 4.1 SignalRGB LCD Faces

SignalRGB's `LCDFaces/` system demonstrates the target model:

- Standalone HTML files with `<meta>` property declarations
- `engine.getSensorValue(sensorName)` returns `{ value, units, name }`
- Properties become JS globals; change handlers: `on<PropName>Changed()`
- Full CSS animations, canvas 2D, SVG, and web fonts
- No manifest — directory-based auto-discovery
- 5 bundled faces: Clock, Custom Text, Simple Sensor, Logo, Pulsing Logo

**Hypercolor already has equivalent infrastructure:**

| SignalRGB | Hypercolor | Status |
|-----------|------------|--------|
| `engine.getSensorValue()` | `window.engine.getSensorValue()` | Shipped |
| `engine.sensors` | `window.engine.sensors` | Shipped |
| `<meta property=... type="sensor">` | `<meta property=... type="sensor">` | Shipped |
| `<meta property=... type="color">` | `<meta property=... type="color">` | Shipped |
| `on<Prop>Changed()` | Control update via LightScript runtime | Shipped |
| Auto-discovery from directory | `register_html_effects()` recursive scan | Shipped |

The only gap is concurrent rendering: SignalRGB presumably runs each face
in its own browser/webview instance. Hypercolor's Servo worker currently
manages one WebView.

### 4.2 Current Servo Worker Architecture

```
ServoWorkerRuntime {
    webview: Option<WebView>,           // ONE at a time
    servo: Servo,                       // Process-global singleton
    rendering_context: Rc<dyn RenderingContext>,  // ONE software GL surface
    delegate: Rc<HypercolorWebViewDelegate>,
    loaded_html_path: Option<PathBuf>,
    script_buffer: String,
}
```

- `Servo` instance: process-global via `OnceLock<Mutex<SharedServoWorkerState>>`
- `SoftwareRenderingContext`: offscreen GL surface at a fixed size
- `WebView`: navigates to one HTML page, receives JS scripts, paints to
  the rendering context, captured via `read_to_image()`
- `WorkerCommand`: `Load`, `Render`, `Unload`, `Shutdown` — all implicitly
  target the single WebView
- Worker thread processes commands sequentially from FIFO mpsc channel

The Servo API **does** support multiple WebViews per `Servo` instance via
`WebViewBuilder::new(&servo, rendering_context)`. Each WebView can use a
different rendering context. `servo.spin_event_loop()` advances all
WebViews in one call. The single-WebView limitation is our code, not
Servo's.

---

## 5. Architecture

### 5.1 Conceptual Model

A display face is an effect in a RenderGroup with a display device target:

```
Scene
├── RenderGroup "Main LEDs"
│   ├── effect: Rainbow Wave (HTML)
│   ├── devices: [LED Strip, Fans, Motherboard]
│   └── canvas: 640×480
├── RenderGroup "AIO Display"
│   ├── effect: System Monitor Face (HTML)
│   ├── display_target: Corsair iCUE LINK LCD
│   └── canvas: 480×480 (auto-sized to display)
└── RenderGroup "Reservoir"
    ├── effect: Minimal Temp Face (HTML)
    ├── display_target: Corsair XD5 LCD
    └── canvas: 480×480
```

Each RenderGroup gets an independent `ServoRenderer` → `ServoSession` →
`WebView`. The EffectPool orchestrates them. The Servo worker thread
multiplexes rendering across all sessions.

### 5.2 Data Flow

```
                ┌─────────────────────────────────┐
                │         EffectPool               │
                │  ┌──────────┐  ┌──────────┐     │
                │  │ Slot A   │  │ Slot B   │ ... │
                │  │ Renderer │  │ Renderer │     │
                │  └────┬─────┘  └────┬─────┘     │
                └───────┼─────────────┼───────────┘
                        │             │
                ┌───────▼─────────────▼───────────┐
                │      Servo Worker Thread         │
                │  ┌────────────┐ ┌────────────┐  │
                │  │ Session A  │ │ Session B  │  │
                │  │ WebView    │ │ WebView    │  │
                │  │ 640×480 GL │ │ 480×480 GL │  │
                │  └─────┬──────┘ └─────┬──────┘  │
                │        │              │          │
                │   spin_event_loop() advances all │
                │        │              │          │
                │   paint → readback    paint → readback
                └────────┼──────────────┼──────────┘
                         │              │
                    Canvas A       Canvas B
                         │              │
                ┌────────▼──┐    ┌──────▼────────┐
                │ Spatial   │    │ Display Worker │
                │ Engine    │    │ (per-group     │
                │ → LEDs    │    │  canvas route) │
                └───────────┘    │ → JPEG encode  │
                                 │ → LCD device   │
                                 └────────────────┘
```

### 5.3 Key Invariants

1. **One `Servo` instance per process.** Multiple `WebView` instances share
   it. `servo.spin_event_loop()` advances all sessions atomically.
2. **Per-session rendering context.** Each session creates its own
   `SoftwareRenderingContext` at the target resolution. No shared GL
   surface contention.
3. **Per-session delegate.** Frame readiness, page load, and console
   messages are tracked independently per session.
4. **Sequential command processing.** The worker thread FIFO-processes
   commands across all sessions. Practical target: 2 concurrent HTML
   sessions (1 effect + 1 face) at 30 fps each. 3+ sessions degrade
   gracefully via FPS downshift. Load/unload commands should not starve
   render commands (see §16.2).
5. **EffectPool handles lifecycle.** Session creation/destruction follows
   RenderGroup reconciliation — same as existing effect slot management.
6. **Display workers receive group canvases directly.** When a display has
   a face RenderGroup, it bypasses viewport remapping and receives the
   group's canvas at native resolution.

---

## 6. Multi-Session Servo Worker

### 6.1 Session Identity

```rust
// crates/hypercolor-core/src/effect/servo/worker_client.rs

/// Internal session handle. Monotonic counter, not serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ServoSessionId(u64);
```

Not a UUID — this is a fast internal handle used only within the Servo
worker protocol. The counter increments per `CreateSession` command and
never wraps in practice.

### 6.2 Extended Command Protocol

```rust
// crates/hypercolor-core/src/effect/servo/worker_client.rs

pub(super) enum WorkerCommand {
    /// Provision a new WebView + rendering context at the given resolution.
    CreateSession {
        session_id: ServoSessionId,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<()>>,
    },
    /// Load an HTML page into an existing session.
    Load {
        session_id: ServoSessionId,
        html_path: PathBuf,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<()>>,
    },
    /// Evaluate scripts and capture a frame from a session.
    Render {
        session_id: ServoSessionId,
        scripts: Vec<String>,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<Canvas>>,
    },
    /// Unload the page in a session (navigate to about:blank).
    Unload {
        session_id: ServoSessionId,
        response_tx: SyncSender<Result<()>>,
    },
    /// Tear down a session's WebView and rendering context.
    DestroySession {
        session_id: ServoSessionId,
        response_tx: SyncSender<Result<()>>,
    },
    /// Shut down the entire worker thread.
    Shutdown {
        response_tx: SyncSender<()>,
    },
}
```

### 6.3 Worker Runtime Transformation

```rust
// crates/hypercolor-core/src/effect/servo/worker.rs

struct ServoSession {
    webview: WebView,
    rendering_context: Rc<dyn RenderingContext>,
    delegate: Rc<HypercolorWebViewDelegate>,
    loaded_html_path: Option<PathBuf>,
    script_buffer: String,
}

struct ServoWorkerRuntime {
    sessions: HashMap<ServoSessionId, ServoSession>,
    servo: Servo,
    next_session_id: u64,
}
```

**Spawn changes:** `ServoWorker::spawn()` no longer takes `width, height`.
The worker creates the `Servo` instance but no initial WebView or rendering
context. Sessions are provisioned on demand.

**`acquire_servo_worker()` signature:** Drops `width, height` parameters.
Returns `Result<ServoWorkerClient>`. The `OnceLock` singleton and circuit
breaker are otherwise unchanged.

**Render flow per session:**

1. Look up `ServoSession` by `session_id`
2. Call `session.rendering_context.make_current()`
3. Resize if `width` or `height` changed
4. Concatenate and evaluate scripts on `session.webview`
5. `self.servo.spin_event_loop()` — advances ALL sessions
6. `session.webview.set_throttled(false)` → `paint()`
7. `session.rendering_context.read_to_image(rect)` → `Canvas`
8. `session.webview.set_throttled(true)`
9. Return `Canvas` via response channel

**Event loop advancement:** `spin_event_loop()` is called once per
`Render` command, but it advances all WebViews. This means a face's
RAF/timer callbacks advance even when the LED effect's render is being
processed, and vice versa. This is correct — both sessions get consistent
time advancement.

### 6.4 Client State Tracking

```rust
// crates/hypercolor-core/src/effect/servo/worker_client.rs

pub(super) struct ServoWorkerClient {
    command_tx: Sender<WorkerCommand>,
    sessions: Arc<Mutex<HashMap<ServoSessionId, ClientStateSlot>>>,
    next_id: Arc<AtomicU64>,
}
```

Per-session state machine (same states as today: Idle → Loading → Running
→ Stopping → Idle). New convenience method:

```rust
pub(super) fn create_and_load(
    &self,
    html_path: &Path,
    width: u32,
    height: u32,
) -> Result<ServoSessionId>
```

### 6.5 ServoRenderer Adaptation

```rust
// crates/hypercolor-core/src/effect/servo/renderer.rs

pub struct ServoRenderer {
    session_id: Option<ServoSessionId>,
    // ... existing fields ...
}
```

- `init_with_canvas_size()` → acquires worker, calls
  `create_and_load()`, stores `session_id`
- `render_into()` → passes `session_id` in `submit_render()`
- `destroy()` / effect switch → calls `destroy_session(session_id)`

Multiple `ServoRenderer` instances in `EffectPool` each hold independent
session IDs. The worker multiplexes rendering across them.

### 6.6 Failure Isolation

Per-session failure handling, not worker-global:

| Failure Type | Scope | Action |
|-------------|-------|--------|
| JS evaluation error | Session | Transient: backoff via circuit breaker |
| Page load timeout | Session | Destroy session, report to EffectPool |
| Render timeout | Session | Destroy session, poison session only |
| Command channel disconnect | Worker | Poison entire worker (unrecoverable) |
| Servo crash | Worker | Poison entire worker |

A face crashing does not kill the LED effect. A LED effect crashing does
not kill the face. Only worker-level failures (channel disconnect, Servo
itself) are globally fatal.

### 6.7 Resource Budget

| Resource | Per Session | 2 Sessions | Notes |
|----------|------------|------------|-------|
| RGBA framebuffer | 480×480×4 = 900 KB | 1.8 MB | Proportional to display resolution |
| JS heap (SpiderMonkey) | ~5–15 MB | ~10–30 MB | Depends on face complexity |
| GL context (software) | ~2 MB | ~4 MB | osmesa/swrast overhead |
| Render time budget | ~16 ms @ 30 fps | 33 ms total | Sequential on worker thread |

`trimmed_servo_preferences()` already minimizes per-session footprint
(disabled JIT, disabled WebXR/WebGPU/WebAudio, single layout thread).

**Scaling note:** 2 concurrent HTML sessions is the practical sweet spot
(1 LED effect + 1 display face). Additional faces compete for the same
33ms frame budget. 3+ sessions work but may force FPS downshift on both
the effect and face. Non-HTML (native) effects impose zero Servo load,
so a native LED effect + 2 HTML faces is viable.

---

## 7. Face Effect Category and Discovery

### 7.1 New Category Variant

```rust
// crates/hypercolor-types/src/effect.rs

pub enum EffectCategory {
    // ... existing variants ...
    /// Full-fidelity LCD display face — dashboards, clocks, artwork.
    Display,
}
```

One line. Serde derives handle serialization as `"display"`.

### 7.2 Face HTML Structure

Faces are standard LightScript HTML effects with `category="display"`:

```html
<!DOCTYPE html>
<html>
<head>
    <title>System Monitor</title>
    <meta description="CPU/GPU/RAM dashboard with animated gauges" />
    <meta publisher="Hypercolor" />
    <meta category="display" />

    <meta property="targetCpuSensor" label="CPU Sensor" type="sensor"
          default="cpu_temp" />
    <meta property="targetGpuSensor" label="GPU Sensor" type="sensor"
          default="gpu_temp" />
    <meta property="accentColor" label="Accent" type="color"
          default="#80ffea" />
    <meta property="showDate" label="Show Date" type="boolean"
          default="true" />

    <meta preset="SilkCircuit Dark"
          preset-description="Neon cyan on dark background"
          preset-controls='{"accentColor":"#80ffea"}' />
</head>
<body>
    <!-- Full CSS/canvas/SVG rendering -->
    <script>
    function onReady() {
        setInterval(() => {
            const cpu = engine.getSensorValue(targetCpuSensor);
            const gpu = engine.getSensorValue(targetGpuSensor);
            // Update DOM elements with sensor values
        }, 1000);
    }
    </script>
</body>
</html>
```

### 7.3 Discovery

Effects discovered from `effects/hypercolor/` (recursive scan). Faces can
live in a `faces/` subdirectory for organization:

```
effects/hypercolor/
    faces/
        simple-clock.html
        system-monitor.html
        minimal-sensor.html
        pulsing-logo.html
    ambient/
        aurora.html
        ...
```

The existing loader (`register_html_effects()`), watcher
(`EffectWatchEvent`), and registry (`EffectRegistry`) handle discovery
without changes. The `Display` category is just a filter parameter for
API queries and UI display.

### 7.4 Starter Faces

Ship with a small set of bundled faces demonstrating the pattern:

| Face | Description | Meters Used |
|------|-------------|-------------|
| `simple-clock.html` | Digital clock + date, SilkCircuit palette | None |
| `system-monitor.html` | CPU/GPU temp, load, RAM with animated arcs | cpu_temp, gpu_temp, cpu_load, ram_used |
| `minimal-sensor.html` | Single large sensor readout, configurable | User-selected sensor |
| `sensor-dashboard.html` | Multi-sensor grid with sparklines | All available sensors |

---

## 8. Per-Group Canvas Routing

### 8.1 Problem

Display workers currently receive the **global composed canvas** via one
`watch::Receiver<CanvasFrame>` from the event bus. All displays see the
same frame — a tiled preview of all RenderGroups composited together. A
face rendering at 480×480 in its own RenderGroup gets downsampled into a
tile of the preview canvas, then the display worker viewport-remaps it
back to 480×480. This is wasteful and lossy.

### 8.2 Per-Group Canvas Channels

```rust
// crates/hypercolor-core/src/bus/mod.rs

/// Per-group canvas streams for direct display consumption.
group_canvases: Mutex<HashMap<RenderGroupId, watch::Sender<CanvasFrame>>>,
```

New methods on `HypercolorBus`:

- `group_canvas_sender(id) -> watch::Sender<CanvasFrame>` — get or create
- `group_canvas_receiver(id) -> watch::Receiver<CanvasFrame>` — subscribe
- `remove_group_canvas(id)` — cleanup on group removal

The `Mutex<HashMap>` guards group creation/destruction (rare, scene
changes). The `watch::` channels are lock-free for per-frame
publish/subscribe.

### 8.3 Render Thread Publication

After `EffectPool::render_group_into()` renders each group's canvas into
`target_canvases`, the render thread publishes per-group frames:

```rust
// crates/hypercolor-daemon/src/render_thread/render_groups.rs

// In render_scene(), after per-group render loop:
pub group_canvases: Vec<(RenderGroupId, Canvas)>,
```

Extend `RenderGroupResult` with per-group canvas snapshots. The caller in
`frame_io.rs` publishes each to the bus:

```rust
// crates/hypercolor-daemon/src/render_thread/frame_io.rs

for (group_id, canvas) in &result.group_canvases {
    let frame = CanvasFrame::from_canvas(canvas, frame_number, timestamp_ms);
    let _ = bus.group_canvas_sender(*group_id).send(frame);
}
```

### 8.4 Display Worker Dual-Source

```rust
// crates/hypercolor-daemon/src/display_output/worker.rs

/// Canvas input source for a display worker.
pub enum DisplayCanvasSource {
    /// Global composed canvas, viewport-remapped for this display.
    Global,
    /// Direct per-group canvas at native display resolution.
    GroupDirect {
        group_id: RenderGroupId,
        receiver: watch::Receiver<CanvasFrame>,
    },
}
```

When a display has a face RenderGroup, its worker subscribes to that
group's canvas channel. It receives the face's output directly at native
resolution — **no viewport remapping, no spatial sampling**. The overlay
compositor still runs if overlays are configured (rare for face displays),
but the expensive viewport crop/scale pass is eliminated entirely.

When no face is assigned, the worker falls back to the global composed
canvas with viewport crop/scale (current behavior).

---

## 9. Face Assignment via RenderGroups

### 9.1 Display Target on RenderGroup

```rust
// crates/hypercolor-types/src/scene.rs

pub struct RenderGroup {
    // ... existing fields ...

    /// When set, renders directly to the target display at native resolution.
    /// The group's canvas dimensions are automatically matched to the display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_target: Option<DisplayFaceTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DisplayFaceTarget {
    /// Device ID of the target display.
    pub device_id: DeviceId,
}
```

### 9.2 Canvas Auto-Sizing

When `reconcile()` encounters a RenderGroup with `display_target`, it
queries the device registry for the display's geometry and overrides the
group's canvas dimensions:

```rust
// crates/hypercolor-daemon/src/render_thread/render_groups.rs

fn resolve_display_canvas_size(
    group: &RenderGroup,
    device_registry: &DeviceRegistry,
) -> (u32, u32) {
    if let Some(target) = &group.display_target {
        if let Some(geometry) = device_registry.display_geometry(&target.device_id) {
            return (geometry.width, geometry.height);
        }
    }
    (group.layout.canvas_width, group.layout.canvas_height)
}
```

This happens at scene activation and device reconnection, not per-frame.

### 9.3 Display Output Routing

The display output thread (`display_output/mod.rs`) checks the active
scene for RenderGroups with `display_target` matching each display device:

- **Face found:** Subscribe to the group's per-group canvas channel.
  Display worker receives the face canvas at native resolution.
- **No face:** Use the global composed canvas with viewport remapping
  (current behavior).

Routing is re-evaluated on scene change and device connect/disconnect.

### 9.4 Overlay Coexistence

The native overlay compositor (Spec 40) still operates on top of the base
canvas in the display worker. If a user has a face AND native overlays
configured, overlays composite on top of the face output. This is the
backwards-compatible path — faces don't remove overlay capability, they
just make it unnecessary for most use cases.

---

## 10. LightScript Meter API for Faces

### 10.1 Sensor Injection (Already Shipped)

The `LightscriptRuntime::sensor_update_script()` function serializes the
latest `SystemSnapshot` into JavaScript that updates `window.engine`:

```javascript
window.engine.sensors = {
    "cpu_load": { value: 42.5, min: 0, max: 100, unit: "%" },
    "cpu_temp": { value: 65.0, min: 0, max: 100, unit: "°C" },
    "gpu_temp": { value: 58.0, min: 0, max: 100, unit: "°C" },
    "gpu_load": { value: 78.0, min: 0, max: 100, unit: "%" },
    "ram_used": { value: 62.3, min: 0, max: 100, unit: "%" },
    // ... component readings
};
window.engine.sensorList = ["cpu_load", "cpu_temp", "gpu_temp", ...];
```

**File:** `crates/hypercolor-core/src/effect/lightscript.rs:568–600`

### 10.2 Property Binding (Already Shipped)

The `type="sensor"` property type creates a sensor picker in the UI.
The selected sensor name becomes a JS global, passed to
`engine.getSensorValue()`:

```javascript
// Face JS:
const reading = engine.getSensorValue(targetSensor);
// reading = { value: 65.0, min: 0, max: 100, unit: "°C" }
```

**File:** `crates/hypercolor-core/src/effect/meta_parser.rs:368–391`

### 10.3 Sensor Update Cadence

Sensors are sampled every 2 seconds by `SensorPoller` and published via
`watch::Sender<Arc<SystemSnapshot>>`. The LightScript runtime injects
the latest snapshot every frame. Faces should poll `getSensorValue()` on
their own cadence (e.g., `setInterval(updateSensors, 1000)`).

### 10.4 No Changes Needed

The entire meter pipeline — polling, snapshot publication, LightScript
injection, `getSensorValue()` API, sensor property type — is already
shipped and works identically for faces as for effects. Each Servo session
gets the same sensor injection via `enqueue_frame_scripts()`.

---

## 11. API Surface

### 11.1 Existing Endpoints (No Changes)

```
GET    /api/v1/effects                          — List all effects (faces included)
GET    /api/v1/effects?category=display         — Filter to face effects
GET    /api/v1/effects/{id}                     — Full metadata + controls
POST   /api/v1/effects/{id}/apply               — Apply effect (works for faces too)
PATCH  /api/v1/effects/current/controls          — Update live controls
POST   /api/v1/effects/rescan                    — Re-discover effects
```

Scene/RenderGroup endpoints already handle face-bearing groups:

```
POST   /api/v1/scenes                            — Create scene with face groups
PUT    /api/v1/scenes/{id}                       — Update scene groups
POST   /api/v1/scenes/{id}/activate              — Activate scene with faces
```

### 11.2 New Convenience Endpoints

```
PUT    /api/v1/displays/{device_id}/face
```

Body: `{ "effect_id": "<uuid>" }`

Creates or updates a RenderGroup in the active scene targeting this
display with the given effect. Sets `display_target.device_id` and
auto-sizes canvas to the display's resolution. Returns the created/
updated RenderGroup.

```
DELETE /api/v1/displays/{device_id}/face
```

Removes the face RenderGroup from the active scene. Display falls back
to the global composed canvas.

```
GET    /api/v1/displays/{device_id}/face
```

Returns the current face assignment (effect, RenderGroup, controls) or
404 if no face is assigned.

### 11.3 MCP Tool Updates

Extend `set_display_overlay` or add a new `set_display_face` tool:

```json
{
    "name": "set_display_face",
    "description": "Assign an HTML face effect to a display device",
    "inputSchema": {
        "device": "string (device ID or name)",
        "effect_id": "string (effect UUID or name)",
        "controls": "object (optional control overrides)"
    }
}
```

---

## 12. UI Integration

### 12.1 Displays Page Updates

The displays page (`crates/hypercolor-ui/src/pages/displays.rs`)
currently shows an overlay stack editor. Add a face assignment section:

- **Face picker** — browse Display-category effects, preview, assign
- **Face controls** — when a face is assigned, show its `<meta property>`
  controls (same inspector used for effects in the main effect browser)
- **Preview** — the existing `/preview.jpg` endpoint shows the face
  output once assigned

### 12.2 Effect Browser Filter

The effect browser (`pages/effects.rs`) gains a category filter pill for
`Display`. Users can browse faces separately from ambient/audio/etc.
effects.

### 12.3 Scene Editor

The scene editor's RenderGroup list shows groups with `display_target`
visually distinguished — display icon, device name badge, auto-sized
canvas dimensions shown as read-only.

---

## 13. Native Overlay Deprecation Path

### 13.1 Short Term (This Spec)

Native overlay renderers remain fully functional. They serve as:
- Fallback when Servo is not compiled (`#[cfg(not(feature = "servo"))]`)
- Lightweight widgets for users who don't need full HTML faces
- Overlay-on-top-of-face for edge cases

### 13.2 Medium Term

Once faces are established and the UI integrates face assignment:
- Default new display configs to face mode (no native overlays)
- Mark native overlay creation as "legacy" in the API catalog
- Prioritize face templates over native renderer improvements

### 13.3 Long Term

If face adoption is high and no users depend on native overlays:
- Feature-gate native overlay renderers behind a build flag
- Remove from default builds
- Retain code for non-Servo platforms (embedded, minimal builds)

---

## 14. Delivery Waves

### Wave 0 — Multi-Session Servo Worker (Critical Path)

**Scope:** Transform the worker from single-WebView to N concurrent
sessions. The core enabling change.

**Files:**
- `crates/hypercolor-core/src/effect/servo/worker_client.rs`
- `crates/hypercolor-core/src/effect/servo/worker.rs`
- `crates/hypercolor-core/src/effect/servo/renderer.rs`
- `crates/hypercolor-core/src/effect/servo/circuit_breaker.rs`
- `crates/hypercolor-core/src/effect/servo_bootstrap.rs`

**Deliverable:** Two `ServoRenderer` instances render different HTML pages
at different resolutions simultaneously on the same worker thread.
Existing single-effect path works identically (session ID 0).

**Risk:** `SoftwareRenderingContext` multi-instance behavior. Test with
`make_current()` alternation between two contexts on the same thread.

### Wave 1 — Face Discovery

**Scope:** `EffectCategory::Display` variant, face HTML files, category
inference.

**Files:**
- `crates/hypercolor-types/src/effect.rs`
- `crates/hypercolor-core/src/effect/meta_parser.rs` (optional inference)
- `effects/hypercolor/faces/*.html` (starter faces)

**Deliverable:** `GET /api/v1/effects?category=display` returns face
effects. Applying a face via the standard effect apply endpoint renders
it in Servo.

### Wave 2 — Per-Group Canvas Routing

**Scope:** Publish per-group canvases to the event bus. Display workers
subscribe to their group's channel.

**Files:**
- `crates/hypercolor-core/src/bus/mod.rs`
- `crates/hypercolor-daemon/src/render_thread/render_groups.rs`
- `crates/hypercolor-daemon/src/render_thread/frame_io.rs`
- `crates/hypercolor-daemon/src/display_output/worker.rs`
- `crates/hypercolor-daemon/src/display_output/mod.rs`

**Deliverable:** A display with a face RenderGroup receives the face's
canvas at native resolution. Preview JPEG shows the face, not the tiled
preview.

### Wave 3 — Face Assignment and API

**Scope:** `DisplayFaceTarget` on RenderGroup, canvas auto-sizing,
convenience API endpoints, MCP tool.

**Files:**
- `crates/hypercolor-types/src/scene.rs`
- `crates/hypercolor-daemon/src/render_thread/render_groups.rs`
- `crates/hypercolor-daemon/src/api/displays.rs`
- `crates/hypercolor-daemon/src/mcp/tools/overlays.rs`

**Deliverable:** `PUT /api/v1/displays/{id}/face` assigns a face. LCD
hardware shows the face at native resolution. LED strips continue their
separate effect.

### Wave 4 — UI Integration

**Scope:** Face picker on displays page, effect browser category filter,
scene editor display target visualization.

**Files:**
- `crates/hypercolor-ui/src/pages/displays.rs`
- `crates/hypercolor-ui/src/pages/effects.rs`
- `crates/hypercolor-ui/src/components/` (face picker component)
- `crates/hypercolor-ui/src/api/displays.rs`

**Deliverable:** Users assign faces to displays from the web UI. Live
preview updates as face renders.

---

## 15. Verification Strategy

### 15.1 Unit Tests

- **Multi-session worker:** Two sessions at different resolutions,
  independent load/render/unload lifecycle, failure isolation
  (`crates/hypercolor-core/tests/servo_session_tests.rs`)
- **Per-group canvas routing:** Group canvas published to bus, subscriber
  receives correct resolution
  (`crates/hypercolor-daemon/tests/display_output_tests.rs`)
- **Face assignment:** RenderGroup with `display_target` auto-sizes canvas
  (`crates/hypercolor-types/tests/scene_tests.rs`)

### 15.2 Integration Tests

- **End-to-end face rendering:** `just daemon-servo` + virtual display
  simulator + face effect → verify preview JPEG shows face content with
  sensor data updating
- **Concurrent rendering:** LED effect + display face simultaneously,
  both at target FPS, neither interfering
- **Hot-reload:** Modify face HTML → watcher triggers → face re-renders
  without restarting daemon

### 15.3 Manual Verification

- **With hardware:** Assign a face to a connected Corsair LCD. Verify
  sensor data flows, CSS animations render smoothly, controls update live.
- **With simulator:** `just simulator-demo` + face assignment via API.
  Preview JPEG in browser shows face at native resolution.
- **MCP tools:** Use `set_display_face` tool from Claude to assign and
  configure faces. Verify `list_display_overlays` shows face status.

---

## 16. Known Constraints and Risks

### 16.1 Display FPS Cap

`DISPLAY_OUTPUT_MAX_FPS` in `display_output/mod.rs` hard-caps display
frame rates at **15 fps**. The target_fps for each display is clamped by
`capped_display_target_fps()`. This contradicts the "up to 60 fps" goal.

**Resolution:** Lift the cap for face-driven displays. When a display
receives its canvas from a per-group channel (face mode), the display
worker should use the face effect's declared FPS target (or the
RenderGroup's desired frame rate) instead of the global 15 fps cap. The
cap exists because overlay compositing + JPEG encode was expensive at
higher rates; with a pre-rendered face canvas arriving at native
resolution, the display worker's only job is JPEG encode + send, which
TurboJPEG handles at <5ms for 480×480.

Add a `face_fps_cap` config field (default 30, max 60) and apply it when
`DisplayCanvasSource::GroupDirect` is active. Keep the 15 fps cap for the
global canvas path (viewport remapping + overlay compositing is still
heavy).

### 16.2 Render Budget Realism

The spec's "~11ms per session at 3×30fps" is **optimistic**. Real
constraints:

- `spin_event_loop()` advances ALL WebViews, not just the target. Its
  cost scales with total DOM complexity across sessions.
- `Load` commands block the worker for up to 5 seconds (page load
  timeout). During a face load, the LED effect's render commands queue
  behind it. **Mitigation:** Prioritize `Render` commands over `Load` in
  the dispatch loop, or use a priority channel.
- `evaluate_script()` has a 250ms timeout per batch. Misbehaving face JS
  can stall all sessions. **Mitigation:** Per-session script timeout
  tracking; destroy sessions that consistently hit timeouts.
- Pixel readback (`read_to_image`) cost scales with resolution. 480×480
  is ~900KB; readback is ~1–2ms on software rendering.

**Practical target:** 2 concurrent HTML sessions (1 LED effect + 1 face)
at 30fps each. 3+ sessions degrade gracefully via the existing FPS
downshift mechanism. The spec should not promise 60fps faces as a default
— it's achievable for simple faces but not guaranteed under load.

### 16.3 Per-Group Bus Shape

The proposed `Mutex<HashMap<RenderGroupId, watch::Sender<CanvasFrame>>>`
conflicts with `HypercolorBus` being `#[derive(Clone)]`. A plain `Mutex`
is not `Clone`.

**Resolution:** Use `Arc<Mutex<HashMap<...>>>`. The `HypercolorBus` already
holds `Arc`-wrapped broadcast/watch senders internally. The group canvas
map follows the same pattern. Alternatively, since group creation is rare,
pre-allocate senders at scene activation and distribute receivers through
`DisplayOutputState` rather than the bus.

### 16.4 Canvas Copy Amplification

`CanvasFrame::from_canvas()` calls `PublishedSurface::from_canvas()` which
does a full `to_vec()` copy. `Canvas` is Arc-backed COW — publishing group
canvases forces a copy, and the next render tick's `get_mut()` forces
another COW clone.

**Resolution:** Use `Canvas::into_inner()` or `Canvas::take()` to move
ownership of the pixel buffer into the `CanvasFrame` without copying.
After the render loop finishes sampling zones from a group's canvas, the
canvas can be consumed (moved) for publication. The render thread
re-creates the canvas next frame anyway via `ensure_group_canvas()`.

### 16.5 Failure Isolation Redesign

The current Servo circuit breaker (`circuit_breaker.rs`) and poison model
(`poison_shared_servo_worker()`) are **worker-global**. The spec proposes
per-session isolation, which requires:

- Per-session circuit breaker instances (or a map of breakers)
- Session-scoped poison that destroys one session without poisoning the
  worker
- Reclassifying fatal error heuristics: "page load timeout" is session-
  fatal, not worker-fatal. "Command channel disconnect" remains worker-
  fatal.
- Updating all `poison_shared_servo_worker_if_fatal()` call sites in
  `renderer.rs` to check session scope first

This is a non-trivial refactor of the error taxonomy. Wave 0 should ship
with a simpler model first: **session errors destroy the session and log
a warning, but only channel-level failures poison the worker.** Refined
per-session circuit breakers can follow in a hardening pass.

### 16.6 Overlay Coexistence Inconsistency

The spec claims "no overlay compositing overhead" when a face is active,
but also says native overlays can composite on top of faces. These are
internally inconsistent.

**Resolution:** Clarify: when `DisplayCanvasSource::GroupDirect` is active,
the display worker **skips viewport remapping** (that's the overhead
saved). The overlay compositor still runs if overlays are configured, but
this is expected to be rare — faces handle their own visual composition.
The "no overhead" claim applies to viewport remapping, not to the overlay
path.

### 16.7 Multi-Group-Same-Display Conflict

If multiple RenderGroups target the same `device_id`, the spec doesn't
define precedence.

**Resolution:** Scene validation (`validate_group_exclusivity()`) already
enforces that a device zone appears in at most one RenderGroup. Extend
this validation to `display_target`: a `device_id` may appear as
`display_target` in at most one group per scene. Reject scenes that
violate this at activation time.

### 16.8 DisplayOutputState Scene Visibility

`DisplayOutputState` currently has **no scene or RenderGroup inputs**. It
knows about devices, spatial layout, and the global canvas — but not
which RenderGroup targets which display.

**Resolution:** Add a `watch::Receiver<Arc<ActiveFaceAssignments>>` to
`DisplayOutputState`, where `ActiveFaceAssignments` maps `DeviceId` to
`RenderGroupId`. Published by the render thread or scene manager on scene
activation/change. The display output thread uses this to decide per-
worker canvas source routing during `reconcile_display_workers()`.

```rust
/// Active face-to-display mappings, published on scene change.
#[derive(Debug, Clone, Default)]
pub struct ActiveFaceAssignments {
    pub assignments: HashMap<DeviceId, RenderGroupId>,
}
```

---

## 17. Recommendation

**Ship Wave 0 first.** Multi-session Servo is the only hard engineering
problem and the sole blocker for everything else. Waves 1–2 can be
developed in parallel once the API surface of Wave 0 stabilizes. Wave 3
is integration glue. Wave 4 is UI polish.

Within Wave 0, validate the multi-`SoftwareRenderingContext` assumption
early — create two contexts on the same thread, alternate `make_current()`,
render different pages, confirm independent pixel readback. If this fails,
the fallback is a second Servo worker thread (heavier but proven).

The native overlay system (Spec 40 Waves 0–2) has served its purpose as
a stepping stone. Faces obsolete it for display devices by giving users
the full power of HTML/CSS/JS with the same LightScript SDK they already
know. The native renderers remain as fallback but should not receive
further investment beyond maintenance.

Start with Wave 0. Ship it. Everything else follows.
