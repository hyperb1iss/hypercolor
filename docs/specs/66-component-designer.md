# Spec 66 — Component Designer

> A visual authoring modal for designing custom LED components — strip
> routes, panels, rings, and scattered art pieces — and saving them as
> reusable attachment templates. The user draws a shape on a 2D canvas
> with wall-plane projections, flows LEDs onto it, verifies wiring
> order, and saves. This completes a half-built system: the
> `AttachmentTemplate` data model, the registry, and the full template
> REST API already ship, and design doc 18 already designed the
> room-mapping `Path` topology. Spec 66 is the missing authoring
> surface, split into a small backend delta and a frontend-heavy modal.

**Status:** Draft (revised after Codex review rounds 1-4, 2026-05-17)
**Author:** Nova
**Date:** 2026-05-17
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`, `hypercolor-ui`
**Depends on:** Interactive Viewport Designer (46) — the modal pattern
**Design basis:** `docs/design/18-room-mapping.md` — coordinate model, `Path` topology, projections
**Related:** Studio Composition UI (65) — launch surface; Multi-Zone Scenes (64) — future segment tie-in; User Media & Layer Stack (60) — future trace-image persistence

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [User-Facing Vocabulary](#4-user-facing-vocabulary)
5. [The Designer Experience](#5-the-designer-experience)
6. [Part I — Backend](#6-part-i--backend)
7. [Part II — Frontend](#7-part-ii--frontend)
8. [The Frontend-Backend Contract](#8-the-frontend-backend-contract)
9. [Delivery Waves](#9-delivery-waves)
10. [Verification Strategy](#10-verification-strategy)
11. [Known Constraints](#11-known-constraints)
12. [Recommendation](#12-recommendation)
13. [Appendix A — File Inventory](#13-appendix-a--file-inventory)

---

## 1. Overview

Hypercolor maps rendered effect canvases onto physical LEDs through
**attachment templates** — reusable shaped components like fan rings,
AIO halos, Strimer cables, and matrix panels. Sixty-two builtin
templates ship in `data/attachments/builtin/`, and the system is
deeper than it looks from the UI:

- `AttachmentTemplate` (`hypercolor-types/src/attachment.rs:187`)
  carries an `LedTopology` describing LED geometry.
- `AttachmentOrigin::User` is a first-class concept — user-authored
  templates load from `ConfigManager::data_dir().join("attachments")`.
- A complete template REST API ships in
  `hypercolor-daemon/src/api/attachments.rs`: list, get, create,
  update, delete, plus category and vendor browse (§6.5).
- The registry is `Arc<RwLock<AttachmentRegistry>>` in `AppState` and
  is mutated live — `create_template` registers a new user template
  without a daemon restart.
- The web UI already calls the list and create endpoints
  (`hypercolor-ui/src/api/devices.rs`).

What does **not** exist is any way to *author the shape itself*.
Today a custom component means hand-writing TOML — the Corsair QL fan
is 34 hand-typed `(x, y)` coordinates. The existing UI surfaces only
bind components to device channels (`attachment_editor.rs`) or pick
from the library (`component_picker.rs`); the only "create" path is a
form that mints a trivial uniform `Strip` or `Matrix` by LED count.

Spec 66 adds the **Component Designer**: a full-screen modal where the
user draws a shape, flows LEDs onto it, checks wiring order, and saves
a reusable component. It is built as a sibling of the shipped Viewport
Designer modal (spec 46), and it serves three flows that share one
canvas: mapping a real strip route around a room, authoring discrete
reusable parts, and remixing existing templates.

### 1.1 Why This Is Small Where It Counts

Because the data model and REST API already exist, the backend delta
is sharp and contained:

- One new `LedTopology` variant — `Path` — for strip routes
  (design doc 18 §3.2 already specifies it).
- Relocating the pure `generate_positions` geometry function into
  `hypercolor-types` so the frontend can preview locally.
- One optional metadata field on `AttachmentTemplate`.

Everything else is the modal. Spec 66 is therefore deliberately
**frontend-heavy**, and §6 / §7 split it so the two tracks can be
built in parallel against the §8 contract.

### 1.2 Build Ownership

Spec 66 is one document with two internally separable parts. **Part I
(Backend)** is workspace Rust — `hypercolor-types`, `hypercolor-core`,
`hypercolor-daemon` — verified with `just verify`. **Part II
(Frontend)** is `hypercolor-ui`, outside the Cargo workspace, verified
with `just ui-build` and `just ui-test`. The two parts are
independently ownable; the §8 contract (three shared-type additions
plus the already-shipped REST surface) is the only seam. Following the
spec 64 / 65 division of labor, Part I is a natural Codex track and
Part II a natural Claude track, but the spec does not hard-assign —
the contract is what decouples them.

### 1.3 Relationship to Studio (Spec 65)

Spec 65 is mid-flight: it folds the standalone `/layout` page into a
Studio Stage view. The Component Designer is a **modal**, deliberately
orthogonal to that redesign — it is launched from a surface, overlays
it, and returns a template id. It works whether spec 65 has landed or
not (§7.7 lists launch points on both the old and new surfaces), so it
neither blocks nor is blocked by spec 65.

---

## 2. Problem Statement

### 2.1 Custom Shapes Mean Hand-Written TOML

The `LedTopology` model supports arbitrary per-LED positions via
`Custom { positions: Vec<NormalizedPosition> }`, and several builtin
fans use it. But authoring those positions means opening a text editor
and typing normalized floats. There is no visual tool. A user with an
LED art piece, a custom panel, or any non-stock shape cannot describe
it without doing coordinate geometry by hand.

### 2.2 A Room Strip Has No Honest Representation

The motivating case: a WLED strip routed around a room — up a wall,
across a shelf, behind a desk. Physically it is **one strip on one
controller**. To map it today the user must split it into several
separate zones in the layout editor and hand-place each, then
hand-edit `led_mapping` to stitch the index order back together. The
data model has no concept of a strip that follows a path, even though
design doc 18 §3.2 designed exactly that — the `Path` topology, called
"the workhorse" — and it was never implemented. The implemented
`LedTopology` is a strict subset of the doc-18 design.

### 2.3 Wiring Order Is Invisible and Error-Prone

LEDs carry two orders: **spatial** (where they sit) and **wiring**
(the order the controller addresses them). When they differ — a strip
soldered to run the other way, a fan whose first LED is not at twelve
o'clock — the `led_mapping: Option<Vec<u32>>` table corrects it. Today
that table is authored as a raw integer array (`[6, 7, 0, 1, 2, ...]`)
with no way to see whether it is right. A wrong mapping produces an
effect that crawls across a component in the wrong order, and the user
has no tool to diagnose it.

### 2.4 No Way to Start From an Existing Component

Sixty-two builtin templates exist, and many user shapes are small
edits of a stock one — a fan with a different LED count, a strip a
little longer. There is no "open this and tweak it" flow. The
`BuiltIn` / `User` origin split that would make forking safe already
exists in the registry; nothing uses it for authoring.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **A visual Component Designer modal.** Draw a shape, flow LEDs onto
  it, save a reusable `AttachmentTemplate`. A full-screen modal in the
  Viewport Designer family (spec 46), reachable from the layout
  canvas, the device attachment panel, and the component picker.
- **A `Path` topology for strip routes.** A strip that follows a
  polyline, LEDs distributed evenly along it — design doc 18 §3.2,
  finally built. This is the honest representation a room strip needs.
- **2D canvas with wall-plane projection.** The user designs on a flat
  canvas and tags which plane it represents (top-down, front wall,
  left/right/back wall). Real-world size is captured in millimetres.
  No 3D viewport (§3.2), but the projection metadata is
  forward-compatible with one.
- **Four shape tools.** Path (pen), Shape (parametric primitives),
  Scatter (hand-placed LEDs), and Trace (place LEDs over a reference
  photo). Each produces a single `LedTopology`.
- **Visual wiring verification.** An LED-chain ribbon showing the
  0..N wiring order, a playback scrub that runs a dot along the chain,
  and direct-manipulation editing of `led_mapping` — reverse, re-base
  LED 0, drag — so the mapping is authored by sight, never by typing.
- **Start-from-template.** Open any builtin or user component, edit
  its geometry, and save. Builtins fork to a new user template;
  user components save in place.
- **Local preview.** The designer renders built-in test patterns
  (index chase, wiring heatmap, rainbow, solid) onto the live LED
  positions entirely in the browser, with no daemon round-trip.
- **A small, contained backend delta.** One topology variant, one
  geometry-function relocation, one optional metadata field. No new
  REST routes — the template API already exists.

### 3.2 Non-Goals

- **A 3D viewport.** Design doc 18 §4.6 sketches an optional Three.js
  3D view; spec 66 does not build it. The render engine samples a 2D
  canvas, so 3D would still need a projection step. The designer
  captures the `ProjectionPlane` per component, which is the data a
  future 3D view would lift into a room box — 2.5D is 3D's
  foundation, not a dead end.
- **Composite / multi-part components.** A v1 component is a single
  `LedTopology`. Design doc 18's `Composite` topology is not built. A
  room with a strip *and* a ring is two components placed in a layout
  — the layout composes; the designer authors one part.
- **Live daemon-effect preview in the designer.** The designer
  previews geometry and wiring with local test patterns. Seeing a real
  effect run is what the layout gives you once the component is
  placed. Authoring geometry and rendering effects stay separate.
- **Persisting the trace reference image.** In v1 the trace photo is
  session-only — it is a backdrop for placing LEDs and is not saved.
  Re-editing the *topology* is fully lossless without it. Persisting
  the photo via the Media system (spec 60) is a noted extension.
- **A component catalog page or new nav entry.** The designer is
  reached from existing surfaces. Spec 65's nav is fixed.
- **Runtime-addressable segments.** Segments (§5.5) are authoring
  metadata — named index ranges on the template. Promoting them to
  layout-addressable regions belongs with spec 64 zones and is out of
  scope here.
- **A registry file-watch.** Externally dropped TOML files still need
  a daemon restart to load. The designer registers through the REST
  create path, which mutates the live registry, so this does not
  affect the designer; it is called out only so the spec does not
  imply a rescan exists.
- **Physical density as a topology field.** `LedTopology` stays
  resolution-independent and count-based. "60 LEDs/m" is an authoring
  convenience the designer uses to *derive* a count; it is not stored
  on the topology. Physical size lives in `physical_size_mm`.

### 3.3 Guiding Principle: One Canvas, Three Flows

The user confirmed all three flows matter equally: map a room route,
author a reusable part, remix an existing component. They are not
three features — they are one tool used three ways, because every flow
is the same gesture: *choose a carrier, flow LEDs onto it, verify,
save*. The design must not privilege one flow with a mode the others
lack. A room route is the Path tool; a reusable fan is the Shape tool;
a remix is Start-from plus any tool. The shared canvas, inspector,
chain ribbon, and save flow are identical across all three.

---

## 4. User-Facing Vocabulary

The UI uses these words. Internal type names never appear.

| Internal type / concept                     | UI term                       | Notes                                                        |
| -------------------------------------------- | ------------------------------ | ------------------------------------------------------------ |
| `AttachmentTemplate`                         | a **component**                | Never "attachment template" or "manifest".                  |
| `AttachmentTemplate` with `origin: User`     | a **custom component**         | Grouped under "My components".                               |
| `AttachmentTemplate` with `origin: BuiltIn`  | a **stock component**          | Read-only; editing forks a copy.                             |
| `LedTopology`                                | the component's **shape**      |                                                              |
| `LedTopology::Path`                          | a **path** (a strip route)     | The Path tool's output.                                      |
| `LedTopology::Custom`                        | **placed LEDs**                | The Scatter tool's output.                                   |
| `LedTopology::Ring/Matrix/PerimeterLoop`     | a **shape**                    | The Shape tool's parametric output.                          |
| A `Path` waypoint                            | a **point**                    | "Click to drop a point."                                     |
| `led_mapping`                                | **wiring order**               | Never "LED mapping" or a raw index array.                    |
| `ComponentSegment`                           | a **segment**                  | A named span of the chain.                                   |
| `ProjectionPlane`                            | the **plane** / **view**       | "Top-down", "Front wall", etc.                               |
| `physical_size_mm`                           | **real-world size**            | Shown in cm or inches per the user's unit preference.        |
| The modal                                    | the **Component Designer**     |                                                              |
| The 0..N wiring strip at modal bottom        | the **LED chain**              |                                                              |
| `compute_path`, `generate_positions`, UUIDs  | (never shown)                  |                                                              |

**Hard rule:** no Rust type name, enum variant, or raw `led_mapping`
array is rendered to the user.

---

## 5. The Designer Experience

This section is the design-level UX. §7 specifies its frontend
implementation; §6 specifies the backend it stands on.

### 5.1 The Modal

A full-screen modal, sized and behaved as the Viewport Designer (spec
46 §5.1): up to 1600×1100, centered, dim backdrop, closes on Esc /
click-outside / Cancel, and confirms before discarding unsaved work.

```
╭─ COMPONENT DESIGNER ──────────────────────────────────────────────────────────╮
│ ▦ Living Room Loop     Start from ▾    ⤺ ⤻          [ Cancel ]   [ Save ▾ ]   │
├─ TOOLS ───┬─⟨ Top-down ⟩  Front wall  Left wall  + ─────────┬─ INSPECTOR ──────┤
│  ✎ Path   │                                                │ Path · 96 LEDs   │
│  ▭ Shape  │    ●─●─●─●─●─●─●─●─●─●─●─●─●─●─●─●             │ Count   ▸ 96     │
│  ⊹ Scatter│    │                            │              │ Density   60 /m  │
│  🖼 Trace  │    ●          ▢ room            ●              │ Real size 1.6 m  │
│  ─────────│    ●                            ●              │ Ends    ◌ open   │
│  PARTS    │    ●─●─●─●─●─●─●─●─●─●─●─●─●─●─●─●             │ ──── segments ───│
│  ▸ Strip  │                                                │ ▾ North    0–31  │
│           │   ▒ ref photo  ·  grid 10 cm  ·  ⌖ snap 45°    │ ▾ Desk    32–63  │
│           │                                                │ ▾ South   64–95  │
├───────────┴──── LED CHAIN · wiring order ──────────────────┴──────────────────┤
│ 0 ●───────────────────────────●────────────────────────────────────────● 95   │
│   ▶ Index Chase   ◀◀ ▮▮ ▶▶   @41    ⇄ reverse   ↳ set LED 0    wiring: direct │
╰────────────────────────────────────────────────────────────────────────────────╯
```

Four regions: a left rail with **tools**, the center **design
canvas** with projection tabs along its top, a right **inspector** for
the selected element, and a bottom **LED chain** ribbon. The top bar
holds the component name, the Start-from picker, undo/redo, and Save.

Unlike the Viewport Designer, which PATCHes a live effect's controls,
the Component Designer edits a **document** — a draft
`AttachmentTemplate`. There is no live-effect coupling. The draft lives
in the modal; Save commits it to the template REST API. Cancel
discards it. This is closer to a vector editor than to the Viewport
Designer's draft/commit-to-effect loop, though it reuses the same
modal shell and unsaved-changes discipline.

### 5.2 Projection Planes

The canvas represents one plane of physical space. Tabs across its top
select which: **Top-down**, **Front wall**, **Left wall**, **Right
wall**, **Back wall**. This is design doc 18's `project_2d` model made
explicit and editable. A component records its plane in
`ComponentDesignerMeta.projection` as **authoring metadata only**: it
is saved on the template, but v1 wires it into nothing downstream —
the attachment suggestion/import path carries no `designer` data, so
the layout cannot consume it without plumbing this spec does not add.
The field is recorded for forward compatibility with a future 3D room
view (§3.2). The canvas shows a grid labelled in real units, and
the inspector's "real-world size" field ties the normalized `[0, 1]`
canvas to `physical_size_mm`.

The plane is metadata, not geometry — the topology is always stored in
normalized `[0, 1]` space, resolution-independent like every other
topology. A single component lives on a single plane in v1; a strip
that climbs from a wall onto a ceiling is two components, or one
top-down component whose path is read as a developed (unfolded)
length. Multi-plane components are a post-v1 extension.

### 5.3 Creating Shapes — The Four Tools

Each tool produces exactly one `LedTopology`.

- **Path** ✎ — the pen. Click to drop points; the strip follows the
  polyline. Double-click or Esc ends it. The path is **open** (two
  ends) or **closed** (a loop). This is the room-route workhorse, and
  it is the one tool that needs a new topology variant
  (`LedTopology::Path`, §6.2).
- **Shape** ▭ — parametric primitives: ring, matrix, rectangle
  perimeter, and polygon. These map to the existing `Ring`, `Matrix`,
  `PerimeterLoop`, and `Custom` topologies. Drag a handle or edit a
  field and the LEDs re-flow. Every stock fan and panel is one of
  these; the Shape tool is how a remix of one stays parametric.
- **Scatter** ⊹ — click to drop individual LEDs anywhere, with
  optional grid snap. Produces `LedTopology::Custom { positions }`.
  This is the tool for an irregular art piece or a set of point
  lights with no geometric relationship.
- **Trace** 🖼 — drop a photo of the real setup as a backing layer,
  calibrate scale by drawing one known length, then use Path or
  Scatter over it. Trace is not a fifth topology — it is a backdrop
  mode for the Path and Scatter tools. The image is session-only
  (§3.2).

### 5.4 Flowing LEDs Onto a Carrier

A shape is a **carrier**; LEDs flow onto it. For a `Path` the user
sets a **count** directly, or a **density** (LEDs/m) which the
designer multiplies by the calibrated real length to derive the count
— density is a convenience, the stored value is always the count. LEDs
distribute evenly by arc length along the polyline. For Shape
primitives the flow is parametric (N around a ring, a `W×H` grid). For
Scatter every LED is explicit.

Dragging the count re-flows the LEDs live. The carrier and the LEDs
are always consistent — there is no "apply" step between editing the
shape and seeing the LEDs move.

### 5.5 The LED Chain Ribbon

The bottom ribbon is the wiring-order view and the spec's answer to
§2.3. It shows the chain as LEDs `0..N-1` in **wiring order** — the
order the controller addresses them. Hovering an LED on the canvas
highlights it in the ribbon and vice versa.

- **Playback scrub** — a playhead runs `0 → N`; the **Index Chase**
  test pattern lights a moving dot along the canvas in wiring order.
  If the dot jumps across the component, the wiring is wrong and the
  user sees it immediately.
- **Reverse** ⇄ — flips wiring direction.
- **Set LED 0** ↳ — re-bases the chain origin to the selected LED.
- **Direct drag** — dragging an LED in the ribbon reorders the chain.

These operations all author one underlying value: `led_mapping`, the
spatial-index → physical-index table. Identity mapping (`None`) is the
default; any ribbon edit produces an explicit mapping. The user never
types an index array.

**Segments.** Separately from the wiring ribbon, the user tags named
regions of the component — "North wall", "Desk", "South wall". A
segment is selected on the **canvas** (drag-select a run of LED dots)
and named in the inspector; a `ComponentSegment` stores it as a name
plus a **spatial-order** index range (§6.4). Segments are deliberately
a *spatial* concept — a region of the drawn shape — held in the same
index space as the canvas and `led_positions`, so they never depend on
`led_mapping` and survive a wiring reverse unchanged. The wiring ribbon
neither displays nor edits segments. Segments are how one strip carries
the structure of a multi-wall run *without being split into separate
devices*: the component stays one `AttachmentTemplate` on one
controller, with the segments as organizational metadata in the
inspector. (Promoting segments to runtime-addressable regions is a
spec 64 concern — §3.2.)

### 5.6 Templates, Remix, and Start-From

The modal opens on a **Start from** picker:

- **Blank** — an empty canvas; pick a tool.
- **A stock component** — loads a builtin's topology for editing.
  Saving forks a new user component (the daemon 403s builtin writes,
  §6.5); the designer mints a fresh id.
- **A custom component** — loads one of the user's own; saving
  updates it in place (`PUT`).
- **This device's topology** — when launched from a device, seeds the
  canvas from that device's current attachment topology.

Loading a component for editing requires its full topology, which
means the frontend must consume `GET /attachments/templates/{id}` —
an endpoint that ships today but is not yet called from the UI (§8).

### 5.7 The Room-Mapping Walkthrough

Concrete end-to-end for the motivating case:

1. From the layout canvas, **Design custom component**.
2. Pick the **Front wall** (or **Top-down**) plane; drop a room photo
   via **Trace**; draw one known length and type its real size.
3. **Path** tool — click points tracing the strip's actual route
   around the room.
4. Set density `60/m`; the designer derives the count from the
   calibrated length.
5. Drag-select the wall **segments** on the canvas; name them in the
   inspector.
6. Scrub **Index Chase** to confirm wiring; **reverse** if the chase
   runs backwards.
7. **Save** as "Living Room Loop". Back in the layout, bind it to the
   WLED device as one component. One device, one component, the real
   shape — no fake splitting.

---

## 6. Part I — Backend

The backend delta. All workspace Rust; verified with `just verify`.

### 6.1 What Already Exists (Do Not Rebuild)

The spec's backend section is mostly *confirmation*. Already shipped:

- **Template REST API** — `hypercolor-daemon/src/api/attachments.rs`,
  registered in `api/mod.rs` and `api/openapi.rs` (§6.5).
- **The registry** — `AttachmentRegistry`
  (`hypercolor-core/src/attachment/registry.rs`), held as
  `Arc<RwLock<AttachmentRegistry>>` in `AppState`, mutated live by the
  create/update/delete handlers.
- **User-template persistence** — `create_template` /
  `update_template` write TOML to
  `ConfigManager::data_dir().join("attachments")` and force
  `origin = User`; builtins are 403-protected.
- **`AttachmentTemplate`, `LedTopology`, `validate_template`** — the
  full geometry vocabulary, in `hypercolor-types` and
  `hypercolor-core`.

The backend work is three additions: a topology variant, a function
relocation, and a metadata field.

### 6.2 `LedTopology::Path`

A new variant on `LedTopology`
(`hypercolor-types/src/spatial.rs:151`):

```rust
Path {
    /// Polyline the strip follows, in normalized [0, 1] canvas space.
    /// At least two waypoints.
    waypoints: Vec<NormalizedPosition>,
    /// LEDs distributed evenly along the path by arc length.
    count: u32,
    /// When true, a closing segment from the last waypoint back to
    /// the first is appended (a loop).
    closed: bool,
}
```

It uses `NormalizedPosition`, not design doc 18's `PhysicalCoord` —
the implemented enum is 2D-normalized throughout, and physical scale
belongs in `physical_size_mm`, not the topology (§3.2). No `density`
field: density is an authoring-time convenience (§5.4).

`led_count()` returns `u32` (`hypercolor-types/src/spatial.rs:235`);
it gains `Path { count, .. } => *count`, joinable with the existing
`Strip | Ring` arm that already returns `*count`.

A new helper, `compute_path`, joins the `compute_*` family in the
relocated topology module (§6.3):

```rust
fn compute_path(
    waypoints: &[NormalizedPosition],
    count: u32,
    closed: bool,
) -> Vec<NormalizedPosition>
```

Algorithm (design doc 18 §3.2, adapted to normalized space): build the
segment list — the closing segment included when `closed` — sum
segment lengths to a total, space `count` LEDs evenly along it
(spacing `total / (count - 1)` for an open path, `total / count` for a
closed one), and linearly interpolate each LED's position along the
polyline. Degenerate inputs return safely, never a `NaN`: `count == 0` → empty;
`count == 1` → the first waypoint; fewer than two waypoints, or a
zero-length polyline (every waypoint coincident) → every LED at the
first waypoint. `validate_template` (§6.2.2) rejects zero-length paths
before storage, but `compute_path` stays defensive regardless.

`generate_positions` gains a `Path` arm dispatching to `compute_path`.

#### 6.2.1 Blast Radius of a New Variant

`LedTopology` is not `#[non_exhaustive]`, so every exhaustive `match`
must gain a `Path` arm. Verified arm-by-arm against the current tree:
**seven exhaustive matches compile-break** and must be updated, plus
**one wildcard `_` match** that compiles unchanged but silently routes
`Path` to default behavior and needs a deliberate decision.

| File                                                     | Match site                                         | Kind                    |
| --------------------------------------------------------- | --------------------------------------------------- | ----------------------- |
| `hypercolor-types/src/spatial.rs`                         | `LedTopology::led_count` (~235)                     | exhaustive              |
| `hypercolor-types/src/topology.rs` (post-§6.3)            | `generate_positions`                                | exhaustive              |
| `hypercolor-core/src/device/mock.rs`                      | `spatial_to_device_topology`                        | exhaustive              |
| `hypercolor-ui/src/components/layout_zone_properties.rs`  | `topology_name`                                     | exhaustive              |
| `hypercolor-ui/src/layout_geometry.rs`                    | orientation arm in `seeded_device_layout` (~225)    | exhaustive              |
| `hypercolor-ui/src/layout_geometry.rs`                    | `topology_visual_units` (~970)                      | exhaustive              |
| `hypercolor-ui/src/layout_geometry.rs`                    | `orientation_for_attachment_topology` (~1087)       | exhaustive              |
| `hypercolor-ui/src/layout_geometry.rs`                    | `normalize_zone_size_for_editor` (~406)             | wildcard `_` — see below |

The wildcard site does not block compilation: `normalize_zone_size_for_editor`
ends in `_ => size`, so a `Path` zone silently gets unclamped default
sizing. BE-2 must consciously decide whether a path zone wants
strip-like clamping — it likely does — and add an explicit `Path` arm
rather than leaving it on the wildcard.

Sites that only *construct* `LedTopology` are unaffected. Five files
hold the eight sites; two of the five are in `hypercolor-ui`, outside
the Cargo workspace — `cargo check --workspace` will **not** catch
them, so BE-2's UI arms are verified with `just ui-build` (§10, §11).

#### 6.2.2 Validation

`validate_template` (`hypercolor-core/src/attachment/registry.rs:454`)
gains `Path` rules: at least two waypoints; every waypoint coordinate
finite (no `NaN` or infinity); a total polyline length greater than a
small epsilon — two or more *coincident* waypoints are a zero-length
path and must be rejected, or `compute_path`'s arc-length
interpolation divides by zero; and `count > 0`. Because `led_count()`
returns `count`, the existing `led_names` / `led_mapping` length
checks then cover `Path` for free.

It also gains a **global LED-count ceiling** applied to every
topology. The existing `LedTopology::led_count()` is unchecked — it
computes `width * height` for `Matrix`, sums the `RingDef` counts for
`ConcentricRings`, and adds the four edges for `PerimeterLoop` — so on
request-controlled input it can wrap or panic *before* any cap
comparison runs. BE-2 therefore adds a checked sibling,
`LedTopology::checked_led_count() -> Option<u32>`: checked multiply
for `Matrix`, checked sums for `ConcentricRings` and `PerimeterLoop`,
`u32::try_from(positions.len())` for `Custom`, and the direct `count`
for `Strip` / `Ring` / `Path`. `validate_template` calls
`checked_led_count` and rejects the template when it returns `None`
(overflow) or a value above the cap — it never calls the unchecked
`led_count()` on unvalidated input. The cap is configurable; its
default is **65536**, far above any real single component (a 32×32
matrix is 1024 LEDs, a room-spanning strip a few hundred), so it
never constrains legitimate use. It exists only to stop a pathological
`Path { count }` or an overflowing `Matrix` from forcing a huge
`Vec<NormalizedPosition>` allocation when `template_detail` (§6.5)
generates positions for the `POST` / `PUT` response. An overflow or a
cap breach is a validation error, never a panic or a silent wrap.

### 6.3 Relocating `generate_positions` to `hypercolor-types`

`generate_positions` and its `compute_*` helpers currently live in
`hypercolor-core/src/spatial/topology.rs`. The frontend needs them: the
designer's local preview (§7.6) computes LED positions from a draft
topology in the browser, with no daemon round-trip. `hypercolor-ui`
depends on `hypercolor-types`, not `hypercolor-core`.

The function is pure geometry — it depends only on `LedTopology`,
`NormalizedPosition`, and the `RING_MARGIN` constant, all of which are
(or belong) in `hypercolor-types`, the zero-dependency vocabulary
crate. The relocation:

- Create `hypercolor-types/src/topology.rs` holding
  `generate_positions`, every `compute_*` helper, `compute_path`
  (§6.2), and `RING_MARGIN`.
- `hypercolor-core/src/spatial/topology.rs` becomes a thin
  `pub use hypercolor_types::topology::*;` re-export, so no
  `hypercolor-core` call site changes.

This is a pure refactor with no behavior change — done as its own wave
(BE-1) ahead of the `Path` variant so `compute_path` lands in its
final home.

### 6.4 `ComponentDesignerMeta`

A new optional field on `AttachmentTemplate`
(`hypercolor-types/src/attachment.rs:187`):

```rust
#[serde(default)]
pub designer: Option<ComponentDesignerMeta>,
```

`#[serde(default)]` keeps every existing builtin TOML and every
persisted user template valid with no migration — a template authored
before spec 66 simply has `designer: None`.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentDesignerMeta {
    #[serde(default)]
    pub projection: ProjectionPlane,
    #[serde(default)]
    pub segments: Vec<ComponentSegment>,
    /// Designer schema tag, e.g. "component-designer/1", for
    /// forward-compatible reads of templates authored by later
    /// designer versions.
    #[serde(default)]
    pub authored_with: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionPlane {
    #[default]
    TopDown,
    FrontWall,
    BackWall,
    LeftWall,
    RightWall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentSegment {
    /// Non-empty display name.
    pub name: String,
    /// Inclusive start, spatial-order index — the same index space as
    /// `led_positions` and the canvas, not the wiring chain.
    pub led_start: u32,
    /// LED span length; must be greater than zero.
    pub led_count: u32,
}
```

This is the *only* data the designer needs that the topology cannot
already express. The reference photo, the cm-per-grid scale, and the
density are authoring-session state, not persisted (§3.2): the photo
is session-only, real size already has `physical_size_mm`, and density
is folded into `count`.

`validate_template` gains, for every `ComponentSegment`: a non-empty
`name`; `led_count > 0` (a zero-length segment is meaningless); and
`led_start.checked_add(led_count)` yielding `Some(end)` with `end`
not exceeding the template's **validated** LED count — the `Some`
value already returned by `checked_led_count()` (§6.2.2), never the
unchecked `led_count()`. Checked arithmetic throughout, because
`led_start` and `led_count` are request-controlled `u32`s and a plain
`+` can overflow. Segment overlap is permitted (the designer UI guides
toward disjoint spans but does not forbid logical overlap).

### 6.5 The Template REST API

Already shipped in `hypercolor-daemon/src/api/attachments.rs`,
registered in `api/mod.rs` and documented in `api/openapi.rs`. Spec 66
adds **no routes**.

| Method | Path                                  | Handler            |
| ------ | ------------------------------------- | ------------------ |
| GET    | `/api/v1/attachments/templates`       | `list_templates`   |
| POST   | `/api/v1/attachments/templates`       | `create_template`  |
| GET    | `/api/v1/attachments/templates/{id}`  | `get_template`     |
| PUT    | `/api/v1/attachments/templates/{id}`  | `update_template`  |
| DELETE | `/api/v1/attachments/templates/{id}`  | `delete_template`  |
| GET    | `/api/v1/attachments/categories`      | `list_categories`  |
| GET    | `/api/v1/attachments/vendors`         | `list_vendors`     |

`list_templates` paginates and filters by category, vendor, origin,
text query, and controller/slot compatibility. `create_template` and
`update_template` force `origin = User` and write a TOML file;
builtins 403 on write. `delete_template` 409s when a device profile
still binds the template.

Two backend tasks here. First, a **contract confirmation**: the
`GET /attachments/templates/{id}` detail response must serialize the
new `designer` field and the full `topology` (including `Path`). If
the detail response type is the `AttachmentTemplate` itself, `designer`
flows automatically; if it is a hand-mirrored struct, it must gain the
field. BE-3 verifies and, if needed, extends it. This is the seam the
frontend's Start-from flow (§5.6) depends on.

Second, a **write-path hardening**. The Component Designer becomes the
primary writer of user templates, so the create/update path must be
robust. Today `create_template` / `update_template` take the registry
write lock and call `register_and_persist_template`, which registers
the template into the live `Arc<RwLock<AttachmentRegistry>>` and
*then* writes the TOML — a failed write leaves a ghost template
registered in memory until the next daemon restart. BE-3 makes this
atomic: validate, serialize, write to a temp file, atomic-rename, and
register only on success (or roll the registry back on failure). The
handler must also validate the request-body `id` as a
filesystem-safe slug before it becomes the `{id}.toml` filename — an
`id` containing `../` or a path separator is a path-traversal write.
Reuse the `sanitize_layout_identifier` pattern already in
`layout_geometry.rs`.

### 6.6 Backend Non-Goals

- No registry file-watch / rescan (§3.2).
- No new persistence format — TOML via the existing manifest
  (`AttachmentTemplateManifest`, `schema_version` reserved-but-inert).
- No `Composite` topology, no `PhysicalCoord`, no `LedDensity` type.

---

## 7. Part II — Frontend

The modal. All `hypercolor-ui`; outside the workspace; verified with
`just ui-build` and `just ui-test`.

### 7.1 Modal Architecture

The Component Designer follows the Viewport Designer's decomposition
(spec 46 §4.3, §10): a thin orchestrator composes focused modules
rather than one monolith. New module
`hypercolor-ui/src/components/component_designer/`:

| Module                | Responsibility                                              |
| --------------------- | ----------------------------------------------------------- |
| `mod.rs`              | Modal shell, orchestrator, draft state, Save/Cancel.        |
| `canvas.rs`           | The design canvas — projection tabs, grid, pan/zoom, render. |
| `tools/path.rs`       | Path (pen) tool.                                            |
| `tools/shape.rs`      | Shape (parametric primitive) tool.                          |
| `tools/scatter.rs`    | Scatter (hand-placed LED) tool.                             |
| `tools/trace.rs`      | Trace (reference-image backdrop) mode.                      |
| `inspector.rs`        | Right-rail property editor for the selection.               |
| `chain_ribbon.rs`     | The LED chain — wiring order, playback, `led_mapping` edits. |
| `preview.rs`          | Local test-pattern preview engine (§7.6).                   |

Draft state — the in-progress `AttachmentTemplate` plus session-only
authoring state (reference image, scale calibration, active tool,
selection) — lives in an orchestrator-owned struct, mirroring the
layout editor's `layout_page_state.rs`. Undo/redo reuses the snapshot
pattern of `layout_history.rs`: each topology mutation pushes a
snapshot; `Ctrl+Z` / `Ctrl+Shift+Z` traverse it.

The modal is a document editor, not an effect-control PATCH loop:
nothing is sent to the daemon until Save. This is the deliberate
divergence from the Viewport Designer (§5.1).

### 7.2 The Design Canvas

`canvas.rs` reuses `layout_canvas.rs`'s proven interaction substrate —
the RAF scheduler for non-reactive drag updates, pointer handling, and
the 4-corner resize handles — rather than reinventing them. It adds:

- **Projection tabs** along the top (§5.2), each a `ProjectionPlane`.
- **A real-unit grid** whose spacing reflects the scale calibration;
  the grid is a render layer, toggleable.
- **Shape and LED rendering** — the carrier as a SilkCircuit-purple
  path or outline, LEDs as cyan dots, LED 0 and the selection in
  coral.
- **The reference-image layer** (Trace mode) beneath everything, at
  adjustable opacity.

Coordinates are normalized `[0, 1]`; the canvas maps them to CSS
pixels for display, exactly as `layout_canvas.rs` does.

### 7.3 The Tool Modules

Each tool owns its pointer interactions and emits topology mutations
to the orchestrator:

- **`tools/path.rs`** — click to append a waypoint, drag a waypoint to
  move it, double-click / Esc to finish, a control to toggle
  `closed`. Emits `LedTopology::Path`. The count and density controls
  live in the inspector (§7.4).
- **`tools/shape.rs`** — pick a primitive (ring, matrix, rectangle
  perimeter, polygon); drag handles and edit inspector fields to set
  parameters. Emits `Ring`, `Matrix`, `PerimeterLoop`, or `Custom`.
- **`tools/scatter.rs`** — click to drop an LED, drag to move, click
  to select-and-delete; optional grid snap. Emits
  `LedTopology::Custom`.
- **`tools/trace.rs`** — a backdrop mode, not a topology producer:
  accepts a dropped image (`<input type=file>` / drag-drop), runs the
  two-click scale calibration, then defers to the Path or Scatter
  tool drawing over the image.

### 7.4 The Inspector

`inspector.rs` renders the selected element's properties: for a path,
count / density / `closed` / real size; for a primitive, its
parameters; for a single LED, its index and position; plus the
component-level fields — name, vendor, category, `physical_size_mm`,
`ProjectionPlane`. `validate_template` rejects an empty `vendor`, so
the designer defaults it to "Custom" and exposes it as an editable
field (§8). Numeric fields commit on blur / Enter / debounce
and support arrow-key nudge, matching the Viewport Designer's input
discipline (spec 46 §6.1). The segment list (§5.5) is an inspector
section.

### 7.5 The LED Chain Ribbon

`chain_ribbon.rs` renders the §5.5 ribbon: the `0..N` strip, the
playback scrub driving the Index Chase pattern, **reverse**, **set LED
0**, and drag-to-reorder. Every interaction mutates the draft's
`led_mapping`. Canvas ↔ ribbon hover is bidirectional — the ribbon is
wiring-order, the canvas spatial-order, and the UI bridges the two
through `led_mapping`. The ribbon neither shows nor edits segments;
segments are a spatial, canvas-side concept (§5.5).

### 7.6 Local Preview Engine

`preview.rs` is the reason §6.3 relocates `generate_positions`. It
takes the draft `LedTopology`, calls `hypercolor_types::topology::
generate_positions` directly in WASM, and renders test patterns onto
the resulting positions on a canvas — entirely client-side, no daemon:

- **Index Chase** — a moving dot at the playhead's index, with a
  fading trail; the wiring-order verifier (§5.5).
- **Wiring Heatmap** — LED `i` colored by `i / N` along a hue ramp, so
  the whole chain's order is legible at a glance.
- **Rainbow** — a hue gradient across the shape; what a real effect
  roughly looks like.
- **Solid** — a sanity check.

The designer previews *geometry and wiring*, not daemon effects
(§3.2). Real-effect preview is the layout's job.

### 7.7 Launch Points

The modal is opened from existing surfaces, returning a saved template
id to the caller:

- **The attachment panel** (`attachment_panel.rs`) — a "Design custom
  component" action beside the existing library picker. Its form-based
  trivial-Strip/Matrix create path stays for the quick case; the
  designer is the visual path.
- **The layout canvas** — when binding a device, a "Design custom
  component" affordance; on save, the new component is available to
  bind. This works on the current `/layout` page and, unchanged, in
  the spec 65 Studio Stage Layout view (§1.3).
- **The component picker** (`component_picker.rs`) — an "Edit" action
  on a user component, and "Duplicate & edit" on any component,
  opening the designer via Start-from (§5.6).

### 7.8 Studio Integration

No special integration work: the designer is a modal, launched from a
surface, overlaying it. Whether the launch surface is today's
`/layout` page or spec 65's Studio Stage, the designer is identical.
Spec 66 and spec 65 do not interlock — they only share the layout
canvas as one of several launch points.

---

## 8. The Frontend-Backend Contract

The entire seam between Part I and Part II:

1. **Three shared-type additions in `hypercolor-types`** — consumed by
   both sides, so they land first:
   - `LedTopology::Path { waypoints, count, closed }` (§6.2).
   - `AttachmentTemplate.designer: Option<ComponentDesignerMeta>`,
     with `ComponentDesignerMeta`, `ProjectionPlane`,
     `ComponentSegment` (§6.4).
   - `generate_positions` (and `compute_path`) available at
     `hypercolor_types::topology` (§6.3).
2. **The template REST API** — already shipped (§6.5); spec 66 adds no
   routes. Two contract points. (a) The `GET .../templates/{id}`
   detail response must serialize `topology` and the new `designer`
   field — BE-3 confirms it. (b) A save (`POST` / `PUT`) sends an
   `AttachmentTemplate` JSON body, and `validate_template` rejects it
   unless `id`, `name`, and `vendor` are all non-empty and the
   `topology` is valid (for `Path`: at least two waypoints,
   `count > 0`). The designer guarantees this — `id` is a minted slug,
   Save is disabled until `name` is non-empty, and `vendor` defaults
   to "Custom".
3. **Frontend API client additions** in
   `hypercolor-ui/src/api/devices.rs`. The UI today has only
   `fetch_attachment_templates` (list → `TemplateSummary`) and
   `create_attachment_template` (`POST`) — and the latter currently
   *discards* the response body's topology by deserializing it as the
   summary type. FE-7 needs:
   - a UI-side `TemplateDetail` type mirroring the daemon's detail
     response — full `topology`, `led_mapping`, and `designer`; the UI
     has only `TemplateSummary` today;
   - `fetch_attachment_template_detail(id)` — `GET .../templates/{id}`
     → `TemplateDetail`; Start-from (§5.6) depends on it;
   - `update_attachment_template(template)` — `PUT .../templates/{id}`
     → `TemplateDetail`, to save edits to a user component in place;
   - `create_attachment_template` retargeted to deserialize the `POST`
     response as `TemplateDetail`. The daemon already returns the full
     detail on both `POST` and `PUT`, so once the client parses it a
     save round-trips the canonical template with no follow-up fetch.

   The category picker uses the static `AttachmentCategory` enum, so
   the designer does **not** consume `GET .../categories` or
   `.../vendors`; template *deletion* is a component-picker concern,
   out of designer scope.

No new daemon routes, no WebSocket additions, no engine changes. Once
the three type additions land, Part I and Part II proceed in parallel.

---

## 9. Delivery Waves

Backend waves verify with `just verify`; frontend waves with
`just ui-build` and `just ui-test`.

| Wave  | Side     | Scope                                                                                              | Gate          |
| ----- | -------- | -------------------------------------------------------------------------------------------------- | ------------- |
| BE-1  | Backend  | Relocate `generate_positions` / `compute_*` / `RING_MARGIN` into `hypercolor-types::topology`; `hypercolor-core` re-exports. Pure refactor. | —             |
| BE-2  | Backend + UI | `LedTopology::Path` + `compute_path` + the `led_count` arm + the new `checked_led_count` method + the seven exhaustive match-arm fixes plus an explicit arm at the wildcard site (§6.2.1); `validate_template` Path and LED-cap rules. Verifies with `just verify` **and** `just ui-build` — the match arms span both. | BE-1 |
| BE-3  | Backend  | `ComponentDesignerMeta` / `ProjectionPlane` / `ComponentSegment` + optional `AttachmentTemplate.designer`; segment validation; confirm the `get_template` detail response; TOML round-trip tests. | —             |
| FE-1  | Frontend | Modal shell + orchestrator + draft state + undo/redo; design canvas with projection tabs, grid, pan/zoom, selection. No tools yet. | —             |
| FE-2  | Frontend | Local preview engine — positions via `hypercolor-types::topology`, the four test patterns.         | BE-1, FE-1    |
| FE-3  | Frontend | Path tool — waypoint drawing, count / density / `closed`, live LED flow.                            | BE-2, FE-1    |
| FE-4  | Frontend | Shape tool (Ring / Matrix / PerimeterLoop / polygon) and Scatter tool. Existing topologies.        | FE-1          |
| FE-5  | Frontend | Trace mode — reference-image backdrop, scale calibration.                                          | FE-3, FE-4    |
| FE-6  | Frontend | LED chain ribbon — wiring order, playback, reverse, set-LED-0, drag (all editing `led_mapping`). Segment tagging — spatial canvas drag-select plus inspector naming (§5.5).               | FE-1, BE-3    |
| FE-7  | Frontend | Start-from picker; `fetch_attachment_template_detail`; load for edit / remix; fork builtins; Save (POST / PUT). | BE-3, FE-1    |
| FE-8  | Frontend | Launch points (attachment panel, layout canvas, component picker); polish; agent-browser QA sweep. | FE-1 – FE-7   |

BE-1 through BE-3 are independently shippable and unblock the gated
frontend waves. FE-1 is the spine every other frontend wave builds on.
FE-4 is pure frontend with no backend gate and can land early; FE-6's
chain-ribbon work is likewise pure frontend, but the wave also carries
segment tagging, which persists `ComponentSegment`s, so FE-6 as a
whole gates on BE-3. **FE-5 (Trace) is the designated descope point:**
the designer is fully usable for all three flows without it — Path,
Shape, and Scatter all draw freehand — so if v1 scope tightens, FE-5
is the first wave to cut, with no rework to the rest.

---

## 10. Verification Strategy

### 10.1 Automated

- **Backend** — `just verify` (fmt + lint + test) green after every BE
  wave. New tests live in `tests/` directories per the workspace
  convention, never inline `#[cfg(test)]`. Required coverage:
  `Path` position generation exercised through the public
  `generate_positions` (open paths, closed loops, degenerate counts,
  single waypoint) — `compute_path` stays private, like every other
  `compute_*` helper, and is covered transitively; `LedTopology::Path`
  `led_count` and the new `checked_led_count` (overflow → `None`, plus a
  cap-breach case); `validate_template` Path, segment, and LED-cap
  rules; and an
  `AttachmentTemplate` TOML round-trip with a `designer` block and
  without one (the `#[serde(default)]` compatibility path).
- **Frontend** — `just ui-test` and `just ui-build` green after every
  FE wave. `just ui-build` is the *only* check that catches the
  `hypercolor-ui` match-arm fixes from §6.2.1, since the UI is outside
  the workspace.

### 10.2 Visual and Manual (agent-browser)

A structured QA sweep in FE-8, against a live daemon, walks the §5.7
room-mapping flow plus:

- Each tool produces a valid component that saves and reloads.
- Start-from: a builtin forks correctly; a user component updates in
  place; the daemon's builtin-write 403 surfaces as a clean
  "duplicate to edit" message.
- The chain ribbon: reverse, set-LED-0, and drag each produce a
  `led_mapping` that the Index Chase pattern then renders in the new
  order.
- Projection tabs, grid, undo/redo, and the unsaved-changes guard.
- No internal type name, enum variant, or raw `led_mapping` array is
  visible anywhere (§4 hard rule) — grep the rendered DOM.

### 10.3 Cross-Model Review

This spec is reviewed by Codex before implementation, iterated across
multiple rounds until the contracts are locked. The FE-8 visual result
is reviewed with the `frontend-design` / `effect-reviewer` lens before
sign-off.

---

## 11. Known Constraints

- **`hypercolor-ui` is outside the Cargo workspace.** `cargo check
  --workspace` does not cover it; two of the five §6.2.1 match-arm
  files live there. Every frontend wave verifies with `just ui-build`
  explicitly.
- **Adding a `LedTopology` variant is a breaking change.**
  `LedTopology` is not `#[non_exhaustive]`; the seven exhaustive match
  sites of §6.2.1 must be updated in lockstep with BE-2, and the one
  wildcard site given an explicit `Path` arm.
- **The trace reference image is session-only.** Re-editing a
  component's geometry is lossless; re-opening it does not restore the
  backdrop photo. Media-backed persistence is a post-v1 extension.
- **The designer previews test patterns, not daemon effects.** It
  authors geometry and wiring; effect preview is the layout's role.
- **v1 components are single-topology.** A multi-part installation is
  multiple components composed in a layout. `Composite` is post-v1.
- **No registry file-watch.** The designer registers through the REST
  create path, which mutates the live registry, so this does not
  affect it — but a TOML dropped into the directory by hand still
  needs a daemon restart.
- **Spec 65 is in flight.** The designer is a modal and does not
  interlock with the Studio redesign; it shares only the layout
  canvas as a launch point.

---

## 12. Recommendation

Build it, in the eleven waves of §9.

The case is unusually strong because most of the system already
exists. The `AttachmentTemplate` model, the registry, user-template
persistence, and the full template REST API all ship today; the web UI
already calls part of that API. Design doc 18 already designed the
`Path` topology this needs and called it "the workhorse." What is
missing is one topology variant, one function relocation, one optional
field — and the modal that finally lets a user *draw* instead of
hand-typing coordinates.

Splitting the spec into a contained backend (§6) and a frontend-heavy
modal (§7) across a three-item contract (§8) lets the two tracks run
in parallel after the shared types land, mirroring the spec 64 / 65
division. The backend risk is the `LedTopology` variant's blast radius
— fully enumerated and code-verified in §6.2.1, eight known sites —
and the frontend risk is scope, contained by the §3.2 non-goals and
the FE-5 descope valve (§9). With those cut, v1 is
the visual authoring surface the half-built attachment system has been
waiting for.

---

## 13. Appendix A — File Inventory

New files:

| Path                                                          | Purpose                                      |
| ------------------------------------------------------------- | -------------------------------------------- |
| `crates/hypercolor-types/src/topology.rs`                     | Relocated `generate_positions` / `compute_*` + `compute_path` |
| `crates/hypercolor-ui/src/components/component_designer/`     | The Component Designer modal (§7.1 modules)  |

Modified files:

| Path                                                          | Change                                              |
| ------------------------------------------------------------- | --------------------------------------------------- |
| `crates/hypercolor-types/src/spatial.rs`                      | `LedTopology::Path` variant; `led_count` arm        |
| `crates/hypercolor-types/src/attachment.rs`                   | `designer` field; `ComponentDesignerMeta`, `ProjectionPlane`, `ComponentSegment` |
| `crates/hypercolor-types/src/lib.rs`                          | Register the `topology` module                     |
| `crates/hypercolor-core/src/spatial/topology.rs`              | Becomes a `pub use hypercolor_types::topology::*` re-export |
| `crates/hypercolor-core/src/attachment/registry.rs`           | `validate_template`: `Path` and segment rules       |
| `crates/hypercolor-core/src/device/mock.rs`                   | `Path` match arm                                    |
| `crates/hypercolor-daemon/src/api/attachments.rs`             | Confirm `get_template` detail carries `designer`; make `register_and_persist_template` atomic; validate template `id` as a safe slug |
| `crates/hypercolor-ui/src/api/devices.rs`                     | `TemplateDetail` type; `fetch_attachment_template_detail`; `update_attachment_template`; `create_attachment_template` parses `TemplateDetail` |
| `crates/hypercolor-ui/src/components/layout_zone_properties.rs` | `Path` match arm (`topology_name`)                |
| `crates/hypercolor-ui/src/layout_geometry.rs`                 | `Path` arms — three exhaustive matches plus the wildcard site (§6.2.1)               |
| `crates/hypercolor-ui/src/components/attachment_panel.rs`     | "Design custom component" launch affordance         |
| `crates/hypercolor-ui/src/components/component_picker.rs`     | "Edit" / "Duplicate & edit" launch affordances      |
| `crates/hypercolor-ui/src/components/layout_builder.rs`       | "Design custom component" affordance on the layout canvas |
