# 30 — SparkleFlinger Implementation Overview

> Reference for how SparkleFlinger actually works in the tree today. The
> companion to `docs/archive/2026-03-sparkleflinger-60fps-evolution.md`, which
> captures the historical intent; this document describes the shipped state
> and invariants.

**Status:** Living document
**Last updated:** 2026-04-10
**Scope:** `hypercolor-daemon::render_thread`, `hypercolor-daemon::performance`,
`hypercolor-daemon::preview_runtime`, `hypercolor-daemon::scene_transactions`,
`hypercolor-daemon::api::{system, ws}`, `hypercolor-cli::commands::status`
**Related:** `docs/archive/2026-03-sparkleflinger-60fps-evolution.md` (archived intent),
`docs/specs/27-render-groups.md`, `docs/specs/36-render-surface-queue.md`

---

## 1. What SparkleFlinger is today

SparkleFlinger is the CPU compositor service that sits between frame producers
and spatial sampling in the Hypercolor render thread. It owns composition, not
scheduling — the render thread wakes, drains scene transactions into a
private render-local world, builds an atomic scene snapshot, asks producers
for surfaces, runs their outputs through SparkleFlinger, and hands the composed
result to the spatial sampler and device backends.

The core type is one unit struct and four helpers, all in
`crates/hypercolor-daemon/src/render_thread/sparkleflinger.rs`:

- `SparkleFlinger` — the compositor. Stateless. One method: `compose`.
- `CompositionPlan` — width, height, and an ordered `Vec<CompositionLayer>`.
- `CompositionLayer` — one `ProducerFrame` plus a `CompositionMode` and an
  `opacity`.
- `CompositionMode` — `Replace`, `Alpha`, `Add`, `Screen`.
- `ComposedFrameSet` — the output: `sampling_canvas`, optional
  `sampling_surface`, optional `preview_surface`, and a `bypassed` flag that
  tells downstream code whether composition was a zero-cost pass-through.

Blend modes are `Replace`, `Alpha`, `Add`, and `Screen`. `Replace` with full
opacity and a single layer is the **bypass fast path** — the source frame's
surface passes through `ComposedFrameSet` unchanged (both `sampling_surface`
and `preview_surface` point at the source's published surface) and no
per-pixel work runs. `compose` consumes the layer directly via `Vec::pop`,
so there is no clone on the happy path.

Blend math runs in premultiplied linear-light sRGB. Every non-bypass pixel
goes `Rgba::to_linear_f32 → BlendMode::blend → RgbaF32::to_srgba`, which
preserves the storage contract while getting correct gamma on the math side.
`Replace` with full opacity inside a multi-layer plan uses a direct
`copy_from_slice` rather than the per-pixel blend loop.

## 2. Render-thread module map

Everything lives under `crates/hypercolor-daemon/src/render_thread/`, with
two adjacent modules in the parent (`preview_runtime.rs`, `scene_transactions.rs`):

```
render_thread/
├── mod (render_thread.rs)     — thread spawn, RenderThreadState, pool sizing,
│                                 spawn/shutdown, small helper tests
├── pipeline_driver.rs         — run_pipeline loop: wake, tick, execute, sleep,
│                                 inactive-render-loop handling
├── pipeline_runtime.rs        — PipelineRuntime, FrameLoopState, RenderCaches,
│                                 RenderSurfaceSnapshot, FrameInputs, StaticSurfaceKey
├── frame_admission.rs         — FrameAdmissionController: EWMA + p95/p99
│                                 hysteresis gate for the 60 FPS tier ceiling
├── frame_scheduler.rs         — FrameScheduler, FrameSceneSnapshot,
│                                 SceneRuntimeSnapshot, SceneTransitionSnapshot
├── frame_state.rs             — EffectDemand, build_frame_scene_snapshot,
│                                 CachedRenderGroupDemand, reconcile_audio_capture,
│                                 reconcile_screen_capture
├── scene_state.rs             — RenderSceneState: drains SceneTransactionQueue
│                                 into a render-local SpatialEngine
├── frame_executor.rs          — execute_frame: the frame lifecycle in one place
├── frame_composer.rs          — compose_frame: chooses single-effect vs
│                                 render-group path, drives SparkleFlinger,
│                                 manages render_surface_pool expansion
├── composition_planner.rs     — CompositionPlanner: compiles layered plans,
│                                 caches last_stable_frame, crossfades
├── sparkleflinger.rs          — the compositor itself (see above)
├── producer_queue.rs          — ProducerQueue: submit_latest/submit_for_generation
│                                 with explicit Latest/Tagged(u64) generations
├── render_groups.rs           — RenderGroupRuntime: per-group canvases,
│                                 per-group spatial engines, effect pool,
│                                 pooled preview compose, retention cache
├── frame_sources.rs           — render_effect_into, static_surface (cached)
├── frame_io.rs                — sample_inputs, publish_frame_updates,
│                                 screen_data_to_canvas, screen-frame suppression
├── frame_pacing.rs            — SkipDecision, NextWake, deadline helpers,
│                                 two-stage wait_until_frame_deadline
└── frame_throttle.rs          — maybe_idle_throttle, maybe_sleep_throttle
```

Outside the render_thread directory:

```
preview_runtime.rs             — PreviewRuntime seam: wraps the event-bus
                                  canvas/screen_canvas watches, atomic
                                  receiver counting, published-frame telemetry
scene_transactions.rs          — SceneTransaction enum, SceneTransactionQueue,
                                  apply_layout_update helper
```

The root `render_thread.rs` holds the spawn machinery, the `RenderThreadState`
struct that is injected from `AppState`, the render-surface pool sizing
heuristic (`desired_render_surface_slots`), and tests for the helper modules
(`frame_pacing`, `frame_throttle`, `frame_io::screen_data_to_canvas`).

`RenderThreadState` owns two keys that matter for composition: the shared
`Arc<RwLock<SpatialEngine>>` that API handlers read, and a dedicated
`Arc<PreviewRuntime>` that the pipeline reports canvas publications into.
The render thread does **not** read from the shared spatial engine after
spawn — it works from a private clone maintained inside `RenderSceneState`
(see §3.2).

## 3. Frame lifecycle

One tick of the render thread, in order. The entry point is
`pipeline_driver::run_pipeline`, which loops, and the thick work happens in
`frame_executor::execute_frame`.

### 3.1 Wake and gate

`pipeline_driver` waits until `next_frame_at` using a two-stage pacing guard:
`tokio::time::sleep_until` for the coarse portion and a spin / `yield_now`
loop for the final microseconds (`frame_pacing::wait_until_frame_deadline`,
`PRECISE_WAKE_GUARD = 1 ms`, spin threshold 150 µs). Then it calls
`RenderLoop::tick()` to check whether the render loop is still running.
If it's paused or stopped, `handle_inactive_render_loop` clears capture
demand and sleeps for `PAUSED_POLL_INTERVAL` (50 ms).

### 3.2 Scene transaction drain and snapshot

`execute_frame` calls
`render.render_scene_state.apply_transactions(&state.scene_transactions)` to
drain every pending `SceneTransaction` into the render thread's **private**
`RenderSceneState` in one atomic sweep. Current transactions are
`ReplaceLayout(SpatialLayout)` and `SetScreenCaptureConfigured(bool)`.
API handlers enqueue transactions through `apply_layout_update`, which also
writes to the shared `state.spatial_engine` so other subsystems see the new
layout immediately — but the render thread reads only from its local copy,
so nothing outside the render loop can mutate mid-frame state.

It then calls `build_frame_scene_snapshot`, which assembles a
`FrameSceneSnapshot` from:

- the render loop's frame token, elapsed ms, and target interval (budget)
- the current `OutputPowerState` watched from session policy
- the `EffectDemand` computed from the active effect engine or active render
  groups. Render-group demand is cached by `active_render_groups_revision`
  in `CachedRenderGroupDemand` so no-op ticks never re-read the registry.
- a `SceneRuntimeSnapshot` built from `SceneManager`: active scene id, active
  transition progress, and `Arc<[RenderGroup]>` from the manager's cached
  active-groups snapshot. `tick_transition` only acquires a `write` lock
  when `is_transitioning()` is true; otherwise the snapshot reads through
  a cheap `read` lock.
- a clone of the current `SpatialEngine` from `RenderSceneState`

From this point on, every downstream stage — producer, compositor, sampler,
backend, metrics — observes the same immutable world. This is Section 11.4
of the design doc: one frame, one coherent world, one `frame_token`.

### 3.3 Capture demand reconciliation

`reconcile_audio_capture` and `reconcile_screen_capture` are called with
`!output_power.sleeping && effect_demand.audio/screen_capture_active`. Each
reconciler carries an `Option<bool>` latch of the last applied desired
state and only touches the `InputManager` when the desired state actually
changes, so flipping between effects doesn't thrash capture startup.

### 3.4 Sleep and idle throttle

`maybe_sleep_throttle` runs first. When `output_power.sleeping` is true, the
throttle distinguishes two off-output behaviors:

- `Release` — clear the zones, publish a black `static_surface` as a sampling
  and preview surface, and exit early. The static surface is cached in
  `RenderCaches::static_surface_cache` keyed by `(width, height, color)`
  so subsequent sleeping ticks reuse the same `PublishedSurface`.
- Any other behavior (hold color) — synthesize a canvas from the cached
  static surface, sample it through the spatial engine, push to backends,
  and publish the resulting frame. This still emits a full `FrameTimeline`
  so `/status` shows honest zeros instead of stale data.

After the first sleep tick, `sleep_black_pushed` is set and subsequent sleep
ticks fast-path out via `SESSION_SLEEP_THROTTLE_SLEEP` (250 ms) without
resynthesising the frame.

`maybe_idle_throttle` runs next. If the effect is not running and screen
capture is inactive, and the previous black frame has already been pushed
(`idle_black_pushed`), it sleeps for `IDLE_THROTTLE_SLEEP` (120 ms) and
returns early. Otherwise the frame runs through the full lifecycle and
`idle_black_pushed` is set at the end so the next fully-idle tick can
throttle.

Both throttle paths bypass composition and the producer stages but still
record `LatestFrameMetrics` with pacing-relevant fields populated.

### 3.5 Input sampling

If `skip_decision == SkipDecision::None`, `sample_inputs` acquires the
`InputManager` lock, samples all input sources via
`sample_all_with_delta_secs`, drains queued input events for broadcast, and
converts screen zones to a canvas if needed. The input manager exposes the
pre-downsampled `canvas_downscale: Option<PublishedSurface>` so the screen
path can bypass the per-pixel conversion when the downscaled surface already
matches the render canvas extent. If `skip_decision` is `ReuseInputs` or
`ReuseCanvas`, the cached `FrameInputs` from the previous tick are reused.

### 3.6 Composition

`compose_frame` is where SparkleFlinger actually runs. It branches on whether
there are active render groups in the scene snapshot.

**Single-effect path:**

1. If no effect is running, try to latch a frame from `screen_queue`
   (`submit_latest` with the input's downscaled surface or a
   `screen_data_to_canvas` fallback, then `latch_latest`), or fall back to
   the cached `static_surface` painted black.
2. If the effect is running and `skip_decision == ReuseCanvas`, try to latch
   from `effect_queue` via `latch_for_generation(effect_generation)`. The
   queue only keeps a single slot: if the tagged generation matches, the
   latch succeeds and is marked `Fresh` the first time, `Retained` on every
   subsequent latch of the same submission.
3. Otherwise clear `effect_queue` and render a fresh frame into a pooled
   render surface lease (`render_effect_into`). If the pool is exhausted,
   `render_effect_frame` expands the pool by one slot up to
   `MAX_RENDER_SURFACE_SLOTS` (12), biased toward
   `desired_render_surface_slots(canvas_receiver_count)`. Beyond that
   ceiling it falls back to an owned `Canvas` publish path that costs a
   full-frame copy. The render surface pool starts at
   `DEFAULT_RENDER_SURFACE_SLOTS` (8) and grows by `2 *
canvas_receiver_count` as preview consumers attach.
4. Hand the `ProducerFrame` to `CompositionPlanner::compile_primary_frame`,
   which adds crossfade layers if a scene transition is active and
   remembers the last stable frame so the next transition can pin its
   base.
5. Run the compiled plan through `SparkleFlinger::compose`.

**Render-group path** (`compose_render_group_frame_set`):

1. If `skip_decision == ReuseCanvas`, try `RenderGroupRuntime::reuse_scene`,
   which does an equality check on the current `active_render_groups_revision`
   and, on hit, returns the previously composed `RenderGroupResult` with
   `reuse_published_zones: true`. The caller skips sampling entirely and
   reads zones from the last published `FrameData` on the event bus watch
   channel.
2. Otherwise call `RenderGroupRuntime::render_scene`, which reconciles the
   per-group canvases and spatial engines (gated by `reconciled_groups_revision`
   so reconcile is a no-op on unchanged scenes), calls
   `EffectPool::render_group_into` for each enabled group with an effect,
   and samples each group's canvas into a shared `zones` buffer via
   `SpatialEngine::sample_append_into_at` (append-in-place, no per-frame
   allocations). The combined layout is cached as an `Arc<SpatialLayout>`.
3. Compose a tiled preview canvas (`compose_preview`) leased from a
   dedicated `preview_surface_pool` (falls back to an owned canvas only
   when the pool is exhausted). Single-group previews blit or bilinearly
   scale the group canvas into the preview extent; multi-group previews
   tile into `ceil(sqrt(count))` columns.
4. Feed the combined preview frame to
   `CompositionPlanner::compile_primary_frame` and
   `SparkleFlinger::compose`.
5. Return a `RenderGroupResult` that carries the combined layout plus a
   `reuse_published_zones` flag, so the sampler stage in `execute_frame`
   can either reuse the previous `FrameData::zones` (on retention) or
   pick up the freshly-sampled zones from the recycled frame buffer.

Both paths produce `RenderStageStats` containing a `ComposedFrameSet` plus
stage-by-stage microsecond timings and a handful of booleans for the
telemetry layer (`effect_retained`, `screen_retained`, `composition_bypassed`,
`scene_active`, `scene_transition_active`, `logical_layer_count`,
`render_group_count`). The render-group layer count is computed via
`effective_render_group_layer_count` so a crossfade over a multi-group
scene reports layers + transition overhead without double-counting.

### 3.7 Spatial sampling

Three routes feed the final zone buffer:

1. **Render-group retention** — if `reuse_published_frame` is true,
   `execute_frame` borrows the zones from the currently published
   `FrameData` on the event bus watch channel and skips sampling entirely.
2. **Render-group fresh** — if `sampled_layout` is set and `sampled_zones`
   is not, the zones were already written into `recycled_frame.zones` by
   `RenderGroupRuntime::render_scene`, so only the layout Arc is carried
   forward.
3. **Single-effect / screen path** — calls
   `scene_snapshot.spatial_engine.sample_into(sampling_canvas,
&mut recycled_frame.zones)`. This reuses the Vec in place, so steady-state
   sampling allocates nothing.

Either way the result is a zone-color slice and an `Arc<SpatialLayout>`
ready for backend routing.

### 3.8 Device push

`BackendManager::write_frame_with_brightness` takes the lock, pushes the
zones to each configured backend with the current effective brightness
from `OutputPowerState`, and returns per-device write stats plus any
queued async write failures. Async failures route back through
`handle_async_write_failures` to the discovery runtime.

### 3.9 Publish

`publish_frame_updates` is the final fan-out:

- `event_bus.frame_sender().send_replace` publishes the `FrameData` on the
  watch channel and hands the outgoing `FrameData` back to
  `RenderCaches::recycled_frame` so the zones buffer is reused on the next
  tick. When `reuse_published_frame` is true this stage is skipped entirely
  and the published frame simply stays put.
- `event_bus.spectrum_sender().send` publishes the audio spectrum snapshot
  only when `spectrum_receiver_count > 0`.
- `HypercolorEvent::AudioLevelUpdate` is broadcast at most every
  `AUDIO_LEVEL_EVENT_INTERVAL_MS` (100 ms) and only when there are event
  subscribers.
- The composed canvas is wrapped as a `CanvasFrame` — preferring the
  slot-backed surface when available and falling back to the owned-canvas
  path otherwise. `preview_runtime.record_canvas_publication` records the
  frame number, timestamp, and publication count, then
  `event_bus.canvas_sender().send` hands the frame to subscribers.
- The dedicated screen canvas is published via
  `event_bus.screen_canvas_sender()`, but
  `should_publish_screen_frame` skips the send when both the new and the
  currently published screen frame are empty, avoiding a chatter loop of
  empty watch updates. `preview_runtime.record_screen_canvas_publication`
  mirrors the main canvas telemetry.
- `HypercolorEvent::FrameRendered` is broadcast with the frame number and a
  `FrameTiming` summary, gated on `subscriber_count > 0`.

The `screen_watch_surface` that feeds the screen canvas is chosen in
priority order: the composed `preview_surface` first (which bypass produces),
then the composed `sampling_surface`, then the raw `inputs.screen_preview_surface`.
Full-frame copy counts and bytes are tallied only when the canvas path
couldn't hand off a slot-backed surface (i.e. it had to clone). These
counters feed the zero-copy audit budget and the admission gate's copy
pressure trigger.

### 3.10 Metrics record

Everything lands in `performance.rs::PerformanceTracker::record_frame`
as one `LatestFrameMetrics`, which includes:

- stage microseconds: input, producer, composition, render, sample, push,
  postprocess, publish, overhead, total
- pacing: wake_late_us, jitter_us
- reuse flags: reused_inputs, reused_canvas, retained_effect, retained_screen,
  composition_bypassed
- composition structure: logical_layer_count, render_group_count, scene_active,
  scene_transition_active
- render surface pool state: slot_count, free_slots, published_slots,
  dequeued_slots, canvas_receiver_count
- copies: full_frame_copy_count, full_frame_copy_bytes
- output_errors
- a `FrameTimeline` with wake-to-publish absolute checkpoints
  (`scene_snapshot_done_us`, `input_done_us`, `producer_done_us`,
  `composition_done_us`, `sample_done_us`, `output_done_us`,
  `publish_done_us`, `frame_done_us`)

The tracker keeps a rolling 120-frame history (`FRAME_HISTORY_CAPACITY`)
for frame-time, jitter, and wake-delay samples plus a reuse history for
aggregate pacing counts.

### 3.11 Deadline advance

At the end of `execute_frame` the admission controller gets to vote.
`FrameAdmissionController::record_frame` consumes a `FrameAdmissionSample`
(total_us, producer_us, composition_us, full_frame_copy_count) and returns
a `FrameAdmissionDecision` carrying the current `ceiling_tier`. That
ceiling is then passed to `RenderLoop::frame_complete_with_max_tier`,
which clamps the adaptive FPS controller's ceiling in one atomic step.

`frame_complete_with_max_tier` returns a `FrameStats` if the frame was a
real tick (not paused/stopped). `SkipDecision::from_frame_stats` turns
that into the next tick's skip decision: `ReuseInputs` after a single
budget miss, `ReuseCanvas` after two consecutive misses, `None` otherwise.
`advance_deadline` projects the next wake time by adding the current
target interval to the previous scheduled start, clamping to `now` so
we never target a time in the past.

### 3.12 Full-tier admission gate

`FrameAdmissionController` lives in `frame_admission.rs` and is what Wave 5
of the design doc promised. It implements symmetric hysteresis around the
60 FPS tier with both fast revocation and slow readmission. The controller
tracks three EWMAs (`α = 0.2`) — total, producer, composition — plus a
rolling 60-frame window of total-frame microseconds for percentile math.

**Revoke thresholds** (any trigger drops the ceiling from `Full` to `High`):

| Signal                         | Threshold                  |
| ------------------------------ | -------------------------- |
| Consecutive copy frames        | ≥ 2                        |
| Consecutive over-budget frames | ≥ 2                        |
| EWMA total                     | > 92 % of full-tier budget |
| EWMA producer                  | > 70 % of full-tier budget |
| EWMA composition               | > 25 % of full-tier budget |
| p95 total                      | > 95 % of full-tier budget |
| p99 total                      | > full-tier budget         |

Percentile-based triggers require at least 10 samples in the rolling
window before they activate, so the controller can't yank the ceiling
during the first few frames after startup.

**Readmit thresholds** (all must hold, for 30 consecutive frames, with at
least 30 samples in the rolling window):

| Signal                         | Threshold                  |
| ------------------------------ | -------------------------- |
| Consecutive copy frames        | 0                          |
| Consecutive over-budget frames | 0                          |
| EWMA total                     | ≤ 80 % of full-tier budget |
| EWMA producer                  | ≤ 60 % of full-tier budget |
| EWMA composition               | ≤ 20 % of full-tier budget |
| p95 total                      | ≤ 85 % of full-tier budget |
| p99 total                      | ≤ full-tier budget         |

A configured ceiling below `FpsTier::Full` is preserved unchanged — the
controller only ever tightens, never exceeds, the user's request. The
controller's output feeds `FpsController::set_max_tier` once per frame;
the existing `RenderLoop::frame_complete` adaptive machinery still drives
the regular tier shifts underneath that ceiling.

## 4. Data flow

```
                    ┌──────────────────────────────────────────────┐
                    │ pipeline_driver::run_pipeline loop           │
                    └───────────────────┬──────────────────────────┘
                                        │
                            frame_executor::execute_frame
                                        │
          ┌─────────────────────────────┼────────────────────────────┐
          │                             │                            │
render_scene_state    build_frame_scene   maybe_sleep_throttle      sample_inputs
 .apply_transactions     _snapshot           maybe_idle_throttle         │
          │                             │                            │
          └──────────► FrameSceneSnapshot ◄────────────────────────────┤
                                        │                            │
                            compose_frame (frame_composer.rs)         │
                                        │                            │
            ┌───────────────────────────┴──────────────┐              │
            │                                          │              │
  single-effect path                        render-group path         │
            │                                          │              │
   latch effect_queue OR                   RenderGroupRuntime          │
   render new effect                       .render_scene or            │
   (render_surface_pool)                   .reuse_scene (cached)       │
            │                                         │               │
            │                                         ▼               │
            │                             RenderGroupResult           │
            │                             (layout + preview frame)    │
            └────────► ProducerFrame ◄────────────────┘               │
                              │                                       │
                 CompositionPlanner::compile_primary_frame            │
                              │                                       │
                        CompositionPlan                                │
                              │                                       │
                   SparkleFlinger::compose                             │
                              │                                       │
                     ComposedFrameSet                                  │
                              │                                       │
       ┌──────────────────────┼─────────────────────┐                 │
       │                      │                     │                 │
 SpatialEngine          BackendManager        event_bus publish       │
  .sample_into          .write_frame          (frame, canvas,         │
  (or reuse             _with_brightness       spectrum, screen)      │
  group zones)                │                     │                 │
                              │              PreviewRuntime.record    │
                              │              FrameRendered event      │
                              │                     │                 │
                              └────────────► FrameTiming / metrics ◄──┤
                                                 │                    │
                                                 ▼                    │
                                  FrameAdmissionController.record     │
                                          │                           │
                                          ▼                           │
                           frame_complete_with_max_tier (ceiling) ────┘
```

## 5. Key invariants

**One frame, one world.** Scene transactions are drained exactly once at the
top of `execute_frame` into `RenderSceneState`. Every downstream stage reads
from the same `FrameSceneSnapshot` until the next tick. The render thread
never reads from the shared `state.spatial_engine` after startup — it owns
its own `SpatialEngine` clone in `RenderSceneState`, which the
`SceneTransactionQueue` syncs on a frame boundary. API handlers writing the
shared engine must also push a `SceneTransaction` (see `apply_layout_update`)
or the render thread will keep running against stale layout data.

**Producers never mutate published surfaces.** `PublishedSurface` (from Spec 36) is immutable post-submit. `render_surface_pool` recycles slots through
the watch channel so long as there are no extra references outstanding.
Under retention pressure the pool grows from `DEFAULT_RENDER_SURFACE_SLOTS`
(8) up to `MAX_RENDER_SURFACE_SLOTS` (12), biased toward
`desired_render_surface_slots(canvas_receiver_count)`, and then falls back
to owned-canvas publishes that cost a full-frame copy.
`RenderGroupRuntime` maintains a separate `preview_surface_pool` for the
group preview compose stage.

**Composition is cheap to skip.** A single-layer `Replace` plan with
`opacity >= 1.0` takes the bypass fast path in `SparkleFlinger::compose`
and passes the source frame's surface straight through as both
`sampling_surface` and `preview_surface`, with no blending work. The layer
is consumed via `Vec::pop` so there is no clone on the happy path.
`composition_bypassed` is reported in telemetry so the bypass rate is
observable.

**Retention is explicit.** `ProducerQueue` carries `ProducerGeneration`
explicitly — `Latest` (screen path, untagged) or `Tagged(u64)` (effect
path, tied to `effect_generation`). `latch_for_generation` only matches a
tagged submission; a generation mismatch invalidates the queue.
`ProducerFrameState::Fresh` is returned the first time a submission is
latched and `Retained` on subsequent latches. `RenderGroupRuntime` has its
own retention path (`reuse_scene`) that keeps the full `RenderGroupResult`
plus a pointer to the already-published `FrameData::zones`, so reuse
doesn't have to re-sample or re-compose.

**Full tier is admission-gated.** `FrameAdmissionController` runs every
frame and can revoke the `Full` ceiling on sustained copy pressure, budget
misses, or percentile overruns. Once revoked the controller requires 30
consecutive clean frames with ≥ 30 samples in the rolling window before
readmitting `Full`. The ceiling is passed into
`RenderLoop::frame_complete_with_max_tier` in the same atomic step as
adaptive tier shifting, so observers always see a consistent `(tier,
max_tier)` pair.

**LED delivery is the hard deadline.** Preview publication happens after
spatial sampling and backend writes in the happy path, and uses `watch`
channels with latest-value semantics so a slow preview consumer never
backpressures the render thread. `PreviewRuntime` wraps the subscription
handles so receiver counts and publication totals are visible without
touching the watch channel internals.

**Work is amortised when nothing changed.** Idle scene-manager reads use
the read lock only; cached `active_render_groups` are an `Arc<[RenderGroup]>`
so `SceneRuntimeSnapshot` clones are pointer-cheap; effect demand is
memoised against `active_render_groups_revision`; the static black/held
surface is cached by `(width, height, color)`; the render-group reconcile
is gated by `reconciled_groups_revision`; the zones buffer is recycled
through the frame watch channel.

**`unsafe_code` is forbidden here.** SparkleFlinger stays in the safe application
surface; the only workspace opt-outs are audited platform interop crates.

## 6. Telemetry surfaces

Every frame records a `LatestFrameMetrics` and a `FrameTimeline` to the
`PerformanceTracker`. The tracker keeps a rolling 120-frame history
(`FRAME_HISTORY_CAPACITY`) for frame-time, jitter, and wake-delay samples,
plus a reuse history for aggregate counts.

Three surfaces read from the tracker:

**REST `GET /api/v1/status`** — includes a `latest_frame` object with frame
token, total ms, wake late ms, frame age ms, logical layer count, render
group count, copy stats, and a `render_surfaces` sub-object with slot state

- canvas receiver count. The `render_loop` object reports `target_fps`,
  `ceiling_fps` (from the admission gate), `actual_fps`, `consecutive_misses`,
  `total_frames`, `fps_tier`, and `state`. A dedicated `preview_runtime`
  sub-object exposes `canvas_receivers`, `screen_canvas_receivers`,
  `canvas_frames_published`, `screen_canvas_frames_published`, and the
  latest canvas/screen frame numbers.

**WebSocket metrics (`MetricsPayload`)** — same data, richer. Top-level
groups are `fps` (target, ceiling, actual, dropped), `frame_time`
(avg/p95/p99/max), `stages` (input_sampling, producer_rendering,
composition, effect_rendering, spatial_sampling, device_output,
preview_postprocess, event_bus, coordination_overhead), `pacing`
(jitter/wake-delay summaries, frame_age_ms, reuse counts),
`timeline` (frame token, budget, layer/group counts, scene flags, all the
timeline checkpoints), `render_surfaces` (slot state), `preview`
(`canvas_receivers`, `screen_canvas_receivers`, `canvas_frames_published`,
`screen_canvas_frames_published`, latest canvas/screen frame numbers),
`copies` (full-frame count + KB), `memory` (daemon_rss_mb, servo_rss_mb,
canvas_buffer_kb), `devices`, and `websocket`
(client_count, bytes_sent_per_sec, frame/canvas payload build and cache-hit
counters). Emitted at the configured metrics cadence on the WebSocket
metrics channel.

**CLI `hypercolor status`** — renders a compact SilkCircuit table with a Render
line showing the actual/target FPS ratio as a coloured bar, then a
second-level line with tier, ceiling, miss count, and total frames. The
Frame line reports total/wake/age, the Surfaces line reports slot state
plus copy counts, and a dedicated Preview line reports canvas and screen
canvas receiver counts and published frame totals.

## 7. What is not here (and why)

**Wave 5 admission policy — shipped.** `FrameAdmissionController` in
`frame_admission.rs` is the gate the design doc called for. It consults
`LatestFrameMetrics::composition_us`, producer, and total timings plus the
rolling-window p95/p99 of `total_us` before it will honour the `Full`
tier. The copy-pressure trigger feeds directly from
`publish_stats.full_frame_copy_count`, so a slow consumer chaining the
render thread into owned-canvas publishes can never keep the full tier.

**Wave 6 daemon preview runtime — partial.** `preview_runtime.rs`
introduces the `PreviewRuntime` seam: it wraps the event-bus canvas and
screen-canvas watches, maintains atomic receiver counters so the render
thread can size `render_surface_pool` without touching the bus internals,
records published frame counts and latest frame numbers, and is the thing
the API/WS layers snapshot for telemetry. What is still ahead is the
formal presentation-resolution boundary described in Sections 6.1 and 9.8
of the design doc — the runtime still publishes raw canvas bytes to the
same `event_bus.canvas_sender()` path, no offscreen-canvas translation,
no dedicated presenter cadence.

**Wave 7 encoded preview — not yet.** Still raw WebSocket canvas frames.
The design doc calls for WebRTC with H.264 + VP8 fallback after Wave 6 is
stable.

**Wave 8 GPU compositor — intentional.** CPU composition at 320×200 is
cheap enough (see the `single_replace_bypass` and `alpha_two_layer_compose`
benches) that the design doc explicitly positions GPU composition as a
post-CPU optimization.

**Minimal producer state model.** `ProducerQueue` carries `Fresh` /
`Retained` plus an explicit `ProducerGeneration` (`Latest` or `Tagged(u64)`).
The full Section 6.6 taxonomy (`static`, `stale`, `paused`, `failed`,
cadence class, stale budget) is not yet implemented. This is fine while
there is effectively one effect producer and one screen producer, but
Servo as an independently cadenced producer will need the full model.

**`ComposedFrameSet::preview_surface` is populated.** It was initially
always `None`; today the bypass path fills it with the source
`PublishedSurface`, and the publish stage in `execute_frame` uses it as
the first-choice source for the dedicated screen canvas watch channel,
falling back to `sampling_surface` and then `inputs.screen_preview_surface`.
A dedicated presentation resolution separate from the sampling canvas is
still Wave 6 work.

## 8. Benchmarks

`crates/hypercolor-daemon/benches/render_pipeline.rs` has three groups:

- `daemon_render_pipeline` — end-to-end pipeline at 60 FPS with 3 mock
  devices × 120 LEDs. Two scenarios: active effect with shared publish, and
  screen passthrough with a shared surface.
- `daemon_publish_handoff` — isolates `CanvasFrame` publish cost with owned
  canvas vs slot-backed surface.
- `daemon_sparkleflinger` — isolates composition cost and CPU/GPU comparisons:
  bypass, two-layer alpha transitions, multi-layer alpha/add/screen face
  composition, preview scaling, LED zone sampling, and end-to-end
  compose-plus-sampling paths.

Run with `just bench-daemon daemon_sparkleflinger`,
`just bench-daemon daemon_render_pipeline`, or
`cargo bench -p hypercolor-daemon --bench render_pipeline`. See
`docs/development/RENDER_PIPELINE_BENCHMARKS.md` for the CPU/GPU decision
workflow.

## 9. Implementation references

The design doc (`29-sparkleflinger-60fps-evolution.md`) is the intent. This
document is the map. For per-stage details, read in this order:

1. `pipeline_driver.rs` — the loop shape
2. `frame_executor.rs` — the lifecycle in one function
3. `frame_composer.rs` — where SparkleFlinger enters the frame
4. `sparkleflinger.rs` — the compositor itself
5. `composition_planner.rs` — transitions and plan compilation
6. `producer_queue.rs` — the explicit generation model
7. `render_groups.rs` — the only real multi-producer feature today
8. `frame_admission.rs` — the Full-tier gate
9. `preview_runtime.rs` — the preview seam and telemetry surface
10. `scene_transactions.rs` — the frame-boundary transaction queue

`render_thread.rs` is small and mostly ceremony — start with
`pipeline_driver.rs` instead.
