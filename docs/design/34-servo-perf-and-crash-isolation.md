# Servo perf & crash isolation plan

Comprehensive audit and plan for Servo HTML-effect rendering. Covers what "30fps cap"
actually means, which optimizations are still on the table ranked by payoff-to-effort,
and the honest crash-isolation story with a path to robustness.

## What "30fps cap" actually means (two things, not one)

Looking at `crates/hypercolor-core/src/effect/servo/renderer.rs:30-31`:

- **`DEFAULT_EFFECT_FPS_CAP: u32 = 30`** — bootstrap default written to
  `window.__hypercolorFpsCap` before the first frame arrives. The SDK's RAF helper
  reads this to gate its animation loop. It's a *pre-frame placeholder*; on the first
  real tick, `enqueue_frame_scripts` overwrites it with
  `FpsTier::from_fps((1.0 / delta_secs).round())`, clamped to
  `MAX_EFFECT_FPS_CAP = 60`. So non-Display effects dynamically track the render
  loop's current tier (10/20/30/45/60).

- **`DEFAULT_DISPLAY_FPS_CAP: u32 = 30`** — hard ceiling for `EffectCategory::Display`
  effects. Per `hypercolor-types/src/effect.rs:80`, Display is "full-fidelity HTML
  display faces for LCD surfaces" — the Corsair LCD, Lian Li Galahad, that class.
  These are permanently pinned via `AnimationCadence::Fixed(30)` at `renderer.rs:579`,
  which also gates `render_due()` so we skip JS eval + readback entirely on frames
  between intervals.

**Why this design:** LCD faces tend to be heavy (clocks, dashboards, gauge layouts)
and run on small surfaces whose native refresh doesn't benefit from >30fps. Pinning
them prevents one Display face from budget-missing and triggering a render-loop
downshift that would penalize every *other* device. It's a QoS separation between
"LED animation effects" and "LCD display chrome."

## Resolution contract (non-negotiable)

Servo should always render at the canvas size configured for the active render
group or display surface. That is already the engine contract:

- `EffectPool::render_group_into` prepares the target canvas at the group's
  configured width and height before handing control to the renderer.
- `EffectSlot::build` initializes each renderer with that same configured size.
- `ServoRenderer::render_into` only reuses completed canvases when their
  dimensions match the frame input exactly.

So a Servo-only "render smaller, then upscale later" path is the wrong shape for
this system. It would add a second presentation contract, duplicate resampling
logic, and make HTML effects special in a way the rest of the render pipeline is
not.

If we want cheaper HTML rendering, the right lever is to lower the configured
canvas size at the source:

- for LED groups, via the render-group or scene canvas dimensions that feed the
  effect pool
- for display faces, via the display surface or layout configuration assigned to
  that group

We should not add per-effect native resolution metadata or a Servo-specific
upscale path.

## Already harvested (don't redo)

- **Skip GPU readback when no new composition** (commit `d549c8e7`). Delegate signals
  `frame_ready`; if false, reuse cached canvas. Biggest single memmove win in profile.
- **Read framebuffer directly and flip rows in place** (commit `aa48aeac`). Bypasses
  Servo's `read_framebuffer_to_image()` which did `glReadPixels` + `Vec::clone` +
  per-row flip. Each byte now moves at most once. ~50% reduction in readback cost.
- **Servo prefs hardened** (`worker.rs:587-629`). JIT disabled, threadpools pinned to 1,
  unused subsystems disabled (devtools, gamepad, IndexedDB, WebRTC, WebXR, WebGPU,
  serviceworkers, worklets). ~20% startup speedup, ~10 MB lower memory footprint.
- **Global jemalloc allocator** via `servo-allocator` on Linux.
- **Per-effect FPS cap throttling** (`renderer.rs:35-59`, `AnimationCadence`). Display
  effects run fixed at 30fps; others match daemon tick rate with early-exit on
  non-due frames.

## The plan — ranked by payoff-to-effort

### Tier 1: Do these next

#### 1. Amortize sensor & audio scripts when data is stale *(easy, medium impact)*

`push_frame_scripts` (`lightscript.rs:353`) pushes `sensor_update_script`
unconditionally, and `audio_update_script` whenever `include_audio_updates` is true.
Control updates are already diffed — extend the same pattern:

- Track the last sensor snapshot or last serialized sensor payload in
  `LightscriptRuntime`; skip the script when the effective payload has not changed.
  Sensors typically update at 1–2 Hz, so at 60fps we'd skip ~58 of 60 sensor scripts.
- For audio, send the zeroed payload once when entering a quiet state, then suppress
  repeated "still silent" updates until audio becomes non-quiet again. This avoids
  emitting the same typed-array assignments on every idle frame while keeping the
  runtime semantics correct.

**Win:** 20–40% reduction in per-frame JS bytes for idle scenes. Directly shrinks the
250ms `SCRIPT_TIMEOUT` budget pressure.

**Risk:** Low. The bootstrap already initializes these to zeros/defaults, so skipping
updates is semantically safe.

#### 2. Instrument total JS/render stage cost before deeper surgery *(easy, high value)*

Before changing worker architecture or script transport shape, add measurements for
the stages we actually control:

- total `evaluate_scripts` wall time
- one Servo event-loop spin after script injection
- `paint()`
- framebuffer readback in `read_framebuffer_into_canvas`

The current embedder API gives us a single completion callback for JavaScript
evaluation, so we should measure end-to-end eval cost, not promise a clean
parse-versus-execute split we do not currently have.

**Win:** Turns "JS might be expensive" into real numbers and tells us whether the next
optimization should target script generation, event-loop behavior, or readback.

**Risk:** Low. Pure instrumentation and logging/metrics plumbing.

### Tier 2: Investigate, then commit

#### 3. Tighten per-tick stall handling around in-flight renders *(medium, high stability impact)*

`SCRIPT_TIMEOUT` is 250ms and `RENDER_RESPONSE_TIMEOUT` is 500ms. A runaway effect
can still leave the main renderer reusing stale frames for far too long.

- Derive a soft stall threshold from the active FPS tier and compare it against
  `pending_render_age()` in the poll path.
- When the soft threshold is exceeded, keep reusing `last_canvas`, increment per-effect
  stall telemetry, and surface the condition in logs/metrics without pretending we
  canceled the worker-side render.
- Keep the existing hard timeouts and fatal-path handling for truly wedged sessions.
  If a render crosses that line, let the existing worker/session teardown semantics
  take over.

**Win:** We stop conflating "the renderer is temporarily late" with "we successfully
killed the work," and we get cleaner degradation without lying to ourselves about
cancellation semantics.

**Risk:** Medium. The logic needs to cooperate cleanly with the current circuit breaker
and session lifecycle.

#### 4. Automatic fallback and degraded-mode UX *(medium, medium UX impact)*

Today when the breaker opens, the active HTML effect just goes dark and the user has
to intervene. We should improve that, but this is a daemon orchestration change, not a
tiny renderer tweak.

- Add an explicit fallback policy in config instead of hardcoding behavior.
- Route degraded-mode recovery through a shared daemon helper that can safely mutate
  either the LED primary effect path or the display-face assignment path.
- Publish an existing `EffectError`-style signal or a purpose-built degraded event so
  the UI can show a toast like "HTML effect crashed; retrying in Xs."

**Win:** Graceful recovery instead of silent darkness.

**Risk:** Medium. Effect application is split across explicit API and MCP paths today,
and display faces already live on a separate lane from LED effects.

### Tier 3: Bigger architectural lifts

#### 5. Subprocess isolation for Servo *(hard, transformative)*

The honest fix for crash isolation *and* the path to parallel HTML effect rendering.
Spawn Servo in a child process, IPC via shared memory for the framebuffer (shmem +
semaphore) and Unix domain sockets for script commands. On child crash, respawn.

- **Shape:** New binary `hypercolor-servo-worker`; daemon talks to it via a new
  module replacing the current in-process `ServoWorker`. The public `ServoRenderer`
  API stays identical.
- **Wins:** Hard crashes no longer touch daemon. OOM contained. Could run multiple
  workers for concurrent effects (e.g., one per LCD face).
- **Cost:** Multi-week project. Build system, packaging, IPC protocol design,
  lifecycle coordination, tests.
- **Prerequisite:** Should probably wait until Tier 1+2 ship and we have real data on
  whether HTML-effect crashes are frequent enough to justify.

#### 6. Async PBO readback *(skip if software-rendering)*

The `bootstrap_software_rendering_context` call at `worker.rs:39` strongly suggests
we're on OSMesa (software GL). In software mode, `glReadPixels` is a CPU memcpy — no
GPU sync to hide, no PBO benefit. **Before investing here, confirm the rendering
backend.** If hardware GL lands later (e.g., headless EGL on systems with GPUs),
revisit: post readback with a fence at end of frame N-1, harvest at start of N, hide
~1 frame of sync latency.

## Crash isolation story (current state)

**Honest assessment: partially robust; daemon survives most crashes, but OOM can
still take the whole process down.**

- **In-process architecture.** Servo runs on a dedicated OS thread
  (`ServoWorker::spawn()` at `worker.rs:1396`), same address space as the daemon.
- **No `catch_unwind`** anywhere in the `effect/` tree (verified by grep). But Rust
  thread panics don't crash the parent process: the `mpsc` channel disconnects,
  `servo_worker_is_fatal_error` (`worker.rs:110`) catches it, and
  `ServoCircuitBreaker` opens for 30s (exponential backoff to 300s).
- **What survives gracefully:** single JS errors, `while(1){}` loops (250ms
  `SCRIPT_TIMEOUT` / 500ms `RENDER_RESPONSE_TIMEOUT`), WebGL context loss,
  Servo-internal panics on that thread. Daemon keeps running; HTML effects go dark
  until the breaker closes.
- **What still takes the daemon down:** OOM. An effect doing `new Uint8Array(1e9)` in
  a loop allocates into the shared address space with no cap. Same for Servo-internal
  unbounded growth.
- **Also soft:** no auto-fallback to a safer effect when the breaker opens. See
  Tier 2 item #4.

## Suggested sequencing

Land Tier 1 together in a single PR: stale payload suppression plus timing
instrumentation are both low-risk and give us better data fast. Then tackle Tier 2
with real numbers in hand, starting with stall handling before fallback UX.

Keep the resolution contract out of this track. If configured canvas sizes are wrong
for a given LED or display workload, fix that at the render-group or display-surface
configuration layer rather than teaching Servo to render at one size and present at
another.

Hold Tier 3 subprocess isolation for a dedicated design doc since the IPC protocol,
packaging story, and worker lifecycle deserve careful thought.
