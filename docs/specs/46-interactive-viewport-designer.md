# Spec 46 — Interactive Viewport Designer Modal

> A dedicated editor UI for authoring any effect that exposes a
> `ViewportRect` control. First-class modes for Web Viewport (Servo
> pane + optional real-page iframe + scroll controls) and Screen Cast
> (live screen-capture pane). A shared crop overlay, aspect lock,
> numeric inputs, snap presets, and apply/cancel flow drive both. The
> inline `ViewportPicker` remains for quick adjustments; the modal is
> where precise work happens.

**Status:** Implemented — `ViewportDesigner` Leptos component shipped in `hypercolor-ui`
**Author:** Nova
**Date:** 2026-04-17
**Packages:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`, `hypercolor-ui`
**Depends on:** Web Viewport Effect (Spec 44), REST/WebSocket API (Spec 10), Preview Stream plumbing (Spec 36)
**Related:** Screen Capture (Spec 14), Servo HTML Effects, Render Pipeline optimizations (commits `53e8eb50`, `c8fac890`, `da97db2f`)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Architecture Decision: Mode-Discriminated Modal](#4-architecture-decision-mode-discriminated-modal)
5. [UX and Modal Layout](#5-ux-and-modal-layout)
6. [Interaction Model](#6-interaction-model)
7. [Screen Cast Mode Specifics](#7-screen-cast-mode-specifics)
8. [Daemon Additions](#8-daemon-additions)
9. [WebSocket and REST Additions](#9-websocket-and-rest-additions)
10. [Leptos UI Components](#10-leptos-ui-components)
11. [Cross-Origin Handling](#11-cross-origin-handling)
12. [Performance and Latency Budget](#12-performance-and-latency-budget)
13. [Testing Strategy](#13-testing-strategy)
14. [Delivery Waves](#14-delivery-waves)
15. [Known Constraints](#15-known-constraints)
16. [Open Questions](#16-open-questions)

---

## 1. Overview

Spec 44 shipped the Web Viewport effect and the shared inline
`ViewportPicker` widget used by Screen Cast and Web Viewport. That
widget works for quick adjustments — the visible effect-control panel
shows a small preview, the user can drag or resize the crop rectangle,
and commits go back to the effect via the existing control-PATCH path.

It does not work for authoring. The preview is small, the crop handles
are imprecise at full LED canvas sizes (1280×1024 maps to a ~180×144
rendered thumbnail in the inline card), and the same limitation hits
both effects that use the picker today:

- **Web Viewport** cannot scroll the underlying page. If the part of
  hyperbliss.tech the user wants to sample is 1800px down the document,
  the inline picker offers no way to get there.
- **Screen Cast** cannot precisely frame a region of the captured
  desktop — the 180×144 card gives no way to nail a hairline crop
  around a specific window or panel on a 4K display.

This spec describes a dedicated **Viewport Designer** modal that gives
the full pixel-accurate editing surface shared by both effects. The
modal discriminates on the effect type:

- **Web Viewport mode** — Servo render + optional real-page iframe
  alongside, scroll sliders that drive Servo's page position.
- **Screen Cast mode** — a single large screen-capture preview, no URL,
  no iframe, no scroll controls (the desktop is already wherever it is).

Both modes share the crop overlay, aspect lock, numeric inputs, snap
presets, fit mode selector, and apply/cancel flow. The inline
`ViewportPicker` stays as-is for the quick path; the modal is opened
by an "Edit viewport" affordance next to the existing widget.

---

## 2. Problem Statement

### 2.1 Tiny preview, imprecise handles (both effects)

`crates/hypercolor-ui/src/components/control_panel/viewport_picker.rs`
lays out the preview inside an effect-control card about 320px wide.
At 1280×1024 source dimensions each rendered pixel occupies about
0.25 CSS pixels. Drag-to-resize can only hit 4×4 pixel accuracy before
the user is clicking between the handle's rendered rows. For effects
sampling a hero banner at a specific x-offset, or sampling a specific
window on a 4K desktop, that is not enough.

This applies equally to Web Viewport and Screen Cast — both use the
same widget and both suffer the same precision ceiling.

### 2.2 No scroll control (Web Viewport)

The Web Viewport effect currently renders whatever Servo lays out at
the top of the document. There is no control input that tells Servo
"scroll the page to y=1200 before painting," and there is no daemon
path to call Servo's scroll API. Users asked to sample content below
the fold have no mechanism.

### 2.3 No interaction with the page (Web Viewport)

Spec 44 explicitly scopes out "interactive webpages" as a non-goal.
Scrolling to arbitrary offsets pulls that constraint back — not full
interactivity, but at minimum the ability to position the page before
sampling. The inline picker has no affordance for this, and any
solution implemented at the inline-picker level would make the widget
too crowded to use in the effect panel.

### 2.4 No reference to the real page (Web Viewport)

A picked viewport shows the Servo render, but the user often wants to
know "what does this look like in a real browser?" Servo's layout and
typography can differ from Firefox/Chrome, especially on pages with
recent CSS features. An iframe of the live page — where supported —
gives them that reference alongside the Servo render.

### 2.5 No way to see the whole desktop at authoring scale (Screen Cast)

Screen Cast's inline preview is the same downscaled thumbnail as Web
Viewport's. Picking a specific window on a 4K monitor means dragging
against a surface where each screen pixel is ~0.08 CSS pixels. The
user cannot see enough detail to know they have the right crop, and
the handles round to screen-pixel chunks the user cannot visually
distinguish. A large modal preview of the screen-capture stream is
the same core need the Web Viewport case hits, just with a different
source surface.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **Shared Viewport Designer modal** that opens from the inline
  `ViewportPicker` card for both Web Viewport and Screen Cast, with
  a mode discriminator selecting the right source pane and control
  set. Both effects go through the same component tree and the same
  draft/commit machinery.
- **Large editing surface** for the rect and fit mode at
  full-resolution source pixels — end of the "tiny thumbnail"
  problem for both effects.
- **Pixel-accurate crop editing** with numeric inputs, arrow-key nudge,
  shift+arrow for 10× step, and optional aspect-ratio lock tied to
  target LED canvas dimensions. Shared across both modes.
- **Web Viewport extras**: live Servo render pane, optional real-page
  iframe pane alongside (when the site permits iframing), URL
  editing, `scroll_x`/`scroll_y` controls plumbed to Servo.
- **Screen Cast extras**: none beyond the shared surface. The mode
  uses the existing `screen_canvas` WS stream as its source pane.
- **Snap presets** for common crops (Full, Center 50%, Hero, Top strip,
  Bottom fold) to avoid manual drag-to-set for standard framings.
  Same preset chips in both modes (presets are geometry-only).
- **Full-resolution preview stream** at the effect's native render
  size so the overlay drag math maps 1:1 to eventual crop pixels.
  Web Viewport: `render_width × render_height`. Screen Cast: the
  screen-capture source width/height.
- **Bidirectional snap** (Web Viewport only): "snap to iframe scroll
  position" button that copies the iframe's current `scrollY` into
  the effect's `scroll_y` control when same-origin. On cross-origin
  the button is disabled with a hover tooltip explaining why (see
  §§ 6.2 and 11.2 for the exact UX).

### 3.2 Non-Goals

- **Input injection into Servo.** Mouse clicks, text entry, and
  keyboard events are explicitly out of scope. Interactive interaction
  happens in the real-browser iframe; the Servo side is scroll-only.
- **Mirroring iframe scroll into Servo continuously.** The iframe is a
  reference surface; users explicitly commit scroll offsets via the
  snap button or numeric input. Continuous mirroring would require
  cross-origin cooperation we cannot assume.
- **Proxy-server workaround for cross-origin iframe blocking.** Running
  arbitrary pages through a same-origin proxy breaks auth, cookies,
  CSP, and SPA routing. Out of scope; we detect the block and degrade
  gracefully.
- **Persistent scroll state across effect restarts.** If the daemon
  restarts the effect, Servo reloads the page and scroll resets to
  `(scroll_x, scroll_y)` as configured; we do not track or restore
  post-load user scroll.
- **A second, separate modal for Screen Cast.** Screen Cast uses the
  same modal with a Screen-Cast-specific source pane — URL bar, scroll
  sliders, iframe pane, and render-size inputs are conditionally
  rendered based on the effect mode. One component tree, one draft
  state shape, two source panes.

---

## 4. Architecture Decision: Mode-Discriminated Modal

### 4.1 Rejected: overlay-on-preview with Servo event injection

The obvious alternative is to stream Servo's render to the modal,
overlay a crop rectangle on it, and forward mouse/keyboard events
through the daemon into Servo so users can interact with the page.
This was seriously considered and rejected:

- **Event round-trip latency**: UI → WebSocket → daemon → Servo
  embedder → Servo paint → WebSocket → UI preview is 200–400ms on a
  heavy site like hyperbliss.tech. Every click feels broken.
- **Servo embedder surface is narrow**: the current Servo integration
  (`hypercolor-core/src/effect/servo/session.rs`) exposes `load_url`
  and `request_render`. Adding mouse/keyboard injection means
  extending the embedder, the worker thread message types, and the
  session handle — material engineering for a worse user experience.
- **Form behavior is broken**: autofill, password managers, input
  method editors, clipboard, drag-and-drop — none of those work when
  we synthesize events. Users would be confused.
- **The daemon becomes a bad browser**: we end up reimplementing what
  every native browser already does.

### 4.2 Rejected: iframe-only, skip Servo preview in modal

Showing only the iframe and trusting the user to mentally map the
crop to what Servo will sample is insufficient. Servo's layout
diverges from WebKit/Gecko/Blink often enough on cutting-edge sites
that "looks right in iframe" does not guarantee "looks right in the
effect." We need the Servo truth surface visible.

Cross-origin blocking also means many real-world URLs (including
anything with `X-Frame-Options: DENY`) will refuse to render at all.
An iframe-only design has a hard failure mode for large swaths of
the modern web.

### 4.3 Chosen: Servo-first modal with optional iframe enhancement

The modal is architected Servo-first. The source pane that actually
drives the crop is always the Servo render (Web Viewport) or the
screen-capture stream (Screen Cast). Any iframe pane is an optional
enhancement layered alongside it — never a dependency.

This framing matters because the iframe **will not be available on
most real-world URLs**. Anything with a login wall, an ads stack, or
a modern security posture sets `X-Frame-Options: DENY` or a strict
`frame-ancestors` CSP — GitHub, Google Workspace, Slack, Notion, and
every SaaS dashboard in that class. We designed against the
assumption that the iframe pane's _steady state_ for serious pages
is "unavailable." Treating it as core UX would make the modal feel
broken on anything interesting.

**Web Viewport mode** — single Servo pane is the baseline. The iframe
pane, when the site allows framing, appears alongside as a reference
surface with real browser chrome for discovery interactions (native
scroll, autofill, forms). The user makes all authoritative
commitments against the Servo pane — drag the overlay, adjust
scroll, set fit mode. The iframe is there to answer "where is the
content I want to sample?" when the Servo render alone is hard to
navigate.

**Screen Cast mode** — single screen-capture pane, no iframe
equivalent, no scroll. The desktop is wherever the user currently
has it; they change it outside the modal.

Shared between modes: overlay, controls bar, numeric inputs, snap
presets, fit mode, aspect lock, apply/cancel. Mode-specific modules
(Servo pane, iframe pane, capture pane, URL bar, scroll controls,
render-size inputs) are composed by a thin modal orchestrator — not
conditionally rendered inside one monolithic component. See § 10 for
the component decomposition.

If the site blocks iframing (or explicitly declines via a clear
user-facing control), the iframe pane collapses and the Servo pane
expands. This is the expected steady state for most URLs, not an
edge case.

### 4.4 Scroll flow

Scroll is the one interaction that must reach Servo. It gets its own
path:

- User drags a scroll slider in the modal → the slider value lives
  in local Leptos state and updates the visual slider position at
  60fps with no network traffic.
- On **pointer-up / blur / keyboard-commit only** (not during drag),
  the modal PATCHes the effect with `{ "scroll_x": N, "scroll_y": N }`
  against the effect-id-scoped endpoint (see § 9.1). Mid-drag PATCHes
  are explicitly not sent — they would just queue ahead of each other
  behind Servo's paint latency.
- Any in-flight scroll PATCH whose preview frame has not yet arrived
  is superseded by a later commit: the daemon tracks a monotonic
  scroll-commit counter, the preview stream tags each emitted frame
  with the counter value that produced it, and the modal ignores
  frames tagged with a stale counter. This keeps "rendering" spinner
  state honest even when the user commits scroll values faster than
  Servo paints.
- Daemon applies the control update, bumps the scroll-commit counter,
  and on the next render tick issues the Servo scroll call before
  requesting the next paint.
- Servo repaints at the new offset, the daemon emits a preview frame
  tagged with the current counter, and the modal's Servo pane updates.

**Latency breakdown**, split into controllable and uncontrollable terms:

| Term                                 | Typical                              | Control lever                        |
| ------------------------------------ | ------------------------------------ | ------------------------------------ |
| Pointer-up → PATCH issued            | ≤ 5 ms                               | local                                |
| PATCH network round-trip (localhost) | ~ 1 ms                               | —                                    |
| Daemon control application           | ~ 1 ms                               | —                                    |
| Servo scroll dispatch + repaint      | 100–300 ms                           | Servo-internal, **uncontrollable**   |
| Preview stream propagation           | 1 × frame interval at subscriber FPS | modal subscribes at 30 fps → ≤ 33 ms |
| Modal decode + render                | ≤ 8 ms                               | local                                |

Dominant term is Servo's paint — reducible only via embedder or
runtime work (pipelining the scroll with the current paint, tuning
layout reflow cost on the Servo side). The controllable terms total
≤ ~50 ms even in the worst case; tuning them further is not worth
engineering effort. During the Servo paint window the Servo pane
shows a `⚡ rendering` badge; the scroll slider locks its position
to the last committed value (not the in-flight value) once the
PATCH has been acknowledged so the user sees the system settling,
not thrashing.

---

## 5. UX and Modal Layout

### 5.1 Modal dimensions and behaviour

- Modal is full-viewport width up to 1600px, full-viewport height up
  to 1100px, centered with a dim backdrop.
- Closes on Esc, click-outside, or Cancel button. Focus management
  needs explicit handling because iframes capture keyboard focus and
  naive Esc listeners on the modal root will not fire while the user
  has clicked into the iframe. See § 10.6 for the focus-trap and
  close-affordance design.
- Unsaved changes confirm before close. The modal holds a draft copy
  of the viewport + scroll + fit values. Note that "draft" here is
  the UX abstraction — the _committed pixel-flushing_ policy lives in
  §§ 6.1/6.2: viewport rect updates flow to the daemon mid-drag
  (80 ms throttle) so the Servo pane previews them live, scroll
  commits fire only on release, and Apply issues a final reconciling
  PATCH for any untouched fields (fit mode, render size, URL). If
  the user cancels, the modal issues a revert PATCH with the open-
  time values to restore any fields the live-editing loop already
  committed. Pending drafts (anything that differs from the
  daemon-acknowledged state at open time) flash a small "Unsaved"
  badge near the Apply button.
- Opens from a new `🖥️ Edit viewport…` button rendered next to the
  inline `ViewportPicker`'s existing "Reset" button, for both Web
  Viewport and Screen Cast effects (Wave 1 ships both — see § 14.1).

### 5.2 Layout — Web Viewport mode (desktop, ≥1280px)

```text
┌─────────────────────────────────────────────────────────────────────┐
│  🌐 URL  [https://hyperbliss.tech                      ] [↻] [go]  │
├───────────────────────────────────┬─────────────────────────────────┤
│                                   │                                 │
│    REAL PAGE (iframe)             │    SERVO RENDER (truth)         │
│                                   │                                 │
│   ┌───────────────────────────┐   │   ┌────────────────────────┐    │
│   │                           │   │   │                        │    │
│   │                           │   │   │ ┌── viewport ──┐       │    │
│   │                           │   │   │ │              │       │    │
│   │   native scroll, click,   │   │   │ │              │       │    │
│   │   forms, autofill, etc.   │   │   │ │   crop rect  │       │    │
│   │                           │   │   │ │              │       │    │
│   │                           │   │   │ └──────────────┘       │    │
│   │                           │   │   │  (drag + handles)      │    │
│   └───────────────────────────┘   │   └────────────────────────┘    │
│                                   │                                 │
│   ↻ Snap Servo to iframe scroll   │   render 1280×720   ⚡ rendering │
├───────────────────────────────────┴─────────────────────────────────┤
│  VIEWPORT   x [0.10] y [0.25] w [0.50] h [0.40]   📐 px  | 🔒 aspect│
│  SCROLL     x [██████░░░░░]   0 /    0                              │
│             y [██████░░░░░] 240 / 3200                              │
│  FIT        (●) Cover   ( ) Contain   ( ) Stretch                   │
│  RENDER SZ  [1280] × [ 720]   [Reset to default]                    │
│  PRESETS    [Full] [Center 50%] [Hero] [Top strip] [Bottom fold]    │
├─────────────────────────────────────────────────────────────────────┤
│            [Cancel]   [Apply]                                       │
└─────────────────────────────────────────────────────────────────────┘
```

### 5.2.1 Layout — Screen Cast mode (desktop, ≥1280px)

```text
┌─────────────────────────────────────────────────────────────────────┐
│  🖥️  SCREEN CAST        source: Monitor 1 (3840×2160)   ⚡ live    │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│                        SCREEN CAPTURE                               │
│                                                                     │
│   ┌───────────────────────────────────────────────────────────┐    │
│   │                                                           │    │
│   │                                                           │    │
│   │                                                           │    │
│   │                    ┌── viewport ──┐                       │    │
│   │                    │              │                       │    │
│   │                    │  crop rect   │                       │    │
│   │                    │              │                       │    │
│   │                    └──────────────┘                       │    │
│   │                    (drag + handles)                       │    │
│   │                                                           │    │
│   │                                                           │    │
│   └───────────────────────────────────────────────────────────┘    │
│                                                                     │
├─────────────────────────────────────────────────────────────────────┤
│  VIEWPORT   x [0.40] y [0.30] w [0.30] h [0.40]   📐 px  | 🔒 aspect│
│  FIT        (●) Cover   ( ) Contain   ( ) Stretch                   │
│  PRESETS    [Full] [Center 50%] [Top-left quad] [Top strip] [...]   │
├─────────────────────────────────────────────────────────────────────┤
│            [Cancel]   [Apply]                                       │
└─────────────────────────────────────────────────────────────────────┘
```

URL bar, iframe pane, scroll sliders, and render-size inputs are
conditionally rendered — they're all Web-Viewport-only. The rest of
the modal is identical between modes.

### 5.3 Layout (narrow, <1024px)

Panes stack vertically: iframe on top at 40% modal height, Servo
render below at 60%, controls collapse into a single accordion
below. Mobile is not a target platform — the modal is usable but
not optimized.

### 5.4 Visual design conventions

SilkCircuit tokens throughout. The crop overlay's border uses the
active effect's accent color (reuses the existing logic in
`viewport_picker.rs` that parses `accent_rgb`). The iframe pane has
a subtle purple glow around its frame when loaded successfully, a
muted red glow when iframe load fails. The Servo pane has a tiny
`⚡ rendering` chip in the bottom-right that fades in during the ~200ms
window after a scroll commit and fades out once a fresh preview
frame arrives.

---

## 6. Interaction Model

### 6.1 Viewport rect editing

The crop rectangle on the source pane has six interaction targets:

- **Body drag** — moves the rect without resizing. Constrained to
  stay fully inside the pane.
- **Corner handles (4)** — resize from that corner. Holds aspect
  ratio when the lock toggle is on.
- **Edge handles (4)** — resize along one axis.
- **Arrow keys** — when the rect has focus, nudge by 1 device pixel
  in the appropriate direction. `Shift+Arrow` is 10 pixels.
- **Numeric inputs** — commit on blur, Enter, or after a 400ms
  debounce. Pixel-mode toggle (📐 px) swaps the four inputs between
  normalized `[0.0, 1.0]` and pixel coordinates.
- **Aspect lock (🔒)** — locks to the ratio of the current LED canvas
  (Primary group width/height). When on, resize preserves ratio;
  drag still moves freely.

**Commit policy (authoritative, referenced by §10 and §16.4):**

All six targets update a local `ViewportRect` signal in Leptos. The
signal flushes to the daemon as follows:

- **During pointer drag** — PATCH at a hard **80 ms throttle** with
  trailing commit, never during the drag itself at 60 fps.
  Viewport-rect changes are cheap in the render pipeline (the crop
  is applied per-frame in `sample_viewport` with no Servo repaint
  needed), so live preview of mid-drag rect values is a nice UX
  affordance at low cost.
- **On pointer-up, blur, Enter, debounce-fire, or keyboard-commit**
  — final PATCH, regardless of what the throttle has already sent.
  Guarantees the committed value matches the user's pointer-release
  position, not whatever the 80 ms throttle happened to sample last.

Scroll has a different policy (see § 6.2) because Servo paint cost
makes mid-drag scroll commits actively wasteful. Do not change these
policies independently — the two decisions were made together.

### 6.2 Scroll editing

Scroll sliders are coarse-to-fine: the visible slider is 0..page
height in pixels with tick marks every 500px, and a secondary
numeric input lets the user type the exact offset.

**Commit policy (authoritative, referenced by §4.4 and §16.4):**

- **Release-only.** No PATCHes during drag. Servo paint cost
  (100–300 ms) would cause mid-drag commits to pile up behind each
  other; the user would then experience a laggy settlement where
  each queued commit forces a redundant repaint.
- **Stale-commit cancellation.** A monotonic scroll-commit counter
  tags each PATCH and the preview frames Servo produces; the modal
  ignores preview frames tagged with a counter older than the latest
  committed value. See § 4.4.
- **Slider position snaps to last-acked value during paint.** The
  slider does not display the in-flight value; it displays the
  daemon-acknowledged committed value. Prevents the "slider jitters
  while Servo paints" anti-pattern.

Page height is an unknown the daemon must supply. The effect learns
it from Servo (see § 8.2). Until the first paint delivers a page
height the slider uses a Wave 1 fallback bound (8000 px) and
surfaces a `↻ waiting for extents` hint. Wave 2 replaces the
fallback with the extents event.

The iframe pane (when available) exposes one additional affordance:
a `↻ Snap Servo to iframe scroll` button. On click, we attempt to
read `iframe.contentWindow.scrollX/scrollY` and commit those as the
effect's `scroll_x/scroll_y`. On cross-origin the read throws and
we surface a one-line toast: "Can't read iframe scroll
(cross-origin). Drag the slider instead." The button is visible but
disabled when we already know the iframe is cross-origin, with a
tooltip explaining why.

### 6.3 Snap presets

Preset chip buttons below the controls commit common crop rectangles
in one click, resetting both viewport and — when sensible — scroll:

- **Full** — `{x: 0, y: 0, w: 1, h: 1}`, scroll `(0, 0)`.
- **Center 50%** — `{x: 0.25, y: 0.25, w: 0.5, h: 0.5}`, scroll
  unchanged.
- **Hero** — `{x: 0, y: 0, w: 1, h: 0.5}`, scroll `(0, 0)`. Matches
  the common "top-half banner" framing.
- **Top strip** — `{x: 0, y: 0, w: 1, h: 0.15}`, scroll `(0, 0)`.
  For sampling page headers or nav bars.
- **Bottom fold** — `{x: 0, y: 0.85, w: 1, h: 0.15}`, scroll left
  unchanged. For footer/CTA sampling at whatever scroll the user has.

Presets are pure viewport/scroll commits; they do not change fit
mode or render size.

### 6.4 Fit mode

Radio group. Changing fit mode modifies the aspect-lock behavior:

- **Stretch** — aspect lock OFF by default; the rect may have any
  ratio because the sampler will stretch regardless.
- **Cover** — aspect lock ON by default, tied to LED canvas aspect.
  The user sees the rect as a preview of what will fill the LED
  output exactly.
- **Contain** — aspect lock ON; the rect is the content rect inside
  the letterboxed output.

Users can always toggle the lock manually per session.

### 6.5 Render size

Two numeric inputs for `render_width` and `render_height`, bounded
by the existing control ranges (640–1920, 360–1080). Reset button
returns both to defaults (1280×720). Changes here trigger a Servo
`resize()` on apply, which triggers a repaint.

### 6.6 Apply / Cancel

- **Apply** — commits the entire draft (viewport, scroll, fit,
  render size, URL if edited) as one batch PATCH to the effect
  controls endpoint, then closes the modal.
- **Cancel** — discards the draft, closes the modal, effect
  controls revert to their pre-open values. If the user edited the
  URL and Servo started loading the new URL during the session,
  cancel issues a second URL change back to the pre-open URL so the
  rendering state matches what the user sees in the effect panel.

---

## 7. Screen Cast Mode Specifics

Screen Cast mode reuses 100% of the modal shell, draft machinery,
overlay, controls bar, snap presets, and apply/cancel flow. The only
mode-specific differences:

### 7.1 Source pane

The single source pane subscribes to the `screen_canvas` WS channel
(`crates/hypercolor-daemon/src/api/ws/relays.rs::relay_screen_canvas`)
at the screen-capture source's native dimensions. No resize of the
stream — full fidelity for authoring. Format negotiation is the same
as Web Viewport's Servo pane (JPEG preferred for bandwidth).

The capture source's width and height come from
`ScreenData.source_width` / `source_height` already carried in the
screen data payload. The modal reads these off the latest preview
frame header to size its overlay accurately and to set
`canvas_dimensions` for aspect lock.

### 7.1.0 Source-dimension changes during an open session

The screen capture source can resize mid-session: the user unplugs a
monitor, drags a window between displays, or changes display scale.
`ScreenData.source_width/height` then changes under the modal. The
same applies to Web Viewport if the user bumps `render_width` /
`render_height` from an external control surface.

Draft behaviour when the source resizes:

- **Normalized mode** — draft values are already fractions; no
  rebase needed. The overlay re-lays out against the new dimensions
  automatically.
- **Pixel mode** — the draft stores pixel coordinates relative to the
  dimensions at modal-open time. When the source resizes, we freeze
  the draft's reference dimensions and surface a banner at the top
  of the modal: `⚠ Source resized to 3840×2160 (was 2560×1440).
[Rebase to new size]   [Keep current values]`. The user chooses:
  "Rebase" rescales the draft pixel values proportionally to the new
  dimensions, "Keep" leaves them (which clamps any out-of-bounds
  edges on Apply).
- Applying the draft always clamps the final rect into the current
  source dimensions, regardless of which path the user took.

The alternative — silently reinterpreting pixel values against new
dimensions mid-edit — was considered and rejected because numeric
inputs would jump under the user's pointer without warning. Explicit
rebase is the honest UX.

### 7.1.1 Relationship to the monitor picker

The xdg-desktop-portal monitor-selection dialog that fires when Screen
Cast starts (via `ashpd` in
`crates/hypercolor-core/src/input/screen.rs`) is entirely upstream of
this modal. It grants Hypercolor access to a specific monitor or
window; `ScreenData.source_width/height` reflects whatever the portal
handed us. This modal picks the rectangular _region within that
surface_ to sample — the piece that is missing today and that the
inline picker is too cramped to do well. If the user wants a
different monitor they reset the capture outside the modal; the
modal's capture pane updates automatically as the daemon's screen
capture source changes.

### 7.1.2 Why not push region selection into the portal

Some compositors (Hyprland, a few KDE variants) expose region capture
through the portal — the user drags a rect in the compositor's own
overlay and we only ever receive the cropped stream. That would
technically remove the need for the modal's Screen Cast mode entirely.
We explicitly don't rely on it:

- **Not standard.** The xdg-desktop-portal Screencast spec covers
  monitor and window selection. Region capture is a per-compositor
  extension with no uniform surface. GNOME, most Wayland session
  managers, and most X11 compositors do not support it.
- **Non-live.** Compositor region dialogs typically commit once at
  grant time. Changing the region means tearing down the stream and
  re-prompting the user — fine for screenshots, too disruptive for
  live LED effects where the user wants to nudge the crop and see
  the lights update immediately.
- **The modal is the portable fallback.** Even on compositors that
  support portal region capture, users may prefer to grant the whole
  monitor once and then frame crops in-app without bouncing through
  the OS security prompt on every tweak.

The right read is: portal region capture is a privacy/scope concern
solved at grant time. The modal is an _authoring_ concern solved
per-effect, at runtime, against the fullest surface the portal will
give us.

### 7.2 Hidden controls

The Web-Viewport-only controls — URL bar, iframe pane, scroll
sliders, render-size inputs, and "snap to iframe scroll" button —
are omitted from the rendered component tree in Screen Cast mode.
The shared `ControlsBar` component takes a `mode: EditorMode` prop
and conditionally renders only the control groups that apply.

### 7.3 Snap preset variations

Screen Cast inherits the same preset chip row (Full, Center 50%,
Hero, Top strip, Bottom fold). For desktop capture two additional
chips are worth adding in Wave 1:

- **Top-left quad** — `{x: 0, y: 0, w: 0.5, h: 0.5}`. Common on
  4K desktops where the user has dev tools in the upper-left.
- **Bottom-right quad** — `{x: 0.5, y: 0.5, w: 0.5, h: 0.5}`.

These are purely additive — they do not change Web Viewport's preset
row. The presets list is mode-parameterized: shared core, mode-
specific extras.

### 7.4 No scroll state

Screen Cast has no scroll concept; the capture source is the live
desktop. The draft state's `scroll_x` / `scroll_y` fields are not
present in Screen Cast mode (draft type is
`ViewportDraft<Mode: EditorMode>` with mode-parameterized fields, or
an enum — see § 10.1).

### 7.5 Entry point

The inline `ViewportPicker` already renders for Screen Cast. We add
the same `🖥️ Edit viewport…` button next to the existing "Reset"
button, inside a match on effect type so we get the right modal mode
on open.

---

## 8. Daemon Additions

### 8.1 New controls on `WebViewportRenderer`

`crates/hypercolor-core/src/effect/builtin/web_viewport.rs` gains
three fields and their accompanying `ControlDefinition`s:

```rust
pub struct WebViewportRenderer {
    // ... existing fields ...
    scroll_x: i32,
    scroll_y: i32,
    last_applied_scroll: Option<(i32, i32)>,
}
```

Control IDs:

- `scroll_x` — pixel offset, 0..32768, default 0, step 1
- `scroll_y` — pixel offset, 0..32768, default 0, step 1

Both are `Slider` controls with a `Source` group. Both ranges are
32768 px so the definition matches the clamp applied in
`set_control` (§ 8.5) — earlier drafts had an asymmetric pair which
the PATCH clamp would silently widen. The upper bound is a sizing
hint, not a validation fence; the daemon does not check against
actual page extents. Servo clamps out-of-range scrolls itself.

`render_width` and `render_height` stay where they are; no change.

### 8.2 Page extent reporting

The UI needs to know the document scrollable dimensions to size the
scroll slider ticks. Two approaches:

- **Static** (Wave 1): we do not report. Sliders use a fixed upper
  bound (e.g., 8000px) and the user drags until they see the
  content they want. Not great.
- **Dynamic** (Wave 2): `ServoSessionHandle` grows a
  `fn document_extents(&self) -> Option<(u32, u32)>` that reads
  the last-known document scroll width/height from a `DocumentExtents`
  message emitted by the Servo worker after each paint.

The daemon publishes extents via a new WS event
`HypercolorEvent::WebViewportExtents { effect_id, width, height }`,
and the UI subscribes to refresh the slider bounds. Wave 2 detail;
Wave 1 ships with static bounds.

### 8.3 Servo scroll plumbing

The existing Servo worker accepts a `WorkerCommand` enum from the
session handle. We add:

```rust
enum WorkerCommand {
    LoadUrl(String),
    RequestRender { scripts: Vec<Script> },
    Scroll { x: i32, y: i32 },   // NEW
    // ...
}
```

The worker handles `Scroll` by calling the Servo embedder's scroll
API on the `WebView` (the exact Servo API surface depends on the
Servo version in the tree; the method is some variant of
`webview.set_scroll_offset(Point2D::new(x, y))`).

The `ServoSessionHandle` exposes:

```rust
pub fn scroll_to(&mut self, x: i32, y: i32) -> Result<()>;
```

### 8.4 Effect integration

In `WebViewportRenderer::render_into`:

```rust
if let Some(session) = self.session.as_mut()
    && self.last_applied_scroll != Some((self.scroll_x, self.scroll_y))
{
    if let Err(error) = session.scroll_to(self.scroll_x, self.scroll_y) {
        note_servo_session_error("web viewport scroll failed", &error);
    } else {
        self.last_applied_scroll = Some((self.scroll_x, self.scroll_y));
    }
}
```

Commit timing lives in the UI (§ 6.2: release-only for scroll, with
stale-commit suppression via the monotonic counter). The effect
itself is idempotent — applying the same scroll twice is free, and
`last_applied_scroll` avoids issuing redundant Servo commands on
every frame. The 80 ms throttle referenced elsewhere in the spec
applies to **viewport-rect** PATCHes (§ 6.1), not to scroll.

### 8.5 `set_control` routing

`set_control` gains two cases:

```rust
"scroll_x" => {
    if let Some(v) = value.as_f32() {
        self.scroll_x = v.round().clamp(0.0, 32768.0) as i32;
    }
}
"scroll_y" => {
    if let Some(v) = value.as_f32() {
        self.scroll_y = v.round().clamp(0.0, 32768.0) as i32;
    }
}
```

The UI sends integer-valued floats; the effect rounds and stores.

---

## 9. WebSocket and REST Additions

### 9.1 Control PATCH

The existing `PATCH /api/v1/effects/current/controls` endpoint is
insufficient for this modal. "Current effect" is a moving target —
another UI client, the CLI, or an MCP tool can change the active
effect between the moment the modal opens (snapshotting controls
for `effect_id = X`) and the moment the user hits Apply. If the
active effect has moved to `Y` at commit time, the modal's Apply
PATCH lands on the wrong effect and silently overwrites its state.

**New endpoint (additive, does not replace the existing one):**

`PATCH /api/v1/effects/{effect_id}/controls`

Writes controls scoped to a specific effect by id. Rejects with
`404 Not Found` if the effect is not loaded. Accepts the same
control-id → value body as the current-effect path.

**Optimistic concurrency (Wave 1, strongly recommended):**

Both the new and existing endpoints honour an optional
`If-Match: <controls_version>` header. The effect tracks a monotonic
`controls_version: u64` that increments on every control mutation.

Version lifecycle:

1. Modal opens → issues `GET /api/v1/effects/{id}`. The response
   body includes the controls payload and the current
   `controls_version` (as a top-level field; also echoed as an
   `ETag` header). The modal stores this value into
   `ViewportDraftCommon::controls_version` (§ 10.1).
2. Modal issues a PATCH with
   `If-Match: <draft.controls_version>`. On success the server
   increments `controls_version` to `N+1`, applies the mutation,
   and returns the new version in the response body as
   `{"controls_version": N+1, ...}` **and** in the `ETag` header.
   The modal reads the new version from either location and
   advances its draft token before the next PATCH.
3. If the server's current version differs from the request's
   `If-Match`, it returns `412 Precondition Failed` with a body
   containing the current server version so the client can decide
   whether to reload or retry. The reconciliation dialog fires at
   this point: "Another client changed this effect's controls while
   you were editing. Reload and re-apply, or overwrite?" — default
   action is "Reload."

Live mid-drag PATCHes (viewport rect at 80 ms throttle, § 6.1) use
this loop: each successful throttled PATCH pulls back the new
version and threads it into the next request. A mid-drag 412 stops
further throttled commits and defers the reconciliation dialog to
pointer-up so a flurry of mid-drag 412s does not spam the user.

Without `If-Match` the server accepts the PATCH unconditionally
(preserves backward compat with existing clients). Modal uses
`If-Match` by default; clients that opt out accept the races.

**Live updates during modal session:**

- Viewport rect — PATCHes with `If-Match` on throttled mid-drag
  commits. If 412 fires mid-drag, stop further mid-drag commits,
  surface the reconciliation dialog on pointer-up.
- Scroll — PATCHes on release only (see § 6.2).
- Other controls (URL, fit, render size) — PATCH on Apply only.

Apply sends one final PATCH with the complete draft and the most
recent `If-Match` value, covering any controls the modal did not
touch during live editing (notably fit mode + render size).

### 9.2 Preview stream

The `web_viewport_canvas` WS channel already exists and carries the
latest Servo-rendered canvas. The modal subscribes with explicit
`width` and `height` matching the effect's `render_width/height` to
get full-resolution frames (the stream is already sized per
subscriber's demand, see `PreviewRuntime` in
`crates/hypercolor-daemon/src/preview_runtime.rs`).

The inline picker's existing subscription uses a smaller size; the
modal's larger subscription coexists via the preview runtime's
per-subscriber demand tracking.

### 9.3 New event (Wave 2)

```json
{
  "type": "event",
  "event": "web_viewport_extents",
  "timestamp": "...",
  "data": {
    "effect_id": "uuid",
    "document_width": 1280,
    "document_height": 3840
  }
}
```

Emitted by the daemon whenever Servo reports a new document extent
(typically after load or after a resize). UI listens and updates
scroll slider bounds.

### 9.4 No new preview channel

The iframe pane loads the URL directly from the browser. It does
not go through the daemon. There is no daemon-side preview stream
for the real page.

---

## 10. Leptos UI Components

All new components live under
`crates/hypercolor-ui/src/components/viewport_designer/`. Module
structure:

```
viewport_designer/
  mod.rs                 -- public component + module plumbing
  modal.rs               -- <ViewportDesignerModal>
  dual_pane.rs           -- <DualPane> layout container
  iframe_pane.rs         -- <IframePane> with load detection
  servo_pane.rs          -- <ServoPane> with overlay + rendering badge
  overlay.rs             -- <CropOverlay> drag/resize logic, reuses
                            control_geometry helpers
  controls_bar.rs        -- <ControlsBar> with numeric inputs, sliders
  snap_presets.rs        -- <SnapPresets> chip row
  draft.rs               -- ViewportDraft state + commit/reset helpers
```

### 10.1 Draft state

Mode-specific fields belong to mode-specific variants — a single flat
struct would let invalid states (a Screen Cast draft with a
`scroll_y` field, a Web Viewport draft with no URL) compile. An enum
makes "Web Viewport implies URL + scroll + render size, Screen Cast
implies neither" type-enforced:

```rust
pub struct ViewportDraft {
    pub common: ViewportDraftCommon,
    pub mode: ModeDraft,
}

pub struct ViewportDraftCommon {
    pub viewport: ViewportRect,
    pub fit_mode: FitMode,
    pub brightness: f32,
    pub aspect_lock: bool,
    pub pixel_mode: bool,
    /// Snapshot of source dimensions at modal-open time. Frozen in
    /// pixel mode; used to detect source resizes during the session.
    /// See § 7.1.0.
    pub source_dimensions_at_open: (u32, u32),
    /// Version token captured from the initial GET; echoed as
    /// `If-Match` on PATCHes. See § 9.1.
    pub controls_version: u64,
}

pub enum ModeDraft {
    WebViewport {
        url: String,
        render_width: u32,
        render_height: u32,
        scroll_x: i32,
        scroll_y: i32,
        /// Monotonic scroll-commit counter for stale-frame
        /// suppression. See § 4.4.
        scroll_commit_seq: u64,
    },
    ScreenCast {
        // Intentionally empty. Screen Cast has no mode-specific
        // draft state beyond the common fields.
    },
}
```

The draft is a Leptos `RwSignal<ViewportDraft>` owned by the modal.
Opening snapshots the effect's current controls and its
`controls_version` into a fresh draft. Live PATCHes flow through the
policy in § 6.1 / § 6.2. Apply sends one final reconciled PATCH
covering any untouched common fields (fit mode, render size). Cancel
discards and — if the live editing loop has already committed
viewport-rect changes to the daemon — issues a revert PATCH with the
open-time values to restore the pre-open state.

Mode-specific control modules (`ScrollControls`, `UrlBar`,
`RenderSizeInputs`) accept a `ModeDraft::WebViewport` projection and
are only instantiated when the modal is in that mode. The controls
bar for Screen Cast never sees the Web Viewport fields.

### 10.2 `<ViewportDesignerModal>` props

```rust
#[component]
pub fn ViewportDesignerModal(
    #[prop(into)] open: Signal<bool>,
    on_close: Callback<()>,
    effect_id: String,
    control_values: Signal<EffectControlState>,
    canvas_dimensions: Signal<(u32, u32)>,  // active LED canvas WxH for aspect lock
    accent_rgb: String,
) -> impl IntoView { ... }
```

### 10.3 Iframe pane behaviour

```rust
#[component]
fn IframePane(url: Signal<String>, on_scroll_snap: Callback<(i32, i32)>) -> impl IntoView {
    let load_status = RwSignal::new(LoadStatus::Loading);
    let iframe_ref: NodeRef<leptos::html::Iframe> = NodeRef::new();

    // Track load via onload/onerror handlers.
    // After onload, attempt to read contentWindow.scrollX to detect
    // cross-origin (throws) — set load_status accordingly.

    view! {
        <div class="iframe-pane">
            <iframe
                node_ref=iframe_ref
                src=move || url.get()
                sandbox="allow-scripts allow-same-origin allow-forms allow-popups-to-escape-sandbox"
                referrerpolicy="no-referrer"
                loading="lazy"
                on:load=...
            />
            {/* Status badge always visible, never auto-hides. */}
            <LoadStatusBadge status=load_status.read_only() on_collapse=... />
            {move || matches!(load_status.get(), LoadStatus::Loaded).then(||
                view! { <SnapButton on:click=... /> }
            )}
        </div>
    }
}
```

**Sandbox policy**: we enable scripts (required for modern pages to
render at all), same-origin within the iframe (so the page can talk
to its own origin's resources), and forms (autofill works). We do
NOT enable `allow-top-navigation` — an embedded page cannot
navigate the outer window, which would otherwise break the modal
workflow. `allow-popups-to-escape-sandbox` lets click-to-new-tab
work for discovery interactions without pulling them back into the
iframe; `no-referrer` limits information leakage to the embedded
site.

**Load-status states**:

| State         | Meaning                                                              | Auto-collapse?               |
| ------------- | -------------------------------------------------------------------- | ---------------------------- |
| `Loading`     | iframe `load` event has not fired yet                                | never                        |
| `Loaded`      | `load` fired and same-origin read succeeded                          | never                        |
| `CrossOrigin` | `load` fired, `contentWindow.scrollX` threw                          | never                        |
| `Blocked`     | `load` fired but `contentDocument` is null AND ≥3s passed since load | never; shows manual collapse |
| `Errored`     | `onerror` fired (rare; typically network failure)                    | never                        |

No state auto-collapses the iframe pane. `Blocked` shows a clear
explanation ("This site can't be previewed here — probably
`X-Frame-Options`. Collapse this pane to focus on the daemon
render?") with a manual `[Collapse]` button. `Loading` stays visible
with a shimmer indefinitely — slow networks and heavy pages would
otherwise false-trigger `Blocked`. The user can manually collapse
the pane at any time via a persistent `[⇤]` control in its header.

### 10.4 Servo pane + overlay

The Servo pane subscribes to `web_viewport_canvas` at the effect's
render dimensions. The canvas renders via the existing
`<CanvasPreview>` component. The overlay is absolute-positioned on
top with `pointer-events: auto` so it captures drag events; the
underlying canvas has `pointer-events: none`.

Drag math reuses `control_geometry::drag_frame_rect` and
`control_geometry::resize_frame_rect` from the inline picker — no
new geometry code.

### 10.5 Dependency on existing preview infrastructure

The preview subscription uses the same `preview_runtime` handles as
the inline picker. On modal open we bump
`preview_consumer_count`; on close we drop it. This keeps the
demand-tracked preview stream sized appropriately and stops it when
no consumers remain.

**Demand-cap guard-rail**: the modal requests the effect's full
render resolution (e.g., 1280×720 for Web Viewport) at 30 FPS. That
demand is summed globally across subscribers in `PreviewRuntime`,
so a modal open by one client raises the stream size for all other
viewers of the same preview. For Wave 1 this is acceptable — the
only other consumer is the small inline picker thumbnail on the
same page. For Wave 3 (see § 14.3) we should consider either (a) a
per-connection demand isolation layer in `PreviewRuntime`, or (b) a
throughput cap (e.g., never negotiate above 30 FPS × 1080p) so a
misconfigured modal cannot saturate the render thread's
publish-stage budget. The cap approach is the less invasive default.

### 10.6 Focus management and close affordances

Iframe elements capture keyboard focus aggressively — once the user
clicks into the iframe's content, `keydown` events fire against
`iframe.contentWindow`, not the outer modal. Standard Leptos
"listen for Escape on the modal container" will not fire. The
modal needs explicit focus handling:

- The modal sets an outer `dialog` element with `aria-modal="true"`,
  trapping Tab focus within it.
- The outer container listens for `keydown` with the capture flag
  so the handler runs before any bubbling target can `preventDefault`.
  Still does not help when focus is inside a cross-origin iframe —
  events never propagate out. So:
- **Always-visible close affordances that do not depend on focus**:
  the modal renders a fixed `✕ Close` button in its top-right
  corner, plus a `Cancel` button in its footer. Click-outside on
  the dim backdrop also closes. Esc works only when focus is outside
  the iframe; that's fine because the click affordances always work.
- Apply + Cancel buttons live in the footer, outside the iframe's
  stacking context, and accept focus on Tab from the iframe in
  same-origin cases. Cross-origin iframes trap focus until the user
  clicks out; we document this in the inline help tooltip on the
  iframe pane.
- Unsaved-changes confirmation dialog: rendered as a secondary
  dialog above the main modal, guaranteed to be outside any iframe's
  focus trap.

---

## 11. Cross-Origin Handling

### 11.1 Detection

After the iframe's `onload` fires we wait 200ms (to let the page's
JS run) and then attempt:

```rust
// web_sys access from Leptos
let cross_origin = match iframe.content_window() {
    Some(win) => win.scroll_x().is_err(),
    None => true,
};
```

If the read throws, we mark load status as `CrossOrigin`. The 3s
timer is a _hint_ that the load may be blocked — we surface a
"Still loading…" message past 3s — but we do NOT auto-collapse the
pane on that signal. Slow networks, heavy pages, and late script
loads all produce false positives for timeout-based blocked
detection. The user manually collapses the pane via the header
`[⇤]` control if they want the Servo pane to expand.

`Blocked` state is reserved for the unambiguous case where both
(a) 3s have passed since load start AND (b) `contentDocument` is
null. Even then we show the manual-collapse prompt rather than
forcing the collapse — some sites recover from what looks like a
block, and we prefer to keep the iframe visible so the user can
retry or confirm.

### 11.2 Degradation

- `CrossOrigin` — iframe still visible, still scrollable by the user,
  "snap Servo to iframe scroll" button is disabled with a hover
  tooltip ("Cross-origin pages don't expose scroll position. Use
  the sliders to the right."). User can manually collapse if they
  want more room for the Servo pane.
- `Blocked` — iframe pane shows a clear message
  ("hyperbliss.tech can't render in a browser frame — probably
  `X-Frame-Options: DENY`. Use the daemon render below.") plus a
  `[Collapse this pane]` button. No auto-collapse.

The Servo pane's width reflects the current iframe-pane state:
visible → 50/50 split; collapsed → Servo fills the modal width.
Expansion/collapse animates over 200 ms so the transition is
visible to the user, not jarring.

### 11.3 User education

A `(?)` tooltip next to the "Real Page" pane label explains the
cross-origin limitation in one sentence. The tooltip links to a
short docs page (future work) with more detail for power users.

### 11.4 No proxy fallback

As stated in § 3.2, we do not proxy pages to work around cross-
origin limits. The trade-offs (breaking auth, CSP, SPAs) outweigh
the convenience gain. The Servo pane is the _primary_ authoring
surface regardless of whether the iframe is available — § 4.3
makes that the core framing — and cross-origin-blocked sites are
the expected steady state for anything with real security posture,
not an edge case.

---

## 12. Performance and Latency Budget

### 12.1 Preview stream

The modal's Servo pane subscribes at render dimensions (e.g.,
1280×720). At 30fps that's ~110 MB/s raw — the preview runtime
already JPEG-encodes (tunable) or ships RGBA depending on subscriber
negotiation. The modal negotiates JPEG at 30fps to keep bandwidth
in hand. Existing JPEG encoding via the scaler pipeline handles this
(see render-pipeline optimization commits `c8fac890`, `da97db2f`).

### 12.2 Drag latency

Local drag state updates at 60fps. PATCHes throttle to 80ms
(12.5/sec). Since Servo does not repaint for viewport-rect changes
(crop happens in `sample_viewport` after the paint), the LED output
response is bounded by one render-loop tick (~16ms).

### 12.3 Scroll latency

Scroll changes force a Servo repaint. The end-to-end budget breaks
into controllable and uncontrollable terms (same table as § 4.4,
repeated here for the performance chapter):

| Term                                 | Typical          | Control lever          |
| ------------------------------------ | ---------------- | ---------------------- |
| Pointer-up → PATCH issued            | ≤ 5 ms           | local                  |
| PATCH network round-trip (localhost) | ~ 1 ms           | —                      |
| Daemon control application           | ~ 1 ms           | —                      |
| Servo scroll dispatch + repaint      | 100–300 ms       | **uncontrollable**     |
| Preview stream propagation           | ≤ 33 ms @ 30 fps | modal subscription FPS |
| Modal decode + render                | ≤ 8 ms           | local                  |

Total ~250–440 ms end-to-end. The controllable terms (≤ ~50 ms
combined) are pinned by:

- Release-only scroll commits (§ 6.2) — no mid-drag queue to drain.
- Stale-frame suppression via commit counter (§ 4.4) — rendered
  preview always reflects the latest committed value, never an
  earlier one that won its paint race.
- Modal subscribes to the preview stream at 30 FPS — higher buys
  nothing for scroll commits since they occur at human speeds.

What we cannot shrink from the daemon side is Servo's paint time on
a heavy site — only embedder/runtime changes (scroll-paint
pipelining, layout reflow tuning) can move it. During
that window the Servo pane shows a `⚡ rendering` badge; the slider
snaps to the last-acked committed value (not the in-flight value)
so the user sees the system settling. Subjectively this reads as
"the system is working," not as lag. If real-world authoring
sessions prove this unacceptable, the escape valves are (a) move
the `web_viewport_canvas` preview to 60 FPS during an open modal
session (saves at most ~16 ms per commit) or (b) pipeline scroll
with the current paint so the next paint starts as soon as scroll
is applied (Servo-embedder work; Wave 3+).

### 12.4 Memory budget

Draft state is ~200 bytes. The modal holds one extra preview
subscription with one extra Arc clone per frame — adds one slot to
the direct-canvas pool (already auto-grown, see commit `bacd878c`).

### 12.5 FPS impact on render loop

The effect's scroll application in `render_into` is cheap when
`last_applied_scroll == current` (one comparison). On scroll change
it's one message-pass to the Servo worker — nanoseconds. The
repaint that follows is on Servo's timeline, not the render loop.

---

## 13. Testing Strategy

### 13.1 Leptos unit tests

`crates/hypercolor-ui/src/components/viewport_designer/*` each gets
`#[cfg(test)]` modules with:

- Draft snapshot / apply / cancel lifecycle tests.
- Overlay drag math tests (reuse existing `control_geometry` test
  infrastructure; new tests for overlay-in-modal coordinate mapping
  at non-inline sizes).
- Snap preset value tests.
- Fit mode → aspect lock state machine tests.

### 13.2 Daemon unit tests

- `WebViewportRenderer` scroll application: given a scroll control
  update, next `render_into` issues exactly one `scroll_to` call;
  repeat with same value makes no additional calls.
- `ServoSessionHandle::scroll_to` plumbs correctly through the worker
  command enum (use a mock worker, verify command dispatch).
- Control set_control parsing: accepts `Float`, rejects malformed,
  clamps out-of-range.

### 13.3 Integration tests

`crates/hypercolor-daemon/tests/`:

- Boot a daemon, apply the Web Viewport effect with `scroll_y=500`,
  subscribe to `web_viewport_canvas`, assert the first post-scroll
  frame differs from a scroll_y=0 frame (same URL). Uses the
  existing test harness.
- WS control PATCH round-trip: open WS, PATCH `scroll_x`, read back
  effect state via `GET /api/v1/effects/current`, verify value
  propagated.

### 13.4 Cross-origin manual test

Not automatable in unit tests (browser-specific behavior). Manual
test matrix:

| Site                      | Expected iframe outcome                                                                                                |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `about:blank`             | Loaded, same-origin, snap works                                                                                        |
| hyperbliss.tech           | Loaded, cross-origin detected, snap button disabled with tooltip (no toast fires since click is blocked at the button) |
| `https://github.com`      | Blocked (`X-Frame-Options`), pane stays visible with "cannot render" message + manual `[Collapse]` button available    |
| `http://localhost:<port>` | Same-origin if UI is on same host, cross-origin otherwise                                                              |

Manual test lives in `docs/design/46-viewport-designer-manual-tests.md`
alongside this spec.

### 13.5 Visual regression

Modal screenshots at ≥1280px and at 1024px breakpoints. Run manually
before each release cut during Wave 1; consider adding a Playwright
snapshot run in Wave 3.

---

## 14. Delivery Waves

### 14.1 Wave 1 — Shared modal, both effects (ships first)

**Scope:**

- `ViewportDesignerModal` with Servo pane for Web Viewport and
  Screen Capture pane for Screen Cast, composed via mode-specific
  pane and controls modules (§ 10).
- Shared `CropOverlay` with drag + resize + arrow nudge.
- Shared `ControlsBar` with numeric inputs, fit mode radio, aspect
  lock toggle, pixel/normalized mode toggle.
- Mode-specific controls (Web Viewport only): `UrlBar`,
  `ScrollControls` with static slider range, `RenderSizeInputs`.
- Shared snap presets; Screen Cast gets two extra chips (top-left
  quad, bottom-right quad).
- `ViewportDraft { common, mode: ModeDraft }` with enum-based mode
  variants (§ 10.1).
- `scroll_x` / `scroll_y` controls on `WebViewportRenderer`.
- `ServoSessionHandle::scroll_to` + worker command.
- Scroll-commit counter threaded through preview frames for stale-
  frame suppression (§ 4.4).
- New `PATCH /api/v1/effects/{effect_id}/controls` endpoint with
  `If-Match: <controls_version>` optimistic concurrency (§ 9.1).
- `controls_version` monotonic counter tracked per effect.
- New `🖥️ Edit viewport…` button in the inline picker card — renders
  for both Web Viewport and Screen Cast.
- Source-resize banner for pixel-mode draft (§ 7.1.0).
- Focus management: always-visible `✕ Close` and `Cancel` affordances
  outside any iframe stacking context (§ 10.6).

**Explicit pre-Wave spike gate.** Before Wave 1 implementation
starts, a short scoped investigation must verify:

1. The current Servo pin exposes a usable scroll API from the
   embedder side, OR `window.scrollTo(x, y)` script injection via
   `request_render(scripts)` is acceptable as a fallback. Spike
   output: a one-page doc naming the path Wave 1 takes.
2. `PreviewRuntime`'s demand-tracking can handle a second subscriber
   at full resolution without regressing the publish-stage budget
   from recent render-pipeline perf work (commits `40674a22`,
   `477c01e4`, `c8fac890`). If it can't, demand cap work lands
   before shipping.

Both are no-go gates. Spike outcomes shape Wave 1's scope before
code is written.

**Acceptance criteria (replaces the prior "95% of authoring value"
assertion — that phrasing was not measurable):**

- A user opens the modal for a Web Viewport effect rendering
  hyperbliss.tech, scrolls to `y = 1800`, sets a 50% × 30% crop
  over a specific hero section, clicks Apply, and the LED output
  reflects the cropped section within one frame of Apply.
- A user opens the modal for a Screen Cast effect on a 4K monitor,
  sets a 200-pixel-wide crop around a specific window, and the LED
  output reflects that crop.
- A second UI client editing the same effect's controls concurrently
  receives a 412 on Apply and the reconciliation dialog fires
  instead of silently overwriting.
- No regression to the render-pipeline perf budget vs pre-spec
  measurements, verified by the existing `producer_us` telemetry.

### 14.2 Wave 2 — Iframe pane (Web Viewport only)

- `IframePane` component, conditionally rendered in Web Viewport
  mode only.
- Load status detection (loaded / cross-origin / blocked).
- Snap-to-iframe-scroll button (same-origin only).
- Page extents event from daemon, dynamic slider bounds.
- Polish pass on the dual-pane layout.

### 14.3 Wave 3 — Polish

- Keyboard shortcuts palette (help overlay, `?` to toggle).
- Viewport presets saved per-effect (user can save "my home page
  hero crop" and recall it).
- Persistent modal size across opens (remember last width/height in
  localStorage).
- Screen Cast source selector (pick which monitor/window the capture
  is targeting) — if the effect gains multi-source support upstream.

---

## 15. Known Constraints

### 15.1 Servo scroll API availability

The exact scroll API on the Servo embedder depends on the Servo
revision pinned in the workspace. The pre-Wave spike (see § 14.1)
is a hard go/no-go gate — Wave 1 cannot ship its scroll story until
the spike resolves. The spike verifies the method signature and
that `set_scroll_offset` (or equivalent) propagates to the layout
thread correctly. If the current pin does not expose a usable
scroll API, we either bump Servo or inject a `window.scrollTo(x, y)`
script via the existing
`request_render(scripts)` path as a fallback. The fallback is less
ideal (depends on JS being enabled, runs after layout) but unblocks
shipping.

### 15.2 Iframe blocking on common sites

`X-Frame-Options: DENY` / strict `frame-ancestors` CSP is the norm
on most commercial sites. The Blocked state is not an edge case —
it's the default for anything with a login, ads, or a security
team. The design assumes the modal is still useful in that state
(Servo pane is complete); testing Wave 2 against that assumption is
critical.

### 15.3 Page height unknowable pre-first-paint

The scroll_y slider is disabled before the first Servo paint
delivers a document extent. For Wave 1's static bounds version, the
slider is live immediately with a hardcoded range. This may let
users drag to offsets that don't exist yet (silently clamped by
Servo). Not great but not broken.

### 15.4 Servo vs real-browser layout divergence

The dual-pane design specifically surfaces this divergence — users
will see that the iframe's rendering differs from Servo's. That
transparency is the feature, not a bug. We document the expected
differences (typography, recent CSS features) in the in-app help.

### 15.5 Accessibility

Drag-to-resize is not accessible without pointer input. The arrow-
key nudge covers keyboard users once the overlay has focus. The
numeric inputs are the primary accessible path and are always
available. Screen readers: overlay handles get ARIA labels
describing their function ("Resize from top-left corner, current
position 10% from left, 25% from top").

---

## 16. Open Questions

### 16.1 Scroll units: pixels vs normalized?

Pixels feel natural ("scroll to 1200px"), but need document extents
to show a meaningful slider range. Normalized (0..1) is trivial but
feels weird on a 10000px page. **Proposed: pixels**, with Wave 2's
extents event to size the slider, Wave 1 shipping with a hardcoded
upper bound.

### 16.2 One scroll control or two?

`scroll_y` is essential. `scroll_x` matters for unusual layouts
(horizontal scrollers, some image galleries). **Proposed: ship
both** — implementation cost is symmetric, and the asymmetry of only
shipping Y would feel weird.

### 16.3 Should the modal remember last view per URL?

If the user opens the modal for `hyperbliss.tech`, picks a crop,
closes, then opens it later for the same URL, should their prior
pick be suggested? **Proposed: no** — the effect's live control
values are the source of truth; the modal always opens against
them. Saved presets (Wave 4) cover the "I want to save this" case.

### 16.4 Drag-commit strategy — _resolved in §§ 6.1/6.2_

Resolved, kept here as a pointer to avoid future re-litigation.
Viewport rect: 80 ms throttle during drag + final commit on
release. Scroll: release-only with stale-commit cancellation via a
monotonic counter. See §§ 6.1 and 6.2 for the authoritative wording
and rationale.

### 16.5 Mobile UX

The modal is not designed for touch or small screens. Is it worth
pursuing a mobile variant? **Proposed: no**. Authoring LED
installations on a phone is niche; the inline picker already works
on mobile for quick tweaks.

---

## Appendix A — Related Commits and Files

For implementers to orient against:

- Web Viewport effect: `crates/hypercolor-core/src/effect/builtin/web_viewport.rs`
- Servo session handle: `crates/hypercolor-core/src/effect/servo/session.rs`
- Inline picker: `crates/hypercolor-ui/src/components/control_panel/viewport_picker.rs`
- Geometry helpers: `crates/hypercolor-ui/src/control_geometry.rs`
- Canvas preview: `crates/hypercolor-ui/src/components/canvas_preview.rs`
- Preview runtime: `crates/hypercolor-daemon/src/preview_runtime.rs`
- WS protocol: `crates/hypercolor-daemon/src/api/ws/protocol.rs`
- Control PATCH endpoint: `crates/hypercolor-daemon/src/api/effects.rs`

Recent perf work this design sits on top of:

- `0ed03505` fix(daemon): drop watch-channel borrows before encoding WS canvases
- `bdaae182` perf(daemon): size render-surface pools for downstream fan-out
- `bacd878c` feat(types): auto-grow render-surface pool under pressure
- `40674a22` perf(daemon): route WS preview scaler through fast_image_resize
- `53e8eb50` perf(types): precompute sRGB↔linear LUTs for byte-paced gamma conversion
- `477c01e4` perf(core): lift Arc::make_mut and bounds checks out of viewport blit
- `c8fac890` perf(core): route viewport sampler through fast_image_resize
- `da97db2f` perf(core): disable alpha premultiply in viewport fast_image_resize path

These collectively bring web-viewport render under the 60fps budget
with headroom, which is what makes running a dual-pane live-streaming
modal practical in the first place.
