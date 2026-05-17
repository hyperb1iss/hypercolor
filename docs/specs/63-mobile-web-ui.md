# Spec 63 — Mobile Web UI Shell

> Implementation-ready specification for a viewport-gated mobile shell in
> `hypercolor-ui`. It delivers a touch-first control surface — effects,
> scenes, devices, brightness, color — as a curated subset of the desktop
> web UI, without making the existing desktop pages responsive.

**Status:** Proposed — implementation-ready after 3 Codex review passes
(2026-05-16)
**Author:** Nova
**Date:** 2026-05-16
**Crates:** `hypercolor-ui` (the mobile shell is entirely UI-side; no
daemon changes are required)
**Related:** Spec 40 (device pairing UI), Spec 46 (interactive viewport
designer — desktop-only), `docs/design/43-hypercolor-leptos-ext-spec.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Architecture](#4-architecture)
5. [Mobile Shell and Navigation](#5-mobile-shell-and-navigation)
6. [Screens](#6-screens)
7. [Reuse Map](#7-reuse-map)
8. [Styling, Touch, and Accessibility](#8-styling-touch-and-accessibility)
9. [Implementation Plan](#9-implementation-plan)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

`hypercolor-ui` is a Leptos 0.8 CSR (WASM, Trunk) web UI built as a
fixed-width desktop application. It has a 224px sidebar, multi-column
page layouts, mouse-only drag interactions, and effectively zero
responsive CSS. On a phone it is unusable.

This spec adds a **mobile shell**: a second Leptos component tree,
selected at runtime by viewport width, rendered *instead of* the desktop
shell on narrow screens. It is deliberately a **control surface subset**,
not a port of the full desktop UI.

The mobile shell adds:

- A viewport-gated branch in `AppRoutes` that picks the mobile shell
  below the `md` breakpoint and the existing desktop shell above it
- A touch-first chrome: a compact top bar and a thumb-reachable bottom
  tab bar
- Four primary screens — Now, Effects, Scenes, Devices — plus a Color
  sub-screen and a minimal Settings screen
- Graceful "open on desktop" notices for authoring surfaces (layout
  builder, displays) that do not belong on a phone

The governing principle: **only the view layer is new.** Every screen
consumes the existing shared contexts (`EffectsContext`, `WsContext`,
`DevicesContext`) and calls methods that already exist. No new business
logic, no second WebSocket connection, no duplicated API client.

---

## 2. Problem Statement

### Current state

- `crates/hypercolor-ui/index.html` has a correct `<meta name="viewport">`
  tag. That is the only mobile-aware thing in the crate.
- The layout shell (`components/shell.rs`) is `fixed inset-0 flex` with a
  `w-56` (224px) sidebar marked `shrink-0`. On a 390px phone the sidebar
  consumes 57% of the viewport.
- Responsive design is essentially absent: roughly 13 Tailwind breakpoint
  prefixes (`sm:`/`md:`/`lg:`) exist across 117 source files.
- Hardcoded pixel dimensions are pervasive: panels at 260–420px, headers
  at 60px, dropdowns at 240–360px, several `w-[...]`/`h-[...]` literals.
- Around 20 interactive elements rely on `hover:`-only visual feedback,
  which never fires on touch.
- The layout builder (`components/layout_canvas.rs`) and the viewport
  designer (`control_panel/viewport_picker.rs`) drive drag and resize
  entirely through `mousedown`/`mousemove` handlers with no touch or
  pointer-event equivalents. Both are dead on touch.

### User-visible failure mode

1. User opens the UI on a phone.
2. The sidebar dominates; main content is squeezed into ~166px.
3. Pages overflow horizontally; fixed-width panels clip.
4. The layout builder cannot be operated at all.

### Why a subset, not a responsive retrofit

A phone is a **control context**, not an authoring context. Nobody builds
spatial LED layouts or crops effect viewports on a 390px screen. The
mobile-relevant actions — switch effects, activate scenes, set brightness
and color, glance at device status — map almost one-to-one onto the
verbs the daemon already exposes via MCP (`set_effect`, `set_color`,
`set_brightness`, `activate_scene`, `set_profile`, `get_status`).

Retrofitting the desktop tree to be responsive is a multi-week effort
that fights a heavyweight WASM bundle, requires porting all mouse-only
drag logic to pointer events, and pollutes every desktop component with
breakpoint prefixes. A dedicated mobile shell is smaller, lower-risk, and
keeps the desktop markup untouched.

---

## 3. Goals and Non-Goals

### Goals

- Render a usable, touch-first control surface on phones (target:
  390px-wide portrait viewport).
- Select the shell at runtime by viewport width, with a clean branch in
  the existing `AppRoutes` component.
- Reuse 100% of the existing shared state, API client, and WebSocket
  layer. Mobile screens are presentation only.
- Keep mobile components mobile-first with plain Tailwind classes, so the
  desktop component tree gains zero breakpoint-prefix churn.
- Keep mobile routes deep-linkable through `leptos_router` so the browser
  back button and shared URLs work.
- Degrade gracefully: authoring-only routes show a friendly "open on
  desktop" notice rather than a broken layout or a 404.

### Non-Goals

- Making the existing desktop pages responsive.
- Bringing the layout builder or viewport designer to touch.
- Reducing the shipped WASM bundle size. The full bundle still loads on
  phones; code splitting and lazy loading are explicitly out of scope
  (see §4.5).
- A tablet-specific layout. Tablets (≥768px) get the desktop shell; they
  have the room.
- PWA, offline support, install prompts, or a native wrapper.
- A "request desktop site" override toggle. The viewport gate is
  authoritative in v1.

---

## 4. Architecture

### 4.1 Viewport gating

The shell choice is driven by a single reactive boolean backed by one
`matchMedia` query. The pure decision logic lives in `route_ui.rs`
alongside the existing `now_playing_canvas_mode`, so it is unit-testable
without a DOM:

```rust
/// The mobile shell engages strictly below the Tailwind `md`
/// breakpoint (768px). Tablets and wider get the desktop shell.
pub const MOBILE_MAX_WIDTH_PX: u32 = 767;

pub fn is_mobile_width(width_px: u32) -> bool {
    width_px <= MOBILE_MAX_WIDTH_PX
}
```

The reactive binding is a thin `web-sys` shim in `mobile/viewport.rs`:

```rust
/// Reactive viewport-class signal. `true` while the layout viewport is
/// narrow enough to warrant the mobile shell. Backed by a single
/// `matchMedia` listener registered for the lifetime of the app.
pub fn use_is_mobile() -> Signal<bool>;
```

Implementation notes:

- Query string: `(max-width: 767px)`, derived from `MOBILE_MAX_WIDTH_PX`.
- Seed the signal from `matches()` on creation, then update it from the
  `MediaQueryList` change event.
- Prefer `leptos_use::use_media_query`, which owns the listener
  lifecycle correctly. A hand-rolled `web-sys` implementation MUST tear
  the `change` listener down with an explicit `on_cleanup` — Leptos does
  not auto-drop raw DOM event listeners — and must declare the required
  `web-sys` features in `Cargo.toml`.

### 4.2 The shell branch

`AppRoutes` already demonstrates exactly this pattern. Today it
`<Show>`-branches on `preview_shell_active` to render a bare
`DisplayPreviewPage` instead of `<Shell>`. The mobile shell slots in as a
nested branch in the fallback arm:

```rust
#[component]
fn AppRoutes() -> impl IntoView {
    let location = use_location();
    let preview_shell_active =
        Memo::new(move |_| location.pathname.get() == "/preview");
    let is_mobile = use_is_mobile();

    view! {
        <Show
            when=move || preview_shell_active.get()
            fallback=move || view! {
                <Show
                    when=move || is_mobile.get()
                    fallback=|| view! { <DesktopApp /> }
                >
                    <MobileApp />
                </Show>
            }
        >
            <DisplayPreviewPage />
        </Show>
    }
}
```

`DesktopApp` is a thin extraction of the current `<Shell>` + desktop
`<Routes>` block (a pure refactor, no behavior change). `MobileApp`
wraps `MobileShell` with the mobile `<Routes>`. Only one `<Routes>` block
is mounted at a time, which `AppRoutes` already relies on today.

### 4.3 Shared state — the architectural crux

This is what makes the estimate one week rather than one month.

Every context provider and the `WsManager` connection are created in the
`App` component body **above** the `<Router>` (`app.rs` lines 517–704):
`ConfigContext`, `WsContext`, the device-metrics store,
`PreviewTelemetryContext`, `FrameAnalysisContext`, `PreferencesStore`,
`EffectsContext`, `ThumbnailStore`, `DevicesContext`, `DisplaysContext`.

Consequences, all favorable:

- Mobile screens call `use_context::<EffectsContext>()` etc. and receive
  the **same instances** the desktop uses. One WebSocket connection, one
  effects index, one set of resources.
- When `is_mobile` flips (a resize or rotation that crosses 768px), the
  `<Show>` swaps `MobileApp` ↔ `DesktopApp`. Only the view tree *below*
  the Router remounts. Contexts, the WS connection, and in-flight
  resources survive untouched.
- Shared paths (`/`, `/effects`, `/devices`) survive the swap because
  the active path lives in the URL and both shells use `leptos_router`.
  Mobile-only paths (`/scenes`, `/color`) have no desktop route, so §5.2
  gives each a desktop-side counterpart; without it a cross-breakpoint
  resize or a desktop deep link lands on the "Not found" fallback.

No context hoisting, no provider duplication, no connection churn. The
mobile screens are pure consumers of state and existing context methods
(`apply_effect`, `stop_effect`, `resume_effect`, `toggle_favorite`,
`refresh_active_scene`).

### 4.4 Module layout

All new code lives under one new module, registered with `mod mobile;`
in `main.rs`:

```
src/mobile/
  mod.rs              # MobileApp — wraps MobileShell + mobile <Routes>
  shell.rs            # MobileShell — top bar + tab bar + routed outlet
  viewport.rs         # use_is_mobile() matchMedia shim
  components/
    top_bar.rs        # compact title bar + settings affordance
    tab_bar.rs        # bottom navigation, 4 tabs
  screens/
    now.rs
    effects.rs
    scenes.rs
    devices.rs
    color.rs
    settings.rs
    desktop_only.rs   # "open on desktop" notice
```

Pure routing/breakpoint helpers (`is_mobile_width`, the active-tab
mapping) extend `route_ui.rs` rather than living in `mobile/`, keeping
view-free logic in one tested place.

### 4.5 Bundle tradeoff (accepted)

Housing the mobile shell in the same crate means phones download the
full WASM bundle, including desktop-only code (layout builder, WebGL
preview runtime, perf charts) that the mobile tree never mounts. This is
a deliberate, accepted tradeoff: one build artifact, one deploy, zero
extraction churn, and direct reuse of every shared module.

Bundle optimization — route-level code splitting, lazy module loading,
or extracting a separate lean mobile crate — is explicitly out of scope.
It remains available as a later, independent optimization if cold-start
on mobile networks proves painful. Per project policy, no performance
ceiling is lowered to paper over bundle weight.

---

## 5. Mobile Shell and Navigation

### 5.1 Chrome

`MobileShell` is a vertical flexbox filling the viewport:

- **Top bar** (`components/top_bar.rs`): ~56px plus top safe-area inset.
  Shows the current screen title, a daemon-connection status dot, and a
  settings affordance that routes to `/settings`. The status dot needs
  connection state on context. `WsManager::new` builds a
  `connection_state` signal locally but the `WsManager` struct does not
  keep it; Wave 1 promotes it to a `WsManager` field and then exposes it
  on `WsContext`.
- **Content outlet**: the routed screen, vertically scrollable, occupying
  the remaining space.
- **Bottom tab bar** (`components/tab_bar.rs`): ~56px plus bottom
  safe-area inset. Four tabs, each an icon plus a short label, every tap
  target ≥44px square.

Safe-area handling: the top and bottom bars pad with
`env(safe-area-inset-top)` / `env(safe-area-inset-bottom)` for notched
and gesture-bar phones. This requires `viewport-fit=cover` on the
viewport meta tag. Today that tag is declared twice — statically in
`index.html` and via `leptos_meta` in `app.rs`. Both must be updated to
`width=device-width, initial-scale=1.0, viewport-fit=cover`, and the
duplication should be resolved to a single source of truth.

### 5.2 Routes

The mobile `<Routes>` block reuses `leptos_router` paths so deep links
and the back button behave:

| Path             | Screen                | Notes                              |
| ---------------- | --------------------- | ---------------------------------- |
| `/`              | Now                   | Tab 1                              |
| `/effects`       | Effects               | Tab 2                              |
| `/effects/:id`   | Effects               | Deep link to a focused effect      |
| `/scenes`        | Scenes                | Tab 3 — new path; no desktop page  |
| `/devices`       | Devices               | Tab 4                              |
| `/color`         | Color                 | Sub-screen, reached from Now       |
| `/settings`      | Settings              | Reached from the top bar           |
| `/layout`        | Desktop-only notice   | Authoring surface                  |
| `/displays`      | Desktop-only notice   | Authoring surface (v1)             |
| fallback         | Redirect to `/`       |                                    |

The bottom tab bar surfaces only the four primary tabs. `/color`,
`/settings`, `/layout`, and `/displays` are reachable but not tabs.

`/scenes` and `/color` are paths the desktop shell does not define. So a
cross-breakpoint resize and desktop deep links do not hit the desktop
"Not found" fallback, the desktop `<Routes>` gains a matching entry for
each — a redirect to `/` is sufficient. Tracked in Wave 1.

### 5.3 Active-tab logic

The mapping from current path to highlighted tab is pure and lives in
`route_ui.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MobileTab {
    Now,
    Effects,
    Scenes,
    Devices,
}

/// Maps a router path to the bottom-tab that should read as active.
/// Non-tab routes (`/color`, `/settings`) resolve to their nearest
/// conceptual parent tab.
pub fn active_mobile_tab(path: &str) -> MobileTab;
```

This is unit-tested directly (see §9), mirroring how
`now_playing_canvas_mode` is covered by `route_ui_tests.rs`.

---

## 6. Screens

Each screen is presentation over existing context state. Behavior below
names the exact context fields and methods consumed.

### 6.1 Now (`/`)

The glance-and-tweak home screen.

- **Active effect**: `EffectsContext.active_effect_name` and
  `active_effect_category`, with playing state from `is_playing`.
- **Power**: a large toggle. Off calls `stop_effect()`; on calls
  `resume_effect()`.
- **Master brightness**: a full-width slider backed by the existing
  `fetch_global_brightness` / `set_global_brightness` client calls
  (`GET`/`PUT /api/v1/settings/brightness`). No daemon change is needed.
- **Quick scenes**: a horizontal row of chips for the first few scenes,
  each tapping through to scene activation. The active scene reads as
  selected, matched by `active_scene_id` (§7).
- **Color entry**: a button routing to `/color`.
- **Live preview** (stretch, not v1-blocking): a small canvas fed by
  `WsContext.canvas_frame`. Gated behind the preview consumer counter so
  it does not add render load when not displayed.

### 6.2 Effects (`/effects`, `/effects/:id`)

- **Search**: a text input filtering `EffectsContext.effects_index`
  through the existing view-free `effect_search` module
  (`IndexedEffect`).
- **Category chips**: quick filters by effect category.
- **List**: vertically scrolling effect rows. Tap calls
  `apply_effect(id)`. The active effect reads as selected.
- **Favorite**: a star control per row calling `toggle_favorite(id)`;
  `favorite_ids` drives the filled state.
- `/effects/:id` scrolls to and focuses the addressed effect.

### 6.3 Scenes (`/scenes`)

- **List**: `api/scenes.rs` today wraps only active-scene read and
  deactivate — it has no list or activate call. Wave 2 extends it with
  `fetch_scenes` and `activate_scene`
  (`POST /api/v1/scenes/:id/activate`), pulled forward from Wave 3
  because the Now screen's quick-scene chips need the same two calls.
  Tap activates a scene; the active row reads as selected, matched by
  `active_scene_id` (§7) rather than by non-unique scene name.
- **Profiles**: cut from v1. Profile activation is its own endpoint,
  `POST /api/v1/profiles/:id/apply` — not the `POST /api/v1/profiles`
  create route — and needs a new `api/profiles.rs` client
  (`list_profiles` plus `apply_profile`). It lands as a fast follow once
  the scenes screen is solid.

### 6.4 Devices (`/devices`)

- **List**: `DevicesContext.devices_resource`. Each row shows device
  name, online/offline state, and an inline per-device brightness slider.
- **Brightness slider**: a native `<input type="range">`. Do not reuse
  `device_brightness_slider.rs` by deleting its `max-w-[180px]` — that
  component is consumed by the desktop layout builder, and editing it in
  place risks a desktop regression. Add an optional width prop to the
  shared component, or build a thin mobile wrapper. The slider writes
  per-device brightness through the device update API.
- **Device sheet**: tapping a row opens a detail sheet — a deliberate
  subset of the desktop `device_detail`: status, brightness,
  enable/disable, identify. The dynamic driver control surface
  (Spec 52) is desktop-only and excluded. If Wave 4 runs tight the
  sheet can ship as a fast follow, with the list row carrying the
  essential controls inline.
- Device list refresh already happens reactively from
  `last_device_event`; mobile inherits this for free.

### 6.5 Color (`/color`)

- A full screen reached from Now. Reuses `components/color_wheel.rs`,
  which **already implements `touchstart`/`touchmove`/`touchend`** — the
  single fiddliest control is touch-ready as-is.
- A brightness/value slider and an apply action. The screen drives the
  builtin `solid_color` effect, whose controls are `color` and
  `brightness`. The apply path does not go through
  `EffectsContext.apply_effect` — that method takes only an id and
  early-returns when the effect is already active, so it can neither
  carry the chosen color nor handle a repeat change. The first apply
  calls `api::apply_effect("solid_color", Some(&ApplyEffectBody { .. }))`
  with `color` and `brightness` in the body; every later change goes
  through `api::update_controls`
  (`PATCH /api/v1/effects/current/controls`) with the same keys.
- Back returns to Now.

### 6.6 Settings (`/settings`)

Minimal by intent:

- Theme toggle (light/dark). The existing `index.html` theme script and
  `localStorage` key are reused; no new theming machinery.
- Daemon connection status and, when required, the API-key entry. The
  existing `ApiKeyPrompt` flow already overlays globally and needs no
  mobile-specific work.
- App version.

### 6.7 Desktop-only notice (`/layout`, `/displays`)

`screens/desktop_only.rs` renders a single calm message — for example,
"The layout builder needs a larger screen. Open Hypercolor on a desktop
to edit layouts." — with a button back to Now. This keeps deep links and
stray navigation from producing a broken or empty view.

---

## 7. Reuse Map

### Reused as-is (no changes)

- `src/api/*` HTTP client and typed endpoints, except `api/scenes.rs`
  (extended below).
- `src/ws/*` message types and the `WsManager` connection.
- `EffectsContext`, `DevicesContext`, `DisplaysContext`, `ConfigContext`
  and their methods.
- `effect_search` (filter logic), `color` (color math), `toasts`,
  `preferences`, `thumbnails`, `storage`.
- SilkCircuit design tokens (`tokens/primitives.css`,
  `tokens/semantic.css`, `input.css`) and the Tailwind v4 setup.

### Reused components

- `color_wheel.rs` — already touch-capable.
- `device_brightness_slider.rs` — native range input. Reused through a
  new optional width prop or a mobile wrapper; its markup is not edited
  in place, because the desktop layout builder depends on it.

### Small additive changes to shared code

Each is a contained edit, not green-field work, but the spec counts
these as new rather than free reuse:

- `connection_state` is promoted to a field on the `WsManager` struct
  (today it is a local signal in `WsManager::new`) and then exposed on
  `WsContext`.
- `EffectsContext` gains an `active_scene_id` field, set from
  `ActiveSceneResponse.id` and `SceneEventHint.scene_id`, so scene lists
  mark the active row by id rather than by non-unique name.
- `api/scenes.rs` gains `fetch_scenes` and `activate_scene`.
- A Color-screen apply path: a first
  `api::apply_effect("solid_color", Some(&ApplyEffectBody { .. }))` call
  carrying the `color` and `brightness` controls, then
  `api::update_controls` for later changes — `EffectsContext.apply_effect`
  takes only an id and early-returns on an already-active one.
- A per-device brightness write path for the Devices screen.

### New

- `src/mobile/` (shell, viewport shim, top bar, tab bar, seven screens).
- `MobileTab` + `active_mobile_tab` + `is_mobile_width` in `route_ui.rs`.
- The `AppRoutes` branch and the `DesktopApp` extraction in `app.rs`.
- Desktop `<Routes>` redirects for `/scenes` and `/color`.
- `mod mobile;` in `main.rs`.
- The `viewport-fit=cover` meta update.

### Excluded from mobile

Layout builder, viewport designer, perf-chart dashboard, install-effect
panel, attachment editor, and the dynamic driver control surface. These
are authoring tools; they remain desktop-only and their code simply is
not mounted by the mobile tree.

---

## 8. Styling, Touch, and Accessibility

- **Mobile-first, no prefixes.** Mobile components render only below
  768px, so they use plain Tailwind utilities with no `sm:`/`md:`
  prefixes. The desktop component tree gains zero responsive-prefix
  churn — a key goal of the same-crate approach.
- **Tokens.** Mobile components consume the existing SilkCircuit
  semantic tokens. No new palette, no new design system.
- **Touch targets.** Every interactive element is ≥44×44px. Tab bar
  items, list rows, sliders, and toggles are sized for thumbs.
- **No hover dependence.** Mobile components never encode meaning in
  `hover:` alone. State is shown with explicit selected/active styling.
- **Safe areas.** Top and bottom chrome pad with `env(safe-area-inset-*)`
  under `viewport-fit=cover`.
- **Motion.** `input.css` already carries a
  `@media (prefers-reduced-motion: reduce)` block; mobile transitions
  respect it.
- **Scrolling.** The content outlet owns vertical scroll; chrome stays
  fixed. Avoid nested scroll traps.

---

## 9. Implementation Plan

### Wave 1: Viewport gating and shell skeleton

- Add `MOBILE_MAX_WIDTH_PX`, `is_mobile_width`, `MobileTab`, and
  `active_mobile_tab` to `route_ui.rs`.
- Add `mobile/viewport.rs` with `use_is_mobile()`.
- Add `mobile/mod.rs`, `mobile/shell.rs`, `mobile/components/top_bar.rs`,
  `mobile/components/tab_bar.rs` with placeholder screens wired to the
  mobile `<Routes>`.
- Promote `connection_state` to a field on the `WsManager` struct
  (`ws/connection.rs`), then expose it on `WsContext` (`app.rs`).
- Add desktop `<Routes>` redirects for `/scenes` and `/color` so a
  resize across the breakpoint cannot land on "Not found".
- Extract `DesktopApp` and add the `is_mobile` branch in `AppRoutes`.
- Register `mod mobile;` in `main.rs`; update both viewport meta tags to
  `viewport-fit=cover` and resolve the duplication.

### Wave 2: Now and Effects screens

- Extend `api/scenes.rs` with `fetch_scenes` and `activate_scene`,
  pulled forward from Wave 3: the Now screen's quick-scene chips need
  both, and the module currently wraps only active-scene read and
  deactivate.
- Build `screens/now.rs`: power toggle, master brightness via
  `set_global_brightness`, quick-scene chips, color entry.
- Build `screens/effects.rs`: search, category chips, list, apply,
  favorite toggle.

### Wave 3: Scenes and Devices screens

- Build `screens/scenes.rs` on the `fetch_scenes` / `activate_scene`
  calls added in Wave 2. Profiles are not in v1; see the fast-follow
  note in §6.3.
- Build `screens/devices.rs`: device list plus a per-device brightness
  control that writes through the device update API. Give
  `device_brightness_slider.rs` an optional width prop, or wrap it, so
  the shared desktop component is not edited in place.
- The device detail sheet (status, enable/disable, identify) is the
  flex item: if the wave runs tight it becomes a fast follow, with the
  list row carrying brightness inline.

### Wave 4: Color, Settings, notices, and polish

- Build `screens/color.rs` reusing `color_wheel.rs`.
- Build `screens/settings.rs` (theme, connection, version) and
  `screens/desktop_only.rs`; route `/layout` and `/displays` to it.
- Polish: safe-area insets, 44px audit, scroll behavior, toast
  placement above the tab bar.

### Candidate files

- `crates/hypercolor-ui/src/main.rs`
- `crates/hypercolor-ui/src/app.rs`
- `crates/hypercolor-ui/src/route_ui.rs`
- `crates/hypercolor-ui/src/mobile/` (new module, per §4.4)
- `crates/hypercolor-ui/src/ws/connection.rs`
- `crates/hypercolor-ui/src/components/device_brightness_slider.rs`
- `crates/hypercolor-ui/src/api/scenes.rs`
- `crates/hypercolor-ui/src/api/profiles.rs` (new — profiles fast
  follow, not v1)
- `crates/hypercolor-ui/Cargo.toml`
- `crates/hypercolor-ui/index.html`
- `crates/hypercolor-ui/tests/route_ui_tests.rs`

### Testing

The view-free helpers (`is_mobile_width`, `active_mobile_tab`) carry unit
tests in `tests/route_ui_tests.rs`, per the project convention that tests
live in `tests/`. The `matchMedia` shim and the screen components are
verified at a 390px viewport rather than by unit test.

---

## 10. Recommendation

Ship the mobile experience as a **viewport-gated shell inside
`hypercolor-ui`**, rendering a curated control-surface subset rather than
a responsive port of the desktop UI.

This is the right call because:

- The crate is already shaped for it. Every shared context lives above
  the `<Router>`, and `AppRoutes` already branches shells for the
  `/preview` route. The mobile branch is additive and low-risk.
- Mobile screens are presentation only. They consume existing context
  methods, so there is no new business logic, no second WebSocket
  connection, and no duplicated API client to keep in sync.
- Desktop markup stays clean. Because mobile components only ever render
  below 768px, they need no breakpoint prefixes, and the desktop tree
  gains none.
- The four-wave plan is contained. Roughly one week reaches a working
  skeleton plus the Now and Effects screens; a complete v1 — scenes,
  color, the device sheet, on-device QA, no desktop regressions — is
  closer to one and a half to two weeks. That still beats a multi-week
  responsive retrofit that would also have to port every mouse-only
  drag interaction to touch.

The accepted tradeoff is bundle weight: phones download the full WASM
binary including desktop-only code they never mount. That is acceptable
for v1 and leaves code splitting or a separate lean crate available as a
later, independent optimization.

Concrete recommendation:

1. Add viewport gating and the `AppRoutes` shell branch.
2. Build the four-tab mobile shell — Now, Effects, Scenes, Devices —
   plus Color and Settings.
3. Reuse all shared state, the API client, the WebSocket layer, the
   color wheel, and the SilkCircuit tokens.
4. Route authoring surfaces to a graceful desktop-only notice.
