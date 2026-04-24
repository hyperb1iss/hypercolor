# Graphics Pipeline Unification Plan

**Status:** Draft plan
**Date:** 2026-04-24
**Scope:** Servo HTML rendering, render-thread composition, display faces, display output, LED output, preview streaming, timing, backpressure, concurrency, and zero-copy behavior.

## 1. Goal

Hypercolor already has the right architectural bones: render groups, immutable published surfaces, latest-value queues, adaptive FPS, display workers, and staged device output. The goal of this plan is not to replace the current architecture. The goal is to make the whole graphics pipeline feel like one deliberate machine:

1. Render-thread work is frame-bounded and never stalls on Servo lifecycle operations.
2. Servo effects and display faces share a coherent scheduling story.
3. Scene composition, display composition, preview, and hardware output use one explainable surface contract.
4. Backpressure is latest-value, bounded, observable, and recoverable.
5. Steady-state canonical surfaces are shared by handle, not copied.
6. GPU acceleration is honest: either active when available, or clearly reported as unavailable.
7. Color and brightness semantics are intentional across LEDs, previews, and LCD displays.

## 2. Success Criteria

- Render thread never blocks on Servo load, unload, destroy, page load, or page teardown.
- Servo page load or destroy timeouts do not produce multi-second frame spikes.
- `Auto` GPU mode either uses GPU when available or is renamed so it does not overpromise.
- Display output retries unchanged frames after transient backend write failures.
- LED output is not starved by bulky LCD display writes on devices that expose both.
- Surface-backed scene canvas publication has zero full-frame copies in steady state.
- WebSocket, display, LED, and Servo backpressure remain bounded during stress scenes.
- Mixed Servo LED effects, Servo display faces, scene transitions, display output, lighting output, and preview streaming survive a 30 minute soak.
- `just verify`, focused Servo/display/LED tests, and render benchmarks pass.

## 3. Current Strengths To Preserve

- The render thread already runs on its own OS thread and Tokio runtime.
- `PublishedSurface` and `RenderSurfacePool` give the pipeline a strong immutable shared-surface model.
- The event bus uses `watch` for high-frequency latest-value streams and `broadcast` for discrete events.
- WebSocket relays use bounded queues and drop under pressure rather than growing memory.
- Servo rendering already coalesces queued frame input and reuses the last completed canvas when a frame is late.
- Display workers isolate JPEG encoding with `spawn_blocking` and keep per-worker reusable buffers.
- LED output queues use latest-value watch channels with per-device send cadence and dropped-frame metrics.
- Adaptive FPS and frame admission already react to budget misses, full-frame copy pressure, late readbacks, and output errors.

## 4. Biggest Risks To Fix

1. Servo lifecycle operations can block the render thread for hundreds of milliseconds to seconds.
2. All Servo sessions share one FIFO worker thread, so one heavy HTML effect or face can starve others.
3. Display output can suppress retry of an unchanged frame after a transient write failure.
4. USB display writes and LED writes share one biased actor, so large LCD frames can delay lighting.
5. Multi-group scene composition still has a CPU per-zone/per-pixel raster path that is not the same story as SparkleFlinger layer composition.
6. The pipeline is not end-to-end zero-copy. It is surface-handle-oriented in the middle, but copies remain at Servo readback, display encoding, USB LED handoff, and some publication edges.
7. GPU acceleration is partially present but not exposed honestly by default behavior.

## 5. Execution Strategy

Do not try to land this as one heroic refactor. Ship in waves that each leave the pipeline better, measurable, and reversible.

Recommended order:

1. Wave 0: Baseline and telemetry.
2. Wave 1: Remove render-thread lifecycle stalls.
3. Wave 3: Fix output backpressure and retry correctness.
4. Wave 2: Improve Servo scheduling and face quality-of-service.
5. Wave 4: Make GPU acceleration honest.
6. Wave 5: Harden zero-copy and copy budgets.
7. Wave 6: Unify composition semantics.
8. Wave 7: Align color and quality semantics.
9. Wave 8: Soak, verify, and update docs.

Waves 2, 4, and 5 can partially overlap after Wave 0, but Waves 1 and 3 should lead because they remove the highest operational risk.

## 6. Wave 0: Baseline And Telemetry

### Task 0.1: Create Deterministic Stress Scenes

**Files:** `benches/`, `crates/hypercolor-daemon/benches/`, test fixtures, optional `docs/perf/`.

Implementation:

- Add repeatable scenarios for a single Servo LED effect, multiple Servo display faces, screen-reactive effects, multi-group scene composition, display face blending, and mixed LCD plus LED hardware.
- Make each scenario emit the same metric names so before/after comparisons are easy.
- Include at least one scene that stresses every output path at once: Servo render, scene canvas, group canvas, display output, LED output, WebSocket preview, and metrics.

Verify:

- Each scenario can run repeatedly without manual setup beyond daemon configuration.
- Metrics include frame p50/p95/p99, copy bytes, Servo queue age, output queue wait, dropped frames, and surface pool saturation.
- Re-running the same scenario produces comparable numbers.

### Task 0.2: Add Missing Pipeline Metrics

**Files:** `crates/hypercolor-daemon/src/render_thread/frame_executor.rs`, `crates/hypercolor-daemon/src/performance/`, `crates/hypercolor-daemon/src/api/ws/protocol.rs`, `crates/hypercolor-core/src/effect/servo/`, `crates/hypercolor-core/src/device/manager.rs`, `crates/hypercolor-core/src/device/usb_backend.rs`.

Implementation:

- Track Servo lifecycle wait time, worker queue depth, per-session pending render age, and soft stall counts.
- Track display write retry count, last write failure age, and worker-local retry state.
- Track USB LED/display lane wait and whether a display write delayed an LED write.
- Track full-frame copy bytes, `PublishedSurface::from_canvas` hot-path usage, and surface pool saturation.
- Track GPU sampling stalls, deferred sampling queue saturation, and late readbacks.

Verify:

- Metrics appear in `/api/v1/ws` metrics output.
- Metrics are zero or near-zero in idle scenes.
- Metrics become nonzero under targeted synthetic pressure.

### Task 0.3: Write The Canonical Pipeline Contract

**Files:** `docs/specs/48-canonical-render-pipeline.md` or a companion design document.

Implementation:

- Define the exact phases: input sample, producer render, scene composition, LED sampling, device queue push, bus publish, preview encode, display encode, transport write.
- Define which phase may allocate, copy, block, or await.
- Define where brightness, color polish, JPEG encoding, and USB packetization are allowed to happen.
- Define when retained frames may be reused.

Verify:

- Every later task can point to the contract it is enforcing.
- The contract explains Servo effects, display faces, direct group canvases, canonical scene canvas, LED output, and WebSocket preview in one pass.

## 7. Wave 1: Remove Render-Thread Lifecycle Stalls

### Task 1.1: Add Explicit Effect Slot Lifecycle States

**Files:** `crates/hypercolor-core/src/effect/pool.rs`, possibly `crates/hypercolor-core/src/effect/traits.rs`.

Implementation:

- Replace the implicit "renderer exists and is ready" assumption with explicit states: `Loading`, `Ready`, `Failed`, and `Retiring`.
- Let the render thread produce a placeholder or retained frame while an HTML effect is loading.
- Preserve existing native renderer behavior as the fast ready path.

Verify:

- Applying an HTML effect no longer blocks frame execution while the page loads.
- A loading effect produces a clear degraded state instead of freezing the pipeline.
- Existing native effects behave unchanged.

### Task 1.2: Move Servo Session Creation And Page Load Off The Render Thread

**Files:** `crates/hypercolor-core/src/effect/servo/renderer.rs`, `crates/hypercolor-core/src/effect/servo/worker_client.rs`, `crates/hypercolor-core/src/effect/servo/session.rs`, `crates/hypercolor-core/src/effect/pool.rs`.

Implementation:

- Start Servo session creation and page load asynchronously.
- Swap the completed session into the effect slot only at a frame boundary.
- Keep all page load timeout handling out of frame execution.
- Preserve the existing `ServoRenderer` trait surface if possible.

Verify:

- A forced Servo page-load timeout does not produce a multi-second render frame.
- Metrics show page load wait time under Servo lifecycle metrics, not frame render time.
- Switching between two HTML effects keeps the render loop alive and publishing.

### Task 1.3: Make Servo Destroy Nonblocking From The Hot Path

**Files:** `crates/hypercolor-core/src/effect/servo/renderer.rs`, `crates/hypercolor-core/src/effect/servo/session.rs`, `crates/hypercolor-core/src/effect/servo/worker_client.rs`, `crates/hypercolor-core/src/effect/pool.rs`.

Implementation:

- Move session close and in-flight render drain to a retirement path outside frame execution.
- Cap render-thread teardown work to cheap state transition only.
- Record late or failed teardown through Servo lifecycle metrics.

Verify:

- Dropping an active HTML effect cannot add a 500ms to 8s frame spike.
- Destroy timeouts are visible in metrics and logs.
- Retired sessions do not leak indefinitely.

### Task 1.4: Publish Degraded-Mode Events

**Files:** `crates/hypercolor-types/src/event.rs`, `crates/hypercolor-core/src/effect/servo/telemetry.rs`, `crates/hypercolor-daemon/src/render_thread/frame_composer.rs`, API protocol files.

Implementation:

- Emit events for `Loading`, `Late`, `Failed`, `Retiring`, and `Recovered` effect states.
- Include effect ID, render group ID, session ID if available, and a concise reason.
- Let UI distinguish loading from failure from retained-frame fallback.

Verify:

- UI/API can observe state transitions during a synthetic Servo stall.
- Event volume is bounded and deduped.

## 8. Wave 2: Servo Scheduling And Face QoS

### Task 2.1: Replace FIFO-Only Servo Command Handling With Per-Session Scheduling

**Files:** `crates/hypercolor-core/src/effect/servo/worker.rs`, `crates/hypercolor-core/src/effect/servo/worker_client.rs`.

Implementation:

- Introduce per-session pending work records.
- Coalesce redundant render requests for each session.
- Schedule sessions by deadline and fairness rather than raw FIFO.
- Preserve synchronous lifecycle response channels only for non-render commands that truly need them.

Verify:

- One heavy display face cannot starve another HTML effect indefinitely.
- Per-session render age stays bounded under a mixed face/effect stress scene.
- Render requests remain latest-value, not unbounded queued work.

### Task 2.2: Make Servo Failure Per-Session When Possible

**Files:** `crates/hypercolor-core/src/effect/servo/worker.rs`, `crates/hypercolor-core/src/effect/servo/renderer.rs`, `crates/hypercolor-core/src/effect/servo/telemetry.rs`.

Implementation:

- Treat JavaScript errors, page failures, and per-session render failures as session-local when the worker remains healthy.
- Keep global worker poisoning for true worker disconnect, unrecoverable runtime state, or shared Servo failure.
- Make the circuit breaker reason specific and visible.

Verify:

- A broken face can fail while another Servo-backed render group continues.
- True worker failure still disables all Servo rendering safely.

### Task 2.3: Suppress Stale Sensor And Audio Script Injection

**Files:** `crates/hypercolor-core/src/effect/servo/lightscript.rs`, `crates/hypercolor-core/src/effect/servo/renderer.rs`.

Implementation:

- Track last serialized sensor payload and skip identical updates.
- Send a quiet audio payload once, then suppress repeated silent updates until audio becomes active again.
- Keep controls diffed as they are today.

Verify:

- JS bytes per frame drop in idle scenes.
- Sensor changes and audio reactivation still reach HTML effects on the correct frame.

### Task 2.4: Write The Servo Subprocess Design

**Files:** `docs/design/46-servo-subprocess-isolation.md` or similar.

Implementation:

- Define a future `hypercolor-servo-worker` process.
- Specify shared-memory framebuffer handoff, command IPC, lifecycle, crash recovery, packaging, and metrics.
- Keep this as design only unless explicitly greenlit.

Verify:

- The design explains how subprocess isolation would preserve the current `EffectRenderer` API.
- The design includes migration risks and rollout waves.

## 9. Wave 3: Output Backpressure And Retry Correctness

### Task 3.1: Fix Display Write Retry Semantics

**Files:** `crates/hypercolor-daemon/src/display_output/mod.rs`, `crates/hypercolor-daemon/src/display_output/worker.rs`.

Implementation:

- Ensure a transient write failure does not permanently suppress an unchanged frame.
- Choose one of two models:
  - Parent records dispatch identity only after worker success.
  - Worker owns retry of its latest encoded frame until success, replacement, or shutdown.
- Track retry attempts and last failure age.

Verify:

- Force a display write failure, recover backend, and confirm the unchanged frame is resent.
- Retry behavior is bounded and does not spin.
- Display preview publication remains correct during failure.

### Task 3.2: Split USB LED And Display Service Policy

**Files:** `crates/hypercolor-core/src/device/usb_backend.rs`.

Implementation:

- Keep one transport actor if protocol ordering requires it, but separate pending LED and display lanes.
- Give overdue LED frames priority over bulky display JPEG writes.
- Consider chunking or rate-limiting display writes when LED output is active on the same device.
- Preserve command and keepalive correctness.

Verify:

- LCD updates do not push LED queue wait past two LED frame intervals.
- Device keepalive remains reliable.
- Display updates continue at their capped FPS.

### Task 3.3: Shorten BackendManager Hot-Path Locking

**Files:** `crates/hypercolor-core/src/device/manager.rs`, `crates/hypercolor-daemon/src/render_thread/frame_executor.rs`.

Implementation:

- Keep route planning and queue lookup cheap inside the manager lock.
- Ensure awaited backend I/O happens through output queues or `BackendIo`, not while holding unrelated manager state.
- Remove unusual lock priming if metrics show it is not buying enough.

Verify:

- Push-stage jitter drops under discovery, API, and display pressure.
- No deadlocks or lock-order inversions are introduced.
- Existing device routing tests pass.

### Task 3.4: Dedupe And Surface Async Output Failures

**Files:** `crates/hypercolor-core/src/device/manager.rs`, `crates/hypercolor-daemon/src/render_thread/frame_executor.rs`, performance metrics.

Implementation:

- Track failure sequence or timestamp per queue.
- Report new failures clearly without logging the same stale error every frame.
- Expose recovery when writes resume.

Verify:

- Repeated write failure creates bounded logs and metrics.
- Recovery clears or supersedes the failure state.

## 10. Wave 4: Make GPU Acceleration Honest

### Task 4.1: Fix `Auto` Compositor Mode

**Files:** `crates/hypercolor-daemon/src/startup/acceleration.rs`, `crates/hypercolor-daemon/src/render_thread/sparkleflinger/mod.rs`, startup tests.

Implementation:

- If GPU probe succeeds in `Auto`, use GPU compositor mode.
- If GPU probe fails, use CPU with an explicit fallback reason.
- Keep explicit `Gpu` mode strict.

Verify:

- `--compositor-acceleration-mode auto` selects GPU on a compatible system.
- `Auto` falls back to CPU with a visible reason on incompatible systems.
- Startup logs and API state report requested and effective modes.

### Task 4.2: Separate Compositor Acceleration From Effect Renderer Acceleration

**Files:** `crates/hypercolor-types/src/config.rs`, `crates/hypercolor-core/src/effect/factory.rs`, daemon config/API docs.

Implementation:

- Rename config or split fields so users understand the difference between compositor acceleration and effect renderer acceleration.
- Make it clear that Servo HTML rendering still involves CPU readback even when scene composition uses GPU.

Verify:

- Config docs are not misleading.
- Existing configs deserialize compatibly.

### Task 4.3: Harden GPU Sampling Backpressure

**Files:** `crates/hypercolor-daemon/src/render_thread/frame_sampling.rs`, `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`, `crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu_sampling.rs`.

Implementation:

- Prefer retained frame reuse when GPU sampling readback would miss budget.
- Make blocking readback opt-in or last resort only.
- Publish metrics for deferred, stale, saturated, blocked, and fallback outcomes.

Verify:

- GPU readback stalls trigger reuse or downshift instead of long frame freezes.
- Metrics distinguish healthy deferred sampling from pathological saturation.

### Task 4.4: Benchmark CPU Versus GPU Compositor

**Files:** render pipeline benches and perf docs.

Implementation:

- Benchmark single replace layer, transition, multi-layer alpha/add/screen, preview scaling, and LED sampling.
- Compare p50/p95/p99, copy bytes, readback stalls, and power if available.

Verify:

- GPU becomes default only when it improves throughput or latency stability.
- CPU fallback remains healthy.
- `docs/development/RENDER_PIPELINE_BENCHMARKS.md` documents the local
  CPU/GPU evidence workflow.

## 11. Wave 5: Copy Budget And Zero-Copy Hardening

### Task 5.1: Move Zone Brightness Into The Routing Plan

**Files:** `crates/hypercolor-core/src/device/manager.rs`.

Implementation:

- Precompute per-zone brightness in `PlannedZoneRoute`.
- Remove per-frame `layout.zones.iter().find()` lookups during writes.
- Keep layout update behavior unchanged.

Verify:

- Output colors match current behavior.
- Routing hot-path complexity drops.
- Device manager tests pass.

### Task 5.2: Eliminate Hot-Path Borrowed-Canvas Publication Copies

**Files:** `crates/hypercolor-types/src/canvas.rs`, `crates/hypercolor-daemon/src/render_thread/frame_io.rs`, composition call sites.

Implementation:

- Audit all `PublishedSurface::from_canvas` call sites.
- Replace hot-path uses with owned canvas handoff, pooled surfaces, or existing `PublishedSurface` handles.
- Keep `from_canvas` available only for cold-path snapshots if still needed.

Verify:

- `full_frame_copy_count` stays zero in steady-state surface-backed scenes.
- Surface identity remains stable for preview and display dedupe.

### Task 5.3: Add Owned LED Frame Handoff Into USB Backend

**Files:** `crates/hypercolor-core/src/device/traits.rs`, `crates/hypercolor-core/src/device/manager.rs`, `crates/hypercolor-core/src/device/usb_backend.rs`.

Implementation:

- Add an owned or recyclable frame payload path so USB backend does not need `colors.to_vec()` on every frame.
- Preserve borrowed `write_colors` for compatibility.
- Recycle payload buffers back to staging where possible.

Verify:

- USB LED output allocation profile drops.
- Existing backends continue compiling.
- USB LED frame bytes match existing output.

### Task 5.4: Continue HAL Protocol `_into` Migration

**Files:** `crates/hypercolor-hal/src/protocol.rs`, representative driver protocol files.

Implementation:

- Prefer `encode_frame_into` and reusable `Vec<ProtocolCommand>` buffers.
- Migrate high-throughput drivers first.
- Preserve protocol correctness and packet order.

Verify:

- Representative drivers encode without per-packet heap churn.
- Protocol tests and driver tests pass.

### Task 5.5: Pool Screen And Web Viewport Preview Buffers

**Files:** `crates/hypercolor-core/src/input/screen/`, `crates/hypercolor-daemon/src/render_thread/screen_canvas.rs`, preview runtime files.

Implementation:

- Reuse downscale buffers for screen capture.
- Avoid fresh canvas allocation for same-size screen and web viewport preview conversion.
- Preserve `PublishedSurface` identity semantics.

Verify:

- Screen-reactive scenes show reduced allocations.
- Screen preview and web viewport preview remain visually correct.

## 12. Wave 6: One Composition Story

### Task 6.1: Define A Shared Layer Graph

**Files:** `crates/hypercolor-daemon/src/render_thread/composition_planner.rs`, `crates/hypercolor-daemon/src/render_thread/sparkleflinger/`, type definitions as needed.

Implementation:

- Define one layer graph for scene groups, transitions, preview, and display face blend.
- Treat direct display groups as sibling outputs with the same surface identity and cadence vocabulary.
- Keep display groups excluded from LED composition unless explicitly configured in a future spec.

Verify:

- The same data model can describe primary scene, custom groups, transitions, direct faces, and face-over-scene blend.
- Existing scene behavior remains intact.

### Task 6.2: Replace Multi-Group Scene Rasterization With Prepared Composition

**Files:** `crates/hypercolor-daemon/src/render_thread/render_groups.rs`, `crates/hypercolor-daemon/src/render_thread/composition_planner.rs`, `crates/hypercolor-daemon/src/render_thread/sparkleflinger/`.

Implementation:

- Replace the per-zone/per-pixel scene canvas raster path with prepared projection caches or SparkleFlinger layer composition.
- Cache projection maps by layout dependency key.
- Preserve zone rotation, scale, edge behavior, and sampling semantics.

Verify:

- Preview and LED output agree for rotated zones and non-default edge behavior.
- Multi-group composition frame time improves or becomes more predictable.

### Task 6.3: Align Display Faces With The Same Surface Contract

**Files:** `crates/hypercolor-daemon/src/display_output/`, `crates/hypercolor-daemon/src/render_thread/frame_io.rs`, `crates/hypercolor-core/src/bus/mod.rs`.

Implementation:

- Keep display faces as direct group canvases, but align naming, identity, cadence, and lifecycle with scene surfaces.
- Make face direct output and face-over-scene blend use the same frame-source contract.

Verify:

- A display face can be direct, blended over scene, or absent without changing the output worker mental model.
- Display preview, physical display output, and group canvas publication agree.

### Task 6.4: Retire Native Widget Path

**Files:** display-face docs, virtual display docs, removed dead display-widget spec.

Implementation:

- Delete the dead display-widget spec instead of preserving a competing fallback path.
- Align display-face and simulator docs around faces as the only rich display composition model.
- Avoid two competing display composition systems.

Verify:

- Docs and UI make the chosen path obvious.
- No runtime path silently composes display content through a deprecated model.

## 13. Wave 7: Color And Quality Consistency

### Task 7.1: Document The Color Contract

**Files:** `docs/specs/48-canonical-render-pipeline.md`, effect authoring docs, display output docs.

Implementation:

- State that canonical scene surfaces are sRGB RGBA.
- State that LED sampling uses linear-light/Oklch-aware polish and hardware-oriented output shaping.
- State that display output preserves LCD-oriented sRGB unless configured otherwise.

Verify:

- Documentation matches code.
- Effect authors know which color space assumptions are safe.

### Task 7.2: Decide Display Brightness Transfer

**Files:** `crates/hypercolor-daemon/src/display_output/encode.rs`, tests.

Implementation:

- Either make display brightness perceptual or explicitly keep byte-space scaling as an LCD policy.
- Add tests so behavior cannot drift accidentally.

Verify:

- Brightness output is predictable and tested.
- LED brightness and display brightness are intentionally different where appropriate.

### Task 7.3: Pin Gaussian Sampling Backend Contract

**Files:** spatial sampler modules, GPU sampler modules, and tests.

Implementation:

- Keep Gaussian-area sampling as a real CPU/prepared spatial sampler mode.
- Make GPU LED sampling reject Gaussian plans explicitly so it falls back to CPU instead of aliasing bilinear or area-average behavior.
- Document the backend split in the canonical render pipeline spec.

Verify:

- Sampling modes behave distinctly in tests.
- Docs match runtime behavior.

### Task 7.4: Add Hardware-Oriented Visual Tests

**Files:** effect tests, visual fixtures, possible golden outputs.

Implementation:

- Test saturation, whiteness ratio, dark-space preservation, and flicker under retained-frame reuse.
- Include low LED-count and high LED-count layouts.

Verify:

- Effects remain LED-good, not just canvas-good.
- Visual regressions are caught before release.

## 14. Wave 8: Verification, Soak, And Cleanup

### Task 8.1: Add Focused Regression Tests

**Files:** crate `tests/` directories.

Implementation:

- Test Servo lifecycle nonblocking behavior.
- Test display retry after write failure.
- Test USB LED priority under display load.
- Test surface identity reuse and zero-copy publication.
- Test GPU auto selection.

Verify:

- Targeted tests fail on current bug shapes and pass after fixes.

### Task 8.2: Add Benchmark Gates

**Files:** `benches/`, CI or local verification docs.

Implementation:

- Track p95 frame time, p99 frame time, copy bytes, queue wait, surface pool saturation, Servo pending age, and display retry counts.
- Keep gates informative initially; tighten after stable baselines.

Verify:

- Regressions are visible before release.
- Benchmarks are stable enough to guide decisions.

### Task 8.3: Run 30 Minute Soak Scenarios

**Files:** soak scripts and docs.

Implementation:

- Run Servo LED plus two display faces.
- Run screen-reactive scene plus display output.
- Run multi-group transition scene.
- Run mixed LCD plus LED device output.
- Run WebSocket preview subscribers at varied FPS and formats.

Verify:

- No runaway memory.
- No persistent backpressure.
- No unrecovered display or LED output failure.
- p95 frame time stays inside the active tier budget or the system downshifts cleanly.

### Task 8.4: Update Architecture Docs

**Files:** `docs/specs/36-render-surface-queue.md`, `docs/specs/42-display-faces.md`, `docs/specs/48-canonical-render-pipeline.md`, relevant design docs.

Implementation:

- Remove stale claims.
- Update diagrams and terminology.
- Document remaining intentional copies and why they exist.

Verify:

- Docs describe the shipped pipeline, not the desired future.
- New contributor can trace input to Servo to composition to display and lighting output in one pass.

## 15. Verification Matrix

| Area | Required Checks |
| --- | --- |
| Rust compile | `just check` |
| Full quality gate | `just verify` |
| Daemon tests | `just test-crate hypercolor-daemon` |
| Core tests | `just test-crate hypercolor-core` |
| HAL tests | `just test-crate hypercolor-hal` |
| UI preview protocol changes | `just ui-test` and `just ui-build` |
| Servo-specific changes | Servo lifecycle tests plus a manual `just daemon-servo` smoke run |
| Display output | Unit tests for encode/render/retry plus simulator preview |
| GPU compositor | CPU/GPU parity tests and benchmarks |
| Performance | Render pipeline benchmarks before and after each wave |
| Soak | 30 minute mixed-output scenarios after Waves 1, 3, 4, 6, and 8 |

## 16. Rollout Notes

- Land Waves 0 and 1 behind metrics and safe fallbacks first.
- Avoid mixing Servo lifecycle refactors with GPU compositor changes.
- Keep compatibility aliases until UI, CLI, docs, and saved config migrations are complete.
- Treat display retry and USB lane priority as correctness work, not optimization.
- Treat zero-copy as a budget with measured exceptions, not a slogan.
- Delete dead display-widget specs instead of carrying stale fallback architecture.

## 17. Recommendation

Keep the current architecture and harden it. The right end state is a render-group scene pipeline with immutable published surfaces, explicit frame phases, bounded latest-value backpressure, nonblocking Servo lifecycle, honest GPU mode selection, and shared composition vocabulary from Servo input all the way to display and lighting output.

This should feel less like separate subsystems handing buffers across fences and more like a tiny SurfaceFlinger for RGB: producers render, the compositor owns truth, consumers derive their own transport-specific bytes, and every slow edge drops or degrades without freezing the heart of the system.
