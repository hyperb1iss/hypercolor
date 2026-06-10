# Spec 69: World-Class Display Faces

**Status:** Draft
**Date:** 2026-06-09
**Builds on:** Spec 42 (display faces pipeline), Spec 43 (face SDK)
**Research:** `docs/research/faces/00-synthesis.md` (cross-model reviewed, all
findings folded in) + five competitive research docs in the same directory

## 1. Overview

The display-face pipeline (spec 42) and face SDK (spec 43) shipped a solid
foundation: multi-session Servo rendering at native display resolution, an
11-blend-mode GPU compositor, per-display assignment with REST/MCP/UI, and a
declarative `face()` authoring API. What never shipped is the layer that makes
faces *good*: display identity reaching the face, layout that adapts to
display shape, a motion system, richer data sources, and faces that use any
of it.

This spec takes faces from "static 480×480 dashboards that shrink to a
postage stamp on Push 2" to world-class: shape-adaptive, animated, deeply
configurable, and fed by data sources no competitor on Linux has. It is
organized as six delivery waves, each independently shippable.

### Goals

1. Every face renders beautifully on every display shape we drive — round
   480×480 Corsair LCDs and the 960×160 Push 2 strip are the two canonical
   targets; new shapes must not require face rewrites.
2. Faces move: eased value transitions, sweeping gauges, ambient layers,
   lifecycle transitions — designed for 15–30fps grace.
3. Faces are configurable end-to-end: typed data sources bound to faces,
   live controls, presets, per-display persistence across scene switches.
4. New flagship faces (now-playing, spectrum) that exercise the platform and
   beat SignalRGB/iCUE/NZXT feature-for-feature on Linux.
5. Authoring stays a joy: hardware-free dev loop on simulator displays,
   documented SDK, fast iteration.

### Non-Goals

- Touch/gesture interaction on faces (no current hardware exposes touch).
- A visual face *editor* (slot binding UI is configuration, not authoring).
- Community marketplace/gallery infrastructure (packaging format is wave 5;
  the storefront is future work).
- Hardware-fallback faces flashed to device memory (noted as future work;
  Corsair LCDs support stored GIFs).
- New display hardware support (BeadaPanel/Turing panels are driver work,
  tracked separately).

## 2. Current State (abridged)

See the research synthesis for the full picture. The load-bearing facts:

- `FaceContext` gets real viewport dims but computes a uniform
  `scale = min(w/480, h/480)` from a hardcoded design basis; on Push 2 every
  face renders as a ~160×160 square centered in the strip
  (`sdk/packages/core/src/faces/face-fn.ts:147-188`).
- The daemon centralizes only `DisplaySurfaceInfo { width, height, circular }`
  (`crates/hypercolor-daemon/src/api/displays.rs:757`); nothing is injected
  into the face page. `FaceContext.circular` is the author's declaration, not
  device truth.
- No motion utilities exist in the SDK; faces ad-hoc lerp at best.
- Audio injection exists but gates on metadata `face()` cannot emit
  (`effect_is_audio_reactive`,
  `crates/hypercolor-core/src/effect/servo/worker/runtime_html.rs:10`).
- Faces already run 30fps on the group-direct path
  (`DISPLAY_FACE_DEFAULT_FPS`,
  `crates/hypercolor-daemon/src/display_output/mod.rs:43`); the cap is not
  configurable and there is no 60fps upshift.
- Face assignment mutates the active scene's zones; switching scenes drops
  faces.
- Data sources: sensors + audio + time. No media, network, or lighting-state.

## 3. Architecture

### 3.1 DisplayDescriptor (types → core → daemon → Servo → SDK)

A single shared type describing a display surface, computed once per device
and injected into the face page before boot.

```rust
// hypercolor-types/src/display.rs (new)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayDescriptor {
    pub api_version: u32,            // 1
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub shape: DisplayShape,         // derived, see below
    pub class: DisplayClass,         // PumpLcd | Strip | Panel
    pub safe_area: DisplayRect,      // largest unclipped rect
    pub target_fps: u32,
    pub pixel_format: DisplayPixelFormat, // Rgb | Yuv420
}

#[derive(...)]
#[serde(rename_all = "snake_case")]
pub enum DisplayShape { Round, Square, Wide, Tall }
```

Derivation rules (pure function, unit-tested):

- `shape`: `Round` if `circular`; else `Wide` if `width/height >= 2.0`;
  `Tall` if `<= 0.5`; else `Square`.
- `safe_area`: for round, the inscribed square (`side = d / sqrt(2)`,
  centered); otherwise the full rect.
- `class`: from the driver's topology hint (`PumpLcd` for Corsair LCD
  family, `Strip` for Push 2); defaults derived from shape when a driver
  doesn't declare one.

Flow: HAL topology hints → `DisplayDescriptor::from_surface(...)` in core →
daemon's `display_surface_info()` refactors onto it → the zone runtime that
owns `display_target` hands it to the face renderer at load → bootstrap
script emits it (only for `ServoProducerRole::DisplayFaceHtml`):

```js
window.hypercolor = window.hypercolor || {};
window.hypercolor.display = { apiVersion: 1, width, height, circular,
  shape, class, safeArea: {x, y, width, height}, targetFps, pixelFormat };
```

The descriptor is versioned (`apiVersion`) and additive-only, NZXT
`window.nzxt.v1` style. The SDK consumes it when present and falls back to
author declarations + viewport measurement when absent (faces keep working
in a bare browser during authoring).

### 3.2 SDK layout system (JS baseline, CSS as enhancement)

`FaceContext` v2 adds device truth and layout helpers:

```typescript
interface FaceContext {
  // existing: container, canvas, ctx, width, height, scale, dpr
  display: {
    shape: 'round' | 'square' | 'wide' | 'tall'
    class: 'pump-lcd' | 'strip' | 'panel'
    circular: boolean          // device truth when descriptor present
    aspect: number
    safeArea: { x: number; y: number; width: number; height: number }
  }
}
```

New `layout/` module in `sdk/packages/core/src/`:

- `grid(area, cols, rows, gap)` — cell rects within a safe area (the spec 43
  §7.2 promise, finally).
- `polar(center, radius, angle)` + `ring(area, n)` — placement on round
  displays.
- `rail(area, n, gap)` — horizontal band layout for strips.
- `anchor(area, position, size)` — corner/edge/center anchoring.
- `fitText(ctx, text, rect, opts)` — binary-search font sizing into a rect.

`face()` gains optional per-shape variants; a face provides either a single
responsive update function or per-shape ones:

```typescript
face('Neon Clock', controls, (ctx) => { ... }, {
  variants: {
    wide: (ctx) => { ... },   // chosen when ctx.display.shape === 'wide'
  },
})
```

Resolution order: exact shape variant → base function. Typography scales by
`min(width, height)` (not the old uniform basis scale). All layout math is
plain JS over the descriptor — **no reliance on CSS media queries**. CSS
grid/flex may be adopted per-feature after the wave-0 pixel probes prove
Servo coverage.

### 3.3 Motion module

New `motion/` module in `sdk/packages/core/src/`:

- Easings: `linear`, `easeOutCubic`, `easeInOutCubic`, `easeOutElastic`,
  `easeOutBack`.
- `tween(from, to, duration, easing)` — time-based track with `.at(t)`.
- `Smoothed(value, halflife)` — frame-rate-independent exponential smoothing
  (replaces ad-hoc `+= delta * 0.08` lerps, which are fps-dependent).
- `spring(stiffness, damping)` — for organic gauge motion.
- `Timeline` — staggered entrance sequencing.
- `transitionOnChange(key, duration)` — eases control/preset changes instead
  of jumping.

Gauges accept motion options (`animate: { easing, duration }`) so every face
gets eased sweeps for free. All motion is computed from `performance.now()`
deltas — correct at 15, 30, or 60fps. Design guidance: slow eased motion, no
fast travel (strobes at 15fps); document in the authoring guide.

### 3.4 Audio opt-in for faces

`face()` options gain `audio: true`, which emits audio-reactive metadata in
the built HTML (tag `audio-reactive`), satisfying `effect_is_audio_reactive`
so the renderer injects `engine.audio` (mel bands, chromagram, flux, levels).
The SDK exposes a typed `AudioAccessor` mirroring `SensorAccessor`.

### 3.5 Typed data-source contract (the slot foundation)

Wave 3 introduces typed, cadenced data sources alongside `sensors` and
`audio` — the contract that the wave-4 binding UI and future community faces
build on. Each source declares: kind, payload type, and update cadence.

| Source | Payload | Cadence | Transport |
|---|---|---|---|
| `media` | `{ playing, track, artist, album, artDataUrl, positionMs, durationMs }` | on change (art only on track change) | MPRIS via zbus (Linux) |
| `net` | `{ rxBps, txBps, iface }` | 1 Hz | sysinfo networks |
| `lighting` | `{ sceneName, effectNames, dominantColors }` | on change | engine state |

Injected as `engine.media`, `engine.net`, `engine.lighting` by the existing
LightScript injection path (same mechanism as `engine.sensors`). Album art
travels as a data URL, re-sent only on track change to keep frame scripts
small. SDK exposes typed accessors. Faces bind these directly in wave 3; the
binding *UI* (users re-pointing slots) is wave 4.

### 3.6 Per-display persistence (model change — semantics settled here)

New daemon-level store: `display_preferences` (persisted alongside profiles),
keyed by device fingerprint:

```
{ effect_id, controls, blend_mode, opacity }
```

**Precedence:** an active scene's `display_target` zone always wins. When no
active-scene zone targets a display, its stored default applies (the daemon
spawns an implicit display zone at activation — runtime-only, never written
back into the scene).

**Scene switches:** defaults survive. Activating a scene with no display
zone for device X keeps X's default face running.

**Reconnect:** same resolution order on device connect (scene zone →
default → nothing).

**API:** `PUT /api/v1/displays/{id}/face` gains `scope: "default" | "scene"`.
Default is `"default"` — assigning a face from the Displays page now sticks
across scenes, which is what users expect. Scene-scoped assignment remains
available (`scope: "scene"` preserves today's behavior and is what the
scene/zone editors use). `DELETE` takes the same scope; clearing the default
while a scene override is active changes nothing visibly until scene switch.
`GET` reports both layers plus which one is live.

**Controls:** live in the preference record for defaults; in the zone for
scene overrides. `PATCH .../face/controls` targets whichever layer is live.

### 3.7 FPS policy

`face_fps_cap` joins daemon config (default 30, max 60). The group-direct
path in `capped_display_target_fps` consults it; devices whose transport
sustains more than 30fps (Push 2 raw path) may be upshifted to 60 where
measured headroom exists. Per the performance-baseline contract: ceilings
are raised or made configurable, never lowered.

## 4. Wave 0 — Platform Plumbing

Foundation work in types/core/daemon. Tasks W0.1–W0.3 are parallel-safe
(disjoint files); W0.4–W0.6 follow.

### W0.1 DisplayDescriptor type + derivation

**Files:** `crates/hypercolor-types/src/display.rs` (new),
`crates/hypercolor-types/src/lib.rs`,
`crates/hypercolor-types/tests/display_descriptor_tests.rs` (new)
**Depends on:** —  **Parallel:** yes

- Define `DisplayDescriptor`, `DisplayShape`, `DisplayClass`,
  `DisplayRect`, `DisplayPixelFormat` with serde (`snake_case`).
- Pure derivation: `DisplayDescriptor::derive(width, height, circular,
  class_hint, target_fps, pixel_format)` implementing §3.1 rules.
- Unit tests: 480×480 round → Round + inscribed safe area; 960×160 →
  Wide + full rect; 240×240 non-circular → Square; tall case; serde
  round-trip.

**Verify:** `just test-crate hypercolor-types` green; `just verify` clean.

### W0.2 MPRIS media input source

**Files:** `crates/hypercolor-types/src/media.rs` (new),
`crates/hypercolor-core/src/input/media.rs` (new),
`crates/hypercolor-core/src/input/mod.rs`,
`crates/hypercolor-core/tests/media_input_tests.rs` (new)
**Depends on:** —  **Parallel:** yes

- `MediaState` type per §3.5; `MediaSource` implementing `InputSource`,
  using the existing workspace `zbus` dep to watch
  `org.mpris.MediaPlayer2.*` (active-player pick: playing > paused >
  most-recent).
- Album art: resolve `mpris:artUrl` (file:// and http(s)) to a bounded
  (≤256×256, JPEG) data URL, refreshed only on track change. Non-Linux:
  source reports unavailable (same pattern as other Linux-only inputs).
- Tests cover the pure parts: player-pick policy, art-refresh gating, state
  diffing (zbus interactions behind a trait so tests don't need a bus).

**Verify:** `just test-crate hypercolor-core` green; manual receipt —
daemon log shows track changes while `playerctl play/pause` toggles.

### W0.3 SDK audio opt-in for faces

**Files:** `sdk/packages/core/src/faces/face-fn.ts`,
`sdk/packages/core/src/faces/context.ts`,
`sdk/packages/core/tests/face-fn.test.ts`
**Depends on:** —  **Parallel:** yes

- `FaceOptions.audio?: boolean` → built HTML emits the `audio-reactive`
  tag meta. Typed `AudioAccessor` passed to update functions (no-op object
  when audio absent).
- Test: built metadata includes the tag iff `audio: true`; accessor shape.

**Verify:** `cd sdk && bun test` green; `just face-build neon-clock`
artifact diff shows no tag (control), a probe face with `audio: true`
shows the tag.

### W0.4 Descriptor plumbing into Servo + bootstrap injection

**Files:** `crates/hypercolor-core/src/effect/servo/renderer.rs`,
`crates/hypercolor-core/src/effect/servo/renderer/load.rs`,
`crates/hypercolor-core/src/effect/servo/renderer/frame_queue.rs`,
`crates/hypercolor-core/src/effect/traits.rs` (load-params surface),
`crates/hypercolor-daemon/src/render_thread/display_lane.rs` (descriptor
hand-off where `display_target` zones build face renderers),
`crates/hypercolor-daemon/src/api/displays.rs` (refactor
`display_surface_info()` onto the shared type),
`crates/hypercolor-core/src/effect/servo/renderer/tests.rs`
**Depends on:** W0.1

- Add `set_display_descriptor(Option<DisplayDescriptor>)` to the face
  renderer path; daemon sets it when building a display-face zone runtime.
- `enqueue_bootstrap_scripts()` emits `window.hypercolor.display` (§3.1)
  for `DisplayFaceHtml` producers with a descriptor present.
- Refactor daemon `display_surface_info()` to build descriptors (one
  source of truth for the API response and the injection).

**Verify:** renderer test asserts the bootstrap script contains the JSON
payload for a Wide 960×160 descriptor; `just verify` clean; manual receipt
— assign a probe face that prints `JSON.stringify(window.hypercolor.display)`
on a simulator display, confirm via preview.jpg.

### W0.5 Servo CSS coverage probes (pixel tests)

**Files:** `crates/hypercolor-core/tests/servo_css_probe_tests.rs` (new,
behind the `servo` feature), probe HTML fixtures under
`crates/hypercolor-core/tests/fixtures/css-probes/` (new)
**Depends on:** —  **Parallel:** yes (can land any time in wave 0)

- Probe pages exercising: flexbox row/column, CSS grid, `clip-path:
  circle()`, aspect-ratio media queries, transforms. Each paints a known
  color into a known quadrant iff the feature works; the test renders at
  480×480 and 960×160 and samples pixels.
- Output: a documented support matrix in the test file header — this gates
  which CSS the SDK layout module may rely on.

**Verify:** `just test-crate hypercolor-core` (servo feature) green;
matrix documented.

### W0.6 Configurable face FPS cap

**Files:** `crates/hypercolor-daemon/src/display_output/mod.rs`,
daemon config (`crates/hypercolor-daemon/src/config.rs` or equivalent),
`crates/hypercolor-daemon/tests/display_output_tests.rs`
**Depends on:** —  **Parallel:** yes

- `display.face_fps_cap` config (default 30, clamp 15..=60);
  `capped_display_target_fps` consults it on the group-direct path.
  Device transport limit still wins (`min`).
- Tests: default unchanged at 30; configured 60 honored when device
  allows; never below existing defaults.

**Verify:** `just test-crate hypercolor-daemon` green; baseline guard —
no default value decreases.

## 5. Wave 1 — SDK Motion + Layout

All SDK work; W1.1 and W1.2 are parallel (disjoint modules), W1.3–W1.5
build on them.

### W1.1 Motion module

**Files:** `sdk/packages/core/src/motion/` (new: `easing.ts`, `tween.ts`,
`smoothed.ts`, `spring.ts`, `timeline.ts`, `index.ts`),
`sdk/packages/core/src/index.ts`,
`sdk/packages/core/tests/motion.test.ts` (new)
**Depends on:** —  **Parallel:** yes

- Implement §3.3. Everything pure and frame-rate independent (`dt`-based);
  `Smoothed` uses half-life math, not per-frame factors.
- Tests: easing endpoints/monotonicity, tween timing, smoothed convergence
  identical across simulated 15fps vs 60fps step sequences, timeline order.

**Verify:** `cd sdk && bun test motion` green.

### W1.2 Layout module + descriptor-aware FaceContext

**Files:** `sdk/packages/core/src/layout/` (new: `grid.ts`, `polar.ts`,
`rail.ts`, `anchor.ts`, `fit-text.ts`, `index.ts`),
`sdk/packages/core/src/faces/context.ts`,
`sdk/packages/core/src/faces/face-fn.ts`,
`sdk/packages/core/tests/layout.test.ts` (new),
`sdk/packages/core/tests/face-context.test.ts` (new)
**Depends on:** —  **Parallel:** yes

- Implement §3.2: `FaceContext.display` populated from
  `window.hypercolor.display` when present, else derived from viewport +
  author options (fallback keeps bare-browser authoring alive).
- Layout helpers per §3.2; typography scale switches to
  `min(width, height)` basis.
- `variants` resolution in `face()` (exact shape → base).
- Tests: descriptor consumption, fallback derivation, grid/polar/rail
  math, fitText convergence, variant selection for all four shapes.

**Verify:** `cd sdk && bun test layout face-context` green.

### W1.3 Animated gauges

**Files:** `sdk/packages/core/src/gauges/{arc,bar,ring,sparkline}.ts`,
`sdk/packages/core/tests/gauges.test.ts`
**Depends on:** W1.1

- Gauges accept `animate?: { easing, duration }` and an optional persistent
  handle (`createArcGauge(...)`) that owns its `Smoothed`/tween state;
  one-shot functional API stays for static draws.
- Sparkline learns threshold bands (color zones) and animated draw-in.

**Verify:** `bun test gauges` green; visual receipt on simulator in W2.

### W1.4 Face component library

**Files:** `sdk/src/faces/shared/components.ts` (new — promoted to
`sdk/packages/core/src/faces/components/` if a third consumer appears),
`sdk/packages/core/tests/face-components.test.ts` (new)
**Depends on:** W1.1, W1.2

- `MetricCard`, `Readout`, `ChartPanel`, `ProgressBar` — DOM builders
  owning their own update/animation state, styled from face tokens,
  laid out via the layout module.

**Verify:** `bun test face-components` green.

### W1.5 Authoring dev loop

**Files:** `justfile`, `sdk/package.json`, small glue script under
`sdk/scripts/` (new)
**Depends on:** W1.2

- `just face-dev NAME`: builds the face on change, installs into the
  daemon effects dir, ensures two simulator displays exist (480×480
  round, 960×160), assigns the face to both, opens the Displays page.
  Iteration loop target: save → preview refresh in under 5 seconds.

**Verify:** manual receipt — run it, edit a color, watch both previews
update; document the loop in the spec PR.

## 6. Wave 2 — Face Glow-Up

One task per face; all four parallel (disjoint files). Shared quality gate:

> **Gate:** the face must look intentional and gorgeous on BOTH canonical
> displays — round 480×480 and 960×160 strip — verified by side-by-side
> simulator screenshots attached to the PR; all motion eased; no layout
> overflow at either size; `bun test` + `just verify` green.

### W2.1 neon-clock

**Files:** `sdk/src/faces/neon-clock/main.ts`  **Depends on:** W1.3, W1.4

- Digit transitions (slide/fade morph on change), dial gains an eased
  sweep-second indicator, ambient glow breathes subtly.
- Wide variant: single-row time + date layout across the strip, no dial.

### W2.2 pulse-temp

**Files:** `sdk/src/faces/pulse-temp/main.ts`  **Depends on:** W1.3, W1.4

- Render the 48-sample history as a threshold-banded sparkline (finally).
- Arc sweeps on appear and eases between values; trend state
  (rising/cooling/steady) drives accent shifts and a pulse on threshold
  crossings.
- Wide variant: left readout + full-width sparkline rail.

### W2.3 sensor-grid

**Files:** `sdk/src/faces/sensor-grid/main.ts`  **Depends on:** W1.3, W1.4

- Rebuild on `MetricCard` components over `grid()`/`rail()`: 2×2 on
  square/round (safe-area aware), 4×1 rail on wide.
- Animated bars, per-card sparkline option, staggered entrance.

### W2.4 silkcircuit-hud

**Files:** `sdk/src/faces/silkcircuit-hud/main.ts`  **Depends on:** W1.3, W1.4

- Motion pass: animated bars, eased metric changes, layered canvas
  background (subtle grid/flow tied to SilkCircuit palette).
- Wide variant: clock left, metrics right in a rail.

## 7. Wave 3 — Typed Data Contract + Flagship Faces

Contract first (W3.1–W3.2), then flagships in parallel.

### W3.1 Data source injection: media, net, lighting

**Files:** `crates/hypercolor-core/src/effect/lightscript.rs`,
`crates/hypercolor-core/src/input/mod.rs` (net source),
`crates/hypercolor-daemon/src/render_thread/` (lighting-state feed),
`crates/hypercolor-core/tests/lightscript_injection_tests.rs`
**Depends on:** W0.2

- Inject `engine.media` (on change; art data URL only on track change),
  `engine.net` (1 Hz), `engine.lighting` (scene name, effect names,
  dominant colors from the spatial sampler) per §3.5, gated per-face by
  metadata opt-ins (same pattern as audio: tags `media`, `net`,
  `lighting`).
- Tests: script assembly includes/excludes each source by metadata;
  art-gating behavior.

**Verify:** `just test-crate hypercolor-core` green.

### W3.2 SDK typed accessors + opt-ins

**Files:** `sdk/packages/core/src/faces/context.ts`,
`sdk/packages/core/src/faces/face-fn.ts`,
`sdk/packages/core/tests/face-context.test.ts`
**Depends on:** W3.1

- `face()` options: `media: true`, `net: true`, `lighting: true` → tags.
- `MediaAccessor`, `NetAccessor`, `LightingAccessor` with safe defaults
  when a source is absent.

**Verify:** `bun test` green; metadata tags asserted.

### W3.3 Flagship: now-playing

**Files:** `sdk/src/faces/now-playing/` (new)
**Depends on:** W3.2  **Parallel:** with W3.4–W3.5

- Round: album art center (circular crop), orbiting progress arc,
  track/artist on polar text paths or fitText bands.
- Wide: art square left, scrolling title (marquee with pause-at-ends),
  progress rail bottom. Eased art crossfade on track change; dominant-art
  color accent option.
- Gate: gorgeous on both canonical displays; behaves when no player is
  active (idle state, not a blank).

### W3.4 Flagship: spectrum

**Files:** `sdk/src/faces/spectrum/` (new)
**Depends on:** W3.2 (audio via W0.3)  **Parallel:** yes

- Wide: mel-band bars across the strip (the Push 2 hero view), peak-hold
  caps, SilkCircuit gradient by intensity.
- Round: radial spectrum around center, chromagram color mode option.
- Motion smoothed for 15–30fps; silence state breathes instead of flatlines.

### W3.5 Flagship: system-pulse

**Files:** `sdk/src/faces/system-pulse/` (new)
**Depends on:** W3.2  **Parallel:** yes

- The component-library showcase: clock + configurable metric cards +
  net throughput + sparklines, every element animated, full layout
  adaptivity. This face is the template the authoring guide walks through.

## 8. Wave 4 — Persistence, Slots UI, Config Polish

### W4.1 Per-display persistence

**Files:** `crates/hypercolor-daemon/src/display_preferences.rs` (new),
`crates/hypercolor-daemon/src/api/displays.rs`,
`crates/hypercolor-daemon/src/render_thread/display_lane.rs`,
`crates/hypercolor-daemon/tests/display_preferences_tests.rs` (new)
**Depends on:** wave 0

- Implement §3.6 exactly: store, precedence, implicit runtime zones,
  `scope` parameter on PUT/DELETE, dual-layer GET, control-patch routing.
- Tests: precedence matrix (scene-only, default-only, both, neither),
  scene-switch survival, reconnect resolution, delete semantics.

**Verify:** `just test-crate hypercolor-daemon` green; manual receipt —
assign default face, switch scenes twice, face survives.

### W4.2 Displays page: persistence + binding UX

**Files:** `crates/hypercolor-ui/src/pages/displays.rs`,
`crates/hypercolor-ui/src/components/control_panel/sensor.rs`
**Depends on:** W4.1

- Scope toggle on assignment (default vs this-scene) with live-layer
  indicator; sensor dropdown gains search, grouping, and live current
  values; data-source bindings surfaced as first-class controls.

**Verify:** `just ui-test` + `just ui-build` green (UI crate is outside
the workspace — run explicitly); visual receipt via agent-browser pass on
the Displays page.

### W4.3 MCP + REST parity

**Files:** `crates/hypercolor-daemon/src/mcp/tools/displays.rs`,
`crates/hypercolor-daemon/tests/mcp_display_tests.rs`
**Depends on:** W4.1  **Parallel:** with W4.2

- `set_display_face` gains `scope`; tool description documents precedence;
  responses include the live layer.

**Verify:** `just test-crate hypercolor-daemon` green.

## 9. Wave 5 — Ecosystem

### W5.1 Face authoring guide

**Files:** `docs/content/` (new page), cross-linked from
`.agents/skills/rgb-effect-design/` and the SDK README
**Depends on:** waves 1–3

- End-to-end walkthrough building system-pulse: descriptor, layout
  variants, motion, data sources, controls, presets, `just face-dev`
  loop, the two-display quality gate. Includes the 15–30fps motion
  design guidance and the Servo CSS support matrix from W0.5.

**Verify:** a fresh agent (or human) can build a working face following
only the guide — dogfood test.

### W5.2 Face packaging format

**Files:** `sdk/packages/core/src/` (manifest emit),
`crates/hypercolor-daemon/src/api/` (install endpoint hardening)
**Depends on:** W5.1

- Faces are already single-file HTML with meta manifests; formalize:
  documented meta contract version, `hypercolor face install <file>` CLI
  verb, validation on install (manifest version, size caps, no external
  network refs beyond fonts). Gallery/marketplace stays future work.

**Verify:** install a face exported from another machine; round-trip test.

## 10. Testing Strategy

- **Rust unit/integration:** descriptor derivation (types), bootstrap
  injection content (servo renderer tests), CSS probes (pixel sampling),
  fps cap policy, persistence precedence matrix, injection gating. All via
  `just verify` / `just test-crate X`.
- **SDK (bun test):** motion math (fps-independence is the key invariant),
  layout solvers, context fallback, accessors, metadata emission.
- **Visual:** simulator displays at 480×480-round and 960×160 are the two
  canonical fixtures; every face PR attaches both screenshots (preview.jpg
  endpoint). agent-browser pass over the Displays page for UI waves.
- **Hardware dogfood:** Corsair LCD + Push 2 on the rig before each wave
  ships; watch the known display-pipeline gotchas (finalized-only routed
  faces, alpha sampling) for regressions.
- **Performance:** profile wave 2 on the software-GL path; FPS controller
  telemetry must show no downshift regression with two faces + one LED
  effect active. Baselines never decrease.

## 11. Risks

- **Servo CSS coverage** — mitigated: JS layout is the baseline; W0.5
  probes gate any CSS adoption.
- **Two faces + LED effect at 30fps on software GL** — watch render-loop
  budget during wave 2/3; adaptive FPS exists; if budget misses appear,
  fix root causes (profiling, render path) per the no-nerf rule.
- **MPRIS player diversity** — player-pick policy is the messy part;
  bounded by trait-mocked tests + dogfood with real players (spotify,
  mpv, firefox).
- **Persistence model change** — the precedence matrix in §3.6 is settled
  before code; W4.1's test matrix is the contract.
- **Art data-URL size** — bounded at 256×256 JPEG, sent only on track
  change; measure script-injection cost in W3.1.

## 12. Acceptance (the "stomp" checklist)

1. Any face on Push 2 uses the full strip; any face on Corsair respects the
   circle. No postage stamps, no clipped corners.
2. Every shipped face animates: eased values, sweeps, transitions — at a
   locked 30fps with headroom.
3. `now-playing` shows album art + progress on a pump cap and the Push 2
   strip on Linux — something no competitor can do at any price.
4. Faces survive scene switches and daemon restarts via display defaults.
5. A new face goes from `bun create` to both simulators in under a minute
   via `just face-dev`.
6. All existing gates stay green: `just verify`, `just ui-test`,
   `bun test`, cargo-deny.
