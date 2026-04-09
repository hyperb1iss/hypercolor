# 28 — Render Pipeline Modernization Plan

> Execution roadmap for moving Hypercolor from clone-heavy canvas handoff to
> explicit surface ownership, reusable transport staging, and an optional GPU
> acceleration lane.

**Status:** Draft
**Date:** 2026-04-08
**Scope:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Primary spec:** `docs/specs/36-render-surface-queue.md`
**Related:** `docs/design/zero-copy-audit.md`, `docs/specs/27-render-groups.md`

---

## 1. Overview

Spec 36 defines the target architecture: explicit front/back/spare surface
ownership, immutable published surfaces, consumer-local staging, and reusable
routing plans. This document turns that architecture into an execution plan.

The plan has one hard rule:

- the CPU pipeline remains the canonical and fully supported path
- GPU composition is optional acceleration, not a new requirement
- every GPU path must preserve identical semantics and fall back cleanly

That keeps Hypercolor aligned with its actual product shape:

- headless Linux daemon first
- mixed renderer backends
- Servo currently produces CPU-visible frames
- LED sampling and most transports still consume CPU data

The immediate goal is not "move everything to GPU." The immediate goal is to
fix ownership, stop hidden copies, and make later acceleration possible.

---

## 2. Decisions and Constraints

### 2.1 Fixed Decisions

- `PublishedSurface` becomes the canonical frame boundary
- renderers stop returning owned `Canvas` values
- preview, display, and transport work happens in consumer-local staging
- routing is compiled on topology changes, not rebuilt every frame
- the default runtime path remains CPU-first

### 2.2 GPU Constraint

GPU composition must remain optional.

That means:

- no feature may require GPU composition to function
- the daemon must run correctly on systems without GPU acceleration
- correctness and output parity are validated against the CPU path
- initialization failure must fall back to CPU without breaking rendering

### 2.3 Servo Constraint

Servo is not currently a GPU-resident producer in Hypercolor.

Today the worker explicitly creates a `SoftwareRenderingContext`, then reads the
frame into CPU memory. That means the first several waves must assume Servo
remains a CPU producer even if an optional GPU compositor appears later.

### 2.4 Scope Boundary

This plan covers:

- render surface ownership
- renderer contracts
- canvas publication
- WebSocket and display staging reuse
- backend routing reuse
- optional GPU composition infrastructure

This plan does not cover:

- shader effect authoring
- replacing Servo
- GPU-native LED sampling in the first pass
- UI protocol redesign

---

## 3. Success Criteria

### Functional

- built-in native effects still render correctly
- Servo HTML effects still render correctly
- screen-reactive effects still receive valid screen data
- display output, WebSocket preview, and LED output preserve current behavior

### Performance

- steady-state publish performs zero full-frame copies of the canonical surface
- preview brightness no longer mutates canonical surfaces
- WebSocket preview payloads are built once per frame per format, not per client
- backend routing plans rebuild only on layout or mapping changes

### Rollout

- CPU path is the default and fully supported mode
- optional GPU mode can be enabled independently
- GPU init failure falls back to CPU automatically
- parity checks exist between CPU and GPU-accelerated outputs
- every wave records before/after benchmark results for the hot paths it changes

---

## 4. Delivery Strategy

The plan is intentionally split into two tracks:

- **Track A: CPU ownership and reuse**
  This is the required path. It solves the current hot-path waste and becomes
  the stable reference pipeline.
- **Track B: Optional GPU acceleration**
  This starts only after Track A produces a clean ownership model. It is an
  additive optimization track, not the main migration path.

This ordering matters. If Hypercolor adds GPU composition before fixing frame
ownership, it will still carry the same ambiguity into a more complex system.

---

## 5. Workstreams

### Workstream A — Instrumentation, Guardrails, and Benchmarks

Measure what the pipeline is doing before and during the refactor:

- full-frame copy count
- preview encode count
- WS payload build count
- routing-plan rebuild count
- per-stage allocation metrics where practical
- repeatable microbenchmarks for touched hot paths
- end-to-end frame benchmarks for representative render scenarios

### Workstream B — Surface Ownership

Introduce `RenderSurfacePool`, `SurfaceLease`, and `PublishedSurface`.

### Workstream C — Renderer Contract

Move renderers from `tick() -> Canvas` to `render_into(...)`.

### Workstream D — Consumer Staging

Push preview, display, and WS encoding into reusable consumer-local buffers.

### Workstream E — Routing Reuse

Replace per-frame routing maps with compiled plans and persistent staging.

### Workstream F — Input Cleanup

Move screen snapshots to shared handles and clean up retained static surfaces.

### Workstream G — Optional GPU Acceleration

Add a GPU composition lane that can consume GPU-backed producers when present,
while preserving the CPU path as the reference implementation.

---

## 6. Execution Waves

### Wave 0 — Baseline and Safety Nets

**Goal:** Make progress measurable and prevent invisible regressions.

#### Task 0.1: Add render-pipeline copy and encode counters

**Files:**
- `crates/hypercolor-daemon/src/render_thread.rs`
- `crates/hypercolor-daemon/src/api/ws.rs`
- `crates/hypercolor-daemon/src/display_output.rs`
- `crates/hypercolor-core/src/device/manager.rs`

**Implementation:**
- add counters for full-frame copies, preview encodes, and payload builds
- emit metrics through existing telemetry surfaces where possible
- keep instrumentation cheap and easy to disable in hot loops if needed

**Verify:**
- `just check`
- manual metrics inspection shows non-zero counters on the current path

#### Task 0.2: Define rollout toggles

**Files:**
- `crates/hypercolor-types/src/config.rs`
- `crates/hypercolor-daemon/src/startup.rs`
- `crates/hypercolor-daemon/src/render_thread.rs`

**Implementation:**
- add explicit render acceleration mode config:
  - `cpu`
  - `auto`
  - `gpu`
- keep default as `cpu`
- ensure unsupported GPU mode falls back to `cpu` in `auto`

**Verify:**
- `just check`
- config serde tests cover defaults and round-trip behavior

#### Task 0.3: Build benchmark harnesses and baselines

**Files:**
- `crates/hypercolor-core/benches/core_pipeline.rs`
- `crates/hypercolor-core/benches/` new targeted benches as needed
- `crates/hypercolor-daemon/benches/` new end-to-end benches as needed
- `scripts/` benchmark helpers if needed

**Implementation:**
- define stable benchmark scenarios for:
  - builtin CPU render
  - Servo frame handoff
  - spatial sampling
  - WS preview encode
  - `frames` payload packing
  - backend routing execution
  - end-to-end mock render loop
- capture an explicit baseline before the refactor proceeds
- document how benchmark data is collected and compared

**Verify:**
- benchmark targets run locally without code changes to business logic
- baseline numbers are recorded for the current pipeline

---

### Wave 1 — Surface Pool Foundation

**Goal:** Replace ambiguous `Canvas` ownership with explicit surface roles.

#### Task 1.1: Add surface pool types

**Files:**
- `crates/hypercolor-types/src/canvas.rs`
- `crates/hypercolor-types/tests/canvas_tests.rs`

**Implementation:**
- add `SurfaceDescriptor`
- add `RenderSurfacePool`
- add `SurfaceLease`
- add `PublishedSurface`
- preserve temporary adapters so the rest of the tree can migrate incrementally

**Verify:**
- `just test-crate hypercolor-types`
- new tests cover dequeue, submit, reuse, and generation changes

#### Task 1.2: Publish shared surfaces on the bus

**Files:**
- `crates/hypercolor-core/src/bus/mod.rs`
- `crates/hypercolor-daemon/src/render_thread.rs`

**Implementation:**
- replace owned-canvas publication with `PublishedSurface`
- remove `CanvasFrame::from_owned_canvas()` from the hot path
- keep watch semantics unchanged

**Verify:**
- `just check`
- daemon tests confirm canvas watch publication still works

#### Task 1.3: Remove in-place preview brightness from canonical surfaces

**Files:**
- `crates/hypercolor-daemon/src/render_thread.rs`
- `crates/hypercolor-daemon/src/api/ws.rs`
- `crates/hypercolor-daemon/src/display_output.rs`

**Implementation:**
- stop mutating the canonical frame before publish
- move preview brightness handling to consumer staging

**Verify:**
- `just check`
- targeted tests show brightness changes no longer force publish-time copies

---

### Wave 2 — Renderer Contract Migration

**Goal:** Make renderers write into owned target storage instead of returning
fresh `Canvas` values.

#### Task 2.1: Change `EffectRenderer` to `render_into(...)`

**Files:**
- `crates/hypercolor-core/src/effect/traits.rs`
- `crates/hypercolor-core/src/effect/engine.rs`
- `crates/hypercolor-core/tests/effect_engine_tests.rs`

**Implementation:**
- replace `tick() -> Canvas` with `render_into(...) -> Result<()>`
- thread a target surface through `EffectEngine`
- preserve paused/error semantics

**Verify:**
- `just test-crate hypercolor-core`
- engine tests cover running, paused, and error states

#### Task 2.2: Migrate built-in renderers

**Files:**
- `crates/hypercolor-core/src/effect/builtin/*.rs`
- `crates/hypercolor-core/tests/*`

**Implementation:**
- update builtins to write directly into target storage
- remove retained frame clones where present

**Verify:**
- `just test-crate hypercolor-core`
- visual parity snapshots where available

#### Task 2.3: Migrate Servo to pooled targets

**Files:**
- `crates/hypercolor-core/src/effect/servo_renderer.rs`
- `crates/hypercolor-core/src/effect/servo_bootstrap.rs`
- `crates/hypercolor-core/tests/servo_renderer_tests.rs`

**Implementation:**
- keep Servo on the CPU path initially
- write readback output directly into pool-backed target storage where possible
- remove renderer-local "previous frame" clones

**Verify:**
- `just test-crate hypercolor-core`
- Servo fallback and frame-retention tests still pass

---

### Wave 3 — Consumer Staging and Payload Reuse

**Goal:** Make preview and display costs proportional to frames, not clients.

#### Task 3.1: Add surface encode caches

**Files:**
- `crates/hypercolor-daemon/src/api/ws.rs`
- `crates/hypercolor-core/src/bus/mod.rs`

**Implementation:**
- cache RGBA/RGB preview payloads per published surface
- share encoded payloads across all matching subscribers

**Verify:**
- `just test-crate hypercolor-daemon`
- multiple WS clients share one encode per frame per format

#### Task 3.2: Rework `frames` payload generation

**Files:**
- `crates/hypercolor-daemon/src/api/ws.rs`

**Implementation:**
- cache packed frame payloads by frame number, zone selection, and format
- remove per-client clone-then-pack of zone colors

**Verify:**
- `just test-crate hypercolor-daemon`
- mixed subscriber tests confirm payload reuse and correct filtering

#### Task 3.3: Keep display output on shared published surfaces

**Files:**
- `crates/hypercolor-daemon/src/display_output.rs`

**Implementation:**
- source display encoding directly from `PublishedSurface`
- preserve existing JPEG buffer reuse
- remove avoidable surface clones

**Verify:**
- `just test-crate hypercolor-daemon`
- manual display-output smoke check under load

---

### Wave 4 — Routing Plan Compilation

**Goal:** Stop rebuilding routing structures every frame.

#### Task 4.1: Add `RoutingPlan`

**Files:**
- `crates/hypercolor-core/src/device/manager.rs`
- `crates/hypercolor-core/tests/device_manager_tests.rs`

**Implementation:**
- compile routing plans from layout and device mapping state
- cache per-device and per-zone route information

**Verify:**
- `just test-crate hypercolor-core`
- route output matches pre-refactor behavior

#### Task 4.2: Reuse device staging buffers

**Files:**
- `crates/hypercolor-core/src/device/manager.rs`
- `crates/hypercolor-core/src/device/usb_backend.rs`

**Implementation:**
- keep persistent staging vectors for per-device output
- avoid per-frame temporary maps and remap vectors

**Verify:**
- `just check`
- manual profiling confirms routing rebuilds only happen on topology changes

---

### Wave 5 — Input and Lifecycle Cleanup

**Goal:** Remove remaining clone-heavy edges around screen data and retained
static frames.

#### Task 5.1: Convert screen snapshots to shared handles

**Files:**
- `crates/hypercolor-core/src/input/traits.rs`
- `crates/hypercolor-core/src/input/screen/mod.rs`
- `crates/hypercolor-core/src/input/screen/wayland.rs`
- `crates/hypercolor-daemon/src/render_thread.rs`

**Implementation:**
- replace cloneable `Canvas` screen payloads with shared snapshot handles
- publish screen preview using the same ownership rules as render surfaces

**Verify:**
- `just test-crate hypercolor-core`
- screen-reactive effects still render from current snapshots

#### Task 5.2: Retain static published surfaces for idle and sleep

**Files:**
- `crates/hypercolor-daemon/src/render_thread.rs`

**Implementation:**
- retain one published static surface for hold/release policies
- avoid regenerating equivalent canvases unnecessarily

**Verify:**
- `just check`
- manual idle/sleep behavior matches current policy

---

### Wave 6 — CPU Path Hardening Gate

**Goal:** Freeze the CPU path as the reference implementation before any GPU
work begins.

#### Exit criteria

- copy and encode counters show the expected steady-state reductions
- render thread, WS preview, and display output behave correctly
- no known canonical-surface mutation remains
- Spec 27 render-group work can assume the new surface model
- benchmark baselines have been updated and compared against Wave 0 numbers

If these criteria are not met, Wave 7 does not start.

---

### Wave 7 — Optional GPU Composition Substrate

**Goal:** Add a GPU acceleration lane without changing the canonical behavior
contract.

#### Task 7.1: Define GPU-capable surface abstractions

**Files:**
- `crates/hypercolor-types/src/canvas.rs`
- `crates/hypercolor-core/src/effect/traits.rs`
- `crates/hypercolor-daemon/src/render_thread.rs`

**Implementation:**
- extend surface descriptors to support CPU and GPU backing kinds
- keep `PublishedSurface` semantics identical regardless of backing
- define explicit readback boundaries rather than implicit CPU materialization

**Verify:**
- `just check`
- CPU mode remains unchanged when GPU types are compiled in

#### Task 7.2: Add an optional GPU compositor path

**Files:**
- `crates/hypercolor-core/src/effect/` new GPU compositor module
- `crates/hypercolor-daemon/src/render_thread.rs`
- `crates/hypercolor-daemon/src/startup.rs`

**Implementation:**
- create a compositor that can combine GPU-backed producers or apply preview
  transforms before one controlled readback point
- keep runtime mode selection explicit
- fall back to CPU composition automatically on init or runtime failure

**Verify:**
- `just check`
- manual boot in `cpu`, `auto`, and `gpu` modes
- forced GPU init failure falls back cleanly in `auto`

#### Task 7.3: Add parity tests between CPU and GPU modes

**Files:**
- `crates/hypercolor-core/tests/`
- `crates/hypercolor-daemon/tests/`

**Implementation:**
- add deterministic parity tests for representative frames
- allow small tolerances only where color math requires it

**Verify:**
- targeted parity tests pass in CI environments that support the mode

---

### Wave 8 — Servo GPU Feasibility Spike

**Goal:** Determine whether Servo can participate in the optional GPU lane
without destabilizing the default path.

#### Task 8.1: Replace `SoftwareRenderingContext` in a spike branch

**Files:**
- `crates/hypercolor-core/src/effect/servo_bootstrap.rs`
- `crates/hypercolor-core/src/effect/servo_renderer.rs`

**Implementation:**
- investigate a hardware-backed headless rendering context
- evaluate whether the embedder can safely expose a texture or framebuffer
  handle instead of forcing CPU readback

**Verify:**
- spike-only proof that Servo can render and produce valid frames
- document whether texture export is practical, fragile, or not viable

#### Task 8.2: Decision gate

Choose one:

- keep Servo CPU-only and let GPU composition accelerate only native/GPU-backed
  producers
- add a bounded Servo GPU path if it proves stable and maintainable

This wave is explicitly optional. Failure here does not block the main plan.

---

## 7. Verification Matrix

### Per-wave standard checks

- `just check`
- targeted crate tests for touched modules
- manual smoke checks for preview, device output, and effect rendering where
  automation is incomplete

### Cross-wave checks

- brightness changes must never mutate canonical published surfaces
- multiple WS clients must not multiply encode cost linearly
- paused, idle, and sleep behavior must preserve current semantics
- render-group readiness must improve, not regress

### Benchmark policy

Every wave must produce benchmark evidence for the hot paths it changes.

Minimum benchmark set:

- microbenchmarks for the modified subsystem
- one end-to-end frame benchmark with mock devices
- before/after comparison against the Wave 0 baseline

Required metrics where relevant:

- frame time
- allocations per frame
- bytes copied per frame
- encode count per frame
- routing execution time

Benchmark results should answer two questions:

- did the intended improvement actually happen
- did we accidentally slow down an adjacent stage

### GPU-specific checks

- `cpu` mode remains the reference output
- `auto` mode falls back automatically
- `gpu` mode failures are explicit and diagnosable
- parity drift is measured, not guessed
- GPU mode benchmarks are compared against CPU mode on the same scenarios

---

## 8. Risks and Mitigations

### Risk 1: Surface API churn spreads too far too fast

**Mitigation:** keep temporary adapters in place during Waves 1 and 2; migrate
callers in layers rather than in one giant cutover.

### Risk 2: Servo remains a CPU wall

**Mitigation:** treat Servo GPU work as an optional spike, not a prerequisite for
the ownership refactor.

### Risk 3: GPU mode becomes the accidental default

**Mitigation:** keep config default at `cpu`; require explicit enablement for
`gpu`; use `auto` only when fallback is proven.

### Risk 4: New caches hide stale-data bugs

**Mitigation:** key caches by surface generation and frame number, and include
adversarial tests for brightness, format, and zone selection changes.

### Risk 5: Routing-plan compilation drifts from live layout behavior

**Mitigation:** preserve existing routing tests and add before/after parity
checks on representative layouts.

---

## 9. Recommendation

Build this in two deliberate stages:

1. complete Waves 0 through 6 and make the CPU path boring, explicit, and fast
2. treat Waves 7 and 8 as optional acceleration work, not as the main refactor

That is the lowest-risk route with the highest leverage. It fixes the current
copy and ownership problems immediately, keeps Hypercolor robust on headless and
mixed-capability systems, and still leaves the door open for a genuinely good
GPU compositor later.

The CPU pipeline should become cleaner because of this work. The GPU pipeline
should exist only if it earns its complexity.
