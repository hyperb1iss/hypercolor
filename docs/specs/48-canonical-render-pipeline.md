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

### 4.3 Direct display groups

Display groups publish their own canvases at device-appropriate dimensions and
cadence. They are siblings of the scene canvas, not layers composited into it.

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
markers that explain why a cached frame or renderer is still valid.

---

## 8. Delivery Status

The core architecture described here is now shipped:

1. Active-scene invalidation observes effect-registry updates.
2. Capture demand observes live registry metadata.
3. Multi-group LED scenes now derive hardware sampling from the canonical
   scene-composition contract instead of a separate per-group LED-only path.
4. Architecture docs and internal vocabulary have been realigned around the
   scene-canvas model.

The remaining work is simplification, not architectural replacement:

1. Keep collapsing pacing, admission, retention, and throttling into one frame-
   policy surface.
2. Keep converging invalidation on a small set of explainable dependency
   tokens.
3. Remove leftover duplicate preview-only helpers and stale terminology where
   they no longer describe real runtime behavior.

---

## 9. Recommendation

Treat the render-group scene pipeline as the permanent architecture. Close the
remaining coherence gaps by hardening invalidation first, then unify preview
composition semantics so the system has one explainable story from input to
output.
