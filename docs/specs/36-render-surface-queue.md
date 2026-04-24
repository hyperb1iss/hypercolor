# Spec 36 — Render Surface Queue and Buffer Ownership

> Implementation-ready specification for replacing clone-heavy canvas handoff
> with explicit surface-slot ownership, phase-separated rendering, and
> reusable transport staging inspired by Android's BufferQueue and
> SurfaceFlinger pipeline.

**Status:** Draft
**Author:** Nova
**Date:** 2026-04-08
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Related:** `docs/specs/27-render-groups.md`, `docs/specs/23-session-power-awareness.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Design Principles](#4-design-principles)
5. [Target Architecture](#5-target-architecture)
6. [Type and API Changes](#6-type-and-api-changes)
7. [Pipeline Changes](#7-pipeline-changes)
8. [Migration Plan](#8-migration-plan)
9. [Verification Strategy](#9-verification-strategy)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

Hypercolor's render pipeline already has one excellent property: the LED zone
path mostly reuses buffers once warm. The weak point is everything around the
canvas itself. Today the canonical render surface is represented by a
copy-on-write `Canvas` backed by `Arc<Vec<u8>>`, and the pipeline relies on
`clone()` to preserve previous frames, preview frames, and screen-capture
surfaces. That looks cheap at the type level but often turns the final publish
path into hidden full-frame copies.

This spec replaces clone-based canvas sharing with an explicit render-surface
queue:

- one producer-side mutable surface lease
- one published front buffer shared by readers
- one spare buffer ready for reuse
- consumer-local staging for transport-specific transforms

The design is inspired by two Android graphics ideas:

- `BufferQueue` uses explicit producer and consumer ownership and does not copy
  buffer contents as they move through the pipeline
- SurfaceFlinger keeps rendering, composition, and display pacing in separate
  phases rather than mutating one shared surface for every consumer

References:

- Android BufferQueue and Gralloc:
  <https://source.android.com/docs/core/graphics/arch-bq-gralloc>
- Android VSync and SurfaceFlinger pacing:
  <https://source.android.com/docs/core/graphics/implement-vsync>

The result should be a pipeline where the canonical render surface is produced
once, sampled directly for LED output, shared cheaply with preview/display
consumers, and only copied when a transport boundary genuinely requires
serialization.

---

## 2. Problem Statement

### 2.1 Current Failure Modes

The audit found five structural issues:

1. The main preview canvas is not truly zero-copy once `cached_canvas` holds an
   aliased `Canvas`. Preview brightness mutation and later publication force
   copy-on-write or `try_unwrap` fallback cloning.
2. Screen preview surfaces are cloned several times before the supposedly owned
   bus handoff, so the owned fast path rarely stays owned.
3. WebSocket `frames` clones `ZoneColors`, then copies them again into a fresh
   binary or JSON payload per client.
4. Backend routing rebuilds transient lookup maps and remap buffers every frame
   even though the routing graph only changes on layout or device-map changes.
5. Renderer reuse is structurally awkward because `EffectRenderer::tick`
   returns an owned `Canvas`, which encourages caching via `clone()` or fresh
   allocation every frame.

### 2.2 Why This Matters

A full-frame canvas is ~256 KB at 320x200 or ~1.17 MB at the 640x480 default.
One hidden clone per frame at 60 FPS is 15-70 MB/s of memory traffic before
transport encoding, display cropping, or screen-capture duplication. That is
not fatal on
desktop hardware, but it is exactly the kind of accidental bandwidth burn that
makes future scaling harder:

- multiple render groups
- higher preview frame rates
- higher render resolutions
- more HTML/Servo effects
- more display-capable devices

### 2.3 Architectural Root Cause

The issue is not that Hypercolor lacks reuse. The issue is that ownership is
implicit.

`Canvas` is trying to do three jobs at once:

- mutable producer surface
- immutable published surface
- cheap historical snapshot

Those jobs have conflicting semantics. A mutable producer surface wants unique
ownership. A published surface wants cheap shared reads. A historical snapshot
wants retention without mutating the canonical copy. Android solves this with
explicit queue states and buffer roles. Hypercolor should do the same.

---

## 3. Goals and Non-Goals

### Goals

- make canonical render-surface handoff explicit and mostly zero-copy
- eliminate clone-driven aliasing from the hot render path
- separate canonical render data from consumer-local transforms
- keep latest-value semantics for slow consumers without hiding wasted work
- preserve ergonomic CPU rendering for builtins and Servo readback
- support future multi-group rendering without multiplying hidden copies
- move per-frame routing work toward reusable plans and staging buffers

### Non-Goals

- changing effect visuals or color science in this spec
- moving the render pipeline to GPU composition
- introducing a generic lock-free MPMC graphics scheduler
- redesigning the REST or WebSocket protocol surface in the same step
- fixing every input/audio allocation in v1 of the refactor

This is a buffer-ownership and pipeline-shape refactor, not a feature rewrite.

---

## 4. Design Principles

### 4.1 Canonical Surfaces Are Immutable After Publish

Once a render surface is submitted for a frame, it must never be mutated again.
Any postprocessing for preview, display output, or transport encoding happens in
consumer-local staging buffers.

This is the single most important rule in the spec.

Canonical surfaces are non-premultiplied sRGB RGBA. Consumers that need a
different representation own the conversion locally: the LED path decodes
sampled sRGB into linear light before hardware output compensation, display
workers may encode JPEG or device-native packets, and websocket relays may
scale or serialize previews. None of those conversions redefine the canonical
surface.

### 4.2 Producer and Consumer Roles Must Be Separate

The effect engine is the producer.

Consumers are:

- spatial sampling for LED output
- preview/event-bus publication
- display-output cropping and JPEG encoding
- WebSocket binary/text encoding

No consumer is allowed to mutate or retain the producer's mutable working
surface.

### 4.3 Reuse Buffers by Role, Not by Accident

Hypercolor should not "reuse" memory by holding `Arc<Vec<u8>>` aliases and
hoping `clone()` stays cheap. It should reuse memory by maintaining dedicated
buffer pools:

- render surface pool
- optional screen-snapshot pool
- per-consumer transport staging buffers
- per-device LED accumulation buffers

### 4.4 Phase Separation Beats Shared Mutation

Android's display pipeline separates app rendering, SurfaceFlinger composition,
and display presentation across phase offsets. Hypercolor does not need full
VSync choreography, but it should adopt the same spirit:

- render phase writes a canonical surface
- sample phase reads it
- publish phase shares it
- transport phase derives wire formats from it

The canonical frame should not be postprocessed in place between those phases.

### 4.5 Latest-Value Queues Are Good, But Work Must Also Be Shareable

`watch` channels are still the right fit for canvas and frame state. The new
requirement is that the latest value be a cheap shared handle, not a structure
that still forces per-consumer frame cloning.

---

## 5. Target Architecture

### 5.1 High-Level Shape

```text
Input Capture
  -> ScreenSnapshotPool (optional)

Effect Producer
  -> RenderSurfacePool.dequeue()
  -> render into Back surface
  -> submit() => PublishedSurface

PublishedSurface
  -> Spatial sampler (borrowed read)
  -> Event bus canvas watch (shared handle)
  -> Display output workers (shared handle)
  -> WS preview encoders (shared handle)

Transport Staging
  -> RGB/RGBA WS buffers
  -> JPEG buffers
  -> per-device LED accumulation buffers
```

### 5.2 Surface Slots

The render path uses a fixed three-slot queue per active render pipeline:

| Slot | Role | Mutability |
|------|------|------------|
| `front` | most recently published frame | immutable |
| `back` | current producer target | mutable while leased |
| `spare` | ready-to-reuse storage | mutable after dequeue |

This is the minimum shape that gives Hypercolor the important BufferQueue
properties:

- the producer always writes into a unique surface
- the published frame remains readable while the next frame renders
- the system can reuse allocation-sized storage instead of reallocating

### 5.3 Surface States

```rust
enum SurfaceState {
    Free,
    Dequeued,
    Queued,
    Acquired,
}
```

State transitions:

```text
Free -> Dequeued -> Queued -> Acquired -> Free
```

Hypercolor does not need Android's full fence model in v1. The render thread is
single-producer, and consumers only need immutable shared reads. A simple state
machine with generation counters is enough.

### 5.4 Two Distinct Surface Kinds

This spec explicitly separates producer and published surface types:

- `RenderSurface`: mutable, uniquely leased, not `Clone`
- `PublishedSurface`: immutable, cheaply cloneable handle to a submitted slot

The current single `Canvas` type should not continue to represent both.

### 5.5 Consumer-Local Staging

Transports that genuinely need copied data keep their own reusable staging:

- WebSocket RGB preview packing
- WebSocket frame binary packing
- JPEG encoding for display output
- per-device LED routing buffers

The canonical render surface is never repacked or brightened in place.

---

## 6. Type and API Changes

### 6.1 New Core Types

```rust
pub struct SurfaceDescriptor {
    pub width: u32,
    pub height: u32,
    pub format: SurfaceFormat,
}

pub enum SurfaceFormat {
    Rgba8888,
}

pub struct RenderSurfacePool {
    // owns fixed-capacity slot storage and generation tracking
}

pub struct SurfaceLease<'a> {
    // unique mutable access to one slot
}

pub struct PublishedSurface {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    // cheap shared handle to immutable bytes in a submitted slot
}
```

### 6.2 Canvas API Evolution

The current `Canvas` type in `hypercolor-types` should evolve into two layers:

1. `CanvasWriter` or `SurfaceLease` for mutable producer access
2. `CanvasView` for borrowed immutable reads

The old `Canvas` API can remain temporarily as a facade over `SurfaceLease` for
builtin CPU effects, but the key semantic changes are:

- producer-side canvas type is not `Clone`
- immutable reads are borrowed or shared-handle based
- "freeze and publish" is an explicit operation

### 6.3 Event Bus Changes

Replace raw byte-copy-centric canvas payloads with shared published surfaces:

```rust
pub struct CanvasFrame {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub surface: PublishedSurface,
}
```

`CanvasFrame::from_canvas()` should be removed.

`CanvasFrame::from_owned_canvas()` should be replaced by a pool submit path:

```rust
let published = surface_pool.submit(lease, frame_number, timestamp_ms);
event_bus.canvas_sender().send(CanvasFrame::new(published));
```

### 6.4 Effect Renderer Contract

The current renderer contract:

```rust
fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas>;
```

should become:

```rust
fn render_into(
    &mut self,
    input: &FrameInput<'_>,
    target: &mut CanvasWriter<'_>,
) -> anyhow::Result<()>;
```

Benefits:

- builtins render directly into leased storage
- Servo readback writes into a specific target surface
- no returned `Canvas` means no renderer-side "save previous frame by clone"

### 6.5 Routing Plan

Add a reusable `RoutingPlan` in `hypercolor-core`:

```rust
pub struct RoutingPlan {
    pub zone_routes: Vec<CompiledZoneRoute>,
    pub device_targets: Vec<CompiledDeviceTarget>,
}
```

The plan is rebuilt only when:

- spatial layout changes
- device mapping changes
- attachment or logical-device routing changes

Per-frame execution uses the plan plus reusable staging buffers, not fresh
lookup maps.

### 6.6 WebSocket Encoding Cache

Add per-frame transport caches keyed by published-surface generation and
requested format:

```rust
pub struct SurfaceEncodeCache {
    pub ws_rgba: OnceLock<bytes::Bytes>,
    pub ws_rgb: OnceLock<bytes::Bytes>,
}
```

For `frames`, add a frame-payload cache keyed by:

- frame number
- selected zone set hash
- binary vs JSON

The server still allocates bytes to send over the socket, but it no longer
rebuilds the same payload independently for every subscriber.

---

## 7. Pipeline Changes

### 7.1 Input Sampling

#### Screen Capture

`ScreenData` should stop carrying an owned, cloneable `Canvas` directly.
Instead, it carries a shared `ScreenSnapshotHandle`:

```rust
pub struct ScreenData {
    pub zone_colors: Vec<ZoneColors>,
    pub snapshot: Option<ScreenSnapshotHandle>,
    pub source_width: u32,
    pub source_height: u32,
}
```

The downscaled screen image is produced once by the screen source and then moved
through the pipeline as a handle.

#### Audio

Audio publication can remain structurally unchanged in phase 1, but the new
surface architecture should avoid introducing any new audio clones. Follow-up
work may move spectrum bins to fixed-capacity arrays or shared storage.

### 7.2 Render Phase

The render thread changes from:

```text
tick effect -> return Canvas -> maybe clone/cache -> mutate preview brightness
```

to:

```text
dequeue render slot
render into leased surface
submit surface
```

No preview brightness mutation occurs here.

### 7.3 Sample Phase

The spatial engine samples directly from `PublishedSurface` bytes or a borrowed
`CanvasView` created from it. This remains zero-copy.

The existing reuse of `FrameData.zones` remains and should be preserved.

### 7.4 Publish Phase

The event bus publishes:

- `FrameData` with recycled zone buffers
- `CanvasFrame` backed by a shared `PublishedSurface`
- optional `ScreenCanvasFrame` backed by a shared `ScreenSnapshotHandle`

The publish phase does not serialize, brighten, crop, or encode.

### 7.5 Preview and Display Consumers

#### WebSocket Canvas Preview

The WS relay should:

1. acquire the latest shared surface handle
2. look up or build the encoded RGB/RGBA payload once per frame/format
3. send cached bytes to all interested clients

#### Display Output

Display workers should consume `PublishedSurface` directly and keep their
existing reusable JPEG buffer and axis-plan caches. The current display path is
already close to the desired shape; it mainly needs to stop cloning frame state
that is already immutable and shared.

#### Preview Brightness

Global output brightness should not be baked into the canonical preview
surface. Preview brightness becomes an encoder concern:

- LED output still applies LED-specific transfer and brightness in device
  staging buffers
- WS preview may apply preview brightness during RGB/RGBA pack
- display output continues using target-local brightness in its own staging

### 7.6 Idle and Sleep Behavior

Idle and sleep should use retained published surfaces rather than regenerating a
new static `Canvas` each wake cycle.

Two cases:

- `Release`: publish an empty surface handle or explicit cleared frame once
- `Static`: retain one published hold surface and schedule refresh only when the
  policy requires reassertion

### 7.7 Multi-Group Future

Spec 27 becomes cheaper after this refactor because each render group can own
its own three-slot surface pool. Without this refactor, each additional group
multiplies current hidden-canvas-copy behavior.

---

## 8. Migration Plan

### Phase 1: Introduce Shared Surface Infrastructure

Files:

- `crates/hypercolor-types/src/canvas.rs`
- `crates/hypercolor-core/src/bus/mod.rs`
- `crates/hypercolor-daemon/src/render_thread.rs`

Tasks:

- add `RenderSurfacePool`, `SurfaceLease`, and `PublishedSurface`
- keep old `Canvas` adapters temporarily where needed
- convert bus canvas publication to published-surface handles

Verification:

- render thread builds
- current preview still renders correctly
- no full-frame copy remains on publish when brightness is 1.0 and no consumer
  mutates the canonical surface

### Phase 2: Change Renderer Contract

Files:

- `crates/hypercolor-core/src/effect/traits.rs`
- `crates/hypercolor-core/src/effect/engine.rs`
- `crates/hypercolor-core/src/effect/builtin/*`
- `crates/hypercolor-core/src/effect/servo_renderer.rs`

Tasks:

- replace `tick() -> Canvas` with `render_into(...)`
- update builtins to render directly into target surfaces
- remove framebuffer retention via `Canvas::clone()`
- have Servo read back directly into pool-backed target storage

Verification:

- builtin effect tests updated
- Servo tests updated
- no renderer stores previous frame by cloning producer storage

### Phase 3: Move Preview and Display to Encoded Caches

Files:

- `crates/hypercolor-daemon/src/api/ws.rs`
- `crates/hypercolor-daemon/src/display_output.rs`

Tasks:

- add encoded preview caches keyed by surface generation and format
- remove per-client clone-then-pack for `frames`
- keep display JPEG buffer reuse, but source directly from shared surfaces

Verification:

- multiple WS clients do not cause repeated per-frame RGB/RGBA repacks
- display output still updates correctly under load

### Phase 4: Compile Routing Plans

Files:

- `crates/hypercolor-core/src/device/manager.rs`
- `crates/hypercolor-daemon/src/discovery.rs`
- any layout/device-map integration points

Tasks:

- compile routing plans on topology changes
- keep reusable per-device staging vectors
- stop rebuilding `HashMap` and `HashSet` routing structures every frame

Verification:

- backend manager tests still pass
- routing output matches pre-refactor behavior

### Phase 5: Optional Input Cleanup

Files:

- `crates/hypercolor-core/src/input/mod.rs`
- `crates/hypercolor-core/src/input/screen/mod.rs`
- `crates/hypercolor-core/src/input/screen/wayland.rs`

Tasks:

- move screen downscale to shared snapshot handles
- add `sample_all_into` style APIs where practical

Verification:

- screen-reactive effects still receive valid frames
- no extra screen-canvas clones before publish

---

## 9. Verification Strategy

### 9.1 Correctness

- `just check`
- `just test`
- `just test-crate hypercolor-core`
- `just test-crate hypercolor-daemon`

### 9.2 Behavioral

- verify builtins, Servo effects, and screen-reactive effects still render
- verify global brightness changes only affect consumer-local output, not the
  canonical published surface
- verify paused, idle, and sleep policies preserve current behavior

### 9.3 Performance

Add targeted instrumentation before and after each migration phase:

- full-frame canvas copy count per second
- preview encode count per frame
- WS payload build count per frame and per client
- routing-plan rebuild count
- bytes allocated per frame in render, publish, and transport stages

Success criteria:

- canonical render-surface publish performs zero full-frame copies in the steady
  state
- one WS preview payload build per frame per format, not per client
- routing-plan rebuilds only on layout or mapping changes
- display output reuses JPEG buffers and does not clone canonical surfaces
- shared USB display traffic yields to overdue LED traffic and reports bounded
  wait metrics

### 9.4 Adversarial Checks

- brightness below 1.0 must not reintroduce hidden copies of the canonical
  surface
- multiple WS clients with mixed RGB and RGBA subscriptions must share encoded
  caches where formats match
- Servo fallback frame retention must not alias active producer storage
- a Servo LED effect, Servo display face, preview stream, device metrics
  stream, audio/screen capture toggles, and shared USB output must soak
  together without unbounded queues, sustained FPS collapse, repeated failure
  spam, or copy-counter growth after warmup

---

## 10. Recommendation

Build this in the following order:

1. Introduce explicit `RenderSurfacePool` / `PublishedSurface` ownership.
2. Change renderers to render into leased surfaces instead of returning cloned
   `Canvas` values.
3. Move preview and display consumers to consumer-local staging and cached
   encodings.
4. Compile backend routing plans instead of rebuilding lookup maps every frame.

This is the right architectural seam because it fixes the actual problem:
Hypercolor currently has decent buffer reuse inside isolated subsystems, but it
does not have a first-class ownership model for the canonical frame.

The clear choice is to adopt a BufferQueue-like front/back/spare surface model,
not to keep chasing isolated `clone()` calls. That gives the project:

- explicit producer/consumer boundaries
- zero-copy steady-state frame publish
- cleaner render-group scaling
- lower preview and transport overhead
- a pipeline shape that is easier to reason about, test, and profile

If we do not make this ownership split, every future performance pass will keep
fighting the same structural ambiguity from a different angle.
