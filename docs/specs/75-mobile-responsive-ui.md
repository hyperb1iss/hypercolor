# Spec 75: Mobile-Responsive Web UI

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
sidebar. Tablets (`md`–`lg`) keep the desktop shell with the collapsed
sidebar as the natural mid-size layout. Hover-dependent affordances
gate on `@media (hover: hover)` rather than width.

## Waves

### Wave 0 — Shell (prototype, shipped on this branch)

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

### Wave 1 — Pointer-event migration

Convert drag surfaces from mouse events to pointer events with
`setPointerCapture` and `touch-action: none` on the drag origin:

- `color_wheel.rs` (the flagship touch surface)
- `resize_handle.rs`, dashboard splitter
- `layout_canvas.rs` zone drag/resize (Studio)
- range sliders already work via native inputs; verify thumb hit areas

Extract the sidebar/mobile-nav active-route matcher into `nav.rs` as a
tested pure function while touching this area. Non-drag `on:mouse*`
uses (hover selection in the command palette, row highlight) stay
mouse-only by design and get audited, not converted.

### Wave 2 — Per-page audits

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

### Wave 3 — Touch polish

44px minimum touch targets, `overscroll-behavior` on scroll containers,
tap-highlight suppression with visible `:active` states, hover-only
affordances gated on `(hover: hover)`, momentum scrolling checks on
iOS Safari and Android Chrome.

### Wave 4 — PWA affordances

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

- Native app wrappers (Tauri mobile) — revisit only if LAN discovery
  or app-store presence becomes a real want.
- Daemon TLS.
- Mobile-specific feature removal: every capability except the Studio
  editor remains reachable on phones.
