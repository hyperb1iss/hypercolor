# 29 — SparkleFlinger and the 60 FPS Evolution

> Evolution plan for moving Hypercolor from a single-source render loop to a
> deadline-driven compositor that can latch multiple producers, preserve timing,
> and optionally hit 60 FPS.

**Status:** Draft
**Date:** 2026-04-09
**Scope:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Related:** `docs/design/13-performance.md`, `docs/design/28-render-pipeline-modernization-plan.md`, `docs/specs/27-render-groups.md`, `docs/specs/36-render-surface-queue.md`

---

## 1. Overview

Hypercolor's render pipeline is already strong in one crucial way: it is
deadline-aware and it can reuse prior work to stay responsive under load.

What it does not have yet is a true composition boundary.

Today the daemon effectively chooses one canonical frame source per tick:

- active effect output
- screen preview output
- cached prior frame
- static black/off surface

That works for the current single-effect model, but it does not scale cleanly
to zones, render groups, mixed producer cadences, or a serious optional 60 FPS
mode.

This document defines the next architectural step: an internal compositor
service, codenamed `SparkleFlinger`, that sits between frame producers and the
existing spatial sampler.

Its job is simple:

- accept surfaces from multiple producers running at different speeds
- latch the newest ready surface from each producer at the frame boundary
- retain the last good surface when a producer misses a deadline
- compose one canonical frame for that tick
- hand that immutable frame to spatial sampling, preview publication, and
  device output

That makes Hypercolor much closer to the SurfaceFlinger model that inspired
Spec 36, while staying sized for a Linux RGB daemon rather than a full mobile
display stack.

---

## 2. Problem Statement

### 2.1 Current Shape

Today the render thread owns almost the entire frame lifecycle:

1. sample inputs
2. render or reuse a canvas
3. spatial-sample into zone colors
4. route colors to devices
5. publish preview and metrics

That is a good single-pipeline design, but it couples together three concerns
that want to evolve separately:

- pacing and frame deadlines
- producer output ownership
- multi-source composition

### 2.2 Why This Matters

The upcoming feature pressure is obvious:

- render groups from Spec 27
- hyperzones that may overlap or composite
- mixed sources such as Servo, native builtins, and screen capture
- optional 60 FPS operation for effects and previews

Without a composition layer, the render thread will keep accreting special
cases around "which source wins this frame?" when what we really need is "which
sources are active, what is their latest latched surface, and how do they
compose before sampling?"

### 2.3 60 FPS Is a Contract, Not a Slogan

Hypercolor should support 60 FPS optionally, not universally.

That means:

- 60 FPS is a target mode the system may admit when the workload fits
- the daemon keeps adaptive fallback to 45/30/20/10 when budget is missed
- no scene graph or effect combination gets to assume 60 FPS is guaranteed
- the system prefers deterministic reuse over jittery partial updates

The right question is not "can every configuration do 60?" The right question
is "can the architecture preserve a clean 16.6 ms deadline when the workload is
eligible, and degrade predictably when it is not?"

---

## 3. Goals and Non-Goals

### Goals

- introduce a real composition boundary before spatial sampling
- preserve the CPU pipeline as the canonical implementation
- make retained last-frame reuse an explicit compositor behavior
- support producers running at different cadences without timing collapse
- enable render groups and hyperzones without multiplying hidden copies
- make 60 FPS an opt-in runtime target with measurable eligibility
- make preview delivery a first-class downstream consumer of composed surfaces
- support both local low-latency preview and efficient remote preview transport
- leave room for optional GPU-backed composition later

### Non-Goals

- replicating the full Android graphics stack
- requiring DMA-BUF, sync fences, or GPU support in v1
- moving device sampling or HID/SMBus encoding to GPU in the first pass
- forcing Servo to become GPU-native before composition work starts
- shipping a generic scene-graph engine with arbitrary effects and filters

---

## 4. Design Principles

### 4.1 SparkleFlinger Is a Phase Boundary

`SparkleFlinger` is not "more rendering logic inside the render thread."

It is the boundary that separates:

- producer-side rendering
- frame deadline and latch control
- final composed frame delivery

Spatial sampling, bus publication, and backend writes all happen after this
boundary.

### 4.2 Producers Own Cadence, SparkleFlinger Owns Presentation

Each producer runs at its own natural speed:

- Servo may update at 20, 30, or 60 FPS depending on content and hardware
- screen capture may arrive on PipeWire cadence
- native effects may happily render at the target render tier
- future hyperzones may be static, reactive, or animated independently

SparkleFlinger owns the presentation clock. Producers do not.

### 4.3 Retention Is a Feature, Not a Fallback Hack

If a producer misses this frame, the compositor retains its previous latched
surface.

That is not a degraded edge case. It is the normal behavior that lets multiple
asynchronous producers participate in one deadline-driven pipeline.

### 4.4 Composition Must Be Cheap to Skip

If the frame has exactly one opaque source and no transforms, the compositor
should bypass blending and publish that surface directly.

SparkleFlinger is a composition service, not a tax that every frame must pay.

### 4.5 CPU First, GPU Optional

At Hypercolor's current render resolution, CPU composition is cheap enough to
ship first and use as the reference path.

GPU acceleration is an implementation option for producer rendering and
composition, not a prerequisite for the architecture.

---

## 5. Current Architecture vs Target Architecture

### 5.1 Current

```text
InputManager::sample_all()
  -> resolve_frame_canvas()
  -> SpatialEngine::sample_into()
  -> BackendManager::write_frame_with_brightness()
  -> HypercolorBus::publish(...)
```

This is effectively:

```text
one chosen frame source
  -> one canonical surface
  -> one spatial sampling pass
  -> N device outputs
```

### 5.2 Target

```text
Producer runtimes
  -> per-producer surface queues
  -> SparkleFlinger latch + compose
  -> canonical composed PublishedSurface
  -> downstream consumers:
       - SpatialEngine sample
       - local preview presentation
       - remote preview encode / transport
  -> BackendManager route + stage
  -> HypercolorBus publish + preview transport
```

This is effectively:

```text
many asynchronous frame sources
  -> one presentation clock
  -> one composed immutable surface per tick
  -> one downstream sampling and output path
```

### 5.3 What Stays the Same

- `PublishedSurface` remains the canonical frame handoff
- spatial sampling still happens after the final visual frame is decided
- backend routing and device output stay downstream consumers
- `watch` channels remain latest-value distribution for preview-like data
- adaptive FPS remains part of the runtime

### 5.4 What Changes

- `render_thread.rs` stops deciding which single source "wins"
- effects and screen capture become producers rather than special cases
- the frame deadline loop latches from producer queues before sampling
- composition becomes explicit rather than incidental

---

## 6. SparkleFlinger Component Model

### 6.1 Core Components

| Component | Responsibility |
|----------|----------------|
| `FrameScheduler` | Owns the presentation clock and target FPS tier |
| `ProducerRuntime` | Runs one producer and submits completed surfaces |
| `ProducerQueue` | Holds `front` / `back` / `spare` surfaces plus metadata |
| `SparkleFlinger` | Latches ready surfaces, composes, and emits the frame for this tick |
| `CompositionPlan` | Describes the ordered set of active layers or groups for the scene |
| `ComposedFrame` | Immutable output surface plus frame metadata |
| `PreviewRuntime` | Presents or encodes composed frames for browser and remote consumers |

### 6.2 Producer Types

Initial producer classes:

- `NativeEffectProducer`
- `ServoEffectProducer`
- `ScreenCaptureProducer`
- `StaticSurfaceProducer`

Future producer classes:

- `RenderGroupProducer`
- `TransitionProducer`
- `SceneOverlayProducer`

### 6.3 Producer Submission Model

Each producer owns a surface queue with explicit roles:

- `front`: most recently submitted surface
- `back`: producer write target
- `spare`: reusable storage

Submission metadata should include:

- producer id
- generation
- produced timestamp
- source frame number
- intended content kind
- dirty or changed flag
- optional estimated render cost

SparkleFlinger only needs the latest submitted surface per producer for v1.

### 6.4 Latch Model

At each frame boundary:

1. collect the active composition plan
2. check each producer queue for a newly submitted surface
3. if a new surface is ready, latch it
4. otherwise retain the previously latched surface
5. compose the ordered result or bypass if composition is unnecessary
6. publish the composed frame downstream

This is the exact behavior we want from SurfaceFlinger and from uchroma's
animation loop: presentation stays stable even when producers do not all update
on the same tick.

---

## 7. Composition Model

### 7.1 V1 Composition Scope

V1 should stay intentionally small.

Supported:

- ordered layers
- rectangular viewport placement
- opacity
- simple blend modes:
  - `replace`
  - `alpha`
  - `add`
  - `screen`
- bypass for single-source opaque frames

Deferred:

- blur and shader filters
- arbitrary masks
- nested scene graphs
- per-layer color management
- sub-rect damage tracking in the first shipping pass

### 7.2 Render Groups and Hyperzones

Spec 27 already gives Hypercolor the right conceptual unit: `RenderGroup`.

SparkleFlinger should treat each render group as a producer-backed layer:

- each group renders into its own canonical surface
- each group has its own layout and effect controls
- SparkleFlinger composes group outputs into the final scene surface
- the spatial sampler runs once on the composed result for the downstream
  device map

This means "hyperzones" are not a separate architecture. They are a scene-level
composition model built on render groups plus SparkleFlinger.

### 7.3 Bypass Fast Path

When the composition plan contains exactly one visible full-frame source with
`replace` semantics, SparkleFlinger should hand that `PublishedSurface`
downstream directly.

That preserves the cheap path for:

- single builtin effects
- screen passthrough
- static surfaces

---

## 8. 60 FPS Strategy

### 8.1 Admission Model

Hypercolor should expose 60 FPS as a target mode, but only admit it when the
runtime believes the workload can sustain it.

Inputs to admission:

- recent total frame time EWMA
- composition cost EWMA
- producer render cost EWMA
- number of active producers
- whether any active producer is marked low-rate or bursty
- preview or display output load

### 8.2 Eligibility Expectations

These are product expectations, not hard guarantees:

| Workload | 60 FPS expectation |
|---------|--------------------|
| single builtin effect | yes on the CPU path |
| screen passthrough | yes on the CPU path |
| builtin + simple overlay composition | likely yes on the CPU path |
| multi-group native composition | maybe, benchmarked |
| Servo-heavy scenes | best effort, often 30-60 depending on hardware |
| mixed Servo + screen + multiple groups | likely needs GPU or lower tier |

### 8.3 Degradation Order

When budget is repeatedly missed, degrade in this order:

1. reuse producer surfaces if the producer did not materially change
2. bypass unnecessary preview or encode work where allowed
3. step down presentation tier: `60 -> 45 -> 30 -> 20 -> 10`
4. preserve stable composition semantics at the lower tier

The system should never silently swap to inconsistent partial updates just to
keep the label "60 FPS."

### 8.4 Benchmarks Required for 60 FPS

Every 60 FPS claim must be backed by:

- steady-state end-to-end frame time under target scene load
- p95 and p99 frame duration
- jitter measurement
- dropped or reused frame counts
- per-stage timings:
  - producer render
  - composition
  - spatial sampling
  - device push
  - preview or publish work

---

## 9. Preview Architecture

### 9.1 Preview Is Part of the Pipeline

Preview is not "just a UI concern."

Once SparkleFlinger exists, preview becomes one of the primary consumers of the
composed frame alongside spatial sampling and device routing.

That means preview architecture must inherit the same guarantees:

- latest ready frame semantics
- deadline-aware reuse
- immutable published surfaces
- no mutation of the canonical composed frame for presentation-only effects

### 9.2 Two Preview Lanes

SparkleFlinger should feed two distinct preview lanes:

- `local_raw`
  - authoritative low-latency preview for the local browser or desktop shell
  - optimized for minimal encode latency and visual smoothness
  - suitable for the current 320x200 canonical canvas and authoring workflows
- `remote_encoded`
  - transport-efficient preview for higher-resolution screen or video-like output
  - optimized for bandwidth, jitter tolerance, and remote delivery
  - suitable for browser clients across a network

These lanes should share one composed frame contract and diverge only at the
consumer boundary.

### 9.3 Local Raw Preview Path

The local preview path should remain raw-frame capable even after encoded
transport exists.

That path should evolve toward:

- worker-owned presentation rather than main-thread painting
- `OffscreenCanvas` presentation where supported
- WebGL as the baseline browser renderer
- optional WebGPU presentation backend where it proves materially better
- one present per animation frame using latest-value frame retention

This path is the right answer for:

- local dashboards
- effect authoring
- immediate "what is the daemon doing right now?" validation
- cases where encode latency would be worse than raw transport cost

### 9.4 Remote Encoded Preview Path

Raw preview frames are not the long-term answer for remote, larger, or
video-rate preview.

The remote path should be treated as a video transport problem:

- encode from the composed frame, not from per-producer inputs
- prefer low-latency streaming semantics over file-style media semantics
- negotiate codec and transport based on client capabilities
- preserve raw WebSocket preview as the debug and fallback path

The likely first shipping shape is:

- WebRTC transport for low-latency browser delivery
- H.264 as the practical broad-compatibility baseline
- VP8 fallback
- AV1 only where support and latency are good enough

### 9.5 Preview Capability Negotiation

Preview clients should not all consume the same transport.

The daemon should eventually negotiate a preview mode based on:

- local vs remote client
- requested resolution
- requested frame rate
- latency sensitivity
- bandwidth constraints
- browser capabilities
- daemon acceleration mode

Possible negotiated modes:

- `ws_raw_rgb`
- `ws_raw_rgba`
- `webrtc_h264`
- `webrtc_vp8`
- future `webtransport_av1` or similar only if justified later

### 9.6 Preview Metrics

Preview quality should be measured explicitly, not inferred from engine FPS.

Required metrics:

- composed frame FPS
- preview target FPS and actual delivered FPS
- encode queue depth
- preview frame age at send time
- browser present latency when measurable
- dropped preview frame count
- codec bitrate for encoded modes
- local raw upload or present time where measurable

### 9.7 Preview Quality Rules

The preview must feel smooth before it is feature-rich.

Rules:

- local preview should prefer freshness over completeness
- one slow browser tab must never backpressure SparkleFlinger
- encoded preview should degrade resolution or bitrate before it destroys latency
- raw preview should be allowed to cap lower than engine FPS when the client asks
- authoritative debug mode must exist so engineering can see uncompressed daemon output

---

## 10. GPU Architecture

### 10.1 Where GPU Actually Helps

GPU acceleration is most useful in three places:

- native GPU-backed producer rendering
- multi-layer composition
- high-rate preview or display presentation

GPU does not magically remove the CPU work that still exists after composition:

- spatial sampling into LED coordinates
- per-device routing
- HID/SMBus/network packet encoding

### 10.2 CPU and GPU Share One Contract

SparkleFlinger must define one abstract surface and queue contract:

- producer dequeues a writable target
- producer submits an immutable published surface
- compositor latches surfaces
- compositor emits one immutable composed frame

The backing may be:

- CPU-visible RGBA bytes
- GPU texture with a CPU-readable fallback
- future shared-handle backing such as DMA-BUF

The semantics must remain identical.

### 10.3 GPU Preview and Encode

GPU becomes especially interesting once preview is treated as its own consumer.

If the composed frame is GPU-backed, the ideal downstream order is:

- compose on GPU
- present locally from the same GPU backing when possible
- feed a hardware or GPU-assisted encoder from that backing when possible
- read back to CPU only for consumers that still require CPU-visible pixels

That means GPU helps preview most when:

- the scene is larger than the canonical LED canvas
- screen or video output is active
- remote preview is enabled at higher frame rates
- the same composed frame would otherwise be uploaded to the browser and also
  copied into an encoder

### 10.4 Servo Reality

Servo is still effectively a CPU producer in Hypercolor today.

That means:

- the first SparkleFlinger implementation should assume CPU-visible Servo output
- CPU composition is not a stopgap; it is the required compatibility path
- GPU composition becomes most valuable when native effects and screen capture
  can stay GPU-backed longer than Servo can

### 10.5 WebGPU, WebGL, and Hardware Encode

WebGPU is promising, but it is not the architecture by itself.

What matters is the split of responsibilities:

- SparkleFlinger defines frame ownership and consumer boundaries
- WebGL or WebGPU can implement local browser presentation
- hardware encode can implement remote preview transport

The design should not require WebGPU to exist everywhere in order for preview
to be excellent. WebGL remains the practical browser baseline. WebGPU should be
used when it measurably improves smoothness, upload cost, or decoded-frame
presentation.

### 10.6 DMA-BUF and Fence Sync

SurfaceFlinger's lower layers are still worth studying, but they are not v1
requirements.

What we should preserve now:

- acquire and release semantics
- queue depth awareness
- immutable published buffers
- explicit ownership transitions

What can wait:

- explicit sync fences
- DMA-BUF import and export
- kernel-backed zero-copy across processes

If the surface contract is clean, those can be added later without rewriting
the compositor model.

---

## 11. Evolution Plan

### Wave 1 — Split Scheduling from Source Selection

**Goal:** Stop treating the render thread as both scheduler and compositor.

Deliverables:

- extract a `FrameScheduler` boundary from the existing render thread loop
- make frame deadline decisions explicit and independently testable
- preserve current single-source behavior through the new boundary

Verify:

- existing render-thread tests still pass
- no regression in current frame pacing benches

### Wave 2 — Producer Queues

**Goal:** Turn effect output and screen capture into explicit producers.

Deliverables:

- add `ProducerQueue` and producer submission metadata
- convert builtin, Servo, and screen paths to submit surfaces
- retain last latched surface per producer

Verify:

- new tests for submit, latch, and retained-surface behavior
- end-to-end frame benches show stable reuse without extra copies

### Wave 3 — CPU SparkleFlinger

**Goal:** Introduce the first real compositor.

Deliverables:

- add `SparkleFlinger` CPU path
- support bypass, `replace`, `alpha`, `add`, and `screen`
- emit one composed `PublishedSurface` downstream

Verify:

- composition unit tests cover ordering, opacity, and bypass
- daemon bench isolates composition overhead at 320x200

### Wave 4 — Render Groups and Hyperzones

**Goal:** Make composition useful for product features.

Deliverables:

- wire render groups from Spec 27 into the producer model
- compile a `CompositionPlan` for the active scene
- sample from the final composed scene surface only once per frame

Verify:

- scene tests cover multiple groups with independent cadences
- device output remains stable when one producer stalls and another advances

### Wave 5 — 60 FPS Admission and Hardening

**Goal:** Make optional 60 FPS operationally honest.

Deliverables:

- add 60 FPS admission checks and runtime tier decisions
- publish reuse counts, latch counts, and composition timing metrics
- define product-level benchmark scenes and baseline numbers

Verify:

- representative scenes show measured p95 and p99 timings
- runtime drops tiers cleanly under load and recovers conservatively

### Wave 6 — Preview Re-Architecture

**Goal:** Make preview a first-class consumer of composed frames.

Deliverables:

- add a `PreviewRuntime` boundary that consumes the composed frame
- preserve raw preview as the authoritative local and debug path
- move browser preview toward worker-owned presentation
- add preview mode negotiation and per-mode telemetry
- define local raw and remote encoded preview scenarios in the bench suite

Verify:

- local preview remains smooth at target FPS without affecting daemon pacing
- slow preview clients drop frames without backpressuring the compositor
- benchmark data includes preview frame age and client-visible delivery rate

### Wave 7 — Remote Encoded Preview

**Goal:** Add a transport-efficient preview lane for network and higher-rate use.

Deliverables:

- add encoded preview pipeline fed from composed frames
- implement at least one low-latency browser-friendly transport
- preserve raw preview fallback and debugging modes
- benchmark bitrate, latency, and degradation behavior under constrained links

Verify:

- end-to-end latency and smoothness are measured, not assumed
- remote preview degrades gracefully under bandwidth pressure
- encoded preview does not change LED or local preview semantics

### Wave 8 — Optional GPU Composition

**Goal:** Accelerate composition without changing semantics.

Deliverables:

- add GPU compositor backend behind the existing acceleration mode
- preserve CPU SparkleFlinger as the reference path
- validate parity between CPU and GPU composed output

Verify:

- parity tests for composition correctness
- benchmark comparison between CPU and GPU for supported scenes
- explicit fallback to CPU on init or runtime failure

### Wave 9 — GPU Preview and Advanced Producer Backings

**Goal:** Explore more SurfaceFlinger-like backings only if justified.

Deliverables:

- evaluate direct GPU-to-preview presentation for local clients
- evaluate hardware-accelerated encode fed from GPU-backed composed frames
- evaluate DMA-BUF-backed surfaces for screen capture or preview
- evaluate fence-style synchronization for GPU-backed producers
- adopt only if the measured win is meaningful and complexity is contained

Verify:

- benchmark evidence beats CPU-backed staging by a meaningful margin
- failure modes remain easy to debug and degrade cleanly

---

## 12. Risks

### 12.1 Over-Architecting for Future GPU Work

The biggest trap is building a compositor so abstract that the CPU path gets
slower or harder to reason about.

Mitigation:

- ship CPU SparkleFlinger first
- keep the surface contract small
- only add GPU-specific complexity behind the same contract

### 12.2 Producer Explosion

If every feature becomes its own producer too early, scheduling and debugging
will get noisy fast.

Mitigation:

- start with a small producer taxonomy
- use render groups as the first real multi-producer feature
- avoid arbitrary scene-graph flexibility before product needs it

### 12.3 Lying About 60 FPS

Advertising 60 FPS without workload qualification would be a product mistake.

Mitigation:

- make 60 FPS optional and measured
- publish actual reuse and miss metrics
- benchmark representative scenes, not toy microbenches only

### 12.4 Solving the Wrong Preview Problem

The preview path can fail in multiple ways:

- daemon-side pacing may be bad
- browser-side presentation may be bad
- transport may be bad
- encoding may be bad

Mitigation:

- measure each boundary separately
- keep raw local preview available for diagnosis
- benchmark browser presentation separately from daemon composition
- do not assume WebGPU or encoding automatically fixes jank

---

## 13. Recommendation

Build `SparkleFlinger` as a CPU-first compositor that sits between producers
and spatial sampling, and treat optional 60 FPS as an admitted presentation
mode rather than a universal promise.

Spec 36 already gives us the correct buffer ownership model. Spec 27 gives us
the correct unit of composition in render groups. The right next move is to
join those two ideas:

- producer queues with explicit ownership
- deadline-driven latching and retention
- one composed immutable frame per tick
- spatial sampling and preview as first-class downstream consumers
- a dual-lane preview model:
  - raw local preview for low-latency authoring and debugging
  - encoded remote preview for efficient network delivery
- optional GPU acceleration later, under the same contract

That gets Hypercolor the parts of SurfaceFlinger that matter most:

- stable presentation under asynchronous producers
- explicit buffer ownership
- clean separation between scheduling and composition

Without inheriting the parts that are too heavy for the daemon today.
