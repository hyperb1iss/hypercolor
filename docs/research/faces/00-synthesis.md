# World-Class Display Faces — Research Synthesis & Plan

**Date:** 2026-06-09
**Inputs:** codebase archaeology (SDK, pipeline, devices) + five competitive research
docs in this directory (`signalrgb.md`, `icue.md`, `nzxt-cam.md`, `watch-faces.md`,
`sensor-dashboards.md`).
**Review:** cross-model reviewed (Codex, xhigh, 2026-06-09) — verdict "go after plan
edits"; all eight findings verified against code and folded in below.

## TL;DR

The Hypercolor face *pipeline* is already best-in-class: multi-session Servo renders
each face at native display resolution, an 11-mode GPU compositor blends faces with
the lighting scene, and per-display assignment ships with REST/MCP/UI. The *faces*
and the *SDK layer above the pipeline* are the weak link: four static dashboards,
no motion system, no layout adaptivity, and the daemon never tells a face what
display it's on. Meanwhile the industry just validated our exact architecture —
Corsair turned iCUE widgets into an HTML platform (May 2026), SignalRGB shipped
"LCD Faces" (June 2026), NZXT has run Chromium-on-LCD for years — and all of them
are Windows-only, shallow on configurability, and treat screens and lighting as
disconnected worlds. The winning move: a display-class-aware layout system + motion
primitives + typed data slots (the watch-industry complication model), MPRIS
now-playing, and faces that blend with the lighting scene — things none of them
can match.

## Where We Stand (codebase findings)

### Strong foundation (keep, build on)

- **Multi-session Servo** (spec 42, shipped): one LED effect + N faces concurrently,
  each face rendered at the display's native resolution. No scaling hacks.
- **GPU display finalize** (`display_finalize.wgsl`): 11 blend modes, circular mask,
  sRGB-correct, YUV420 output, viewport transforms. Faces compose *with* the
  lighting scene — no competitor does this.
- **Assignment model**: `Zone.display_target` per scene; REST
  (`/api/v1/displays/{id}/face` + controls/composition PATCH), MCP
  (`set_display_face`), Displays page UI with live JPEG preview. All shipped.
- **Face SDK** (spec 43, shipped): `face()` declarative API, `FaceContext`
  (width/height/scale/circular/dpr), `SensorAccessor` with formatting,
  `sensor()`/`font()` controls, gauges (arc/bar/ring/sparkline), `ValueHistory`,
  SilkCircuit tokens. Build pipeline via `just faces-build`.
- **Hidden capabilities**: the renderer injects the full audio surface (mel bands,
  chromagram, spectral flux, level/bass) into any page whose metadata is
  audio-reactive (`audio_reactive` flag, `Audio` category, or `audio` tag —
  `effect_is_audio_reactive` in `servo/worker/runtime_html.rs`). Caveat: the
  `face()` SDK API has no way to emit that metadata today (no `audio` option, no
  tags) — small SDK gap to close before any spectrum face. Virtual display
  simulator (`POST /api/v1/simulators/displays`) enables hardware-free authoring.

### Display inventory

| Device | Resolution | Aspect | Shape | Face path FPS |
|---|---|---|---|---|
| Corsair LCD family (5 SKUs) | 480×480 | 1:1 | round | 30 (`DISPLAY_FACE_DEFAULT_FPS`) |
| Ableton Push 2 | 960×160 | 6:1 | wide strip | 30 (raw path also 30) |

The 15fps `DISPLAY_OUTPUT_MAX_FPS` cap applies only to the non-face composed-canvas
JPEG path and previews; group-direct face routing already defaults to 30
(`capped_display_target_fps`, `display_output/mod.rs:1006`).

### The gaps that make faces "super basic"

1. **No display identity reaches the face.** `FaceContext.circular` is the
   *author's* declaration; the daemon knows `DisplaySurfaceInfo { width, height,
   circular }` and never injects it. Spec 43 §7.1 promised "or the daemon reports
   a circular display" — never wired.
2. **Uniform scale, no reflow.** `scale = min(w/480, h/480)` from a hardcoded
   480×480 design basis. On Push 2 every face becomes a 160×160 postage stamp
   centered in a 960-wide strip. Spec 43 §7.2's `grid()` / `circularMask()`
   layout helpers were never built.
3. **Ad hoc smoothing, no animation system.** A couple of faces lerp values
   (`pulse-temp`'s `smoothValue`), but the SDK has no easing/tween/keyframe
   utilities. Gauges fill but never sweep; digits swap with no transition;
   `pulse-temp` records a 48-sample history and never draws the sparkline.
   Canvas layer is cleared and unused in half the faces.
4. **Data ceiling.** Sensors + audio + time only. No media/now-playing (no MPRIS
   source exists in core), no network stats, no lighting-state mirror.
5. **Config UX gaps.** The UI has a real sensor dropdown
   (`control_panel/sensor.rs`) but it's a flat list of well-known labels — no
   search, no live values, no grouping. Face assignment is scene-scoped;
   switching scenes drops faces.
6. **FPS ceiling is fixed, not configurable.** Faces already run 30fps on the
   direct path; there's no user-facing `face_fps_cap` and no 60fps upshift for
   transports that could take it. (Spec 42 §16.1's configurable cap remains
   unbuilt — but the framing is "make it configurable/up-shiftable", not "raise
   from 15".)

### Per-face review

| Face | Quality | Worst gap |
|---|---|---|
| neon-clock | 7/10 | digits swap instantly; dial is static decoration |
| pulse-temp | 7.5/10 | history buffer never rendered; arc doesn't sweep |
| sensor-grid | 6/10 | hardcoded 2×2; CSS-div bars; no motion |
| silkcircuit-hud | 6.5/10 | zero animation of any kind |

## Competitive Landscape (June 2026)

| | SignalRGB | iCUE | NZXT CAM | Hypercolor today |
|---|---|---|---|---|
| Face tech | HTML (Lightscript) | HTML on QtWebEngine | Chromium Web Integration | HTML on Servo |
| Shipped | 2026-06-04 | widgets May 2026 | years | shipped |
| Display info to face | none documented | manifest device targets + slot sizes | `window.nzxt.v1` (w/h/shape/fps) | **none** |
| Data binding | audio + undocumented sensors | sensors (3 max in editor) | 1 Hz monitoring callback | sensors + full audio DSP |
| Now-playing | ✅ launch face (Windows media session) | — | Spotify embed | ❌ |
| Custom faces | Pro paywall (~$45/yr), no docs | marketplace + CLI + AI skill | URL only, no local files | free, open |
| Lighting ↔ screen | separate | separate (Murals never drives LCDs) | RGB ring sync only | **GPU-composited together** |
| Layout editor | none for faces | slot grid (chafing) | none (community built NZXT-ESC) | none |
| Linux | never | never | never (liquidctl static only) | **native** |

**Validated patterns worth stealing**

- **Typed slots / complications** (entire watch industry, iCUE widget slots,
  Stream Deck layouts): data providers declare *what* (typed values), faces
  declare *how each type renders*, users bind sources to slots in an editor.
- **Per-shape layout variants over shared content** (WFF clip-shape + resource
  overlays, Garmin per-shape resources). Nobody does fluid reflow; everybody
  does declared variants. Right answer for round 480×480 vs 6:1 strip.
- **Versioned injected display API** (`window.nzxt.v1`): geometry + shape +
  target fps handed to the page. Ours should also carry safe-area and display
  class.
- **Declarative data-binding with per-source refresh cadence** (WFF expressions,
  Rainmeter measures, Turing theme.yaml): lets the engine budget refresh per
  element — ideal for 15–30fps USB LCDs.
- **Dual-context page** (NZXT `?kraken=1`): same HTML renders the face on-device
  and its config preview in the app.
- **Single-file portable themes + gallery** (.SENSORPANEL, .rmskin, .icuewidget,
  .watchface): the community flywheel mechanic.
- **Hardware fallback face** (iCUE flashes a face to device memory for
  software-dead operation) — long-term candidate, Corsair LCDs support GIF
  storage.

**Their unforced errors (avoid)**

- Lighting and screens as separate worlds (iCUE Murals can't touch the LCD).
- Paywalled/undocumented authoring (SignalRGB), URL-only with no local files
  (NZXT), 1 Hz data (NZXT), 3-sensor editor ceiling (iCUE Constructor).
- Forced updates removing features (SignalRGB, 2026-06 backlash).

## Architecture Recommendation

Six pillars, ordered by leverage:

### 1. Display identity & adaptive layout (the reflow story)

- **Define a shared `DisplayDescriptor` and inject it** before face load:
  `window.hypercolor.display = { apiVersion, width, height, aspect, shape:
  'round'|'square'|'wide'|'tall', circular, safeArea: {x,y,w,h}, class:
  'pump-lcd'|'strip'|'panel', targetFps, pixelFormat }`. Honest scoping: today
  the daemon only centralizes `DisplaySurfaceInfo { width, height, circular }`.
  Shape class, safe area, target fps, and pixel format exist as scattered
  driver/pipeline knowledge and must be *computed and plumbed* into a new
  `DisplayDescriptor` type (hypercolor-types), then injected into the Servo
  session before face boot. SDK `FaceContext` consumes it and falls back to
  author declarations off-daemon.
- **Display classes + layout variants in the SDK**: `face()` accepts either one
  responsive layout or per-class variants (`layouts: { round, square, wide }`).
  SDK ships layout primitives: safe-area-aware grid, polar/radial placement for
  round displays, rail layout for strips, anchor positioning. **JS-driven layout
  from the descriptor is the baseline** (works regardless of engine coverage);
  CSS grid/flex/media-queries are a progressive enhancement, adopted only after
  Servo coverage is validated with screenshot/pixel tests against the simulator
  displays.
- **Typography scales by min-dimension**, not uniform basis scale; per-class
  density (a strip face shows one row of large glyphs, not a shrunken square).

### 2. Motion system

- SDK animation module: tween/spring/keyframe tracks, easing curves, value
  smoothing, staggered entrances — designed for 15–30fps (slow eased motion,
  no strobe-prone fast movement).
- Gauges gain motion for free: eased value transitions, sweep-on-appear,
  threshold pulse states baked into arc/bar/ring/sparkline options.
- Face lifecycle transitions (enter/exit/preset-change) and microinteractions
  (control changes ease instead of jump).
- Ambient layers: subtle particle/flow background helpers tied to SilkCircuit
  palettes and optionally to audio (renderer injection exists; needs the
  `face()` audio opt-in below).
- `face()` gains an `audio: true` option that emits audio-reactive metadata
  (the renderer's `effect_is_audio_reactive` gate already honors it).
- Make the face FPS ceiling configurable per spec 42 §16.1 (`face_fps_cap`),
  with 60fps upshift where the transport allows. Faces already default to 30;
  never lower existing tiers.

### 3. Typed data slots (complications)

- Data layer exposes **typed sources**: `temperature`, `load`, `memory`,
  `rpm`, `throughput`, `clock`, `media`, `audio`, `lighting-state`. Each
  declares type, unit, range, refresh cadence.
- Faces declare **slots** (`metric`, `gauge`, `sparkline`, `text`, `art`) with
  supported types; users bind sources to slots in the UI. One face × N
  bindings replaces N hardcoded faces.
- **New sources**: MPRIS now-playing (track/artist/album art/position — Linux
  native, the thing SignalRGB can't ever do), network throughput, active
  effect/scene state (lighting mirror).

### 4. Configurability & persistence

- Sensor picker polish: the dropdown exists; add search, live current values,
  and grouping. Slot binding editor on the Displays page (wave 4).
- Per-display face persistence independent of scenes (daemon-level
  `display_preferences`), so scene switches keep faces unless the scene
  explicitly overrides. Design questions to settle in the spec — today's
  assignment *mutates the active scene's zones*, so this is a real model
  change: precedence (scene override vs display default), reconnect behavior,
  deletion semantics, and where face controls live when no scene owns them.
- Theme/palette control type; richer presets; preview-true config (we already
  render real JPEG previews — keep that honest).

### 5. The faces themselves

- **Glow-up the four**: animated digit morphs + sweeping dial + round/wide
  variants (neon-clock); rendered sparkline + trend states + eased arc
  (pulse-temp); componentized adaptive grid 2×2 round / 4×1 strip
  (sensor-grid); motion + layered glow pass (silkcircuit-hud).
- **New flagships**: `now-playing` (album art, scrolling title, progress —
  killer on Push 2), `spectrum` (audio visualizer using mel bands/chromagram —
  born for 960×160), `system-pulse` (slot-driven dashboard showcase),
  `lighting-mirror` (live scene canvas + effect info).
- Shared component library (`MetricCard`, `Readout`, `ChartPanel`) so faces
  compose instead of copy-paste.

### 6. Authoring DX & ecosystem

- Face dev loop: `just face-dev NAME` → SDK dev server + virtual display
  simulators (already exist!) at round/square/strip presets, hot reload.
- Authoring guide in docs/content (every competitor has zero or paywalled
  docs).
- Later: single-file face packages + gallery/sharing; agent skill for face
  authoring (iCUE ships one — ours can be better).

## Delivery Waves

| Wave | Scope | Key items |
|---|---|---|
| **0 — Platform plumbing** | types/core/daemon | `DisplayDescriptor` type + computation + Servo injection; `face()` audio opt-in (metadata emission); MPRIS input source; Servo CSS coverage pixel-tests on simulator displays; configurable `face_fps_cap` + 60fps upshift where transport allows |
| **1 — SDK: motion + layout** | sdk | animation module; animated gauge options; display classes + layout primitives (safe-area grid, polar, rail — JS baseline); component library; descriptor-aware `FaceContext` |
| **2 — Face glow-up** | sdk faces | all four faces adaptive (round + strip variants) and animated; quality bar: every face must be gorgeous on both Corsair 480×480 and Push 2 960×160 |
| **3 — Data contract + flagships** | types/core/sdk faces | minimal typed data-source contract first (`media`, `throughput`, `lighting-state` join `sensors`/`audio` as typed, cadenced sources — no binding UI yet); then now-playing, spectrum, system-pulse, lighting-mirror built *on* that contract |
| **4 — Slots & config UX** | types/daemon/ui | slot binding editor; per-display persistence (precedence/reconnect/deletion semantics per §4); theme controls; sensor picker polish (search, live values, grouping) |
| **5 — Ecosystem** | docs/sdk | authoring guide; face-dev loop polish; packaging/sharing format |

Waves 0–2 are the core "stomp" milestone: adaptive, animated faces nobody else
has on Linux. Waves 3–4 are the moat (MPRIS + slots). Wave 5 compounds it.
The wave 3 ordering is deliberate (per review): the typed source *contract*
lands before the flagship faces so now-playing binds a `media` source instead
of bespoke globals — no rewrites when the binding UI arrives in wave 4.

## Risks & open questions

- **Servo CSS coverage** (media queries, grid, clip-path): JS-driven layout
  from the descriptor is the baseline; CSS is adopted per-feature only after
  the wave-0 pixel tests prove it.
- **Frame budget**: more motion + more sessions at 30fps on the software-GL
  path; FPS controller downshifts exist, and faces must be authored for low-fps
  grace. Profile during wave 2.
- **Per-display persistence is a model change**, not a flag: today's face
  assignment mutates active scene zones. The precedence/reconnect/deletion
  semantics in §4 must be settled in the spec before wave 4 lands.
- **Slot model scope creep**: the wave 3 contract is deliberately minimal
  (typed sources only); the binding UI waits for wave 4, informed by real
  faces rather than speculation.
- **Spec 42 §16 leftovers** (Render-over-Load prioritization, per-session
  circuit breaker) become more visible with more concurrent faces — track, not
  block.

## Sources

- `signalrgb.md` — LCD Faces launch state, Lightscript schema, paywall/docs gaps
- `icue.md` — widget platform (.icuewidget), hardware classes, Constructor limits
- `nzxt-cam.md` — `window.nzxt.v1` injection contract, Web Integration, NZXT-ESC
- `watch-faces.md` — WFF/complications/per-shape variants/ambient budgets
- `sensor-dashboards.md` — measures/meters, widget taxonomy, portable formats
- Codebase: specs 42/43, `sdk/packages/core/src/faces/`, `display_output/`,
  `display_finalize.wgsl`, `api/displays.rs`, HAL corsair/lcd + push2
