# Spec 65 — Studio: Unified Composition UI

> Replace the Assets and Displays pages with a single surface-centric
> composition workspace. Every lighting target and device screen is a
> selectable entry; selecting one loads its layer stack and a live
> preview. Media becomes its own catalog page. The full multi-zone UX —
> N zones, each a device partition with its own layer stack — is designed
> here as one cohesive workspace, delivered in waves. The new UI is built
> in parallel with the existing pages behind a runtime feature flag,
> reusing the already-polished layer manager.

**Status:** Draft (revised after Codex cross-model review, 2026-05-17)
**Author:** Nova
**Date:** 2026-05-17
**Crates:** `hypercolor-ui`
**Depends on:** User Media & Layer Stack (60), Interactive Viewport Designer (46)
**Pairs with:** Multi-Zone Scenes (64) — Spec 65 owns the zone UX, Spec 64 owns the engine and API
**Forward-compatible with:** Mobile Web UI (63)
**Related:** brainstorm decision `episode_7dbe6d909236`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [User-Facing Vocabulary](#4-user-facing-vocabulary)
5. [Page Architecture](#5-page-architecture)
6. [The Studio Page](#6-the-studio-page)
7. [The Media Page](#7-the-media-page)
8. [Surface Model](#8-surface-model)
9. [Multi-Zone UX](#9-multi-zone-ux)
10. [Reusing the Layer Manager](#10-reusing-the-layer-manager)
11. [Parallel Build and Feature Flag](#11-parallel-build-and-feature-flag)
12. [API Surface and Backend Dependencies](#12-api-surface-and-backend-dependencies)
13. [Mobile and Responsive Behavior](#13-mobile-and-responsive-behavior)
14. [Delivery Waves](#14-delivery-waves)
15. [Verification Strategy](#15-verification-strategy)
16. [Known Constraints](#16-known-constraints)
17. [Recommendation](#17-recommendation)
18. [Appendix A — File Inventory](#18-appendix-a--file-inventory)

---

## 1. Overview

Spec 60 landed the layer-stack substrate: every render group carries
`Vec<SceneLayer>`, media is a first-class layer source, and the daemon
exposes per-group layer endpoints. The UI that shipped with it does not
match the model. The `/assets` page is a media library with the
layer-stack editor bolted into its right rail, below the selected file's
metadata. The `/displays` page is a parallel face-assignment tool for the
same construct — a render group with a layer stack — addressed to device
screens instead of the LED canvas.

Both pages edit the same thing. Spec 60 §1 states the architecture
plainly: content source (effect, media, screen, web) is independent of
consumer (LED via spatial sampling, or device screen via direct routing),
and one layer-stack contract serves both. The UI splits that one contract
across two pages by content type and by consumer, so a user composing a
gif over an effect has to know whether the destination is "an asset thing"
or "a display thing" to find the editor.

Spec 65 replaces both pages with **Studio**, a surface-centric composition
workspace, and moves the media library to its own **Media** catalog page.
In Studio the user picks a surface — a lighting zone or a device screen —
from a left rail, sees its live composited output on a center stage, and
edits its layer stack on a right rail.

Studio is designed as **one complete UX including multi-zone**. A scene
will soon hold N lighting zones, each a partition of device outputs with
its own layer stack (spec 64). Designing the workspace around a single
surface and bolting zones on later would force a second redesign. Instead
the per-surface model — name, Stage, layer stack — is built so that "more
zones" is simply "more rows in the left rail." Spec 65 specifies the whole
zone UX (§9); it delivers in waves, with the zone-creation and
device-assignment waves activating once spec 64's engine and API land.

The new pages are built in parallel with the existing ones behind a
runtime feature flag, so the redesign can be iterated against a live
daemon without disturbing the working `/assets` and `/displays` pages
until a staged cutover (§11.4).

### Build Ownership

Spec 65 is the **frontend** half of a two-spec pair. Spec 64 — Multi-Zone
Scenes — is the **backend** half: per-group LED sampling,
`UnassignedBehavior` enforcement, `SceneManager` zone lifecycle, and the
`/scenes/:id/zones` REST surface. Per the agreed division of labor,
**Codex implements spec 64 (backend)** and **Claude implements spec 65
(frontend)**. Studio Waves 1-8 touch only `hypercolor-ui` and depend on no
spec 64 work, so they are buildable and shippable independently. Waves
9-10 are the only frontend work that waits on the backend, and they gate
on **named daemon capabilities** (§9.6), not wave numbers. §12.2 records
two API additions spec 64 must make for the multi-zone UI to be
implementable; that list is direct input to the spec 64 build.

### Cross-Model Review

This spec was reviewed by Codex before implementation. The review
endorsed the surface-centric paradigm and flagged contract gaps — chiefly
a missing write API for scene-level unassigned behavior, the
device-output (not whole-device) granularity of assignment, the
"All Lights" naming breaking once zones exist, and a too-aggressive
single-step cutover. This revision incorporates all of them.

---

## 2. Problem Statement

### 2.1 The Layer Editor Has No Home

To composite layers today, the user opens `/assets`, selects a file, and
scrolls the right rail past the file's type, size, and pixel dimensions to
reach a "Layer Stack" sub-panel. The single most important authoring
surface in spec 60 is presented as an afterthought attached to file
metadata. A user who wants to stack an effect under a gif does not
intuitively go to a page named "Assets."

### 2.2 The Same Model Is Split Two Ways

`/displays` edits display-role render groups; `/assets` edits the primary
LED render group. These are the same type with the same layer-stack
contract. The split forces the user to learn an internal distinction
(LED consumer vs. screen consumer) that the architecture explicitly treats
as irrelevant to authoring.

### 2.3 Internal Jargon Leaks Into the UI

The live `/assets` page shows a group selector reading "Primary · Primary"
and layer rows labelled `Effect 09862c9f-6561-45e6-a636-940c0bdef7a2` —
the raw `RenderGroupRole` and a layer UUID. "Render group" and bare UUIDs
mean nothing to the target user, who is a streamer or PC builder, not an
engine developer. The UI must speak in user terms.

### 2.4 Both Pages Feel Lifeless

Both pages are sparse three-column layouts with large empty regions, a
single thumbnail or device row carrying most of the content, and little
use of the component library (`preview_cabinet`, `perf_charts`,
`device_card`, the canvas preview surfaces) that the rest of the app uses.
They work, but they read as scaffolding, not a finished product.

### 2.5 The Model Is About to Get Wider

Spec 64 adds multi-zone: several `Custom` LED render groups per scene, each
a partition of device outputs with its own layer stack. A UI split by
content type and consumer has no room for that axis. Whatever replaces
these pages must have a place for N lighting zones — multiple layer stacks
with different device outputs assigned to each — with no second redesign.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **Progressive simplicity.** The one-zone, one-layer case is trivially
  simple; multi-zone, multi-layer, and power controls are disclosed
  extensions, never permanent fixtures (§3.3). This goal governs every
  other one.
- **One composition workspace.** A `/studio` page that unifies layer
  editing for every surface — LED zones and device screens alike.
- **Complete multi-zone UX.** The full design for N zones — zone
  lifecycle, device-output assignment, the unassigned entry, the
  multi-zone Stage — is specified here (§9). The workspace is extensible
  to multiple layer stacks with different device sets with no second
  redesign.
- **Media as a catalog page.** A `/media` page for library management
  (upload, tag, search, see references), distinct from composition.
- **Reuse the layer manager under a defined contract.** The spec 60
  `LayerPanel` / `LayerRow` components are preserved and relocated, not
  rewritten; §10 pins the prop/event contract Studio depends on.
- **No internal jargon in the UI.** "Render group," role names, and raw
  UUIDs never appear. The vocabulary of §4 is the only user-facing
  language.
- **Stage with output and layout views.** The center preview toggles
  between live composited output and the spatial device-placement canvas,
  absorbing the standalone `/layout` page. In multi-zone the layout view
  doubles as the device-output assignment surface (§9.3).
- **Multi-surface authoring.** An Add-layer target scope (§6.6) so a user
  can add a layer to several surfaces at once, not only the selected one.
- **Parallel build behind a flag, staged cutover.** New pages ship
  alongside the existing ones, gated by a runtime preference; the default
  flips before any old page is deleted (§11.4).

### 3.2 Non-Goals

- **The multi-zone engine.** Per-group LED sampling and
  `UnassignedBehavior` enforcement are spec 64's scope. Spec 65 designs
  and builds the multi-zone *UI*; its zone waves (§14, Waves 9-10) call
  spec 64's API and activate per the capability gates of §9.6.
- **Defining new daemon API.** Spec 65 adds no routes itself. It does,
  however, **require** two API additions from spec 64 (§12.2); specifying
  and building those is spec 64's job.
- **Engine or render-pipeline changes.** UI crate only.
- **Mobile-specific layout.** Studio collapses responsively (§13) but the
  dedicated mobile treatment is spec 63's scope.
- **Effects-page redesign.** The Effects gallery stays as the effect
  browser; §5.3 records how it relates to Studio.

### 3.3 Guiding Principle: Progressive Simplicity

The core paradigm is one sentence: **a zone has a layer stack and a set of
devices.** That sentence must be obvious and effortless for the common
case — one LED zone, one or two layers, maybe a screen. Everything beyond
it is a *progressive extension* that appears only when the user's setup
makes it meaningful. The UI must never demolish the user's brain with a
wall of buttons, tabs, and panels they do not yet need.

Complexity scales with configuration, not with the feature set:

- A one-zone user never sees zone management: no `+ New zone` control at
  all, no Unassigned entry, no zone filter, no All-zones Stage mode, no
  Effects apply-target selector. The Lights section is a single row.
- The Add-layer target scope selector (§6.6) is hidden while only one
  surface exists — there is nothing to scope to.
- Advanced per-layer controls — transform, color adjust, parameter
  bindings — stay collapsed behind a disclosure, as they already are.
- Adding a second zone, a third layer, or a screen reuses the exact
  controls the user already knows. Extension is recognition, not
  relearning — the natural next step, not a new mode.

When any feature in this spec would add a control, tab, or panel, the
default is to disclose it on demand. A permanent control that a
single-zone, single-layer user does not need is a bug against this
principle, and §15.2 verifies the minimal baseline explicitly.

---

## 4. User-Facing Vocabulary

The UI uses exactly these words. The internal type is never shown.

| Internal type / concept                | UI term                          | Notes                                                        |
| --------------------------------------- | -------------------------------- | ------------------------------------------------------------ |
| `RenderGroup` (LED role)                | a **zone** (a **Light**)         | Listed under the **Lights** section.                        |
| `RenderGroup` (Display role)            | a **Screen**                     | Listed under the **Screens** section.                        |
| `RenderGroupRole::Primary`, sole LED zone | **All Lights**                 | Used **only** while Primary owns every LED output (§9.2).    |
| `RenderGroupRole::Primary`, with other zones present | **Default zone**      | Once Custom zones exist, Primary no longer covers everything. Renameable. |
| `RenderGroup.name`                      | the zone's name                  | User-typed ("Keyboard", "Case Fans") once multi-zone exists. |
| `RenderGroupRole::Custom` group         | a **zone**                       | Just another row under Lights. "Custom" is never shown.      |
| `DeviceZone` (one device output/segment)| a **light** / a device **output**| The unit of zone assignment (§9.3). Grouped visually by device. |
| `SceneLayer`                            | a **layer**                      |                                                              |
| `LayerSource` variant                   | a **source**                     | Picker tabs: Effect, Media, Screen Capture, Web Page, Color. |
| `LayerBlendMode`                        | **blend mode**                   | Grouped per spec 60 §12.4.                                   |
| `LayerRuntimeState` / `LayerHealthEvent`| a layer **health** pill          | Spec 60 §6.5; already rendered by the layer manager.         |
| `Scene.unassigned_behavior`             | what **unassigned lights** do    | Plain words: turn off / hold last colors / follow a zone.    |
| Layer / asset / group / device UUID     | (never shown)                    | Resolve to display names; UUIDs only in dev tooling.         |
| The composition workspace               | **Studio**                       |                                                              |
| The center live preview                 | the **Stage**                    |                                                              |
| `SpatialLayout` editing                 | **layout** (a Stage view)        |                                                              |
| Asset / media file                      | **media**                        | The word "asset" is retired from the UI entirely.            |

**Hard rule:** no Rust type name, role enum, or UUID is rendered to the
user. Any place that does today (the "Primary · Primary" selector, the
`Effect <uuid>` layer label) is a bug this spec fixes.

---

## 5. Page Architecture

### 5.1 Navigation

With the flag on, the sidebar nav is:

```
Dashboard
Effects      effect browser (unchanged)
Studio       composition workspace          [new]
Media        media catalog                  [new]
Devices
Settings
```

`Studio` replaces both `Assets` and `Displays`. `Layout` is removed as a
top-level entry; its device-placement canvas becomes a Stage view inside
Studio (§6.3). With the flag off, the nav is unchanged from today
(`Assets`, `Displays`, `Layout` present; `Studio`, `Media` absent).

### 5.2 The Two-Catalog, One-Workspace Shape

Effects and Media are **catalogs** — browse, search, inspect, manage.
Studio is the **workspace** — where catalog items become layers on a
surface. This is the consistent mental model: you discover content in a
catalog, you compose it in Studio. Composition is not strictly one
surface at a time: the Add-layer flow (§6.6) can target several surfaces
at once, so "put this gif on every screen" is one action.

### 5.3 Relationship to the Effects Page

The Effects page stays the effect browser. While there is one LED zone,
applying an effect from it seeds or replaces the effect layer of
**All Lights**, exactly as "apply effect" does today, and the sidebar
"Now Playing" reflects it.

Once Custom zones exist this is no longer unambiguous — spec 64 §6.2/§9.5
says `effects/apply` targets the `Primary` (Default) zone and may leave it
empty. The Effects page therefore gains an explicit **apply target**
selector once more than one zone exists: *Default zone*, *a specific
zone*, or *all light zones*. Only the *Default zone* target uses
`effects/apply` (which spec 64 §3.2 deliberately keeps Primary-only); a
specific-zone or all-zones target instead issues per-group layer
mutations — adding or replacing each target zone's effect layer — because
spec 64 gives `effects/apply` no per-zone target. With a single zone the
selector is hidden and behavior is unchanged. "Now Playing" then reflects
the Default zone and is labelled as such, not as "All Lights."

---

## 6. The Studio Page

A three-rail workspace.

```
┌─ LIGHTS & SCREENS ─┐┌─ STAGE ───────────────────┐┌─ LAYERS ──────────┐
│ LIGHTS             ││  ◐ Output    ○ Layout     ││ All Lights        │
│  ● All Lights      ││                           ││                   │
│                    ││   ┌───────────────────┐   ││ ▤ paimon.gif      │
│ SCREENS            ││   │  live composited  │   ││   Screen · 80%    │
│  ▢ Corsair LCD     ││   │  preview canvas   │   ││ ✦ Aurora Wave     │
│  ▢ Push 2          ││   └───────────────────┘   ││   Replace · 100%  │
│                    ││                           ││ ───────────────   │
│ + New zone   ⊘     ││   320×480 · 30fps         ││ + Add layer       │
└────────────────────┘└───────────────────────────┘└───────────────────┘
```

### 6.1 The Surface Rail (Lights & Screens)

A left rail with two sections. **Lights** lists LED zones — today exactly
one row, **All Lights**; with spec 64, one row per zone plus an Unassigned
entry (§9). **Screens** lists display-face surfaces, one per screen
device, each showing the device name, an aspect badge (`WIDE` / `ROUND`),
a small live thumbnail, and any degraded-state indicator (§6.7).
Selecting a row sets the editing context for the Stage and the Layers
rail.

A `+ New zone` control is **not shown at all** until the daemon
advertises the `zone-crud` capability (§9.6). Per §3.3 a single-zone user
sees no zone-management affordance whatsoever — not even a disabled
placeholder. When the capability appears the control appears with it, at
the foot of the Lights section, and multi-zone becomes a fill-in of the
rail Studio already has, not a redesign.

The rail reuses `device_card`-family styling and live-thumbnail patterns
so it reads as a populated, living panel, not an empty list.

### 6.2 The Stage — Output View

The default Stage view shows the selected surface's live composited
output. For an LED zone this is that zone's canvas preview (the
`canvas_preview` stream; per-zone via spec 64 `ZonePreviewFrame` once
available). For a **Screen** it is that device's face preview. The Stage
shows resolution and current FPS, reusing the `preview_cabinet` /
`canvas_preview` components.

### 6.3 The Stage — Layout View

Toggling to **Layout** replaces the preview with the spatial
device-placement canvas — the existing `layout_builder` / `layout_canvas`
/ `viewport_designer` components, lifted from the retired `/layout` page.
For an LED zone this shows that zone's device outputs on the canvas; with
one zone, that is every LED output. In multi-zone the Layout view is also
the device-output assignment surface (§9.3). For a **Screen** the Layout
toggle is hidden — a single LCD has no spatial placement to edit.

### 6.4 The Layers Rail

The right rail is the selected surface's layer stack: the spec 60
`LayerPanel` component, reused under the contract pinned in §10. It
provides top-to-bottom ordering, drag reorder, blend mode, opacity, the
transform/color disclosure, per-layer health pill, enable toggle, delete,
the five-source "Add layer" picker, and `If-Match` optimistic-concurrency
handling against `layers_version`. Spec 65 does not redesign it; it
reparents it from the `/assets` right rail into the Studio rail and fixes
its labels (§6.5).

### 6.5 Jargon Scrub in the Layers Rail

Three label defects are corrected, all label-only, no behavior change:

- The group selector "Primary · Primary" becomes the surface name
  ("All Lights" / the zone name), or is dropped since the selected
  surface is already shown at the rail header.
- Layer rows labelled `Effect <uuid>` resolve the effect id to the
  effect's display name. Media layers already show the filename.
- Any `v15` / `layers_version` debug text is removed from the
  user-facing surface (the value is still used internally for `If-Match`).

### 6.6 Add-Layer Target Scope

The "Add layer" flow exposes a **target scope** so a layer can be added to
more than the selected surface in one action:

| Scope             | Effect                                                             |
| ------------------ | ------------------------------------------------------------------ |
| This surface       | Default. Adds to the selected surface only.                        |
| Selected surfaces  | Adds to a multi-selected set of rows in the surface rail.           |
| All light zones    | Adds to every LED zone.                                            |
| All screens        | Adds to every display-face surface.                                |
| Whole scene        | Every surface.                                                     |

For a media source, a scene-or-broadcast scope uses spec 60's existing
scene-wide media broadcast routing where applicable; otherwise the UI
issues the per-group `create_layer` mutation per target, sequentially,
surfacing partial failure rather than silently dropping it. This keeps
the use case "the same gif on all four fan groups" a single gesture
instead of four repeats.

Per §3.3, the target-scope selector is **hidden entirely when only one
surface exists** — with a single zone and no screens there is nothing to
scope to, so "Add layer" simply adds to that surface with no extra
control. The selector appears the moment a second surface does.

*Selected surfaces* uses the surface rail's own multi-select
(ctrl/cmd-click to add a row, shift-click for a range) and is offered only
while a multi-selection is active. Scopes that would be a no-op for the
current scene are hidden — *All screens* does not appear with no screens
connected — and *Whole scene* excludes the Unassigned entry, which has no
layer stack to receive a layer.

### 6.7 Display (Screens) Parity

`/displays` is only deleted (Wave 8) once Studio's Screens surfaces reach
parity with it. The parity checklist, verified in Wave 7:

- Assign and clear a face — as a layer add/remove on the Screen surface.
- Per-device live preview and the full-screen preview link.
- Degraded transport/render state surfaced on the Screen row and Stage.
- Simulator / virtual display rows behave as today.
- Legacy `DisplayFaceTarget` blend/opacity scenes still render and are
  presented as a layer (spec 60 already migrates these; Studio must show
  the migrated result correctly).

An item not yet at parity blocks the **Wave 7 default flip** — Studio must
not become the default while a Screen can do less than the page it
replaces. The Wave 8 deletion additionally requires the soak (§11.4).

---

## 7. The Media Page

`/media` is the catalog: a responsive thumbnail grid with drag-drop
upload, search, MIME filter, and a per-item detail panel showing
intrinsic size, duration, and content hash. Showing which scenes
reference a file needs a backend reference lookup the current scene API
does not provide (`GET /scenes` returns summaries only); it is listed as
an optional backend addition in §12.2, and the panel omits reference
information until that endpoint exists. It is the catalog half of today's
`/assets` page, kept and
polished — the layer-stack sub-panel is removed from it (that work now
lives in Studio).

The same catalog browser is embedded as the **Media** tab of Studio's
"Add layer" picker, so adding a media layer never requires leaving the
workspace. The grid is one shared component used in both places.

The word "asset" does not appear on this page or anywhere in the UI; it
is "Media" throughout.

---

## 8. Surface Model

A **surface** is the UI presentation of one render group. The surface
rail is built from the active scene's groups:

- LED-role groups → the **Lights** section. Today there is one, the
  `Primary` group, presented as **All Lights** (§9.2 covers the rename
  once more zones exist).
- Display-role groups → the **Screens** section, one per screen device.

Each surface exposes the same three things: a name, a live preview
(Stage), and a layer stack (Layers rail). Selecting a surface is purely a
UI state change; it issues no mutation. Every edit inside Studio goes
through the existing per-group layer and control endpoints addressed by
the group's id, which the UI holds but never displays.

This uniform per-surface model is the extensibility guarantee: a zone, a
screen, and "All Lights" are the same shape, so adding zones is adding
rows, never rebuilding the editor.

---

## 9. Multi-Zone UX

This section specifies the complete multi-zone UX. It is designed in full
now so the workspace is extensible; it ships in the later waves of §14,
each affordance gated on a named daemon capability (§9.6).

### 9.1 The Shape of Multi-Zone

A scene holds N LED zones. Each zone is a name, a set of assigned device
outputs, and its own layer stack — "multiple layer stacks with different
device outputs assigned to each." Because §8 already gives every surface a
name, a Stage, and a Layers rail, multi-zone needs no change to the
per-surface editor. Selecting the "Case Fans" zone loads its layer stack
and its preview exactly as selecting "All Lights" does today. That
invariance is the proof the paradigm is extensible.

The five brainstorm use cases land as:

| Use case                                              | Studio shape                                            |
| ------------------------------------------------------ | ------------------------------------------------------- |
| Gif as the whole lighting surface, blended with effect | One zone (All Lights), layers: effect + media           |
| Gif on the Corsair AIO screen, blended with effect     | The Corsair Screen surface, layers: effect + media      |
| 12 fans, 4 groups of 3, different media per group      | 4 zones, 3 fans each, each zone a one-media layer stack |
| A face on the NZXT screen blended with the effect      | The NZXT Screen surface, layers: effect + media         |
| Keypress ripple stacked on the main effect on keyboard | A "Keyboard" zone, layers: main effect + ripple effect  |

The first two and the fourth work in single-zone Studio (Waves 1-8). The
third and fifth need zones (Waves 9-10).

### 9.2 Zone Lifecycle and the Default Zone

`+ New zone` (enabled per §9.6) creates a `Custom` zone via
`POST /scenes/:id/zones`. A zone row carries an inline rename, a color
swatch, an enable toggle, a small live preview, and a delete affordance.

The `Primary` group is the **Default zone**. Its label is **"All Lights"**
only while it is the *sole* LED zone and owns every LED output. The moment
a second zone exists, Primary may no longer cover everything (spec 64
§6.2 permits an empty Primary), so its row relabels to **"Default zone"**
or whatever name the user gives it. Promoting a different zone to default
is offered where spec 64 supports it (`make_primary`), framed as
"make this the default zone," never "make primary." The Effects apply
target (§5.3) follows the same rule.

### 9.3 Device-Output Assignment in the Stage Layout View

The unit of zone assignment is a **`DeviceZone`** — one device output or
addressable segment, not a whole physical device. Spec 64 §6.4 is explicit
that a multi-channel device may be partially assigned: one channel in a
zone, another in none. The Studio UI models assignment at that
granularity and must not assume a device maps to one zone.

Assignment lives in the Stage **Layout** view, which already renders
device outputs on the spatial canvas. In multi-zone the Layout view gains:

- Each device output on the canvas is tinted with its owning zone's
  color; outputs of one physical device are visually grouped so the user
  still sees the device, but can select its outputs independently.
- An **Unassigned tray** docks in the Layout view, holding outputs that
  belong to no zone.
- Dragging an output (or a multi-selected set of outputs) onto a zone
  issues `POST /scenes/:id/zones/:zone_id/devices` with `DeviceZone`
  payloads. Exclusivity — an output leaving its previous zone — is handled
  by the daemon (spec 64 §6.3); the UI reflects the result.
- A zone filter dims outputs outside the selected zone so the user edits
  one zone's membership at a time, with an "All zones" mode showing the
  whole partition color-coded.
- Dragging an output to the Unassigned tray, or a "remove from zone"
  action on a selected output, issues
  `DELETE /scenes/:id/zones/:zone_id/devices/:dz` so an output can leave a
  zone without joining another.

A "select all outputs of this device" affordance keeps the common
single-zone-per-device case a one-click action without hiding the
finer granularity.

### 9.4 The Unassigned Entry

Device outputs in no zone appear as a distinct **Unassigned** entry at the
bottom of the Lights section. It is a synthetic rail entry, **not a
surface** in the §8 sense: it has no layer stack and no Stage of its own.
Selecting it shows only the unassigned outputs and a control for the
scene's `unassigned_behavior`, in plain words: "Unassigned lights:
turn off · hold last colors · follow <zone>."

This control **writes a scene-level field**, which neither spec 60 nor
spec 64 currently exposes a route for. §12.2 records this as a required
spec 64 API addition; the Unassigned behavior control is gated on the
`scene-unassigned-behavior-write` capability (§9.6) and is not built
until that route exists. Until then the Unassigned entry is read-only:
it lists unassigned outputs and shows the current behavior without
letting the user change it.

### 9.5 The Multi-Zone Stage

The Stage shows the selected zone's output or layout. An **All zones**
Stage mode shows a tiled preview, one tile per zone (spec 64 §11.4), each
labelled with the zone name and its top layer, for a scene-wide glance.
Output and Layout toggles apply to the selected zone.

### 9.6 Capability Gates

Multi-zone affordances gate on **named daemon capabilities**, advertised
by the daemon (for example in a `GET /api/v1/capabilities` payload or the
existing status response), not on spec 64 wave numbers. This decouples the
two builds: each UI affordance lights up when, and only when, its backing
capability is actually present.

| Studio affordance                          | Required capability                              |
| -------------------------------------------- | ------------------------------------------------ |
| `+ New zone`, zone rows, rename/color/delete | `zone-crud` + `multi-zone-sampling` + `zone-device-assignment` |
| Correct rendering of multiple zones          | `multi-zone-sampling` — per-group LED sampling    |
| Device-output assignment                     | `zone-device-assignment` — the devices sub-routes |
| Per-zone Stage preview                       | `zone-preview-frames` — `ZonePreviewFrame`        |
| The Unassigned behavior **write** control     | `scene-unassigned-behavior-write` (§12.2)         |

Zone creation is offered only when `zone-crud`, `multi-zone-sampling`,
**and** `zone-device-assignment` are all present. A user who can create a
zone but cannot render it correctly, or cannot move devices into it, is
left with an empty unusable zone — so all three gate `+ New zone`
together. Per-zone preview is treated as an enhancement: if
`zone-preview-frames` is absent the Stage falls back to the composited
scene preview rather than blocking the feature.

---

## 10. Reusing the Layer Manager

The layer manager is `LayerPanel` and `LayerRow`, currently defined as
**inline functions inside `pages/assets.rs`** (around lines 489 and 626).
It is the polished, working surface this spec must not regress.

Wave 1 has two distinct deliverables, kept separate on purpose:

1. **Pure extraction.** Move `LayerPanel` / `LayerRow` into a standalone
   module (`components/layer_panel/`) with no behavior change, and
   repoint `/assets` at it. Verified by the old page behaving identically.
2. **The Studio-ready contract.** Document and test the component's
   prop/event surface so Studio (Wave 4) can mount it without surprises.
   The contract covers: the surface identity it edits (scene id + group
   id), the `layers_version` it carries for `If-Match`, the five-source
   Add-layer picker, per-layer health-pill rendering (spec 60 §6.5),
   transform/adjust/binding sub-panels, and the mutation callbacks
   (`on_layers_mutated` and friends). "Reuse unchanged" means the
   *behavior* is unchanged; it does not mean the contract is left
   implicit. Wave 1 makes it explicit and test-covered.

Reusing one component for both the old and new pages means the polished
manager cannot drift: there is exactly one implementation, proven against
`/assets` before Studio consumes it.

---

## 11. Parallel Build and Feature Flag

### 11.1 The Flag

A runtime preference `studio_ui_beta: bool` (default `false`) is added to
`preferences.rs`, surfaced as a toggle in Settings ("Studio UI (beta)").
A runtime preference, not a Cargo feature, so the redesign can be flipped
on and off against a live daemon during iteration without a rebuild.

### 11.2 Parallel Pages

New code lives in new modules — `pages/studio/` and `pages/media.rs` —
beside the untouched `pages/assets.rs`, `pages/displays.rs`, and
`pages/layout.rs`. Routes for `/studio` and `/media` are registered
unconditionally. When the flag is off they are unlinked; navigating to
them directly **redirects** to the corresponding old page unless a dev
override query parameter is present, so a half-built beta page is never
reachable by accident in a default build.

`sidebar.rs` chooses the nav set from the flag: flag off → today's nav
(`Assets`, `Displays`, `Layout`); flag on → the §5.1 nav.

### 11.3 The `assets` → `media` Rename

Because the build is parallel, nothing is renamed in place. The new page
is `media.rs` from the start; `assets.rs` is left alone and deleted only
at the Wave 8 cleanup. This keeps the old page a stable reference while
the new one is built.

### 11.4 Staged Cutover

Cutover is two waves, not one, so the runtime flag keeps its value as a
rollback path through the risky window:

- **Wave 7 — default-on soak.** After the QA sweep (§15.2) and the §6.7
  display-parity checklist pass, flip `studio_ui_beta` default to `true`.
  The flag and the old pages are **retained**. Anyone hitting a Studio
  problem flips the flag off and is back on the working old UI
  immediately. The soak runs until Studio is confirmed stable in real use.
- **Wave 8 — cleanup.** Only after the soak: remove the flag and the
  Settings toggle, delete `pages/assets.rs`, `pages/displays.rs`,
  `pages/layout.rs` and their routes, and drop now-dead helpers.

Multi-zone Waves 9-10 land on the cut-over Studio and need no flag of
their own beyond the §9.6 capability gates.

---

## 12. API Surface and Backend Dependencies

### 12.1 Endpoints Studio Consumes

Studio Waves 1-8 add no daemon endpoints and depend on no spec 64 work.
Every endpoint they call already exists from spec 60 and the current API:

- Spec 60 per-group layer endpoints — list, create, update, reorder,
  delete layers, patch layer controls, with `If-Match`.
- Existing scene endpoints — read the active scene and its groups.
- Existing layout endpoints — for the Stage Layout view.
- Existing device and media endpoints — for the Screens list and the
  Media catalog.
- The WebSocket preview and per-device preview streams.

Waves 9-10 additionally consume spec 64's `/scenes/:id/zones` endpoints
and `ZonePreviewFrame` message.

### 12.2 Required Spec 64 / Backend Additions

The Codex review found two capabilities the multi-zone UI needs that the
current specs do not provide. These are **inputs to the spec 64 build**,
recorded here so the frontend and backend tracks stay in contract:

1. **Scene-level unassigned-behavior write.** §9.4's control writes
   `Scene.unassigned_behavior`. Spec 64's zone endpoints are per-zone and
   do not cover it. Spec 64 must expose a write — a dedicated
   `PATCH /api/v1/scenes/:id/unassigned-behavior`, or explicit inclusion
   of the field in `PUT /scenes/:id` — with concurrency semantics and an
   event/refetch path, advertised as the `scene-unassigned-behavior-write`
   capability.
2. **A capability advertisement.** §9.6 gates affordances on named
   capabilities. The daemon must advertise which multi-zone capabilities
   are live, extending `GET /status` or adding `GET /capabilities`. This
   is a hard requirement, not a preference: there is no probe fallback,
   because feature-detection by trial mutation is unsafe. Until the
   advertisement exists, Studio treats every multi-zone capability as
   absent and shows only the single-zone UI.

A third item is optional, not a blocker:

3. **Media reference lookup.** §7's media detail panel would show which
   scenes reference a file. The current scene API returns summaries only.
   If a reference endpoint is added the panel surfaces it; otherwise the
   panel omits reference information. This blocks nothing.

Items 1 and 2 are not spec 65's to build. They are flagged to the spec 64 (backend)
track and must be resolved there before Waves 9-10 can be implemented.

---

## 13. Mobile and Responsive Behavior

The three-rail layout collapses responsively: on narrow viewports the
Surface rail and Layers rail become slide-over drawers and the Stage takes
the full width. Spec 65 ships this responsive collapse so Studio is usable
on a phone-width viewport; spec 63 owns any deeper mobile-specific
treatment and navigation.

---

## 14. Delivery Waves

| Wave | Scope                                                                        | Gate |
| ---- | ---------------------------------------------------------------------------- | ---- |
| 1    | Extract `LayerPanel`/`LayerRow` to `components/layer_panel/`; repoint `/assets`; document and test the §10 contract. | — |
| 2    | `studio_ui_beta` preference + Settings toggle; flag-driven nav swap; `/studio` and `/media` routes with off-flag redirect (§11.2). | — |
| 3    | `/media` catalog page; shared catalog-grid component (also the Add-layer Media tab). | — |
| 4    | Studio shell — three rails; Lights & Screens list; surface selection; Layers rail (Wave 1 component); Stage Output view. | — |
| 5    | Stage Layout view — embed `layout_builder`/`layout_canvas`; Output/Layout toggle; retire `/layout` link. | — |
| 6    | Jargon scrub (§6.5); friendly names; Add-layer target scope (§6.6); visual polish; responsive collapse (§13). | — |
| 7    | Full UI QA sweep (§15.2); §6.7 display-parity checklist; flip `studio_ui_beta` default to `true` — flag and old pages **retained** (default-on soak). | — |
| 8    | After soak: remove flag and toggle; delete `assets.rs`, `displays.rs`, `layout.rs` and their routes. | Soak confirms Studio stable |
| 9    | Zone lifecycle — `+ New zone`, zone rows, rename/color/delete, Default-zone relabel, the Unassigned entry (read-only until its capability lands). | `zone-crud` + `multi-zone-sampling` + `zone-device-assignment` |
| 10   | Device-output assignment in the Layout view; multi-zone tiled Stage; per-zone preview; Unassigned **write** control. | `zone-device-assignment`, `zone-preview-frames`, `scene-unassigned-behavior-write` (per affordance, §9.6) |

Waves 1-6 land behind the flag and do not affect the default UI. Wave 1
is verifiable against the live old page before any new surface exists.
Waves 3-6 are each independently demoable by flipping the flag. Wave 7
makes Studio the default while keeping the rollback path; Wave 8 removes
the old UI only after the soak. Waves 9-10 are additive on the shipped
Studio and each affordance activates per its §9.6 capability gate.

---

## 15. Verification Strategy

### 15.1 Automated

`just ui-test` passes after every wave. `just ui-build` produces a
working WASM bundle. Component tests cover the extracted `LayerPanel`
(Wave 1 — behavior identical to pre-extraction, plus the §10 contract)
and the shared catalog grid.

### 15.2 Visual and Manual (agent-browser)

The directive to "test every tiny aspect" is a structured QA sweep in
Wave 7, driven by `agent-browser` against a live daemon on `:9430`:

- Every surface in the rail selects, previews, and loads its layer stack.
- Add, reorder, blend, opacity, transform, adjust, enable, delete, and the
  health pill each work on each layer source type, on a Light and a
  Screen surface.
- The Add-layer picker's five tabs each add a working layer; the §6.6
  target scope adds to multiple surfaces and reports partial failure.
- The Stage Output/Layout toggle works; Layout is correctly hidden for
  Screens.
- The §6.7 display-parity checklist passes in full.
- Media upload, search, filter, tag, and delete work; reference counts
  are correct.
- No internal jargon, no UUID, and no debug text is visible anywhere
  (grep the rendered DOM snapshot).
- The flag toggles cleanly both directions; off-flag beta routes redirect.
- Responsive collapse works at narrow widths.
- **Minimal baseline (§3.3).** With one zone and no screens, the
  brain-demolishing controls are all absent: no `+ New zone`, no
  Unassigned entry, no zone filter, no All-zones Stage mode, no Effects
  apply-target selector, no Add-layer target-scope selector. Adding a
  second surface makes exactly those appear, and nothing else changes.

The brainstorm use cases that single-zone Studio supports — gif as full
surface, gif on the Corsair screen blended with the effect, a face on a
screen — are each walked end to end. The two multi-zone use cases are
added to the sweep when Waves 9-10 land.

### 15.3 Cross-Model Review

This spec was cross-checked with Codex pre-implementation (see §1). The
Wave 6 visual result is reviewed with the `frontend-design` /
`effect-reviewer` lens before the Wave 7 default flip. Waves 9-10 are
reviewed against spec 64 §11 for parity.

---

## 16. Known Constraints

- **`hypercolor-ui` is outside the Cargo workspace.** `cargo check
  --workspace` does not cover it. Every wave verifies with `just ui-test`
  and `just ui-build` explicitly.
- **The flag doubles maintained UI surface** between Wave 2 and Wave 8.
  This is the accepted cost of not breaking the working pages; Wave 8
  removes the duplication after the soak confirms Studio is stable.
- **Single-zone until spec 64.** Studio ships showing one Light
  ("All Lights"). The disabled `+ New zone` affordance signals the model
  to the user in the interim.
- **Waves 9-10 depend on spec 64.** If spec 64 slips, single-zone Studio
  still ships and cuts over at Wave 7; the multi-zone UX is fully designed
  and waits. Spec 65 is not blocked on spec 64 for its core value. The two
  §12.2 API additions are hard prerequisites for Wave 10 and must be
  tracked on the backend.
- **Layout view for Screens is intentionally absent.** A single LCD has
  no spatial placement; hiding the toggle is correct, not a gap.

---

## 17. Recommendation

Build it, in the ten waves of §14.

The architecture already says these are one thing; spec 60 §1 is explicit
that source and consumer are independent and share one layer-stack
contract. The current two-page split contradicts that and buries the most
important authoring surface under file metadata. Studio presents the model
the engine actually has: surfaces, each with a stack, each with a preview.

Designing the multi-zone UX in full now (§9) is what makes the workspace
genuinely extensible. The per-surface model — name, Stage, layer stack —
is identical for one zone or twelve, so multi-zone is more rows in a rail,
not a redesign. Studio is built as one cohesive UX and delivered in waves:
single-zone Studio cuts over first and is a complete replacement for
Assets and Displays on its own; the zone-creation and device-assignment
waves activate on top, per capability, once spec 64's engine and API
exist.

The Codex review confirmed the paradigm and corrected the contracts: the
unassigned-behavior write is a real backend gap (§12.2), assignment is
output-grained not device-grained (§9.3), "All Lights" is a single-zone
label (§9.2), and cutover is staged so the flag stays a live rollback
(§11.4). With those folded in, the risk is contained — no engine work in
this spec, no new frontend API, no rewrite of the polished layer manager,
and a default flip that is reversible until the Wave 8 cleanup.

---

## 18. Appendix A — File Inventory

New files:

| Path                                                  | Purpose                                      |
| ------------------------------------------------------ | -------------------------------------------- |
| `crates/hypercolor-ui/src/components/layer_panel/`     | Extracted `LayerPanel` / `LayerRow`          |
| `crates/hypercolor-ui/src/pages/studio/`               | Studio page — rails, stage, surface state    |
| `crates/hypercolor-ui/src/pages/media.rs`              | Media catalog page                           |
| `crates/hypercolor-ui/src/components/media_grid.rs`    | Shared catalog grid (Media page + picker tab)|

Modified files:

| Path                                              | Change                                            |
| -------------------------------------------------- | ------------------------------------------------- |
| `crates/hypercolor-ui/src/pages/assets.rs`         | Repoint at extracted `LayerPanel` (Wave 1); deleted Wave 8 |
| `crates/hypercolor-ui/src/components/sidebar.rs`   | Flag-driven nav set                               |
| `crates/hypercolor-ui/src/preferences.rs`          | Add `studio_ui_beta`                              |
| `crates/hypercolor-ui/src/pages/mod.rs`            | Register `studio`, `media` modules                |
| `crates/hypercolor-ui/src/route_ui.rs`             | `/studio`, `/media` routes; off-flag redirect     |
| `crates/hypercolor-ui/src/pages/effects.rs`        | Apply-target selector once multiple zones exist   |
| `crates/hypercolor-ui/src/components/settings_sections.rs` | Studio UI beta toggle                     |

Deleted at cleanup (Wave 8):

| Path                                          | Reason                                |
| ---------------------------------------------- | ------------------------------------- |
| `crates/hypercolor-ui/src/pages/assets.rs`     | Replaced by Media + Studio            |
| `crates/hypercolor-ui/src/pages/displays.rs`   | Replaced by Studio Screens surfaces   |
| `crates/hypercolor-ui/src/pages/layout.rs`     | Replaced by Studio Stage Layout view  |

Backend additions tracked on the spec 64 build (§12.2):

| Capability                          | Need                                          |
| ------------------------------------ | --------------------------------------------- |
| `scene-unassigned-behavior-write`    | Write route for `Scene.unassigned_behavior`   |
| Capability advertisement             | Daemon advertises live multi-zone capabilities |
| Media reference lookup (optional)    | Endpoint listing scenes referencing a media file |
