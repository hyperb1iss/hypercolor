# Spec 41 — Virtual Display Simulator

> A daemon-owned software display device that can be added to layouts,
> receive the real post-viewport and post-overlay display output path, and be
> inspected visually in browser and TUI without requiring physical LCD
> hardware.

**Status:** Draft
**Author:** Nova
**Date:** 2026-04-11
**Crates:** `hypercolor-types`, `hypercolor-daemon`, `hypercolor-tui`
**Depends on:** Spatial Layout (06), Display Output (10 §display), Render Surface Queue (36),
Display Overlay System (40)
**Related:** `docs/specs/31-effect-developer-experience.md`,
`docs/design/06-personas-workflows.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Approach Options](#4-approach-options)
5. [Recommended Architecture](#5-recommended-architecture)
6. [Types and Persistence](#6-types-and-persistence)
7. [API and Preview Surface](#7-api-and-preview-surface)
8. [Delivery Waves](#8-delivery-waves)
9. [Verification Strategy](#9-verification-strategy)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

Hypercolor already has two useful visual surfaces:

- the **global canvas preview**, which shows the effect renderer output
- the **physical display output path**, which crops the canvas through a
  display viewport, applies overlays, brightness, and transport-specific
  encoding

What is missing is a way to inspect the **final per-display result** without
owning the target hardware. That gap matters for overlay development, layout
editing, CI coverage, contributor onboarding, and day-to-day iteration when a
Corsair pump LCD or Push 2 is not plugged in.

This spec introduces a **virtual display simulator**: a software-only display
device that registers with the daemon like a normal display-capable device,
can be placed in a layout like any other display, and receives frames through
the real display-output pipeline. Instead of sending bytes to USB or the
network, the simulator captures the composed frame into a daemon runtime store
that browser and TUI surfaces can inspect.

The key invariant is simple:

> If a simulator frame looks right, it has passed through the same viewport,
> overlay compositor, and brightness path that a real display would use.

That makes the simulator materially more valuable than a generic canvas
preview.

---

## 2. Problem Statement

### 2.1 The Current Preview Is Upstream of Display Output

`/preview`, WebSocket canvas streaming, and the TUI preview all show the main
effect canvas. They do **not** show:

- display-specific viewport crops
- per-display overlay stacks
- circular display masks
- per-device display brightness
- display-worker caching and refresh behavior

That means Spec 40's overlay work can be tested deeply in unit and integration
tests, but visual validation still depends on a physical LCD.

### 2.2 Layout Editing Needs a Display-Attached Inspection Target

The user request here is not "give me another preview window." The useful
workflow is:

1. create a display-like thing
2. add it to the spatial layout
3. bind overlays or display zones to it
4. inspect that specific rendered output visually

That implies the simulator must behave like a real display device in the
layout model, not like a UI-only crop widget.

### 2.3 CI and Contributor Experience Need Hardware-Free Coverage

Our own design notes already call out device simulation as a core developer
experience gap. Contributors should be able to validate display routing,
overlay composition, and preview behavior without owning a Corsair or Push 2.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- Register one or more software display devices with width, height, and
  circular metadata
- Allow those simulated displays to participate in the normal layout system
- Route simulated displays through the real display-output path, including:
  viewport sampling, overlays, brightness, and stable-frame skipping
- Expose the simulator's latest frame for visual inspection in browser and
  eventually TUI
- Make simulator-backed verification scriptable for CI and local development

### 3.2 Non-Goals

- Full USB/HID or UDP protocol emulation for every hardware family
- Replacing physical verification for transport-specific bugs, JPEG artifacts,
  firmware quirks, or panel color differences
- Introducing a separate parallel "preview compositor" that can drift from the
  real display pipeline

### 3.3 Explicit Scope Boundary

This spec is about **display-output simulation**, not generic LED-grid
simulation from Spec 31 §7.6. LED-grid preview is useful, but it answers a
different question.

---

## 4. Approach Options

### Option A — UI-Only Crop Preview

Add a preview panel that crops the global canvas according to a selected
display zone.

**Pros**

- cheap to build
- no daemon device model changes

**Cons**

- does not exercise the real display-output pipeline
- misses overlays, display brightness, circular masking, and worker behavior
- cannot be added to a layout as a real target

### Option B — Daemon-Owned Virtual Display Device

Register a synthetic display device in the daemon, route it through the real
display-output path, and capture the final frame in-memory for visual
inspection.

**Pros**

- exercises the correct pipeline
- addable to layouts like a real display
- useful for overlays, CI, and browser/TUI inspection
- future-proof for multiple simulated displays

**Cons**

- requires daemon device registration and persistence work
- needs a runtime frame store and preview surface

### Option C — Full Protocol Simulator Framework

Build a framework that emulates hardware protocols and panel behavior.

**Pros**

- ideal for backend-driver work
- can power protocol CI later

**Cons**

- far too large for the immediate need
- delays the useful overlay and layout workflow

### Recommendation

Choose **Option B** now. It gives us the real behavior we need without
overcommitting to protocol emulation.

---

## 5. Recommended Architecture

### 5.1 Simulator Is a Real Display Device in the Daemon Model

Each simulator is represented as a synthetic display device with:

- stable `DeviceId`
- `DeviceTopologyHint::Display { width, height, circular }`
- family/vendor values that clearly identify it as simulated
- always-available lifecycle state unless explicitly disabled

This keeps layout, display listing, overlay configuration, and device-target
selection aligned with existing codepaths.

### 5.2 Use a Daemon-Local Simulated Display Backend

The recommended implementation is a daemon-local backend that implements the
existing display write contract but stores the final frame in a
`SimulatedDisplayRuntime` instead of talking to hardware.

This is intentionally stronger than sprinkling special-case branches into the
display worker:

- `DisplayOutputThread` still believes it is writing to a device
- brightness and overlay compose happen unchanged
- simulator and hardware paths stay structurally aligned
- tests can assert on the captured final frame directly

### 5.3 Runtime Surface Model

Each simulator keeps:

- the latest final display frame after overlays and brightness
- frame metadata: frame number, timestamp, width, height
- optional cached preview encodings for PNG/JPEG if needed later

The stored surface should be **post-compositor, pre-transport** data. That is
the highest-value inspection point:

- transport-independent
- visually faithful
- reusable by browser/TUI preview code

### 5.4 Preview Surface

The simulator preview surface should be separate from the global canvas
preview. The global canvas answers "what did the effect render?" The simulator
surface answers "what would this display actually show?"

Initial browser UX can be as small as:

- a simulator list
- a selected simulator frame canvas
- frame metadata

TUI support can follow later using the same runtime frame source.

---

## 6. Types and Persistence

### 6.1 Simulator Definition

Add a persisted daemon-side simulator definition:

```rust
pub struct SimulatedDisplayConfig {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub enabled: bool,
}
```

### 6.2 Persistence

Persist simulator definitions alongside other daemon-owned user state rather
than inventing a one-off scratch file. They are configuration, not telemetry.

### 6.3 Device Identity

The simulator's `DeviceId` must be stable across restarts so:

- layouts keep their zone bindings
- overlay configurations remain attached
- preview surfaces can be addressed consistently

This should use persisted simulator IDs rather than generated-at-startup IDs.

---

## 7. API and Preview Surface

### 7.1 Management API

Add simulator CRUD endpoints:

- `GET /api/v1/simulators/displays`
- `POST /api/v1/simulators/displays`
- `PATCH /api/v1/simulators/displays/{id}`
- `DELETE /api/v1/simulators/displays/{id}`

These APIs manage the synthetic device definitions, not the live frame data.

### 7.2 Inspection API

Add read-only frame inspection:

- `GET /api/v1/simulators/displays/{id}`
- `GET /api/v1/simulators/displays/{id}/frame`

The initial frame route can return PNG for simplicity. WebSocket streaming can
arrive in a later wave if needed.

### 7.3 Preview Page

Extend the existing preview surface rather than building a second throwaway
page:

- `/preview` gains a mode switch between canvas and simulator
- or `/preview?display=<simulator-id>` renders the selected simulator frame

This keeps the preview shell familiar and avoids needless UI sprawl.

### 7.4 TUI

TUI support should consume the same simulator runtime and render the selected
simulator frame with the existing preview machinery. This is a follow-on wave,
not a blocker for daemon usefulness.

---

## 8. Delivery Waves

### Wave 0 — Spec and Constraints

**Goal:** Lock the architecture before implementation drift starts.

- Decide simulator = synthetic display device, not crop-only widget
- Decide daemon-local backend over ad hoc worker branching
- Define persistence and API surface

**Exit criteria:** This spec is accepted as the implementation reference.

### Wave 1 — Daemon Simulator Registry and Backend

**Goal:** Make simulated displays exist as routable devices.

- Persist `SimulatedDisplayConfig`
- Register simulator devices during daemon startup
- Add daemon-local simulated display backend
- Keep simulator lifecycle visible in `GET /api/v1/devices` and `GET /api/v1/displays`

**Exit criteria:** A simulator appears as a display device and can be targeted
by layouts and overlays.

### Wave 2 — Runtime Frame Capture and REST Inspection

**Goal:** Make the simulator visually inspectable.

- Capture final display frames in `SimulatedDisplayRuntime`
- Add frame inspection routes
- Extend `/preview` to display simulator output

**Exit criteria:** A user can create a simulated display, assign it in a
layout, and inspect the final rendered result in the browser.

### Wave 3 — TUI Integration

**Goal:** Inspect simulator frames in terminal workflows.

- Add simulator selection in the TUI preview surface
- Reuse existing fast preview rendering path where possible

**Exit criteria:** A developer can inspect a simulated display from `hyper tui`
without needing a browser.

### Wave 4 — CI and Developer Tooling

**Goal:** Make simulator workflows scriptable and repeatable.

- `just simulator-demo` or equivalent helper
- test helpers that seed a simulator, apply an effect/overlay, and fetch the
  rendered frame
- optional snapshot-style integration tests around simulator frame output

**Exit criteria:** Overlay and display workflows can be exercised in CI without
physical LCD hardware.

---

## 9. Verification Strategy

### 9.1 Daemon Correctness

- simulator CRUD tests
- startup persistence tests
- `GET /api/v1/displays` includes simulated displays
- layout binding works with simulator device IDs

### 9.2 Pipeline Correctness

- simulator receives display-output frames through the normal path
- overlay composition is visible in simulator output
- brightness affects simulator frames exactly as it affects hardware frames
- stable-frame skipping still prevents redundant simulator updates

### 9.3 Visual Inspection

- browser preview renders simulator frames at correct aspect ratio
- circular simulators are visibly masked as circular
- multiple simulators can coexist without frame mix-ups

### 9.4 Honest Boundary

Simulator verification reduces hardware dependence, but it does **not** replace
real-device checks for:

- transport encoding artifacts
- firmware protocol quirks
- panel-specific color and contrast behavior

Physical verification remains part of the release bar for display features.

---

## 10. Recommendation

Build the simulator as a **virtual display device**, not as a cropped preview
panel.

That choice keeps the architecture honest: the feature exists to validate what
the display worker would actually show, including overlays, brightness, and
viewporting. A UI-only crop tool would be quick, but it would answer the wrong
question.

The best first implementation slice is:

1. persisted simulated display definitions
2. daemon-local simulated display backend
3. runtime frame capture
4. browser inspection via the existing preview shell

That gives Hypercolor a practical "software LCD" for layout and overlay work,
while preserving room for later TUI inspection and broader device-simulator
frameworks.
