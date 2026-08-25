+++
title = "Renderer internals"
description = "The EffectRenderer trait, EffectSource variants, factory dispatch, the Servo session model, and the GPU compositor lane."
weight = 30
+++

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

- **Compiled-in Rust renderers**: pure CPU canvas effects registered in
  `crates/hypercolor-core/src/effect/builtin/`. Selected by `EffectSource::Native`.
- **Servo HTML/WebGL2 renderers**: HTML files executed inside a headless Servo
  browser engine. Covers TypeScript canvas effects and GLSL shaders bundled as
  WebGL2 by the SDK. Selected by `EffectSource::Html`.

There is no wgpu native **shader-effect** lane available at runtime (the
compositor's GPU lane is a separate, shipped path; see
[GPU compositor lane](#gpu-compositor-lane)). `EffectSource::Shader`
exists in the type system as a reserved variant, but the factory bails immediately:

```rust
// crates/hypercolor-core/src/effect/factory.rs
EffectSource::Shader { path } => bail!(
    "shader effect '{}' is not runnable yet (source: {})",
    metadata.name,
    path.display()
),
```

For the effect-renderer resolver specifically, `RenderAccelerationMode::Gpu`
returns an error and `Auto` silently falls back to CPU with
`fallback_reason = "gpu effect renderer acceleration is not available yet"`.
The wgpu compute/fragment shader lane is planned future work. GLSL effects work
today because they run as **WebGL2 inside Servo** as `EffectSource::Html`, not
through any wgpu path. None of this constrains the compositor, whose own
`compositor_acceleration_mode` key can and does resolve to GPU.

The doc-comments on `EffectSource::Native` used to claim it was "rendered by
`WgpuRenderer`", a type that was never built. They now describe the real behavior:
`Native` resolves a compiled-in CPU builtin by path stem.

---

## The three `EffectSource` variants

`EffectSource` is the discriminant that routes effect metadata to a renderer. Defined
in `crates/hypercolor-types/src/effect.rs`:

```rust
pub enum EffectSource {
    /// "Native": dispatches to a compiled-in CPU renderer keyed by path stem.
    Native { path: PathBuf },
    /// HTML/Canvas/WebGL effect executed by ServoRenderer.
    Html { path: PathBuf },
    /// GPU shader lane, not runnable yet. Factory returns Err.
    Shader { path: PathBuf },
}
```

What each variant **actually does** at runtime:

| Variant  | Renderer                          | GPU | `servo` feature required |
|----------|-----------------------------------|-----|--------------------------|
| `Native` | Rust struct from `builtin/`       | No, CPU only | No |
| `Html`   | `ServoRenderer`                   | GPU framebuffer import when available (default `auto`); CPU readback is the fallback path | Yes |
| `Shader` | None; factory returns `Err`       | N/A | N/A |

The `source_stem()` helper extracts the file stem of the source path as
`Option<&str>`. The factory uses it as the lookup key for native effects, falling
back to the effect's display name when the stem is unavailable.

`EffectState` (also in `hypercolor-types`) tracks the registry lifecycle:

```
Loading → Initializing → Running → Paused → Destroying
```

- `Loading`: source files discovered, metadata parsed and validated.
- `Initializing`: `init_with_canvas_size` called; HTML load or resource
  allocation in progress.
- `Running`: `render_into` called every render tick.
- `Paused`: renderer alive, not producing frames (crossfade transitions).
- `Destroying`: `destroy()` called; Servo session or other resources released.

---

## `EffectRenderer` trait

The full trait surface is in `crates/hypercolor-core/src/effect/traits.rs`. Every
renderer (built-in Rust or Servo-backed HTML) implements it.

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

    // Control and asset binding
    fn initialize_controls(&mut self, controls: &ControlSet)
        -> anyhow::Result<()>;  // default projects one full delta
    fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>)
        -> anyhow::Result<()>;
    fn bind_asset_library(&mut self, _library: Arc<RwLock<AssetLibrary>>) {}
    fn set_display_descriptor(&mut self, _descriptor: Option<DisplayDescriptor>) {}

    // Optional secondary preview stream
    fn preview_canvas(&self) -> Option<Canvas> { None }
}
```

The trait is `Send` but **not `Sync`**. Renderers are therefore held behind a `Mutex`,
never an `RwLock`. Servo's renderer is pinned to one OS thread, which makes `Sync`
impossible. The per-zone renderer slots live in `EffectPool`
(`crates/hypercolor-core/src/effect/pool.rs`), keyed by `EffectSlotKey`.

`render_into` is the canonical CPU path. The engine passes a reusable canvas,
which avoids allocating a new target for every frame. `render_output` adds the
GPU-resident path without weakening that contract.

### `FrameInput` fields

`FrameInput` is passed by reference on every tick:

```rust
pub struct FrameInput<'a> {
    pub time_secs: f64,            // seconds since effect activation
    pub delta_secs: f32,           // time since previous frame
    pub frame_number: u64,         // monotonic counter starting at 0
    pub audio: &'a AudioData,      // always present; AudioData::silence() when no source
    pub interaction: &'a InteractionData,
    pub screen: Option<&'a Arc<ScreenBranchPublication>>,
    pub sensors: &'a SystemSnapshot,
    pub sources: FrameDataSources<'a>,  // media / net / lighting for display faces
    pub canvas_width: u32,
    pub canvas_height: u32,
}
```

The screen publication is shared by reference count, so renderers that queue frames
retain it without copying pixels. GPU-resident publications carry no CPU pixels and
therefore read as absent screen content to CPU renderers.

The default canvas dimensions are **640×480** (`DEFAULT_CANVAS_WIDTH` /
`DEFAULT_CANVAS_HEIGHT` in `hypercolor-types::canvas`). Both values are configurable
and can change live at a frame boundary. Never hardcode them.

Animate against `delta_secs` or `time_secs`, not `frame_number`: the render loop
runs at adaptive FPS across five tiers (10 / 20 / 30 / 45 / 60). The integer frame
counter is monotonic but not wall-clock proportional.

`FrameDataSources` bundles optional typed data beyond audio:

```rust
pub struct FrameDataSources<'a> {
    pub input_availability: InputSourceAvailability, // routed interaction source lifecycle
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

Before dispatch, the factory resolves the requested `RenderAccelerationMode` for
the effect-renderer lane (this resolver is separate from the compositor's
startup resolution of `compositor_acceleration_mode`):

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
                    controls: ControlSet
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
and delivered through one ordered `apply_controls` batch when mapped values change.
The mapping supports configurable deadband and temporal smoothing. A renderer that
rejects a delta receives one authoritative snapshot replay through
`initialize_controls`.

There are two frame production paths on the pool:

- `render_zone_into` / `render_layer_into`: writes pixels into a caller-owned
  `Canvas`. Standard path.
- `render_zone_output` / `render_layer_output`: returns an `EffectRenderOutput`,
  enabling GPU-resident frames. Used by the compositor when the `servo-gpu-import`
  feature is active.
- `advance_layer_output`: ticks a renderer forward without requiring the caller to
  consume a frame immediately. Used for prefetch/pipeline staging.

---

## `EffectRegistry`

`crates/hypercolor-core/src/effect/registry.rs` is the central index of all
discovered effects, keyed by `EffectId` (UUID v7).

Key operations:

- `register(entry)`: add or replace an entry; bumps the monotonic generation counter
  when metadata, source path, or modification time changes.
- `rescan()`: full filesystem rescan that re-registers all HTML effects and prunes
  deleted files. Called at startup and when the file watcher detects bulk changes.
- `reload_single(path)`: fast-path single-file hot-reload triggered by the watcher
  on a single `.html` change.
- `prune_missing()`: removes entries whose source file no longer exists on disk.
  Native effects are exempt since they have no on-disk source to check.

Each entry's `EffectId` is a UUID v7 minted when the entry is first registered, and
`register()` replaces by that id, so a renamed effect file keeps the id it already
holds and existing scene references stay valid.

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

- `worker`: OS thread spawn and teardown, `ServoWorkerRuntime`, the shared
  `SERVO_WORKER` global.
- `worker_client`: client-side `Idle → Loading → Running` state machine and the
  command channel.
- `session`: `ServoSessionHandle` per effect, bridging a `ServoWorkerClient` to the
  renderer.
- `renderer`: the `EffectRenderer` facade that drives the worker from the render loop.
- `delegate`: `WebViewDelegate` implementation handling frame readiness, console
  messages, and page-load state.
- `circuit_breaker`: consecutive-failure tracker with exponential cooldown.
- `gpu_import` and `gpu_import_backend`: the zero-copy GPU frame import lane and its
  per-platform backends.
- `memory`: Servo memory accounting.
- `telemetry`: the counters and timings surfaced as `status.effect_health`.

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
the render loop, so a slow Servo close never stalls output to devices. Despite the name,
this is a detached close; sessions are not pooled or reused across effect activations.

### Per-frame flow

Each `render_into` call drives four steps in sequence:

1. `poll_load_task`: check whether the HTML file has finished loading and advance
   the session state if so.
2. `queue_frame`: capture the current `FrameInput` for injection. Data sources
   included (audio, interaction, sensor, media, lighting, net) are gated per-effect
   by metadata tags to avoid injecting unnecessary payload.
3. `poll_in_flight_render`: check whether the previous render request has returned
   a completed frame. If so, latch it into `last_canvas`.
4. `try_submit_queued_frame`: if the worker is idle, submit the queued frame input
   as a new render request.

The output is the **most recently completed frame**, not a synchronous per-tick
render. Servo renders on its own thread; the completed frame (GPU-imported, or CPU
pixel readback on the fallback path) arrives one or more ticks later. While a frame is in flight, `render_into` returns the previous canvas. During
initial load before any frame is ready, a placeholder canvas is returned.

### Animation cadence

By default, HTML canvas effects and WebGL2 shader effects run with
`AnimationCadence::MatchRenderLoop`: the host submits a new render request each
tick. Display-category effects instead take a fixed cadence cap.

Separately from cadence, HTML effects that declare neither a `webgl` nor a `canvas2d`
renderer tag run in `host_driven_animation` mode. In that mode the per-tick frame
payload calls the page's render entry point directly, so the host drives every painted
frame. Tagging an effect `webgl` or `canvas2d` turns the mode off, leaving the page's
own `requestAnimationFrame` loop to drive its animation inside Servo while the host
reads back whatever the page has most recently painted. On macOS host-driven animation
is always on regardless of tags. The meta parser inserts both tags automatically from
the effect's `renderer=` meta.

### Display face sessions

When `metadata.category == EffectCategory::Display`, the factory calls
`ServoRenderer::new_display_face()` instead of `ServoRenderer::new()`. This sets the
`ServoProducerRole` to `DisplayFaceHtml`, affecting telemetry tracking and frame
payload assembly. The daemon calls `set_display_descriptor` before
`init_with_canvas_size` so the face can adapt its layout to device truth (shape, safe
area, FPS policy) before the first frame. See [display faces](@/effects/display-faces.md)
for the full face authoring contract.

### GPU framebuffer import

Servo GPU framebuffer import ships on all three platforms and is governed by the
`rendering.servo_gpu_import.mode` config key (`ServoGpuImportMode`, default
`auto`): `auto` attempts import when startup capabilities indicate it can work
and falls back to CPU readback otherwise, `on` requires import and reports frame
errors instead of silently reading back, and `off` disables it. When import is
active the renderer submits with a GPU preference via
`try_submit_queued_frame_with_gpu_preference`, and
the `render_output` override returns `EffectRenderOutput::Gpu(ImportedEffectFrame)`,
bypassing the CPU readback entirely (on Linux the zero-copy path goes through the
`hypercolor-linux-gpu-interop` crate). CPU readback remains the fallback path,
not the only path. This import lane is entirely separate from the unimplemented
`EffectSource::Shader` lane.

### Circuit breaker

`circuit_breaker.rs` tracks consecutive Servo render failures per session. After a
configurable threshold, the session enters an exponential-backoff cooldown before new
renders are submitted. This prevents a single broken HTML effect from poisoning the
shared Servo worker process.

---

## GPU compositor lane

The scene compositor (SparkleFlinger, in
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/`) has a shipped GPU
lane alongside its CPU path. The `compositor_acceleration_mode` config key
selects it (`RenderAccelerationMode`, default `auto`). At daemon startup the
mode is resolved against a wgpu adapter probe: `auto` takes the GPU lane when a
compatible non-software adapter passes the probe and falls back to CPU with a
recorded reason otherwise; `gpu` requires the lane and refuses software
adapters; `cpu` forces the CPU path. SparkleFlinger is then constructed with the
resolved mode, and a runtime GPU failure downgrades the compositor back to the
CPU path rather than dropping frames.

On the GPU lane, per-producer surfaces upload to textures and the blend,
transform, and preview passes run as wgpu compute shaders. Spatial sampling runs
on the GPU too (`gpu_sampling.rs`, `sample.wgsl`): the prepared zone plan
becomes a buffer of per-LED sample points (nearest, bilinear, or area), and
sampled `ZoneColors` return through triple-buffered async readback slots so the
render thread never blocks on the GPU.

Area sampling is where the GPU lane changes the algorithm, not just the venue.
Instead of the CPU path's `(2r+1)²` reads per LED, the compositor builds a
summed-area table over the composed canvas (`gpu_area_sat.rs`, `area_sat.wgsl`):
workgroup prefix scans accumulate 64-bit-wide per-channel sums in 256-wide
tiles, and a hierarchical scan (`area_hierarchy.wgsl`) stitches the tiles into
full-canvas prefix sums. Any-radius area samples then cost four table lookups.

Resource management on this lane is transactional. Zone sampling plans and
projected-scene resources are prepared first (`prepare_zone_sampling_plan`,
`prepare_projected_scene_resources`, `prepare_canvas_resize`), producing a
preparation object that is either applied atomically (`apply_zone_sampling_plan`
activates the new layout's sample-point buffers, keyed by plan generation) or
discarded with a fallback, so a failed allocation never leaves the compositor
holding half-built state. Display finalize also runs on the GPU
(`display_finalize.wgsl`): scene and face textures blend per display (replace,
alpha, tint, luma-reveal, add, screen modes) with brightness, edge, and viewport
transforms, writing both RGBA output and YUV420 planes through its own
triple-buffered readback set.

## Native shader lane status ⚡

There is no runnable wgpu compute or fragment **shader-effect** lane in the
current release; the GPU compositor lane above is what runs on the GPU today.

The planned architecture has `EffectSource::Shader` dispatching to a `WgpuRenderer`
with the effect-renderer acceleration resolver enabling it. Neither exists as
callable code today. The factory returns an error for `Shader` sources, and the
effect-renderer resolver returns an error for `Gpu` mode. When this path lands it
will be documented in a dedicated page and the resolver will return a real GPU
resolution.

If you need GLSL today, the correct path is **GLSL-as-WebGL2 via the TypeScript SDK**:
see [GLSL effects](@/effects/glsl-effects.md).

---

## Cross-links

- [Render pipeline](@/architecture/render-pipeline.md): the compositor and
  FPS controller that call into `EffectPool`.
- [Event bus](@/architecture/event-bus.md): how completed canvas frames are published
  downstream to devices and the preview WebSocket channel.
- [Adding an effect](@/contributing/adding-an-effect.md): how to implement and
  register a compiled-in `EffectRenderer` in `builtin/`.
- [GLSL effects](@/effects/glsl-effects.md): GLSL fragment shaders running as WebGL2
  inside Servo.
- [Display faces](@/effects/display-faces.md): full-screen HTML faces for LCD devices,
  the `Display` category, and the `set_display_descriptor` contract.
- [Native Rust effects](@/effects/native-rust-effects.md): authoring compiled-in Rust
  `EffectRenderer` implementations: `FrameInput`, `Canvas` API, controls, registration,
  and testing.
