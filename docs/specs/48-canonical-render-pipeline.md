# 48 — Canonical Render Pipeline

**Status:** Active (core architecture landed; remaining work is simplification)
**Author:** Nova
**Date:** 2026-04-21
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`
**Supersedes:** Spec 45 as the canonical render-pipeline target
**Related:** Specs 40, 42, 47; `docs/design/13-performance.md`

---

## 1. Overview

Hypercolor now renders primarily through scenes and render groups, not through
the legacy single-effect mental model. This spec defines the canonical
architecture for that pipeline so the code, docs, and future refactors all
point at the same thing.

The core idea is simple:

- A frame starts from one active scene.
- The active scene resolves to a set of active render groups.
- Shared inputs are sampled once for the frame.
- Each active group renders independently.
- The pipeline emits three kinds of output from that same scene:
  - LED colors for hardware output
  - a canonical scene canvas for preview and scene-level consumers
  - direct group canvases for display-face targets

This spec also defines the invalidation rules that keep hot reload, capture
demand, preview composition, and hardware output coherent.

---

## 2. Problem Statement

When this spec was written, the codebase was already much closer to a unified
scene model than the older specs described, but a few critical seams still
behaved like separate systems:

1. Effect registry reloads can change metadata or source content without
   invalidating active renderers or capture-demand caches.
2. The canonical scene preview for multi-group scenes is composed with
   rectangle blits, while the physical LED path uses full spatial sampling
   semantics including rotation and edge behavior.
3. Performance and architecture docs still describe an older, simpler hot path
   than the one the daemon actually runs today.

That gap is now largely closed. The remaining work is simplification and
vocabulary cleanup inside the same architecture, not a new render model.

---

## 3. Canonical Model

### 3.1 Active scene ownership

- There is always one active scene.
- The render thread consumes the active scene only through its resolved active
  render groups and runtime policy.
- Render groups are the only composition primitive for live rendering.

### 3.2 Frame inputs

Every frame samples shared runtime inputs once:

- audio
- screen capture
- interaction state
- sensors
- output power state
- device capability context needed for routing decisions

These inputs are then fanned out to all active render groups for that frame.
No group owns its own capture loop.

### 3.3 Render groups

Each enabled render group with an effect owns:

- one effect instance
- one target canvas or direct surface
- one spatial layout contract
- one role in scene composition

Render-group roles remain:

- `Primary`: the scene-wide effect target for the main LED composition path
- `Custom`: additional LED composition layers or scoped groups
- `Display`: direct-canvas groups targeting display devices

### 3.4 Canonical outputs

A frame may produce all of the following:

1. **LED output**
   - authoritative for physical devices
   - derived from spatial sampling semantics, not from preview shortcuts

2. **Canonical scene canvas**
   - authoritative preview of the active scene's shared LED composition
   - consumed by UI preview, websocket preview streams, and any future scene-
     level display/export paths

3. **Direct display canvases**
   - authoritative for `Display` groups
   - bypass LED sampling and are delivered to display workers as direct group
     outputs

Display groups do not participate in LED composition unless a future spec says
they do. LED preview and LED hardware output must continue to exclude display-
only groups.

---

## 4. Composition Semantics

### 4.1 Single-group fast path

When exactly one non-display group already matches the preview extent, the
pipeline may render directly into the preview surface and reuse that surface as
the scene canvas, provided the resulting LED sampling semantics are unchanged.

This is an optimization, not a semantic fork.

### 4.2 Multi-group scenes

For multi-group LED scenes, the canonical scene canvas must represent the same
spatial intent as the LED path. Preview shortcuts that ignore zone rotation,
edge behavior, or sampling semantics are not canonical.

Acceptable implementations include:

- a shared scene compositor that uses the same transform rules as the sampler
- a scene-canvas reconstruction path derived from the same prepared sampling
  plans used for LED output
- a future GPU path that produces both preview and LED samples from the same
  composition graph

What is not acceptable as the final state:

- a preview path whose visible output disagrees with hardware for the same
  active scene

### 4.3 LED sampling backends

Nearest, bilinear, area-average, and Gaussian-area sampling are canonical LED
sampling modes. The CPU/prepared spatial sampler implements all four and is the
semantic source of truth.

SparkleFlinger's GPU LED sampler currently accepts nearest, bilinear, and
area-average prepared plans. Gaussian-area plans must not be silently lowered to
bilinear or area-average on the GPU path; they fall back to the CPU sampler so
the Gaussian kernel remains real.

### 4.4 Direct display groups

Display groups publish their own canvases at device-appropriate dimensions and
cadence. They are siblings of the scene canvas, not layers composited into it.

### 4.5 Color and output policy

The canonical scene canvas is non-premultiplied sRGB RGBA. It is the shared
visual artifact for preview and display-adjacent consumers, but the LED
hardware path must decode sampled sRGB values into linear light before applying
LED output compensation, brightness policy, and transport encoding. Preview
consumers may apply UI-only presentation transforms, but those transforms must
not feed back into hardware output.

Display-face canvases follow the same non-premultiplied sRGB RGBA convention
until a device worker performs a device-specific transform such as viewport
sampling, circular masking, JPEG encoding, or USB packetization. Display output
brightness is an LCD policy: it scales encoded sRGB bytes before JPEG output
so display previews and physical LCDs stay predictable. LED brightness is a
separate hardware-output policy that decodes sampled sRGB into linear light
before applying device/output shaping. Display output may run at a different
cadence than LED output, but it must yield to overdue LED frames on shared
device transports.

Any future renderer, compositor, or display worker should document the exact
boundary where it changes color space, alpha representation, compression, or
transport ownership. Silent in-place mutation of a canonical surface is outside
the architecture.

---

## 5. Invalidation Rules

Any change that can affect the meaning of the active render groups must
invalidate all caches derived from them. That includes more than scene edits.

### 5.1 Active-scene dependency invalidation

The render pipeline must invalidate active-group derived caches when any of the
following change:

- the active scene or its groups
- group controls, bindings, or layouts
- effect registry entries referenced by active groups
- display-target routing data that changes direct-output policy

This invalidation must cover at least:

- live effect instances
- cached capture demand
- retained scene frames whose semantics are no longer valid
- any routing cache keyed by active-group topology

In practice, the active-scene path now expresses this through explicit
dependency tokens instead of scattered revision pairs:

- `SceneDependencyKey` for render-thread caches derived from active groups and
  effect-registry semantics
- `DisplayTargetDependencyKey` for display-output routing caches derived from
  device-registry state and live display-face routing

Effect-registry semantic invalidation should likewise flow through explicit
mutation surfaces such as register, remove, and `update`. Legacy raw mutable
access may remain for compatibility, but it is not the semantic invalidation
contract.

### 5.2 Hot reload contract

If an effect file is rescanned, reloaded, or replaced and that effect is active
in the current scene, the next frame must observe the updated registry entry
without requiring the user to reapply the scene or effect manually.

### 5.3 Capture demand contract

Audio and screen capture demand are properties of the active scene's effective
metadata, not of stale cached metadata. Registry reloads that flip
`audio_reactive` or `screen_reactive` must take effect on the live pipeline.

---

## 6. Explainability Contract

The architecture is explainable if a contributor can tell the story in one pass:

1. Shared inputs are sampled once.
2. Active render groups render independently.
3. The pipeline emits LED output, a canonical scene canvas, and any direct
   display canvases.
4. All caches invalidate whenever active-group semantics change.

If a subsystem cannot be described as part of that story, it is either a local
optimization or a design smell.

---

## 7. Simplification Targets

The following simplifications are explicitly in scope for future work:

### 7.1 One canonical scene compositor

Preview and hardware should share one composition contract. Different fast
paths are fine; different semantics are not.

### 7.2 One frame-policy surface

Adaptive FPS, frame admission, retained-scene reuse, and idle throttling should
eventually read like one policy system rather than several adjacent ones.

### 7.3 One invalidation vocabulary

The code should converge on a small set of clear dependency tokens or revision
markers that explain why a cached frame or renderer is still valid. The render
thread and display-output path now both use named dependency keys, so the
remaining work is to keep shrinking one-off local cache identities toward that
same level of explicitness.

---

## 8. Delivery Status

The core architecture described here is now shipped:

1. Active-scene invalidation observes effect-registry updates.
2. Capture demand observes live registry metadata.
3. Multi-group LED scenes now derive hardware sampling from the canonical
   scene-composition contract instead of a separate per-group LED-only path.
4. Architecture docs and internal vocabulary have been realigned around the
   scene-canvas model.
5. Render-thread and display-output cache validity now hang off explicit
   dependency-key contracts instead of loose revision-field comparisons.
6. Effect-registry semantic invalidation now flows through explicit mutation
   surfaces only.

The remaining work is simplification, not architectural replacement:

1. Keep collapsing pacing, admission, retention, and throttling into one frame-
   policy surface.
2. Keep converging the remaining local cache identities on the same explicit
   dependency-token style.
3. Keep shrinking compatibility aliases and stale terminology where they no
   longer describe real runtime behavior, without reintroducing duplicate scene
   and preview execution paths.

### 8.1 Soak contract

Pipeline changes are not complete until they survive an end-to-end soak that
exercises all active lanes together:

1. Servo HTML LED effect plus at least one Servo display face.
2. Audio and screen capture demand toggled while the scene remains active.
3. LED output, display output, preview websocket metrics, and device metrics
   subscribed simultaneously.
4. At least one shared USB transport carrying both LED and display traffic.
5. Runtime checks for frame-budget misses, queue drops, copy counters, Servo
   lifecycle waits, USB display-delay counters, and async output failures.

The acceptance bar is not merely "no crash." A passing soak must show bounded
queues, latest-value backpressure instead of unbounded buffering, no sustained
adaptive-FPS collapse, no repeated identical failure spam, and no unexplained
copy-counter growth after warmup.

See `docs/development/GRAPHICS_PIPELINE_SOAK.md` for the repeatable 30-minute
runbook and report locations.

---

## 9. Recommendation

Treat the render-group scene pipeline as the permanent architecture. Close the
remaining coherence gaps by hardening invalidation first, then unify preview
composition semantics so the system has one explainable story from input to
output.
