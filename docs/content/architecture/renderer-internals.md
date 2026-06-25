+++
title = "Renderer internals"
description = "The EffectRenderer trait, EffectSource variants, factory dispatch, the Servo session model, and GPU lane status."
weight = 30
+++

# Renderer internals

This page is a reference map of how effect rendering works from metadata discovery
through pixel output. It covers the `EffectRenderer` trait contract, the three
`EffectSource` variants and what each one actually does at runtime, how the factory
resolves a renderer instance, how `EffectPool` manages per-zone slots, and where the
Servo session lifecycle fits in.

The [render pipeline](@/architecture/render-pipeline.md) covers the broader compositor
loop. This page goes one level deeper into the renderers themselves.

---

## The two runnable paths

Before diving into types, the most important thing to establish: there are exactly
**two runnable rendering paths today**.

- **Compiled-in Rust renderers** — pure CPU canvas effects registered in
  `crates/hypercolor-core/src/effect/builtin/`. Selected by `EffectSource::Native`.
- **Servo HTML/WebGL2 renderers** — HTML files executed inside a headless Servo
  browser engine. Covers TypeScript canvas effects and GLSL shaders bundled as
  WebGL2 by the SDK. Selected by `EffectSource::Html`.

There is no wgpu/GPU native shader lane available at runtime. `EffectSource::Shader`
exists in the type system as a reserved variant, but the factory bails immediately:

```rust
// crates/hypercolor-core/src/effect/factory.rs
EffectSource::Shader { path } => bail!(
    "shader effect '{}' is not runnable yet (source: {})",
    metadata.name,
    path.display()
),
```

`RenderAccelerationMode::Gpu` returns an error; `Auto` silently falls back to CPU
with `fallback_reason = "gpu effect renderer acceleration is not available yet"`.
The wgpu compute/fragment shader lane is planned future work. GLSL effects work
today because they run as **WebGL2 inside Servo** as `EffectSource::Html`, not
through any wgpu path.

The doc-comments on `EffectSource::Native` ("rendered by `WgpuRenderer`") are stale
aspirational text from an earlier design. Treat them as forward-looking internal notes,
not current behavior.

---

## `EffectSource` — the three variants

`EffectSource` is the discriminant that routes effect metadata to a renderer. Defined
in `crates/hypercolor-types/src/effect.rs`:

```rust
pub enum EffectSource {
    /// "Native" — despite the WgpuRenderer comment, this dispatches to a
    /// compiled-in CPU renderer keyed by path stem.
    Native { path: PathBuf },
    /// HTML/Canvas/WebGL effect executed by ServoRenderer.
    Html { path: PathBuf },
    /// GPU shader lane — not runnable yet. Factory returns Err.
    Shader { path: PathBuf },
}
```

What each variant **actually does** at runtime:

| Variant  | Renderer                          | GPU | `servo` feature required |
|----------|-----------------------------------|-----|--------------------------|
| `Native` | Rust struct from `builtin/`       | No — CPU only | No |
| `Html`   | `ServoRenderer`                   | No (CPU readback); optional GPU import via `servo-gpu-import` | Yes |
| `Shader` | None — factory returns `Err`      | N/A | N/A |

The `source_stem()` helper extracts the file stem of the source path as
`Option<&str>`. The factory uses it as the lookup key for native effects, falling
back to the effect's display name when the stem is unavailable.

`EffectState` (also in `hypercolor-types`) tracks the registry lifecycle:

```
Loading → Initializing → Running → Paused → Destroying
```

- `Loading` — source files discovered, metadata parsed and validated.
- `Initializing` — `init_with_canvas_size` called; HTML load or resource
  allocation in progress.
- `Running` — `render_into` called every render tick.
- `Paused` — renderer alive, not producing frames (crossfade transitions).
- `Destroying` — `destroy()` called; Servo session or other resources released.

---

## `EffectRenderer` trait

The full trait surface is in `crates/hypercolor-core/src/effect/traits.rs`. Every
renderer — built-in Rust or Servo-backed HTML — implements it.

```rust
pub trait EffectRenderer: Send {
    // Lifecycle
    fn init(&mut self, metadata: &EffectMetadata) -> anyhow::Result<()>;
    fn init_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> anyhow::Result<()>;         // default delegates to init()
    fn destroy(&mut self);

    // Frame production
    fn render_into(
        &mut self,
        input: &FrameInput<'_>,
        target: &mut Canvas,
    ) -> anyhow::Result<()>;
    fn render_output(
        &mut self,
        input: &FrameInput<'_>,
    ) -> anyhow::Result<EffectRenderOutput>;  // default wraps render_into
    fn advance_output(&mut self, input: &FrameInput<'_>) -> anyhow::Result<()>;
    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas>;  // legacy

    // Control and asset binding
    fn set_control(&mut self, name: &str, value: &ControlValue);
    fn bind_asset_library(&mut self, _library: Arc<RwLock<AssetLibrary>>) {}
    fn set_display_descriptor(&mut self, _descriptor: Option<DisplayDescriptor>) {}

    // Optional secondary preview stream
    fn preview_canvas(&self) -> Option<Canvas> { None }
}
```

The trait is `Send` but **not `Sync`**. The daemon's `AppState` wraps `EffectEngine`
behind a `Mutex`, never `RwLock`. Servo's renderer is pinned to one OS thread, which
makes `Sync` impossible.

`tick` is a legacy convenience wrapper that allocates a fresh `Canvas` and calls
`render_into`. Prefer `render_into` for new renderers — it lets the engine pass a
pre-allocated target and avoids an allocation per frame.

### `FrameInput` fields

`FrameInput` is passed by reference on every tick:

```rust
pub struct FrameInput<'a> {
    pub time_secs: f32,            // seconds since effect activation
    pub delta_secs: f32,           // time since previous frame
    pub frame_number: u64,         // monotonic counter starting at 0
    pub audio: &'a AudioData,      // always present; AudioData::silence() when no source
    pub interaction: &'a InteractionData,
    pub screen: Option<&'a ScreenData>,
    pub sensors: &'a SystemSnapshot,
    pub sources: FrameDataSources<'a>,  // media / net / lighting for display faces
    pub canvas_width: u32,
    pub canvas_height: u32,
}
```

The default canvas dimensions are **640×480** (`DEFAULT_CANVAS_WIDTH` /
`DEFAULT_CANVAS_HEIGHT` in `hypercolor-types::canvas`). Both values are configurable
and can change live via `SceneTransaction::ResizeCanvas`. Never hardcode them.

Animate against `delta_secs` or `time_secs`, not `frame_number` — the render loop
runs at adaptive FPS across five tiers (10 / 20 / 30 / 45 / 60). The integer frame
counter is monotonic but not wall-clock proportional.

`FrameDataSources` bundles optional typed data beyond audio:

```rust
pub struct FrameDataSources<'a> {
    pub media: Option<&'a MediaState>,    // MPRIS now-playing
    pub net: Option<&'a NetStats>,        // 1 Hz network throughput
    pub lighting: Option<&'a LightingState>, // active scene, dominant colors
}
```

Display faces use these; standard canvas effects can ignore them.

### `EffectRenderOutput`

The richer `render_output` path returns an enum allowing GPU-resident frames when the
`servo-gpu-import` feature is enabled:

```rust
pub enum EffectRenderOutput {
    Cpu(Canvas),
    Gpu(ImportedEffectFrame),  // only with servo-gpu-import feature
    Pending,                   // no completed frame available yet
}
```

Most native effects use `render_into` (always CPU). The default `render_output`
implementation allocates a `Canvas` and delegates to `render_into`. Servo can return
`Gpu` frames when zero-copy GPU import is available, bypassing the CPU readback.

---

## Factory dispatch

`crates/hypercolor-core/src/effect/factory.rs` is the single point where an
`EffectSource` variant is resolved to a `Box<dyn EffectRenderer>`.

```
EffectSource::Native { path }
  └── stem = path.file_stem().to_str()
      └── create_builtin_renderer(stem)    // match arm in builtin/mod.rs
          └── Ok(Box<dyn EffectRenderer>)

EffectSource::Html { path }
  ├── #[cfg(feature = "servo")]
  │   ├── category == Display  →  ServoRenderer::new_display_face()
  │   └── otherwise            →  ServoRenderer::new()
  └── #[cfg(not(feature = "servo"))]
      └── Err("html effect '...' requires the `servo` feature")

EffectSource::Shader { path }
  └── bail!("shader effect '...' is not runnable yet")
```

Before dispatch, the factory resolves the requested `RenderAccelerationMode`:

| Requested mode | Effective mode | Outcome |
|----------------|----------------|---------|
| `Cpu`          | `Cpu`          | Proceeds normally |
| `Auto`         | `Cpu`          | Falls back silently; `fallback_reason` set |
| `Gpu`          | —              | Returns `Err` immediately |

To register a new built-in Rust effect, add a match arm in
`crates/hypercolor-core/src/effect/builtin/mod.rs` keyed on the source stem string
and add the corresponding entry to `builtin_metadata()` for registry discovery. The
factory wires the rest automatically. See [adding an effect](@/contributing/adding-an-effect.md)
for the full native built-in walkthrough.

---

## `EffectPool` and slot management

`crates/hypercolor-core/src/effect/pool.rs` manages the live set of renderer
instances. Each active (zone, layer) pair gets one `EffectSlot`.

```
EffectPool
  └── slots: HashMap<EffectSlotKey, EffectSlot>
              ├── key: (ZoneId, SceneLayerId)
              └── slot
                    effect_id
                    registry_metadata / registry_source_path / registry_modified
                    metadata              (with live control bindings applied)
                    display_descriptor    (set for Display-category zones)
                    renderer: Box<dyn EffectRenderer>
                    controls: HashMap<String, ControlValue>
                    binding_state         (sensor→control smoothing state)
                    elapsed_secs / frame_number
```

`EffectPool::reconcile` is called each render tick with the current zone list. It
diffs the desired set against the live slots and:

- Drops slots for zones or layers that are no longer active. `destroy()` is called
  via the slot's `Drop` implementation.
- Builds new `EffectSlot` instances when the active effect changes, when the
  registry entry is modified (hot-reload), or when the display descriptor changes.
- Calls `sync_layer_state` on existing slots to push updated control values without
  a full rebuild.

Sensor bindings (`ControlBinding`) are evaluated each frame in `apply_sensor_bindings`
and pushed to the renderer via `set_control` only when the mapped value changes. The
mapping supports configurable deadband and temporal smoothing.

There are two frame production paths on the pool:

- `render_group_into` / `render_layer_into` — writes pixels into a caller-owned
  `Canvas`. Standard path.
- `render_group_output` / `render_layer_output` — returns an `EffectRenderOutput`,
  enabling GPU-resident frames. Used by the compositor when the `servo-gpu-import`
  feature is active.
- `advance_layer_output` — ticks a renderer forward without requiring the caller to
  consume a frame immediately. Used for prefetch/pipeline staging.

---

## `EffectRegistry`

`crates/hypercolor-core/src/effect/registry.rs` is the central index of all
discovered effects, keyed by `EffectId` (UUID v7).

Key operations:

- `register(entry)` — add or replace an entry; bumps the monotonic generation counter
  when metadata, source path, or modification time changes.
- `resolve_id(id)` — resolves a compatibility alias to a canonical `EffectId`.
- `rescan()` — full filesystem rescan: re-registers all HTML effects, prunes deleted
  files. Called at startup and when the file watcher detects bulk changes.
- `reload_single(path)` — fast-path single-file hot-reload triggered by the watcher
  on a single `.html` change.
- `prune_missing()` — removes entries whose source file no longer exists on disk.
  Native effects are exempt since they have no on-disk source to check.

HTML effects support **compatibility aliases** — multiple `EffectId` values that
resolve to the same canonical entry. This allows renamed effects to retain existing
scene references without breaking user data.

The `generation` counter increments on any structural change. The engine compares
generations to decide whether an `EffectPool` reconcile is needed.

---

## Servo renderer and session model

For `EffectSource::Html` effects, the factory creates a `ServoRenderer`
(`crates/hypercolor-core/src/effect/servo/renderer.rs`). The renderer is a facade
over a shared Servo worker thread. Servo's runtime is pinned to one OS thread, but
`ServoRenderer` is `Send` so it can be stored in the pool and driven from the render
loop on any thread.

### Worker architecture

The Servo subsystem is split into focused modules:

- `worker` — OS thread spawn and teardown, `ServoWorkerRuntime`, the shared
  `SERVO_WORKER` global.
- `worker_client` — client-side `Idle → Loading → Running` state machine and the
  command channel.
- `session` — `ServoSessionHandle` per effect, bridging a `ServoWorkerClient` to the
  renderer.
- `renderer` — the `EffectRenderer` facade that drives the worker from the render loop.
- `delegate` — `WebViewDelegate` implementation handling frame readiness, console
  messages, and page-load state.
- `circuit_breaker` — consecutive-failure tracker with exponential cooldown.

### Session lifecycle

A `ServoSessionHandle` wraps the per-effect browser session:

```
ServoSessionHandle
  ├── worker: ServoWorkerClient       // channel to the shared Servo OS thread
  ├── session_id: ServoSessionId
  ├── render_width / render_height
  ├── pending_render: Option<PendingServoFrame>
  └── last_canvas: Option<Canvas>     // most recently completed CPU frame
```

The session is created via `ServoSessionHandle::new_shared`, which acquires a client
handle to the shared `SERVO_WORKER` global. When the renderer is destroyed,
`recycle_servo_session` queues the teardown on the worker thread rather than blocking
the render loop — a slow Servo close never stalls output to devices. Despite the name,
this is a detached close; sessions are not pooled or reused across effect activations.

### Per-frame flow

Each `render_into` call drives four steps in sequence:

1. `poll_load_task` — check whether the HTML file has finished loading and advance
   the session state if so.
2. `queue_frame` — capture the current `FrameInput` for injection. Data sources
   included (audio, interaction, sensor, media, lighting, net) are gated per-effect
   by metadata tags to avoid injecting unnecessary payload.
3. `poll_in_flight_render` — check whether the previous render request has returned
   a completed frame. If so, latch it into `last_canvas`.
4. `try_submit_queued_frame` — if the worker is idle, submit the queued frame input
   as a new render request.

The output is the **most recently completed frame**, not a synchronous per-tick
render. Servo renders on its own thread; CPU pixel readback arrives one or more ticks
later. While a frame is in flight, `render_into` returns the previous canvas. During
initial load before any frame is ready, a placeholder canvas is returned.

### Animation cadence

By default, HTML canvas effects and WebGL2 shader effects run with
`AnimationCadence::MatchRenderLoop` — the host submits a new render request each
tick. Effects tagged `webgl` or `canvas2d` switch to `host_driven_animation` mode;
the Servo animation loop drives the cadence instead. This distinction matters for
effects with internal `requestAnimationFrame` loops: host-driven effects respect the
Servo animation timer rather than being throttled to the render loop tier.

### Display face sessions

When `metadata.category == EffectCategory::Display`, the factory calls
`ServoRenderer::new_display_face()` instead of `ServoRenderer::new()`. This sets the
`ServoProducerRole` to `DisplayFaceHtml`, affecting telemetry tracking and frame
payload assembly. The daemon calls `set_display_descriptor` before
`init_with_canvas_size` so the face can adapt its layout to device truth (shape, safe
area, FPS policy) before the first frame. See [display faces](@/effects/display-faces.md)
for the full face authoring contract.

### GPU import (optional)

When the `servo-gpu-import` feature is enabled, the renderer can request GPU-resident
frames via `request_render_gpu`. The `render_output` override returns
`EffectRenderOutput::Gpu(ImportedEffectFrame)` when a GPU-resident frame is available,
bypassing the CPU readback entirely. This is an optional zero-copy path using the
platform interop crate (`hypercolor-linux-gpu-interop` on Linux). It is entirely
separate from the unimplemented `EffectSource::Shader` lane.

### Circuit breaker

`circuit_breaker.rs` tracks consecutive Servo render failures per session. After a
configurable threshold, the session enters an exponential-backoff cooldown before new
renders are submitted. This prevents a single broken HTML effect from poisoning the
shared Servo worker process.

---

## GPU lane status ⚡

There is no runnable wgpu compute or fragment shader lane in the current release.

The planned architecture has `EffectSource::Shader` dispatching to a `WgpuRenderer`
with `RenderAccelerationMode::Gpu` enabling it. Neither exists as callable code today.
The factory returns an error for `Shader` sources; the acceleration mode resolver
returns an error for `Gpu` mode. When this path lands it will be documented in a
dedicated page and the resolver will return a real GPU resolution.

If you need GLSL today, the correct path is **GLSL-as-WebGL2 via the TypeScript SDK**:
see [GLSL effects](@/effects/glsl-effects.md).

---

## Cross-links

- [Render pipeline](@/architecture/render-pipeline.md) — the compositor and
  FPS controller that call into `EffectPool`.
- [Event bus](@/architecture/event-bus.md) — how completed canvas frames are published
  downstream to devices and the preview WebSocket channel.
- [Adding an effect](@/contributing/adding-an-effect.md) — how to implement and
  register a compiled-in `EffectRenderer` in `builtin/`.
- [GLSL effects](@/effects/glsl-effects.md) — GLSL fragment shaders running as WebGL2
  inside Servo.
- [Display faces](@/effects/display-faces.md) — full-screen HTML faces for LCD devices,
  the `Display` category, and the `set_display_descriptor` contract.
- [Native Rust effects](@/effects/native-rust-effects.md) — authoring compiled-in Rust
  `EffectRenderer` implementations: `FrameInput`, `Canvas` API, controls, registration,
  and testing.
