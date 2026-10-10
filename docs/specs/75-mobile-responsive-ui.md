# Spec 75: Mobile-Responsive Web UI

Status: In progress (status line added 2026-08-25; this file previously carried none).
Wave 0 shipped: `MobileNav` (`crates/hypercolor-ui/src/components/mobile_nav.rs`) and
the responsive breakpoint pass are live. Wave 1 shipped on 2026-10-10 and passed
emulated touch QA; its real-device pass is still owed (see Wave 1 below). Waves 2
through 4 are open. This spec supersedes the archived Spec 63, which designed a
separate mobile shell that was never built.

Make `hypercolor-ui` fully usable on phones and tablets as a pure
Leptos + Tailwind effort. No new frameworks, no daemon changes beyond
possible preview-cap tuning. The daemon already serves the built UI as a
SPA fallback and every endpoint derives from `current_page_location()`,
so a phone on the LAN reaches the full app at `http://<host>:9420`
today; this spec is about making what loads there actually work.

## Current State

Measured on main at the time of writing:

- 28 responsive breakpoint classes across the whole crate; layouts
  assume a wide viewport.
- 89 `on:mouse*` handlers versus 25 pointer/touch handlers; every drag
  surface is mouse-only.
- Card grids (`effects`, `devices`, `media`) already use
  `auto-fill,minmax(...)` and reflow to one column unaided.
- Dashboard stats panels already collapse to full width below `xl`.
- The preview WebSocket path already negotiates per-host format and
  width caps (`ws/preview.rs`), so remote phones get compressed frames.

## Breakpoint Strategy

Desktop layout is the default; mobile overrides use `max-md:` so the
desktop DOM and classes stay untouched. `md` (768px) is the single
phone/desktop boundary: below it the bottom tab bar replaces the
sidebar. Tablets (`md` to `lg`) keep the desktop shell with the collapsed
sidebar as the natural mid-size layout. Hover-dependent affordances
gate on `@media (hover: hover)` rather than width.

## Waves

### Wave 0: Shell (prototype, shipped on this branch)

- `MobileNav` bottom tab bar from the shared `nav_model`, safe-area
  padding, active-route indicator. Sidebar hidden below `md`.
- `<main>` bottom padding matches the bar height plus safe-area inset;
  `viewport-fit=cover` enables the inset env vars.
- Page header: `px-4` on phones. Toolbar overflow stays visible at
  every width: Effects and Devices hang non-portaling filter panels
  off toolbar children, and any `overflow` value on the row becomes a
  44px clip box that shreds them.
- Dashboard hero row stacks: full-width 16:9 preview over a
  fixed-height favorites panel, splitter hidden.

### Wave 1: Pointer-event migration

Shipped 2026-10-10. Every drag, resize, and press-hold control runs on
pointer events. The original plan, for reference: convert drag surfaces
from mouse events to pointer events with `setPointerCapture` and
`touch-action: none` on the drag origin, covering the color wheel, the
resize handles and dashboard splitter, Studio zone drag and resize, and a
check of native range sliders.

The active-route matcher extraction planned alongside it had already
landed as `route_ui::route_is_active`, which both `sidebar.rs` and
`mobile_nav.rs` use and `tests/route_ui_tests.rs` covers.

#### Gesture model

`crates/hypercolor-ui/src/pointer_gesture.rs` holds one model that every
drag surface follows, with its ownership rules unit-tested in
`tests/pointer_gesture_tests.rs`:

- A press claims the gesture for its pointer and captures that pointer on
  the pressed element. Moves and the release arrive there wherever the
  pointer goes, so no surface needs window listeners.
- Capture goes on the pressed element, never an ancestor. Chromium
  retargets `click` and `dblclick` to the capture target, so capturing on
  the Studio canvas slot would swallow the box's own click and the
  double-click that enters a device.
- One pointer owns a gesture. A second finger that lands mid-drag is
  ignored outright (on the Studio canvas that includes selection), which
  keeps every surface single-finger and pinch-free.
- A release commits. `pointercancel`, or a capture lost before the
  release, cancels: the surface restores its press-time state (geometry,
  color, rect, or panel size) and records no history entry. A gesture
  whose end never arrived (the owner re-presses, or no longer holds
  capture) is rolled back on the next press instead of locking the
  surface.
- Drag surfaces set `touch-action: none`, so the browser never turns a
  drag into a pan or zoom.
- Mouse and pen presses cancel their default actions, as the old
  `mousedown` handlers did, which keeps text selection and focus moves
  out of drags. Touch presses keep theirs: in Chromium, cancelling a
  touch `pointerdown` gives the tap's `click` a `detail` of 0 and
  suppresses `dblclick`, which broke double-tap until QA caught it.
- Outside-press dismissal (color picker, control dropdowns, preset menu,
  component picker, dashboard layout menu) listens for `pointerdown`.
  A drag surface that cancels `pointerdown` suppresses the compatibility
  `mousedown` even for a real mouse, so `mousedown` dismissal would stop
  closing popovers whenever a drag began.
- On coarse pointers the cursor-sized handles (Studio box corners,
  viewport picker grips, splitters) get an invisible grab margin through
  the `touch-grab` class: about 8px past a bordered handle's edge and
  10px past a borderless splitter's. Fine pointers are unchanged.

#### Census

Counted in `crates/hypercolor-ui/src`:

| Measure | Before (main 67387466a) | After |
| --- | --- | --- |
| `on:mouse*` handlers | 32 | 8, all hover-only |
| Window or document mouse listeners | 11 | 0 |
| `on:touch*` handlers | 3 | 0 |
| `on:pointer*` and capture handlers | 5 | 49 |
| Drag callbacks typed `MouseEvent` | 2 | 0 |

No interactive control uses `on:mouse*`. The eight survivors are hover
affordances that a tap neither needs nor breaks; on touch, the
compatibility mouse events from a tap leave the last-tapped item lightly
highlighted:

- `layout_canvas.rs`, box `mouseenter`/`mouseleave`: the zone tree mirrors
  the box under the cursor.
- `shell.rs`, command palette rows, two `mousemove`: hover moves the
  keyboard highlight; a tap still runs the row through `click`.
- `pages/studio/device_card.rs`, two `mouseenter`/`mouseleave` pairs:
  hovering a device card highlights its outputs.

Window and document pointer listeners that remain:

- Five `pointerdown` outside-press dismissal handlers. Detecting a press
  outside a popover needs a document or window listener by definition.
- `layout_zone_properties.rs`, window `pointerup` and `pointercancel`
  (unchanged): native range inputs own their drag, and these only close
  the undo bracket that a slider press opens, wherever the release lands.

Drag surfaces outside this wave's pointer-event scope, all HTML5 drag and
drop rather than mouse handlers:

- Dashboard panel reorder (`pages/dashboard/panel_frame.rs`). Touch
  support depends on each mobile browser's drag-and-drop support and is
  unverified; Wave 2's dashboard audit owns a pointer-driven reorder if
  phones need one. Show, hide, width, and reset remain plain buttons.
- Studio palette cards (`layout_palette/devices.rs`, `zone_rows.rs`) set
  drag data that nothing reads, so the drag is a no-op on every input.
- Media library file drop (`pages/media.rs`) is an operating-system file
  drop; the upload button covers touch.

#### Touch QA matrix

Run 2026-10-10 in HeadlessChrome 146 driven over CDP: real touch input
through `Input.dispatchTouchEvent` with touch emulation on (so
`pointer: coarse` matches), and real mouse input through
`Input.dispatchMouseEvent`. The UI ran against an isolated daemon built
without drivers, in its own network namespace with throwaway config and
data directories. Studio used a twelve-output scene fixture, as the e2e
Studio specs do.

| Surface | Check | Touch | Mouse |
| --- | --- | --- | --- |
| Studio canvas, box | Drag moves and commits one undo step; page does not scroll | Pass | Pass |
| Studio canvas, box | Release far outside the canvas commits at the clamped edge, selection kept | | Pass |
| Studio canvas, box | Cancel mid-drag restores geometry, no history entry | Pass | |
| Studio canvas, box | Second finger mid-drag ignored (no move, no selection) | Pass | |
| Studio canvas, box | Tap or click selects; empty-canvas click deselects | Pass | Pass |
| Studio canvas, box | Double-tap or double-click enters the device without nudging | Pass | Pass |
| Studio canvas, box | Shift-press toggles selection without dragging | | Pass |
| Studio canvas, handles | Corner resize grows the box | Pass | Pass |
| Studio canvas, handles | Grab margin on coarse pointers only | Pass | Pass |
| Studio zone-tree splitter | Drag resizes (touch cancel restores) | Pass | Pass |
| Studio bottom-panel splitter | Drag resizes (touch cancel restores; mouse release persists) | Pass | Pass |
| Studio zone properties | Range slider drag is one undoable edit | Pass | |
| Color wheel | Ring drag changes the color (touch also square, with no page scroll) | Pass | Pass |
| Color wheel | Drag keeps tracking outside the canvas and popover | Pass | Pass |
| Color wheel | Cancel mid-drag restores the press-time color | Pass | |
| Color wheel | Second finger does not steer | Pass | |
| Color wheel | Press outside dismisses; release outside does not | Pass | Pass |
| Color wheel at 390x844 | All of the touch checks above | Pass | |
| Effects panel splitter | Drag resizes; cancel restores | Pass | |
| Effects page | Pressing a drag surface still closes an open dropdown | | Pass |
| Viewport picker (Screen Cast) | Grip resize and frame move | Pass | Pass |
| Viewport picker | Cancel mid-move restores the rect; second finger ignored | Pass | |
| Viewport picker | Release far outside ends cleanly, no stuck grab cursor or selection | | Pass |
| Dashboard splitter (1024x768) | Drag resizes and clears the body resize class (touch cancel restores) | Pass | Pass |
| Dashboard layout menu | Press outside dismisses | Pass | Pass |
| Dashboard at 390x844 | Splitter hidden; swipe over the preview scrolls the page; no horizontal scroll | Pass | |

The e2e Studio, Effects, and UI Playwright specs (35 tests) also pass;
their synthetic Studio drags now dispatch `PointerEvent`s.

Not exercised by the matrix:

- Interactive canvas preview input. No interactive effect runs on a
  driverless daemon. The canvas now sets `touch-action: none` while
  interactive and releases held buttons on lost capture; this is covered
  by code review only.
- The full-page layout workspace's palette-column splitter. Studio mounts
  the workspace in compact mode only, so nothing renders it today.
- The Screen Cast picker's preview box renders 2px tall when no aspect
  ratio is passed (a separate, pre-existing bug), so QA gave it a 16:9
  aspect ratio in the page to exercise the drag.

Owed before Wave 1 is closed: the real-device pass from Verification
below, on at least one Android Chrome phone and one iOS Safari phone,
over the same matrix. Emulation cannot judge finger size, palm
rejection, or system edge gestures that fire `pointercancel`.

### Wave 2: Per-page audits

Page by page below `md`, in priority order: Dashboard, Effects,
Devices, Settings, Media, Studio. Effects and control panels are the
core phone use case (browse, apply, tweak live controls, brightness).
Studio's spatial editor stays tablet-and-up; phones get a read-only
zone summary with per-zone effect switching rather than a cramped
editor. Modals and dropdowns get a phone pass (SilkSelect already
portals, so clipping is contained).

Wave 2 also owns three decisions the shell prototype surfaces:

- Toolbar width strategy on phones. Horizontal scrolling requires the
  Effects/Devices filter panels to portal first (SilkSelect-style
  `fixed` positioning); until then toolbars must fit or wrap.
- A phone home for the sidebar's non-nav functions, which `hidden
  md:flex` removes wholesale: Now Playing controls, global
  brightness, and the scene chip. Likely a bottom-sheet off a
  now-playing surface, or a dashboard card.
- An extension-item policy for the bottom bar: six core tabs fit a
  390px phone, and every extension nav item shrinks all of them.
  Probably a "More" overflow tab past six.

### Wave 3: Touch polish

44px minimum touch targets, `overscroll-behavior` on scroll containers,
tap-highlight suppression with visible `:active` states, hover-only
affordances gated on `(hover: hover)`, momentum scrolling checks on
iOS Safari and Android Chrome.

### Wave 4: PWA affordances

Web app manifest, maskable icons, standalone display, theme color.
Constraint to document in the README: a plain-http LAN origin is not a
secure context, so service workers and Chrome's install prompt need
TLS on the daemon (out of scope here); iOS add-to-home-screen works
regardless.

## Verification

Each wave verifies in a real mobile viewport (browser devtools device
emulation at 390x844 plus at least one real phone against a live
daemon) before it merges. Wave 1 additionally verifies drag surfaces
with actual touch input, not emulated mouse events.

## Non-Goals

- Native app wrappers (Tauri mobile): revisit only if LAN discovery
  or app-store presence becomes a real want.
- Daemon TLS.
- Mobile-specific feature removal: every capability except the Studio
  editor remains reachable on phones.
