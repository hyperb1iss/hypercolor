# Spec 75: Mobile-Responsive Web UI

Status: In progress (status line added 2026-08-25; this file previously carried none).
Wave 0 shipped: `MobileNav` (`crates/hypercolor-ui/src/components/mobile_nav.rs`) and
the responsive breakpoint pass are live. Wave 1 shipped on 2026-10-10 and passed
emulated touch QA; its real-device pass is still owed (see Wave 1 below). Waves 2
and 3 were built on 2026-10-31 and passed emulated touch QA; their real-device pass
is owed with Wave 1's. Wave 4 is open. This spec supersedes the archived Spec 63,
which designed a separate mobile shell that was never built.

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
- A release commits. So does releasing the pressing button while another
  stays held: pointer events report that as a `pointermove`, and the old
  `mouseup` handlers ended the drag there. `pointercancel`, or a capture
  lost before the release, cancels: the surface restores its press-time
  state (geometry, color, rect, or panel size) without a geometry edit in
  history. A gesture whose end never arrived (the owner re-presses, or no
  longer holds capture) is rolled back on the next press instead of
  locking the surface.
- An element that leaves the DOM mid-drag loses capture to the document,
  not to itself. The Studio canvas (Escape clears the selection and with
  it the resize handles) and the layout workspace splitters (rendered
  under `Show`) therefore cancel from a window `lostpointercapture`
  listener, removed on cleanup.
- The color popover stays open under a live wheel drag: the wheel marks
  its canvas with `data-color-wheel-dragging`, and the popover's
  outside-press and scroll dismissal skip while it is set, so a second
  finger landing outside cannot unmount the wheel mid-drag.
- A control that unmounts mid-drag because its effect or page changed
  drops the gesture without rolling back, as the mouse handlers did.
  Rolling back there would write through the control session, which
  targets whichever effect is active by then; a correct rollback needs
  delivery bound to the press-time target that outlives the session.
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

Six of those eleven, the drag listeners in the resize handle, the
viewport picker, and the layout workspace, also leaked. Leptos 0.8's
`window_event_listener` handle neither removes its listener on drop nor
registers a cleanup, and the old code kept the handles only as unused
bindings, so every mount added listeners that were never removed. The
five dismissal listeners went through leptos-use, which cleans up. The
Studio canvas `resize` listener, the layout workspace's undo shortcuts,
and the dashboard's fullscreen Escape handler leaked the same way and
are now removed on cleanup too; a stale undo shortcut used to panic on
the next Ctrl+Z after leaving Studio.

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
- Two `lostpointercapture` window listeners (Studio canvas, layout
  workspace splitters), for captured elements that leave the DOM
  mid-drag. Both are removed on cleanup.
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
| Studio canvas, box | Releasing the left button while the right stays held commits there | | Pass |
| Studio canvas, handles | Corner resize grows the box | Pass | Pass |
| Studio canvas, handles | Grab margin on coarse pointers only | Pass | Pass |
| Studio canvas, handles | Escape mid-resize removes the handles and cancels; later moves paint nothing | | Pass |
| Studio zone-tree splitter | Drag resizes (touch cancel restores) | Pass | Pass |
| Studio bottom-panel splitter | Drag resizes (touch cancel restores; mouse release persists) | Pass | Pass |
| Studio zone properties | Range slider drag is one undoable edit | Pass | |
| Color wheel | Ring drag changes the color (touch also square, with no page scroll) | Pass | Pass |
| Color wheel | Drag keeps tracking outside the canvas and popover | Pass | Pass |
| Color wheel | Cancel mid-drag restores the press-time color | Pass | |
| Color wheel | A second finger pressing outside mid-drag leaves the popover open; release commits; a later outside tap dismisses | Pass | |
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
  interactive, holds a button down while any pointer holds it,
  reconciles each pointer's buttons from the event's `buttons` mask (so a
  chorded press or release arriving as a `pointermove` reaches the
  effect), and releases a pointer's buttons when it is cancelled or loses
  capture.
  The button bookkeeping (`HeldButtons`) is unit-tested; the wiring is
  covered by code review only.
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

Landed 2026-10-31. Every page was audited below `md` in priority order
(Dashboard, Effects, Devices, Settings, Media, Studio), with the three
shell decisions this wave owned settled first.

#### Phone shell

- **Now-playing strip.** The sidebar's player has no home below `md`,
  so a compact strip (`components/phone_now_playing.rs`) docks above the
  bottom tab bar on every page, like a mini player: a live thumbnail (the
  palette swatch on pages with their own preview), what is playing, and
  play/pause and next at 44px. Tapping it opens a bottom sheet with the
  live preview, the full transport (shuffle, previous, play/pause, next,
  and a jump to the effect controls), global brightness, every zone of a
  multi-zone scene with its own pause, and the scene list. The scene list
  is a radio group with one tab stop, the checked scene; arrow keys, Home,
  and End move focus and check the scene they land on. `<main>` pads by
  the bar, the strip, and the home-indicator inset (`.shell-main`,
  `--phone-nav-h` and `--phone-strip-h`), and toasts sit above both.
- **One player model.** The sidebar and the strip read one shared
  `NowPlaying` context (`components/now_playing.rs`): the zone the player
  steers, transport actions, the ambient `--np-*` palette, and global
  brightness with its single throttled writer. The stepping, shuffle, and
  caption rules are pure and tested (`tests/now_playing_tests.rs`).
- **Scene picker on phones.** Earlier drafts of this spec listed a
  sidebar "scene chip". By this wave it had already moved into the page
  headers as `ScenePicker` (Dashboard, Effects, Studio), which phones
  render. The sheet adds a scene switcher reachable from every page;
  creating, renaming, and deleting scenes stay in the header picker.
- **More tab.** The bottom bar keeps the six core tabs. When an extension
  contributes nav items (`mobile_only` ones included), a seventh More tab
  appears and those items live in its sheet; with none, the bar is
  exactly the six (`nav::phone_nav`, tested in `nav.rs`). Seven tabs on a
  360px phone are 51px each, above the 44px floor.
- **Bottom sheet.** `components/bottom_sheet.rs` is the phone dialog: the
  shared `Modal` behavior (dialog semantics, Escape, backdrop tap, focus
  trap and restore) plus a grab area that drags the sheet down. A release
  past `min(96px, 25%)` of the sheet's height, or a downward flick of
  0.5px/ms after 24px, dismisses; a cancel or lost capture snaps back.
  The flick speed decays with the time since the last move (40ms time
  constant), so a release after a pause is judged by position alone. The
  drag follows the Wave 1 gesture model. The rules are tested in
  `tests/bottom_sheet_tests.rs`.
- **Mount, not just hide.** `viewport::is_phone_layout` is a synchronous
  `(width < 48rem)` signal, provided at the app root; every phone rule in
  `input.css` uses the same query, as Tailwind's `max-md:` does. Surfaces that cost
  work mount on one side of the boundary only: the strip on phones, the
  sidebar's live thumbnail (which kept a hidden preview stream running on
  phones) on desktop, and the Studio editor on tablet and desktop.

#### Pages

- **Dashboard.** The header's layout gear is 44px on phones. The panels'
  floating width, hide, and reorder bar is revealed by hover only, so on
  touch it sat at opacity 0 yet stayed tappable: a tap near a panel's top
  right corner could hide the panel blind. It is gone on touch screens; the
  layout menu still shows, hides, and resets panels. The
  favorites' remove button, the same pattern, now shows on touch. The
  preset strip gives its dropdown a full row on phones, with the four
  actions at 44px on a second row.
- **Effects.** Phones split the page into Browse and Controls under two
  tabs. Browse is the grid at two compact cards per row (the description
  yields to the title); tapping a card applies it and the strip shows it
  at once. Controls is the live preview and controls, always docked. The
  view lives in the URL: Controls is `?view=controls` and Browse is the
  bare page. Every switch (the tabs, typing a search, opening the
  filters) writes the URL with `replace`, and the page reads its view
  back from the query (`pages/effects/phone_view.rs`). A view kept beside
  the URL went stale: the query is a memo, so the sheet's link to
  `?view=controls` did nothing after a visit through it and a switch to
  Browse. It is a full tabs pattern rather than a segmented control,
  because the tabs swap which content the page shows: both tabs control
  one tab panel (the page body), the selected tab is the one tab stop,
  and Left, Right, Home, and End select and focus as they move.
- **Devices.** The detail drawer's close button is 44px on phones. The
  page's Escape listener leaked on every visit (Leptos 0.8 never removes
  a window listener on its own) and is removed on cleanup now.
- **Settings.** On phones the tab strip follows the scroll spy, sliding
  the active tab to the middle, and fades at its trailing edge. The About
  card's config path wraps instead of widening the page to 487px.
- **Media.** The grid comes first at two tiles per row; a tapped file
  opens its details in a bottom sheet instead of a rail stacked over the
  grid. The kind chips scroll in place with an edge fade; the strip fills
  the toolbar row and clips vertically, so the chips' finger margins
  cannot make it scroll up and down.
- **Studio.** Below `md` (a phone, or a desktop window narrower than
  768px) Studio shows `pages/studio/phone_summary.rs` instead of the
  editor, under a note that names the room the editor needs ("Arrange on
  a bigger screen") rather than a kind of device: a card per LED zone with its hardware ("12 outputs on 4
  devices"), what it plays, a pause toggle, and a Change button that opens
  an effect picker for that zone alone (favorites first, display faces
  excluded). Screens are listed read-only. The editor never mounts below
  `md`. Counting and filtering are tested in
  `tests/studio_phone_summary_tests.rs`.
- **Modals and dropdowns.** `Modal` leaked its keydown listener on every
  open; it is removed on unmount now, which every sheet relies on. Menu
  rows in every dropdown, the scene picker trigger, SilkSelect triggers,
  and toolbar search and buttons reach 44px on coarse pointers.

Toolbar width strategy: toolbars fit or wrap. The two strips that scroll
in place (Media's kind chips, Settings' section tabs) hold buttons only,
no dropdowns, so a clip box cannot shred a panel, and each fades at its
trailing edge (`edge-fade-x`) so the cut-off item reads as more to come.

### Wave 3: Touch polish

Landed 2026-10-31.

- **Targets.** Cursor-sized controls get an invisible finger margin on
  coarse pointers (`touch-hit`, and `touch-hit-y` for chips and tabs
  packed side by side), or grow to 44px outright where layout allows.
  Range inputs take a 44px hit box and a 22px thumb on coarse pointers
  over the same visible track: the background is clipped to the track,
  which overrides the inline track colors some sliders set, and a
  `3px / 50%` radius keeps the track's ends round under the padding.
- **Hover.** Every custom `:hover` rule in `input.css` sits behind
  `@media (hover: hover)`; Tailwind 4's `hover:` variant already does. A
  tap no longer leaves a card or button stuck in its hover look.
  Hover-revealed row actions (`touch-reveal`) show outright on touch
  screens, matched as `(hover: none) and (pointer: coarse)` so a
  headless browser, which reports neither hover nor a pointer, keeps the
  desktop look. The eight remaining `on:mouse*` handlers stay hover-only
  by design (see the Wave 1 census) and need no gate: on touch they only
  move a highlight.
- **Taps.** `-webkit-tap-highlight-color` is transparent; presses show
  through each control's own `:active` state.
- **Scrolling.** Below `md` every scroll owner contains its overscroll
  (page scrollers, sheet bodies, drawers, lists, and dropdown panels;
  horizontal strips contain sideways overscroll), and the document has
  none, so reaching an edge never drags the page
  under a sheet or triggers pull-to-refresh on the fixed shell. Momentum
  scrolling is native on both target engines for `overflow: auto`, so no
  `-webkit-overflow-scrolling` is needed.
- **Safe areas.** The tab bar, the strip, `<main>`, toasts, and every
  sheet body pad by `env(safe-area-inset-bottom)`.
- **Focus zoom.** Text fields are 16px below `md`, since iOS Safari zooms
  the page into any smaller field on focus.

#### Touch QA matrix (waves 2 and 3)

Run 2026-10-31 with the Wave 1 method: HeadlessChrome 156 over CDP with
touch emulation (`pointer: coarse`, no hover) and real touch input
through `Input.dispatchTouchEvent`, against an isolated daemon built
without drivers, in its own network namespace with throwaway config and
data, reaching neither the live daemon nor a dev server. The daemon was
seeded with a saved three-zone scene, two more saved scenes, two
simulated displays, favorites, and three media files. The More tab ran
against a scratch embedder that registers two extension nav items, one
of them `mobile_only`. The rows from the Wave 2 and 3 fix round were
run against the build before each fix first, and failed there.

CDP touch dispatch reaches the page 12 to 80ms per move here, far below
flick speed, so the flick rule is checked with synthetic pointer events
dispatched in the page at controlled spacing, as the e2e Studio specs
do for drags.

| Surface | Check | 390x844 | 360x800 |
| --- | --- | --- | --- |
| Now-playing strip | Docked above the tab bar on every page; its targets are 44px or more | Pass | Pass |
| Now-playing strip | Next steps the effect; pause shows Paused, resume clears it | Pass | Pass |
| Player sheet | A tap on the strip opens it | Pass | Pass |
| Player sheet | A short slow drag keeps it; a cancelled drag snaps back; a long drag dismisses; a backdrop tap dismisses | Pass | Pass |
| Player sheet | Previous and shuffle step the effect; the controls button opens Effects in Controls | Pass | Pass |
| Player sheet | A brightness drag reaches the daemon; the slider's hit box is 44px | Pass | Pass |
| Player sheet | Switching to another scene and back | Pass | Pass |
| More tab | Six core tabs plus More, each at least 44px (55px and 51px wide) | Pass | Pass |
| More tab | The sheet lists the extension items; a tap navigates and closes it; More shows active on an extension route | Pass | Pass |
| Effects | `?view=controls` opens Controls with the preview mounted; typing a search or opening Filters returns to Browse, where the preview unmounts | Pass | Pass |
| Effects | After leaving Controls by the tab, a search, or Filters, the sheet's controls link lands on Controls again; Browse drops `view` from the URL | Pass | Pass |
| Effects | Both tabs control one tab panel that names the selected tab; one tab stop; ArrowRight and Home select and focus | Pass | Pass |
| Player sheet | A fast pull released at once dismisses; the same pull released after a 350ms pause keeps the sheet | Pass | Pass |
| Player sheet | The scene radios keep one tab stop; ArrowDown and ArrowUp move focus and check the scene | Pass | Pass |
| Sliders | Settings, device brightness, and control sliders (and the sidebar's at 1024px with touch) keep a thin, round track inside a hit box of 44px or more | Pass | Pass |
| Every page | Every scroll owner, open filter, scene, and select panels included, contains its overscroll | Pass | Pass |
| Studio | The note reads "Arrange on a bigger screen" at both sizes and in a 700px desktop window | Pass | Pass |
| Effects | Tapping a card applies it and the strip shows it; the card keeps no hover look | Pass | Pass |
| Effects | A touch drag moves a control slider; its hit box is at least 44px | Pass | Pass |
| Studio | The editor never mounts; a zone's picker switches only that zone; a card's pause and resume work | Pass | Pass |
| Media | A tile opens its details in a sheet | Pass | Pass |
| Every page | No horizontal overflow, no page errors, no visible control under 44px | Pass | Pass |
| Touch polish | Tap highlight transparent; the page scroller contains overscroll; the document has none; text fields are 16px | Pass | Pass |

Light theme: the Effects grid, the Studio summary, and the player sheet
were checked at 390x844. The strip and sheet captions moved from the
palette tint to theme text tokens after cyan on white proved unreadable.

Desktop regression: every page at 1440x900, before (main 2e871a0b0) and
after, each shot twice to mask live content. Studio, Media, Devices,
and Settings are pixel-identical. Dashboard and Effects differ only in
live content: Dashboard's readouts (uptime, fps, and the engine, frame,
and preview numbers), and on Effects the running effect's card art,
which the gallery captures from live frames. A headless desktop reports
neither hover nor a pointer, so its run also proves the touch-only rules
stay off there. The
desktop sidebar still lists an extension item and hides a `mobile_only`
one.

Not exercised: safe-area insets (no emulated notch; the padding is
checked in CSS), iOS Safari focus zoom and momentum scrolling (the
16px fields and native `overflow: auto` are checked instead), and the
real-device pass, which stays owed for waves 1 through 3 together.

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
