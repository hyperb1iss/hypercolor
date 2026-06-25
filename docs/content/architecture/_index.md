+++
title = "Architecture overview"
description = "Crate graph, render pipeline, SparkleFlinger compositor, adaptive FPS tiers, and the 640x480 canvas."
weight = 80
sort_by = "weight"
template = "section.html"
+++

Hypercolor is a daemon-first RGB lighting engine. A background service owns the render thread, manages hardware connections, and exposes REST, WebSocket, and MCP interfaces. Clients — the web UI, TUI, CLI, and AI assistants — connect to the daemon and never touch hardware directly.

This section goes deep on how that works: the crate boundaries that keep the system coherent, the frame pipeline that turns effect output into per-LED colors, the compositor that blends multiple producers, and the renderer paths available to effect authors.

## In this section

- [Render pipeline](@/architecture/render-pipeline.md) — Frame lifecycle from input sampling through SparkleFlinger composition to device write.
- [Event bus](@/architecture/event-bus.md) — How broadcast and watch channels serve different data-freshness contracts across all subscribers.
- [Renderer internals](@/architecture/renderer-internals.md) — Deep dive on the Servo HTML path, the native Rust path, the display-face variant, and the GPU acceleration roadmap.

---

## Crate graph

The project is split into focused crates with strict one-way dependency boundaries. The shared vocabulary lives at the bottom; application binaries sit at the top and never import each other.

{% mermaid() %}
graph TD
    T[hypercolor-types] --> HAL[hypercolor-hal]
    T --> CORE[hypercolor-core]
    T --> LGI[hypercolor-linux-gpu-interop]
    T --> WPI[hypercolor-windows-pawnio]
    HAL --> CORE
    LGI --> CORE
    WPI --> CORE
    T & CORE --> DAPI[hypercolor-driver-api]
    DAPI --> HUE[hypercolor-driver-hue]
    DAPI --> NL[hypercolor-driver-nanoleaf]
    DAPI --> WLED[hypercolor-driver-wled]
    DAPI --> GV[hypercolor-driver-govee]
    DAPI --> NET[hypercolor-network]
    HAL & HUE & NL & WLED & GV --> DB[hypercolor-driver-builtin]
    CORE & HAL & DB & NET --> D[hypercolor-daemon]
    CORE --> CLI[hypercolor-cli]
    T --> TUI[hypercolor-tui]
    CORE & T --> TRAY[hypercolor-tray]
    APP[hypercolor-app] --> D & TRAY
    T --> UI["hypercolor-ui (excluded from workspace)"]
    LE[hypercolor-leptos-ext] --> UI & D & TUI
{% end %}

**Golden rule:** `hypercolor-hal` must never depend on `hypercolor-core` — that would be circular. Network drivers depend on `driver-api`, not on `core` directly.

| Crate | Role |
|---|---|
| `hypercolor-types` | Zero-dependency shared vocabulary — canvas, effect, color, and API types |
| `hypercolor-core` | Engine: render loop, effect registry, SparkleFlinger, spatial sampler, input pipeline, scene management |
| `hypercolor-hal` | Hardware abstraction: USB/HID/SMBus protocol encoding and transport |
| `hypercolor-linux-gpu-interop` | Linux zero-copy GL to wgpu texture import; stubbed on other platforms |
| `hypercolor-windows-pawnio` | Windows SMBus via the PawnIO kernel driver; stubbed on other platforms |
| `hypercolor-driver-api` | Stable trait boundary between the daemon and all driver implementations |
| `hypercolor-driver-builtin` | Compile-time bundle of HAL and network drivers, assembled via feature flags |
| Network drivers | `driver-hue`, `driver-nanoleaf`, `driver-wled`, `driver-govee`, `network` |
| `hypercolor-daemon` | Daemon binary: render-loop host, REST/WebSocket/MCP server on `:9420` |
| `hypercolor-cli` | The `hypercolor` CLI binary |
| `hypercolor-tui` | Ratatui terminal UI library |
| `hypercolor-tray` | System tray applet |
| `hypercolor-app` | Unified desktop shell: supervises the daemon, owns the tray, handles autostart |
| `hypercolor-leptos-ext` | Leptos 0.8 extension helpers for the web UI and TUI |
| `hypercolor-ui` | Leptos 0.8 CSR web app compiled to WASM via Trunk — excluded from the workspace |

{% callout(type="warning") %}
`hypercolor-ui` targets `wasm32-unknown-unknown` and is excluded from the Cargo workspace. `cargo check --workspace` does not cover it. Build the UI separately with `just ui-dev` or `just ui-build`.
{% end %}

---

## Render pipeline

The render thread runs on a dedicated OS thread and drives all frame production. At the start of each frame it samples every input source, runs effect producers, composites their surfaces, maps LED positions, and writes device output — in that order.

```
InputManager::sample_all()          → audio FFT, screen capture, MIDI/keyboard
build_frame_scene_snapshot()        → active scene, effect groups, live control state
SparkleFlinger::compose_frame()     → blend per-producer surfaces into one canonical RGBA canvas
SpatialEngine::sample()             → map canvas pixels to LED positions → ZoneColors
BackendManager::write_frame()       → group by device, queue async protocol sends
HypercolorBus::publish()            → broadcast frame data, canvas preview, timing metrics
```

The full lifecycle — with timing budgets, snapshot mechanics, and the zone mapping contract — is detailed in [Render pipeline](@/architecture/render-pipeline.md).

### The canvas

All effect paths write into a `Canvas` — a row-major RGBA pixel buffer at a configurable resolution. The default is **640x480**, declared as constants in `hypercolor-types/src/canvas.rs`:

```rust
pub const DEFAULT_CANVAS_WIDTH: u32 = 640;
pub const DEFAULT_CANVAS_HEIGHT: u32 = 480;
```

At 640x480 the buffer is roughly 1.17 MB per frame. Effects render in normalized `[0.0, 1.0]` spatial coordinates and remain resolution-independent — tune `daemon.canvas_width` / `daemon.canvas_height` in your config for your hardware density without touching effect code. Canvas dimensions retune at frame boundaries via `SceneTransaction::ResizeCanvas`; never hardcode pixel dimensions in effects or drivers.

Sampling from the canvas to LED positions supports three interpolation strategies: nearest-neighbor, bilinear (the default), and area averaging. Bilinear reads 4 surrounding pixels and blends by distance; it is gamma-correct via precomputed sRGB LUTs, which makes it essentially free in the hot path.

### SparkleFlinger

SparkleFlinger is the frame compositor inside the daemon. Each active producer — an HTML effect running in Servo, a native Rust effect, a screen capture source — publishes its latest surface at its own cadence. SparkleFlinger latches the newest surface per producer at the frame deadline and blends them into one canonical canvas using the configured blend mode and layer transform.

This design decouples producer cadence from the render deadline. Servo might deliver frames at 30 fps while a native effect runs at 60 fps and a screen capture arrives whenever PipeWire hands one over. SparkleFlinger handles all of that without coupling or stalling.

Blend modes available for layer compositing are defined in `BlendMode` (in `hypercolor-types/src/canvas.rs`): `Normal`, `Add`, `Screen`, `Multiply`, `Overlay`, `SoftLight`, `ColorDodge`, and `Difference`. A single full-opacity layer with no transform takes a bypass fast path with no per-pixel work.

### Adaptive FPS: five tiers

The `FpsController` manages frame timing across five `FpsTier` variants, shifting automatically based on measured render performance:

| Tier | FPS | Frame budget |
|---|---|---|
| `Minimal` | 10 | 100 ms |
| `Low` | 20 | 50 ms |
| `Medium` | 30 | ~33.3 ms |
| `High` | 45 | ~22.2 ms |
| `Full` | 60 | ~16.6 ms |

Downshift is aggressive: two consecutive budget misses trigger an immediate drop. Upshift is conservative: the controller requires sustained headroom over a configurable window before stepping up. This prevents oscillation when the system is near a tier boundary.

Never reduce these tier ceilings or default fps values as a workaround. If a frame is taking too long, diagnose the actual bottleneck.

---

## Effect renderer paths

Hypercolor supports two rendering paths, both producing an RGBA `Canvas` that feeds the spatial sampler.

### Servo path (HTML/Canvas/WebGL)

The primary authoring surface. An embedded Servo browser engine runs headlessly on a dedicated worker thread, keeping the `EffectRenderer` trait `Send` while Servo itself stays pinned to one OS thread. Effects are `.html` files that use Canvas 2D, WebGL2, and the `@hypercolor/sdk` JavaScript API.

**Startup cost:** The Servo worker initializes once per daemon session, not per effect load. Initial startup takes a few seconds — subsequent effect loads into the running worker are fast. The circuit breaker tracks consecutive failures with exponential cooldown so transient faults cannot poison the shared worker. A separate `ServoSessionHandle` manages `Idle → Loading → Running` state transitions.

GLSL effects in the SDK compile to WebGL2 and run inside Servo; there is no separate native GLSL lane. The `EffectSource::Shader` variant exists in the type system but bails immediately at renderer creation:

```
shader effect '<name>' is not runnable yet (source: <path>)
```

GPU acceleration via wgpu is future work; treat any reference to a "native shader path" as referring to the compiled-in Rust effects, not GPU shaders.

Display-face effects (`EffectCategory::Display`) use a dedicated `ServoRenderer::new_display_face()` constructor. This variant targets LCD surfaces attached to devices and gets a different compositor path through SparkleFlinger. See [Renderer internals](@/architecture/renderer-internals.md) for the display-face render path.

The Servo feature is compile-time gated. Without the `servo` feature in `hypercolor-core`, `EffectSource::Html` effects return an error at renderer creation time and the `web_viewport` builtin is unavailable. Use `just daemon-servo` to run the daemon with Servo enabled.

### Native Rust path

The always-available path. Native effects implement the `EffectRenderer` trait directly in Rust and are compiled into `hypercolor-core/src/effect/builtin/`. They require no GPU and no browser engine.

Registration happens in `core/src/effect/builtin/mod.rs` via `register_builtin_effects()`. The `create_builtin_renderer()` function maps effect name strings — derived from the `EffectSource::Native` path stem — to renderer instances. The native set covers solid fills, gradients, rainbow cycling, breathing, audio-reactive pulse, color waves, color zones, screen cast, media player integration, and calibration patterns. The `web_viewport` builtin is added behind the `servo` feature flag.

See [Renderer internals](@/architecture/renderer-internals.md) for the full `EffectRenderer` trait contract and how to implement a native effect.

---

## Audio capture

Audio input arrives through the `InputManager`. The render loop calls `sample_all()` each frame; audio data arrives as an FFT spectrum and RMS level. Effects declare `audio_reactive: true` in their metadata and receive `AudioData` injected into their `FrameInput` on each render call.

Audio-reactive HTML effects receive the spectrum as a JavaScript global injected by the Servo renderer before each frame. Native effects receive it directly as typed Rust data via `FrameInput::audio`. See [Audio setup](@/guide/audio-setup.md) for configuration and [Effects: audio](@/effects/audio.md) for authoring patterns.

---

## Playlists and library

The daemon maintains a library of saved presets, favorites, and playlists. Playlists are ordered sequences of scene configurations the engine can step through automatically — time-based, event-triggered, or manually advanced. The library API lives under `/api/v1/library/` and includes favorites CRUD and playlist management. Scenes and zones are covered in [Studio](@/studio/_index.md); the library API surface is documented in [REST reference](@/api/rest.md).

---

## Key decisions

| Decision | Choice | Rationale |
|---|---|---|
| Language | Rust | Performance for the render thread, safety for USB HID, ecosystem match for Servo and Ratatui |
| Effect renderer | Servo HTML + native Rust | Web platform for authoring, compiled Rust for built-in utilities; GPU lane is future work |
| Frame compositor | SparkleFlinger | Decouples producer cadence from frame deadlines; enables mixed-rate sources and render groups |
| Canvas resolution | 640x480 (configurable) | Resolution-independent normalized coordinates; tune per hardware density |
| Adaptive FPS | Five tiers, 10–60 fps | Fast downshift on budget miss; slow upshift to prevent oscillation |
| Event bus | `broadcast` + `watch` | Discrete events need history; high-frequency frame data needs only the latest value |
| Web UI | Leptos 0.8 WASM | Type-safe fine-grained reactivity in the Rust ecosystem |
| API server | Axum | tokio-native, first-class WebSocket, serves the embedded SPA |
| Wire format | zerocopy structs (HAL) | Zero-allocation frame encoding at 60 fps |
| Config | TOML | Rust ecosystem standard, human-readable |
| License | Apache-2.0 | Permissive open source |
