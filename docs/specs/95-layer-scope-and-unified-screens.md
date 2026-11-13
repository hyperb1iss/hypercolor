# Spec 95: Layer scope and unified screens

> A layer can only cover a whole zone today, so "ripples on just the keyboard"
> means building a second zone and re-running the base effect inside it, where
> it no longer lines up with the rest of the rig. Screens are worse: each LCD is
> both a box in an LED zone and a separate hidden zone of its own, with a third
> storage place for its "default" face. This spec gives a layer an optional
> scope (the outputs of its zone it applies to), renders scoped layers without
> breaking the base effect underneath, and then makes a screen's face nothing
> more than a layer scoped to that screen.

**Status:** Draft rev 2. W1 (per-screen backdrops, section 5.10) is implemented on the `nova/screen-backdrops` branch and is not merged; nothing else in this spec is built. Rev 2 folds in two independent reviews (engine and rendering; compatibility, migration, and UX). Section 13 records every finding and what became of it.
**Crates:** `hypercolor-types` (layer and scene types, API contracts), `hypercolor-core` (scene manager, spatial, effect pool), `hypercolor-daemon` (scene domain, render zones, SparkleFlinger, display lane and output, REST, MCP, stores), `hypercolor-ui` (Studio)
**Supersedes:** Design 73 §6.2 "Screens" and the Screens rows of §6.3 and §9; Spec 65 §4 (the Screen row), §6.6, §6.7, §8; Spec 60 §5.2.1 "Per-layer canvas dimensions"; Spec 69 §8 W4.2. Section 3 lists each one with its reason.
**Amends:** Spec 60 §5.1 and §8.2; Spec 64 §6.1 and §9.6; Spec 69 §3.6; Spec 86 §3.1, §3.2, §5.1, §5.2
**Author:** Nova
**Date:** 2026-11-11

---

## Table of contents

0. [How to read this spec](#0-how-to-read-this-spec)
1. [Summary](#1-summary)
2. [Problem](#2-problem)
3. [Goals, non-goals, and what this supersedes](#3-goals-non-goals-and-what-this-supersedes)
4. [Part A: layer scope, the model](#4-part-a-layer-scope-the-model)
5. [Part A: layer scope, rendering](#5-part-a-layer-scope-rendering)
6. [Part B: screens as scoped layers](#6-part-b-screens-as-scoped-layers)
7. [Part C: Studio](#7-part-c-studio)
8. [Delivery waves](#8-delivery-waves)
9. [Risks and the spikes that retire them](#9-risks-and-the-spikes-that-retire-them)
10. [Decisions taken, owner may overrule](#10-decisions-taken-owner-may-overrule)
11. [Open question for the owner](#11-open-question-for-the-owner)
12. [Appendix: file inventory](#12-appendix-file-inventory)
13. [Review record](#13-review-record)

---

## 0. How to read this spec

**Verified against proposed.** Every statement about how the code behaves
today carries a `path:line` citation, read on 2026-11-11 against `main` at
`d01dca1c6`, or a route on the owner's live daemon. A statement with no
citation is a proposal. Paths are relative to `crates/` unless they start with
`docs/`, `sdk/`, or `python/`. After a file's first full citation, later ones
shorten it to its trailing path, and files under
`hypercolor-daemon/src/render_thread/` are cited from that directory
(`render_zones/...`, `sparkleflinger/...`, `display_lane.rs`,
`frame_sampling.rs`).

**Ship state.** The tree is at 0.9.0 (the workspace `Cargo.toml:12`). The 1.x
stability promise (`docs/content/api/stability.md`) is not in force yet, but
the Python client on PyPI already exposes `set_display_face`
(`python/src/hypercolor/client.py:1091`) and the MCP tool is published. This
spec is written to the 1.x rules: every wire change is additive, and every
stored document that loads today keeps loading. That is a settled constraint
(§10), not a later fork.

**Vocabulary.** A *scene* is a whole-rig configuration. A *zone* is a flexible
partition of the rig with its own canvas and layer stack. An *output* is one
device segment a zone claims (the code calls the list `layout.zones` and the
type `Output`, `hypercolor-types/src/spatial.rs:345`; on the wire it is a zone
*member*, `hypercolor-types/src/api/scene.rs:304-316`). A *screen* is an output
that is a device LCD. A *face* is an HTML effect built for a screen. These
terms are new here:

- **Scope**: the set of a zone's outputs a layer applies to. No scope means
  the whole zone.
- **Scope class**: the set of outputs in a zone that see exactly the same
  layers. An engine term; it never reaches a user.
- **Frame**: the rectangle on the zone canvas a scoped layer's source fills.
- **Screen layer**: a layer whose scope holds only screens. It renders at each
  screen's panel resolution.
- **Kept face**: today's per-display default face, under the name this spec
  uses for it.

Making face controls findable in today's Studio shipped on `main` while this
spec was drafted (`hypercolor-ui/src/pages/studio/face_composition.rs:409-413`).
This spec builds on that and replaces the surfaces it patched.

---

## 1. Summary

**Part A** adds `scope` to `SceneLayer`: an optional list of output ids from
the layer's own zone. The one-zone-per-output rule is untouched. The engine
groups a zone's outputs into scope classes, composes one texture per class
from shared producer frames (the base effect renders once and stays continuous
under every class), and samples each output from its class's texture. A scoped
layer is framed to its outputs through one source-to-zone mapping that
composition and input both use, so a ripple centred in the layer is centred on
the keyboard and lands under the finger.

**Part B** makes a face a screen layer in the zone that places the screen. The
engine still renders it at panel resolution in a dedicated target, because a
face inside a zone canvas would lose most of its pixels; that target becomes an
engine detail the scene format and the UI never mention. Stored display zones
keep loading and convert, only when the conversion is exact, the next time
their scene is activated. The `/displays/{id}/face*` routes and the
`set_display_face` MCP tool become views over the new model, specified field
by field in §6.7.

**Part C** removes the Screens tab. A screen is a box on its zone's canvas and
a card in the device pane; its face and settings open from the box. Any output
or selection of outputs can take a layer from the same place.

---

## 2. Problem

### 2.1 A layer cannot target part of a zone

`SceneLayer` carries a source, blend, opacity, transform, adjustments,
bindings, and an enabled flag (`hypercolor-types/src/layer.rs:73-107`). It has
no field that limits where it applies. A zone composes its layers bottom to top
into one canvas at the zone's extent
(`hypercolor-daemon/src/render_thread/render_zones/layer_rendering.rs:245-257`),
and every output samples that canvas.

The only way to put an effect on one device is a second zone. The live
"Studio Session" scene on the owner's rig shows the cost (`GET /api/v1/scene`,
2026-11-11): the Default zone runs Bubbles on 67 outputs, and the ROLI zone
(2 outputs) and the Push 2 zone (6 outputs) each run their own Bubbles under
Touch Ripple in `screen` blend. That is five HTML effect renderers where the
intent needs two, and three unsynchronized copies of the base effect on three
canvases, which Design 73 §7.2 already called "almost never the intent"
(`docs/design/73-studio-zone-tabs.md:612-614`).

### 2.2 A screen is three things at once

1. **An output in an LED zone.** Each LCD is a member of a zone with the
   segment name `Display` (`hypercolor-core/src/spatial/mod.rs:49`, `:147-151`;
   five such members sit in the live Default zone). The LED sampler skips it
   (`:153-157`); its placement is the viewport that crops the scene canvas as
   the screen's backdrop (`hypercolor-daemon/src/display_output/mod.rs:945-1007`).
2. **A zone of its own.** The daemon creates a `Display` role zone per
   connected screen (`hypercolor-core/src/scene/mod.rs:669-737`, driven by
   `hypercolor-daemon/src/domain/display.rs:1020-1034`). It has no outputs,
   renders at panel resolution (`domain/display.rs:59-75`), and composites over
   the backdrop with a blend mode and opacity stored on
   `DisplayFaceTarget` (`hypercolor-types/src/scene.rs:164-178`).
3. **A preference outside the scene.** A per-display default face lives in its
   own store (`hypercolor-daemon/src/display_preferences.rs:39-48`) and is
   materialized as a runtime-only zone that is hidden whenever the scene's
   display zone has an effect layer (`hypercolor-core/src/scene/mod.rs:1621-1643`).

The live rig shows why that confuses. The Push 2 face (System Pulse) and the
pump face (Pulse Temp) are scene layers. One Lian Li TL panel shows Neon Clock
from the stored default (`GET /api/v1/displays/{id}/face` reports
`live_scope: default`, `scene_assigned: false`); the other two TL panels have
no face. The scene document lists all three TL display zones with no layers at
all, so the clock's hour format has no layer row to live on. Studio now patches
that with a separate card that names the default and edits it in place
(`face_composition.rs:257-258`, `:412-413`), which makes the setting reachable
and keeps the two-place model visible.

The split is also wired through the daemon as special cases: applying an effect
refuses a display zone (`hypercolor-core/src/scene/mod.rs:1473-1475`), scene
clear and member edits skip or refuse them
(`hypercolor-daemon/src/domain/scene_tree.rs:413-426`, `:874-888`), and only a
display zone takes a canvas size from a request
(`hypercolor-core/src/scene/mod.rs:1163-1166`). In the UI it is
`SurfaceKind::Screen` (`hypercolor-ui/src/zones/surface.rs:13-19`), the Screens
tab (`hypercolor-ui/src/pages/studio/zone_strip.rs:3`), the default-face card,
and a "This screen / All screens" picker scope
(`hypercolor-ui/src/components/layer_panel/source.rs:95-115`).

---

## 3. Goals, non-goals, and what this supersedes

### 3.1 Goals

- Any layer can apply to any subset of its zone's outputs, at any position in
  the stack, with every blend mode, while layers beneath it stay continuous
  across the whole zone.
- "Just the keyboard" means exactly the keyboard's LEDs, including when the
  keyboard's box overlaps another output's box on the canvas.
- An input-reactive layer scoped to a device responds to that device's keys
  and touches at the right positions, whatever the device's rotation and the
  layer's own transform.
- A screen is an ordinary output. Its face is a layer scoped to it, edited in
  the same stack as every other layer, at the panel's full resolution.
- Every stored scene, backup, and preference that loads today keeps loading,
  and every REST route and MCP tool that works today keeps working.
- No performance baseline moves down. Scoped rendering costs are stated in
  §5.8 and are paid only by scenes that use scope.

### 3.2 Non-goals

- **A node graph or per-layer masks.** The stack stays one linear list per
  zone (Spec 60 §3.2, `docs/specs/60-user-media-and-layer-stack.md:237-238`;
  Spec 65 §3.2, `docs/specs/65-studio-composition-ui.md:210-220`). Scope
  filters that list per output; it adds no ports, edges, or painted masks.
- **Soft scope edges.** A scope is a set of devices, not a region of canvas,
  so its boundary is the device's own edge.
- **A zone-coordinate effect limited to some outputs.** A scoped layer is
  always framed to its outputs (§5.5). "Continue the rig-wide sweep, but only
  on these three fans" is a different feature, a window into the zone's
  canvas, and an unscoped layer cannot provide it either. It is deferred, with
  the additive door it would use named in §10.
- **An output in two zones.** Exclusivity stays
  (`hypercolor-types/src/scene.rs:416-438`; Spec 64 §6.3). Concurrent scenes
  stay a non-goal (Spec 64 §3.2).
- **Per-zone or per-layer frame rates** (Spec 86 §0). Screens keep the cadence
  caps they have today (`display_output/mod.rs:1029-1062`).
- **Rig-level layers for LED outputs.** A layer that follows a keyboard across
  scenes is the "Surfaces" north star of Spec 64 §3.2 and stays out.
- **Changing how faces are authored.** The face SDK and `DisplayDescriptor`
  are untouched.

### 3.3 Supersedes and amends

| Document and section | What it says | What changes | Why |
| --- | --- | --- | --- |
| Design 73 §6.2 "Screens" (`docs/design/73-studio-zone-tabs.md:457-464`) | LCDs are not zones in the UI vocabulary; one Screens tab holds them | Superseded. No Screens tab. A screen is a box and a card in its zone's tab | The tab made a screen a third kind of thing and hid the fact that it already sits in a zone. The owner asked why screens are special; they should not be |
| Design 73 §6.3 "What leaves the pane" (`:507-510`) and §9 "the Screens tab keeps All screens" (`:675`) | The Screens section moves to its own tab; the picker keeps an "All screens" target | Superseded. Screens are device cards in their zone's section. "All screens" survives as a scene-wide command (§7.5) | One target control replaces two, and the scene-wide gesture is kept |
| Design 73 §1.4 (`:125-137`) | A scene holds zones plus a separate "Screen surface" per LCD | Amended. A scene holds zones. A screen is an output a zone claims, and its stack is that zone's layers scoped to it | The hierarchy loses a node |
| Spec 65 §4 Screen row (`docs/specs/65-studio-composition-ui.md:294`), §8 (`:528-545`) | A Display role group is a "Screen" surface with its own stack | Superseded | A surface is a zone; a screen is an output |
| Spec 65 §6.6 (`:446-476`) | Add-layer target scopes: this surface, all zones, all screens, whole scene | Superseded by layer scope (§4, §7.2). Design 73 §9 had already deleted three of the five | One model: the target is a set of outputs |
| Spec 65 §6.7 (`:478-502`) | Screens parity is built on layer routes addressed to display zones | Superseded. Those routes stay as views with per-operation adapters (§6.7); Studio addresses the owning zone | The display zone stops being authored |
| Spec 60 §5.1 | `SceneLayer` fields | Amended: adds `scope` | Part A |
| Spec 60 §5.2.1 (`docs/specs/60-user-media-and-layer-stack.md:455-460`) | Every layer renders at its zone's canvas dimensions; layers do not override it | Superseded. A scoped layer renders at its own extent (§5.5) | A layer framed to a keyboard or a panel needs its own aspect and size |
| Spec 60 §8.2 (`:888-896`) | Display-face zones are siblings of the scene canvas, not layers inside it | Amended. True of the engine, no longer true of the scene format: a face is authored as a layer in a zone, and the engine still renders it beside the canvas and never lets LED composition observe it | The invariant protected LED output and panel resolution. Both survive as engine facts |
| Spec 64 §6.1 (`docs/specs/64-multi-zone-scenes.md:418-430`) | `Display` is one of three zone roles | Amended. `Display` is no longer authored. The enum value stays for stored documents and for the views in §6.7 | Part B |
| Spec 64 §9.6 | `effects/apply` replaces the target zone's stack | Amended: it replaces the zone's unscoped layers (§4.4) | A face or a keyboard layer must survive clicking an effect card |
| Spec 69 §3.6 (`docs/specs/69-world-class-faces.md:220-249`) | Two user-facing assignment layers, `default` and `scene`; `default` is what the UI writes | Amended. The routes keep both values and the store keeps its role. One precedence case changes and is stated (§6.5). Studio stops creating `default` assignments and stops presenting it as a place (§7.5) | The owner could not find a face's settings because they lived on the layer the scene does not show |
| Spec 69 §8 W4.2 (`:585-597`) | A scope toggle on the Displays page | Superseded. Not built on a page that no longer exists; not rebuilt | Same |
| Spec 86 §3.1, §3.2 | One sampling entry and one compute pass per zone texture | Amended: one per scope class texture (§5.4) | Part A |
| Spec 86 §5.1, §5.2 | Scene projection and zone previews take one frame per zone; a display's box in the scene canvas is its backdrop on hardware | Amended: projection takes one frame per output's class, and a screen's backdrop is resolved from its own class frame, not from the flattened scene canvas (§5.9, §5.10) | Part A, and overlapping screens |

---

## 4. Part A: layer scope, the model

### 4.1 Type change

In `hypercolor-types/src/layer.rs`, `SceneLayer` gains one field after
`enabled`:

```rust
/// Outputs of the owning zone this layer applies to. `None` applies the
/// layer to the whole zone, including canvas no output covers.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub scope: Option<LayerScope>,
```

```rust
/// The outputs one layer is limited to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LayerScope {
    /// Output ids of the owning zone: the value `Output::id` holds in the
    /// zone's layout and `ZoneMember::id` carries on the wire. Sorted,
    /// unique, never empty.
    pub outputs: Vec<String>,
}
```

Decisions behind the shape:

- **A struct, not a bare list.** `Some(vec![])` would be a third state with no
  meaning. The struct also leaves room for an additive field later.
- **Whole zone is absence.** A layer without a scope serializes exactly as it
  does today, because the field is skipped when `None`.
- **No `deny_unknown_fields`**, matching `SceneLayer` itself
  (`layer.rs:71-73`), so the type can grow. The cost of that tolerance is the
  downgrade hazard §4.6 closes.
- **Output ids, not device ids.** Assignment is per output (Spec 64 §6.4). A
  Push 2 with pads in scope and buttons out of it must be expressible.
- **`SceneLayer::from_effect`** (`layer.rs:112-135`) sets `scope: None`.
- **`SceneLayer::normalized`** (`layer.rs:139-145`) sorts and dedupes
  `outputs`.
- **`SceneLayer::validate`** (`layer.rs:148-163`) rejects an empty `outputs`
  list and an empty id string. It does not check membership; that needs the
  zone (§4.2).

`PortableZone` already embeds `Vec<SceneLayer>`
(`hypercolor-types/src/portable_setup/scene.rs:40`), so scope travels in
backups with no change to the portable types.

### 4.2 Validation and lifecycle rules

**R1. A request validates the scope ids it introduces.** Membership is checked
against the target zone at commit time, and only for ids the request brings
in:

- **Create:** every scope id must be a current output of the zone.
- **Replace:** every scope id that the replaced layer's scope did not already
  hold must be a current output. Ids carried over from the replaced layer may
  be dormant (R2).
- **Internal rewrites validate nothing.** Apply (§4.4), the carry rule (R4),
  and display-zone conversion (§6.6) move or keep existing layers without
  re-checking their scopes.

So after a scoped keyboard is unassigned, applying a new base effect still
succeeds and the keyboard's layer stays in the stack, dormant. The check lives
in `SceneManager::insert_zone_layer` (`hypercolor-core/src/scene/mod.rs:1204-1233`),
which gains the replaced layer's scope as context when it is called from the
replace path (`hypercolor-daemon/src/domain/scene.rs:1863-1881`). A failure is
a new `LayerMutationError::ScopeOutputMissing { layer_id, output }`, which the
REST layer maps to its validation error.

**R2. Stored documents are tolerant.** `Scene::validate`
(`hypercolor-types/src/scene.rs:440-518`) does not reject a scope id that is
not a member. A scene saved on one rig loads on another, and a backup restores
before its devices are bound. An id that names no current output is *dormant*:
it contributes nothing and is kept.

**R3. A layer with no live output renders nothing.** When every id in a scope
is dormant, the layer is dormant: the render thread allocates no renderer for
it and it appears in no scope class. A scope is never widened. A keyboard layer
turning into a whole-rig layer because the keyboard left is the one outcome
this rule exists to prevent.

**R4. Layers follow their outputs between zones.** When one membership edit
moves a set of outputs *M* from zone A to zone B, each layer in A is handled
against the whole set:

| Layer scope in A | Result |
| --- | --- |
| No scope | Stays in A, unchanged |
| Every live id is in *M* | The layer moves to the top of B's stack, keeping its id, its relative order among moved layers, and its scope |
| Some live ids in *M*, some not, and at least one live id is an LED output | Stays in A; the ids in *M* are removed from its scope |
| A screen layer (every live id a screen) with some live ids in *M*, some not | Split first: each screen in *M* gets its own copy directly above, under a new layer id, scoped to it alone, and the original keeps the screens that stay. The copy then moves by the second row, so the moved screen keeps its face in B |
| No id in *M* | Unchanged |
| No live id at all (the layer is already dormant) | Unchanged. An edit that moves other outputs never carries it |
| Every live id leaves in the one edit, but to more than one zone | Stays in A with its scope intact and goes dormant. No one zone holds what it named, and removing every id would leave an empty scope |
| Every live id is in *M*, but B already holds a layer with the same id, or an earlier layer with that id is carried to B by the same edit | Stays in A with its scope intact and goes dormant. Layer ids are unique within a zone, not across a scene, and a carry never overwrites or drops a layer |

The rule is evaluated per edit, not per output: a two-output device moved
together must carry a layer scoped to both. It runs in the member-edit
transaction (`hypercolor-daemon/src/domain/scene_tree/member_edits.rs`) and in
`SceneManager::assign_outputs_to_zone`, which the device-segment route
(`POST /scene/zones/{zone}/members`) calls once for all of a request's
segments; `assign_output_to_zone` (`core/src/scene/mod.rs:957-1014`) is the
one-output case.

`EditMembersResponse` (`hypercolor-types/src/api/scene.rs:676-679`) gains a
`layer_moves` list: one record per layer the edit moved or narrowed, with the
layer id, its zone and index before and after, and its scope before and after.
The response always carries the list, empty when nothing was carried, and its
scopes are the sorted, unique scopes the edit committed.

`EditMembersRequest` accepts `layer_moves` back when replaying a receipt, and
the field's absence and its emptiness mean different things:

- **Omitted:** the daemon applies the rule above.
- **Present, including an empty list:** exactly those moves are applied and
  the rule does not run. An empty list carries nothing. This is what lets an
  undo restore a stack position and a narrowed scope, which the rule run in
  reverse cannot do.

A replayed move must find its layer at the slot it names with the scope it
names, or the edit is refused as a conflict. It may widen a scope only to
outputs its destination zone holds once the edit's outputs have moved, and it
may not land a layer id a zone already uses. `layer_moves` is valid only beside
explicit `changes`, never beside an `assignment` target. Reversing every
record of a receipt (swap the two slots and the two scopes) and sending it
with the reversed membership changes undoes the edit exactly. The one
exception is the split of a shared screen layer: the split is not a move, so
the undo brings the moved screen's copy back as a layer of its own beside the
original, and does not rejoin them.

**R5. A scope follows its device segment, and waits when the segment is
gone.** A scope names outputs by id, and output ids are not stable per device
segment: a stored layout mints its own, so the same segment of the same device
can be held under a different id by each layout. Two rules follow.

- **A wholesale replacement of a zone's outputs carries its scopes across.**
  When a primary zone that follows the active layout takes a layout whole, or
  an effect apply does the same, each scope id the new outputs lack is matched
  through the old output it named: if that device and segment is held exactly
  once among the outputs being replaced and exactly once among the outputs
  replacing them, the id is rewritten to the new output's. That is identity,
  not widening. It needs nothing but the zone, so it holds at startup and in a
  snapshot-locked scene. If either side holds the segment more than once,
  nothing says which one the layer means and the id is left alone: it is
  dormant until its own output returns, so a trip through another layout and
  back leaves the layer where it was.
- **An id whose segment is gone stays and goes dormant.** When an output is
  unassigned (`core/src/scene/mod.rs:1016-1043`), when its device is
  forgotten, or when a layout holds no output for its segment, its id stays in
  every scope that names it. Re-adding an output under that id revives the
  layer, and so does a later layout that holds the segment under the id the
  scope still names. Studio shows a dormant layer with a warning and a remove
  action (§7.3).

Screens are the exception to the second rule: a layer scoped only to a screen
does not wait dormant when the screen leaves. It returns to the screen's
stored display zone (§6.6).

**R6. A deleted zone takes its lighting.** `delete_zone`
(`core/src/scene/mod.rs:850-877`) removes the zone and its stack, whole-zone
and LED-scoped layers alike. Its outputs become unassigned, as today. Its
screens are unplaced by that, and an unplaced screen keeps its stack: each
screen's layers are stored as its display zone (§6.6), as a face survives a
zone's deletion today.

**R7. A scope edit is a replace.** Changing a layer's scope goes through
`PUT /scene/zones/{zone}/layers/{layer}`, the verb Studio's layer updates
already use (`hypercolor-ui/src/api/layers.rs:91-105`). A new scope means a new
frame and render extent (§5.5), so the renderer re-initializes whatever route
carried the change.

### 4.3 Dormant layers and the document

A dormant layer is not a health state and adds no `LayerHealth` variant. A
client computes it from the scene document: the scope's ids intersected with
the zone's `members` is empty.

### 4.4 Apply and clear

Two existing actions replace or clear a whole stack, and both would destroy
scoped layers.

- **Apply** (`POST /effects/{id}/apply` and every caller of
  `apply_effect_to_zone` and `upsert_primary_zone`). Both end in the private
  `replace_effect_layer_stack`, which assigns the zone a one-layer stack
  (`core/src/scene/mod.rs:561`, `:1482`, `:1893-1908`). It changes to replace
  the zone's **unscoped** layers with the one new layer and keep every scoped
  layer above it in its existing order, dormant ones included. For a zone with
  no scoped layer this is today's behavior exactly. Setting a zone's color
  (`SceneMutation::set_runtime_zone_color`) follows the same rule: the color
  fill replaces the zone's unscoped layers and every scoped layer stays above
  it.
- **Clear** (`POST /scene/clear`) keeps a layer only when every id in its scope
  is a live output of the zone and a screen. Every other layer is lighting and
  is removed: an unscoped layer, a layer scoped to LED outputs, a layer scoped
  to a screen and an LED output together, and a layer whose scope names an
  output that has left the zone, whatever else it names. Which outputs are
  screens is read from the zone's own layout. Clear is the "stop" gesture
  (`hypercolor-types/src/api/scene.rs:331`) and today it skips display zones
  (`scene_tree.rs:421-426`), so faces already survive it. A keyboard layer is
  lighting and goes dark with the rest. The cost of one rule is accepted: a
  face whose screen has been unassigned is dormant, cannot be shown to be a
  face, and is removed by a clear. The one layer a clear keeps beyond the rule
  is a face waiting for its device (§6.6), which the scene manager remembers is
  a face.

### 4.5 REST, OpenAPI, MCP, events

All additive.

- **Create.** `CreateLayerRequest` (`hypercolor-types/src/api/scene.rs:424-443`)
  gains `scope: Option<LayerScope>`. Omitted means whole zone.
- **Replace keeps the scope it is not told to change.** Existing clients
  replace a layer by sending back the fields they know
  (`hypercolor-ui/src/api/layers.rs:26-39` builds the request from a layer
  with no scope field). If an omitted scope meant "whole zone", every such
  client would silently widen a scoped layer the first time it changed a blend
  mode. So `ReplaceLayerRequest` (`api/scene.rs:450-469`) takes `scope` as a
  tri-state, the pattern `PatchZoneRequest.color` already uses (`:366-377`):
  omitted keeps the replaced layer's scope, `null` clears it to whole zone, and
  a value sets it.
- **Responses.** `ZoneResource.layers` is `Vec<SceneLayer>` (`:243`), so the
  field appears wherever layers are read: `GET /scene`, the layer routes, and
  `GET /scenes/{id}`.
- **Errors.** R1 failures answer with the existing validation error code. No
  new error code.
- **OpenAPI.** `LayerScope` is a new schema; `SceneLayer` and the two request
  schemas gain the optional property. The Python client is regenerated from
  it.
- **Capabilities.** `"layer-scope"` joins the capability list
  (`hypercolor-daemon/src/api/system.rs:102-109`) in the wave that makes the
  request field live, so a UI talking to an older daemon hides the control.
- **Events.** None new. A scope edit publishes `LayerStackChanged` with
  `Updated` (`hypercolor-types/src/event.rs:852-857`). A carried layer (R4)
  publishes `LayerStackChanged` for both zones beside the existing membership
  events.
- **MCP.** `set_effect` keeps its arguments
  (`hypercolor-daemon/src/mcp/tools/effects.rs:28-66`) and takes the §4.4
  behavior. One new tool, `add_layer`, stacks an effect without replacing
  anything: `effect` (id, name, or unique substring), `zone` (id or name,
  default the primary zone), `outputs` (device or output names or ids; omitted
  means whole zone), `blend`, `opacity`, `controls`. It returns the updated
  zone resource. New tools are additive under the stability rules.

### 4.6 Storage boundary

A build that predates this spec reads a scoped layer and ignores the scope,
because `SceneLayer` tolerates unknown fields. After a rollback, "ripples on
the keyboard" becomes ripples on the whole rig, silently. The boundary that
prevents it lands in the first wave that can persist a scope (W2), and it
covers every container that holds scene layers:

| Container | Today | After W2 |
| --- | --- | --- |
| Named scenes, `scenes.json` | Schema 2, the only version read (`hypercolor-daemon/src/scene_store.rs:21`, `:311-317`; `hypercolor-daemon/src/durable_stores.rs:212-214`) | Schema 3. Reads 2 and 3. A schema 2 file upgrades in memory with no change to its scenes |
| The default scene, inside the runtime session `runtime-state.json` | Unversioned (`hypercolor-daemon/src/runtime_state.rs:206-222`; `durable_stores.rs:319-327`). It holds `default_scene_zones: Vec<Zone>` and denies unknown top-level fields (`runtime_state.rs:27-35`) | Gains a top-level `schema_version`, starting at 1. A file without it is version 0 and upgrades in memory. An older build refuses the file on the unknown field instead of misreading it |
| Backup manifests | Schema 1, oldest readable 1 (`hypercolor-types/src/portable_setup/backup.rs:10`, `:17`) | Schema 2. Reads 1 and 2, with an identity upgrade |

This is one coordinated storage migration. It changes version numbers and no
scene content.

A restore left pending across a downgrade is outside this boundary: the
restore journal holds the setup being restored, scene layers included, under
its own version, and an older build recovers it without the scope. That is
accepted because the only way to reach it is a crash inside one restore
followed by a downgrade before the next start. A staged backup record is
inside the boundary: it carries the backup's manifest, and an older build
validates that manifest's schema before applying it.

- **Downgrade.** An older build refuses all three files. Going back to it
  means restoring the data directory, or a backup, taken before the upgrade.
  The release note says so. The wave's tests confirm what an older build does
  with a refused runtime session, so the note can state it exactly.
- **Fixtures.** The shipped `portable_backup_v1.json` stays untouched and keeps
  decoding (`hypercolor-types/tests/portable_backup_tests.rs:109-121`). The
  suite already demands a shipped manifest for every readable schema
  (`:123-140`), so W2 adds `portable_backup_v2.json`, exported by the build
  that introduces it. The scene store has no frozen file today; its tests
  build stores in code (`hypercolor-daemon/tests/scene_store_tests.rs:48-75`).
  W2 adds one captured from 0.9.0, with display zones and a default face, so
  W5 has a real document to convert.
- **Config.** `hypercolor.toml` is untouched in every wave.

---

## 5. Part A: layer scope, rendering

### 5.1 What the code does today

- **One composite per zone.** `ZoneRuntime::render_zone_frame` renders each
  enabled layer's producer frame, builds one `CompositionPlan` at the zone's
  canvas extent, and composes it (`render_zones/layer_rendering.rs:40-293`).
  `CompositionPlan::with_layers` forces the bottom layer's mode to `Replace`
  (`sparkleflinger/mod.rs:412-416`) and leaves its opacity alone.
- **A processed layer is one compute pass.** It reads the destination and the
  source and writes the output
  (`sparkleflinger/gpu/compositor/layers.rs:17`, `sparkleflinger/blend.wgsl:356-382`).
  A fully opaque replace at the target's size skips the pass and copies or
  bypasses (`layers.rs:28-31`, `:54-75`; `sparkleflinger/mod.rs:307-343`). The
  CPU twin is `compose_layer` (`sparkleflinger/cpu.rs:396-437`).
- **A layer already has a placement, in target pixels.** The pass maps each
  destination pixel into the source through an anchor, a per-axis scale, one
  rotation, and a fit, all in the target's pixel space
  (`blend.wgsl:200-251`, `sparkleflinger/transform.rs:93-122`); a pixel outside
  the placed source reads as transparent (`blend.wgsl:243-245`, `:269-272`).
- **An output's placement rotates in normalized canvas units.**
  `OutputPlacement::local_to_canvas` scales by the output's normalized size,
  rotates, and translates, all before any pixel conversion
  (`hypercolor-core/src/spatial/sampler/mod.rs:65-91`). On a canvas that is not
  square, that is a different transform from a rotation in pixels.
- **A rectangle-limited layer already exists.** The scene projection draws one
  layer per output, placed on that output's rectangle with
  `sample_target_space: true` (`render_zones/projection.rs:205-254`), which
  samples the source at the destination pixel's own position
  (`blend.wgsl:246-249`) and forces alpha to 1 (`:276-278`). The CPU twin is
  `rasterize_zone_projection` (`projection.rs:404-441`), which works in
  normalized units (`:465-481`).
- **LED truth is per zone.** With the GPU backend, each zone composes into its
  own retained texture and one dispatch samples every zone's LEDs from its own
  texture with one compute pass per texture (Spec 86 §2, §3;
  `render_zones/scene_assembly.rs:268-312`, `frame_sampling.rs:45-70`,
  `sparkleflinger/gpu/sampler.rs:18-21`,
  `sparkleflinger/gpu_sampling.rs:908-915`). A lone zone at the scene extent
  takes a faster path that samples the compositor output directly
  (`scene_assembly.rs:186-263`, `:314-328`).
- **Output boxes overlap.** Design 73 §2 records the Default zone canvas as "a
  soup of overlapping boxes" (`docs/design/73-studio-zone-tabs.md:157`), and
  Spec 86 §5.1 records a shipped bug where a ripple from one zone painted onto
  LCDs because boxes overlapped in the scene canvas.
- **Sampling footprints are wider than a point.** Layouts default to bilinear
  sampling (`hypercolor-types/src/spatial.rs:523-525`), and area and Gaussian
  modes read a neighborhood (Spec 86 §11.1).
- **An output's shape does not affect sampling.** `Output::shape` is "for the
  editor's visual appearance" (`spatial.rs:407-409`). LEDs sample at positions
  generated from topology inside the output's rectangle.

### 5.2 Realizations evaluated

**(i) A canvas-space mask.** Rasterize the scoped outputs' rectangles into a
coverage mask and multiply the scoped layer's alpha by it inside the zone's
one composite. The zero-shader variant reuses the projection trick: add the
layer once per scoped output with `sample_target_space`.

- Keeps one texture per zone and one sampling pass, so Spec 86 is untouched.
  A scoped layer costs the pass it would cost unscoped, plus a cached mask.
- **Fails on overlap.** An unscoped output whose box overlaps a scoped
  output's box samples the scoped layer. On the owner's rig the boxes overlap
  by default, and a fan ring's box encloses the LCD at its hub. This is the bug
  class Spec 86 §5.1 already had to fix once.
- **Fails at edges.** A bilinear, area, or Gaussian footprint that straddles
  the mask edge mixes scoped and unscoped pixels, so the scoped device's edge
  LEDs are diluted and its neighbors pick up a fringe. A soft mask edge makes
  both worse.
- The per-output variant applies the layer twice where two scoped boxes
  overlap (wrong for every mode except an opaque replace) and discards the
  source's alpha (`blend.wgsl:276-278`). A real mask needs a new texture
  binding in `blend.wgsl`, a CPU twin, a rasterizer for rotated rectangles,
  and invalidation on every layout edit.

**(ii) A separate texture, blended per LED at sample time.** Compose scoped
layers into their own texture, sample both textures for a scoped output, and
blend the two colors per LED.

- Membership is exact: an LED belongs to one output, so overlap and edges
  cannot leak.
- **Needs a second blend implementation.** Eleven blend modes, linear-light
  math, and opacity would be re-implemented in LED space and held in parity
  with `blend.wgsl` and `cpu.rs`.
- **Is wrong for most sampling modes.** A blend of two averaged colors is not
  the average of the blended pixels for any non-linear mode (multiply, screen,
  overlay, soft light, dodge, difference, and add once it clamps). It matches
  only under nearest sampling.
- **Cannot express stack order.** An unscoped layer above a scoped one has no
  place in "base texture plus scoped texture". Every scoped layer in the
  middle of a stack would split the zone into more textures and more LED-space
  blends.
- Previews and screen backdrops read canvases, not LEDs, so they would need
  their own answer.

**(iii) Scope classes: a texture per distinct stack, selected per output at
sample time.** Group the zone's outputs by the exact list of layers that apply
to them. Compose one texture per group with the existing compositor, from the
same producer frames. Each output samples its group's texture.

- Membership is exact, as in (ii): overlap and footprints cannot leak, because
  an output never samples a texture that holds another class's layers.
- Every blend mode, every sampling mode, and every stack order is correct by
  construction, because blending happens in the canvas with the one existing
  compositor and sampling happens once, afterwards.
- The base effect renders once. Its frame is shared by every class, so it is
  continuous across the zone.
- Rotation, shape, and overlap never enter LED truth. They matter only to the
  frame (§5.5) and to pictures made for people (§5.9).
- Costs more composites, textures, and sampling work per class beyond the
  first (§5.8).

### 5.3 Recommendation

**Build (iii).** It is realization (ii)'s exact membership with realization
(i)'s canvas-space blending, and it extends Spec 86's doctrine by one word:
the texture of an output's scope class is the source of truth for that
output's LEDs.

Rejected: (i), because it cannot keep a scoped layer off overlapping
neighbors, which is the feature's whole promise; (ii) as specified, because
LED-space blending is a second compositor that is wrong under the default
sampling mode. Soft edges are rejected with (i): a scope's edge is a device
boundary.

### 5.4 Scope classes

For a zone with enabled canvas layers *L* (bottom to top; screen layers are
excluded once Part B lands, §6) and outputs *O*:

- The **effective stack** of an output *o* is every layer in *L* with no
  scope, or whose scope contains *o*, in stack order.
- A **scope class** is a maximal set of outputs with the same effective stack.
- The **base class** is the stack of unscoped layers. It always exists, even
  when no output uses it, because it is what the zone's canvas shows where no
  output sits, what `UnassignedBehavior::Fallback` samples, and the bottom of
  the zone's preview.
- An output whose effective stack is empty is black. The zone is active, and
  nothing is drawn there.

A zone with *n* outputs has at most *n* + 1 classes: one per output in the
worst case, plus the base class. A zone with no scoped layer has exactly one,
and every code path below must then be identical to today's. That is a test,
not an aspiration (§8, W2).

**Runtime model** (new `render_zones/scope_classes.rs`):

```rust
/// One zone's outputs grouped by the layers that apply to them.
pub(super) struct ZoneScopeClasses {
    /// `classes[0]` is the base class.
    pub(super) classes: Vec<ScopeClass>,
    /// Class per output, in `zone.layout.zones` order.
    pub(super) class_of_output: Vec<ScopeClassIndex>,
}

/// Index into `ZoneScopeClasses::classes`. Built with a checked conversion;
/// a zone whose classes do not fit is refused at reconcile like any other
/// resource it cannot admit.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ScopeClassIndex(u32);

pub(super) struct ScopeClass {
    /// Indices into `zone.layers`, bottom to top.
    pub(super) layers: Vec<usize>,
    /// Indices into `zone.layout.zones`.
    pub(super) outputs: Vec<usize>,
    /// Longest run of layers shared with the base class from the bottom.
    pub(super) shared_base_prefix: usize,
}
```

Classes are built at reconcile, when the zone's layers or layout change, and
ordered by the first output that uses them. Everything keyed by `ZoneId` on the
scene-zone path becomes keyed by `(ZoneId, ScopeClassIndex)`: the retained
texture requirement (`sparkleflinger/mod.rs:78-82`), the held frame and
composition identity (`render_zones/model.rs:155-185`), the spatial engine, and
the sampling entry.

**Composition.** `render_zone_frame` keeps its layer loop: each enabled,
non-dormant layer produces one frame per pass, whatever number of classes use
it. The plan-building tail runs once per class:

- The base class composes as today.
- A class whose stack is the base stack plus layers that all sit above it (the
  common case, a scoped overlay on top) starts from a copy of the base class's
  texture and adds one pass per extra layer.
- Any other class composes its full stack from the shared producer frames.
- A class whose bottom layer is scoped composes over the opaque black frame
  the projection already uses (`render_zones/render_pass.rs:130-138`), and the
  layer keeps the blend mode and opacity it asks for there. An unscoped bottom
  layer still has its blend forced to `Replace`.

Each class has its own composition key and skip (Spec 86 §2.1). A pending
layer is skipped for the classes that use it, exactly as the loop skips it
today (`layer_rendering.rs:175-183`).

**Sampling.** Each class is one entry in `ZoneSamplingSet`
(`frame_sampling.rs:45-58`): its texture and a prepared plan holding only its
outputs. The dispatch is unchanged in shape: one encoder, one submit, one
readback, one compute pass per texture, each binding one texture
(`gpu_sampling.rs:908-915`; Spec 86 §3.2). No simultaneous-binding limit
applies. The combined routing layout (`render_zones/zone_state.rs:63-79`) lists
each zone's outputs grouped by class so sampled colors and routing stay
index-aligned. The cached-result digest covers every class texture.

**Fast paths.** The lone-zone path (`scene_assembly.rs:186-263`) and the
single-effect passthrough (`render_zones/frame_helpers.rs:82`) apply only when
the zone has one class. A zone with more takes the per-zone path, and reconcile
admits its class textures even when it is the only zone. A lone zone at the
scene's extent keeps its picture when it does: its base class frame is the
ground of the scene canvas, in place of black, and each output's box is then
drawn from its class (§5.9).

**Transitions.** Spec 86 §4.1 blends in LED space when either scene is sampled
per zone. A zone with more than one class is sampled per class, so it is on
that path in both directions with no further test. A fade out of per-zone LEDs
into a single-zone scene samples the incoming zone's own frame and blends that.
When the incoming frame is on the CPU its colors are in hand, and the sampling
set resolves with no GPU dispatch, on either compositor backend.

### 5.5 Framing: one mapping from a layer's source to the zone

**Decision: a scoped layer is always framed to its outputs.** Its source fills
the *frame* of its scope, not the zone canvas.

- For one output, the frame is that output's own placed rectangle, rotation
  included, so the layer is upright relative to the device however the device
  sits on the canvas.
- For several outputs, the frame is the axis-aligned bounding box, in
  normalized canvas units, of their placed rectangles, so the layer is
  continuous across them.
- The layer's own `transform` (`layer.rs:330-338`) applies inside the frame.
- Framing never decides whether a layer renders. Dormancy is membership alone
  (R3): a layer whose scope names an output of the zone is framed, whatever
  that output's geometry. An output with no area takes no part in the frame
  while another named output has some. When none has, the frame is formed from
  the collapsed outputs themselves, each given one pixel of the zone's long
  side along an axis it has no extent on, so a strip with no thickness is
  framed along its own line and a point at its centre. Geometry that is not
  finite forms no frame at all, and the source then fills the zone canvas
  while still reaching only the outputs its scope names.

Why framing: the owner's example is a ripple on a keyboard. Keystrike places
each key press by its QWERTY cell as a fraction of the whole canvas
(`sdk/src/effects/keystrike/main.ts:142-155`). Unframed, the keys would land
across the entire rig's canvas and the keyboard's small box would show a sliver
of it. Framed, the canvas is the keyboard. The same holds for a GIF "on the
keyboard" and for a face on a panel.

**Why the existing transform fields cannot carry it.** A
`CompositionTransform` is an anchor, a per-axis scale, and one rotation, all
applied in target pixels (`sparkleflinger/mod.rs:99-106`,
`blend.wgsl:203-210`). A frame is placed by an output placement, which rotates
in normalized units (`sampler/mod.rs:72-84`). Two things do not fit:

1. On a canvas that is not square, a rotation in normalized units is a
   rotation plus a shear in pixels. One rotation angle cannot express it.
2. A framed layer with its own rotation composes two rotations with a
   non-uniform scale between them. That product is a general affine, not an
   anchor, a scale, and an angle.

**The mapping.** One value describes where every source pixel of a layer lands
on its zone. Spaces: source pixels *s*; frame units *f* in the unit square;
normalized canvas units *n*, the space placements live in; zone pixels *z*.

1. **Source to frame pixels.** The layer transform keeps today's meaning with
   the frame as its target. With frame size (*wf*, *hf*) in zone pixels, fit
   draw size *D* computed against the frame (`transform.rs:124-181` with the
   frame in place of the target), scale *k*, rotation *t*, and anchor *a*:
   *q* = *a* (*wf*, *hf*) + R(*t*) ((*s* / source size - 1/2) *D* *k*).
   `Cover` adds a crop rectangle in source pixels; `Tile` and `Mirror` repeat
   in source pixels.
2. **Frame pixels to frame units.** *f* = *q* / (*wf*, *hf*).
3. **Frame units to normalized canvas.** The output placement's own chain,
   without edge behavior: *n* = *c* + R(*r*) ((*f* - 1/2) *e*), with centre
   *c*, extent *e* (size times scale), and rotation *r* for one output, or the
   bounding box with *r* = 0 for several (`sampler/mod.rs:70-84`).
4. **Normalized canvas to zone pixels.** *z* = *n* (*W*, *H*).

The frame's pixel size (*wf*, *hf*) is the length of the frame's two edge
vectors after step 4. The product of the four steps is one 2 by 3 affine from
source pixels to zone pixels, and it carries the shear from item 1. The
placement holds it as two affines, zone pixels to frame units and frame units
to source pixels, because the frame's edge is applied between them.

**The frame's edge.** A frame places a layer. It does not clip it on LED
output. A matrix or a strip puts its first and last LEDs on the border of its
placed rectangle, and the sampler reads the pixel at `n * (W - 1)` where
composition puts the frame's edge at `n * W`. A border LED therefore lands up
to half a pixel outside its own frame, bilinear sampling reads a pixel further,
and area sampling reads its whole radius further. Cut at the frame, those LEDs
would read the layer beneath. So a scoped layer's edge runs on past its frame
(`FrameEdge::Extend`): frame units clamp before the source lookup, and a pixel
outside the frame shows what the nearest point of the frame's edge shows.
Nothing else reads those pixels, because each class is sampled with only its
own outputs. The clip exists as `FrameEdge::Clip`, used where a class frame is
drawn into an output's box in a picture (§5.9).

**Where it lives.** A new `hypercolor-core/src/spatial/layer_placement.rs`,
beside the placement math it reuses, because the daemon's compositor and the
core's input mapping both consume it:

```rust
/// Where one layer's source lands on its zone's canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerPlacement {
    /// Zone pixels to frame units, row-major 2 by 3. A zone pixel is inside
    /// the frame when both results are within 0 and 1.
    pub zone_to_frame: [f32; 6],
    /// Frame units to source pixels, row-major 2 by 3.
    pub frame_to_source: [f32; 6],
    /// The source rectangle the fit mode shows, in source pixels.
    pub source_crop: [f32; 4],
    pub repeat: SourceRepeat,
    pub source_size: [u32; 2],
    /// What shows beyond the frame: its edge run on, or nothing.
    pub edge: FrameEdge,
}

impl LayerPlacement {
    /// A layer's source framed and placed, its edge running on. `None` when
    /// the frame or the transform is degenerate.
    pub fn new(
        frame: &LayerFrame,
        transform: LayerTransform,
        source_size: (u32, u32),
        zone_size: (u32, u32),
    ) -> Option<Self>;

    /// A picture of the whole canvas seen through `frame` and clipped to
    /// it: each target pixel inside the frame shows the source at its own
    /// normalized position.
    pub fn window(
        frame: &LayerFrame,
        source_size: (u32, u32),
        target_size: (u32, u32),
    ) -> Option<Self>;

    /// The source point a zone pixel shows, or `None` where the layer shows
    /// nothing: outside a clipped frame, or where the placed source does not
    /// reach.
    pub fn source_point(&self, zone_px: [f32; 2]) -> Option<[f32; 2]>;
}
```

`CompositionLayer` gains `placement: Option<LayerPlacement>` beside
`transform` (`sparkleflinger/mod.rs:193-201`). Three consumers use the one
value, and none re-derives it:

- **CPU composition** has a sampler branch in `sparkleflinger/transform.rs`
  that walks zone pixels through `zone_to_frame`, the edge rule, and
  `frame_to_source`.
- **GPU composition** carries the same two affines and the crop in
  `ComposeParams` (`blend.wgsl:1-17`), with a value that selects them over
  the anchor, scale, and rotation path and says how the edge behaves.
- **Input** maps a touch to a zone pixel and then through `source_point`
  (§5.6).

**Blend modes and placement.** Outside a clipped frame the destination passes
through for every mode. `Replace` on an unplaced layer writes the source
everywhere, and a placed source does not reach everywhere, so a placed
`Replace` composes as alpha with the source treated as opaque wherever the
placement draws: black where the placed source does not reach, and nothing at
all outside a clipped frame.

**Unscoped layers do not change.** They keep composing through the existing
fields, which is what lets §8's "one class is identical to today" test hold.

**Render extent.** A framed effect, web, or color layer renders at the frame's
aspect ratio with its long side equal to the zone canvas's long side, and its
short side rounded to an even number of pixels with a floor of 16. A keyboard
frame of aspect 4.4 in a 640 by 480 zone renders at 640 by 146. A device never
gets fewer pixels along its long side than the zone canvas has along its own.
The bound is the square of the canvas's long side: a square frame in a 640 by
480 zone renders at 640 by 640, a third more pixels than the zone canvas.
§5.8 counts that. The effect pool takes its extent per slot today from the zone
(`hypercolor-core/src/effect/pool.rs:166-177`); the extent becomes a property
of the slot, which is already keyed by zone and layer (`:64`).

### 5.6 Input-reactive layers

Verified today:

- The render thread reads one `InteractionData` per frame and hands the same
  reference to every zone (`render_zones/model.rs:77-87`). It is not per zone
  and not per device.
- Host keyboard state is key names with no device and no position
  (`hypercolor-core/src/input/traits.rs:480-485`). An effect that wants
  positions invents them, as Keystrike does.
- Device touches are per device: each contact carries a device id, an optional
  segment, and a position on that device's own surface (`traits.rs:453-467`).
- Each zone builds a `DeviceSurfaceMap` from its own layout, which maps a
  device-surface point to the canvas point the LED under the finger samples
  (`hypercolor-core/src/spatial/device_surface.rs:1-12`, `:66-107`, `:136-149`;
  `render_zones/device_surfaces.rs:1-7`).
- That canvas point is published to the effect as it is
  (`hypercolor-core/src/effect/lightscript/payload.rs:159-182`). The layer's
  own transform is not applied, so today a touch on a layer that is scaled,
  moved, or rotated lands away from where the effect draws it.
- A touch on a device the layout does not place carries no canvas point, which
  Touch Ripple skips (`sdk/src/effects/touch-ripple/main.ts:150-152`).

The canvas point an effect receives is the point of **its own source** that
shows under the finger.

1. The zone's `DeviceSurfaceMap` resolves the touch to a normalized canvas
   point, as today, and also reports which output it resolved through.
2. If the layer has a scope and that output is not in it, the touch carries no
   canvas point.
3. Otherwise the point converts to zone pixels and goes through the layer's
   `LayerPlacement::source_point`. `None` (the finger is outside the placed
   source) means no canvas point.

The surface map carries the layer's view as one value.
`DeviceSurfaceMap::for_layer` returns the zone's map with the layer's
placement and scope attached, each surface records the output it belongs to,
and `segment_source_position` resolves a touch to the layer's own source point
or to nothing. `TouchCanvas` (`payload.rs:147-153`) keeps the one map it had.
Because composition draws through the same `LayerPlacement`, the point the
effect is told about and the point the LED samples are the same by
construction, including the layer's own anchor, scale, rotation, and fit.

Results, with no change to any effect:

- A touch on a scoped device lands at the right point of the framed canvas.
- A touch on a device outside the scope carries no canvas point, so a ripple
  scoped to the ROLI ignores the Push 2's pads. Today that takes a zone each.
- Keystrike's key cells span the frame, which is the keyboard.
- An unscoped layer with a transform gets a `LayerPlacement` built over the
  whole canvas for input only. Its mapping equals what the existing fields
  draw, since a rotation in pixels is an affine, and it corrects the offset
  described above.

Host keyboard and pointer input is not filtered by scope. It is the host's
input, not a device's, and it reaches every interactive layer as today.

### 5.7 The CPU path

With the CPU compositor backend, each zone composes to a CPU canvas and samples
with its own `SpatialEngine` (Spec 64 §12.1; Spec 86 §6). Scope classes map
onto it directly: one CPU canvas and one engine per class, built from the same
shared layer frames, sampled per class, and assembled in the combined layout's
order. The placement branch in `transform.rs` and the frame-edge rule in
`cpu.rs` land beside their shader twins, under the existing GPU and CPU parity
tests. A frame produced on the GPU never enters this path through a readback
(Spec 86 doctrine 2).

### 5.8 Cost and admission

Per scene on the GPU backend. "Class" means a class beyond the base.

| Cost | Amount | Notes |
| --- | --- | --- |
| Producer | One renderer per scoped layer, at up to the square of the zone canvas's long side in pixels | One per layer, whatever number of classes use it. 640 by 640 in a 640 by 480 zone in the worst case |
| Base copy | One texture copy at the zone extent per class that extends the base stack, per changed frame | The class starts from the base class's pixels |
| Composite | One compute pass at the zone extent per layer the class adds; a class that cannot extend the base pays one pass per layer in its whole stack | Copy and bypass shortcuts apply as today (`layers.rs:54-75`) |
| Retained texture | One RGBA texture at the zone extent per class | About 1.6 MB at the evidence rig's 640 by 630 canvas (Spec 86 §9 item 2) |
| Stabilization | One texture copy per changed class per frame | Skipped when the class's composition key matches (Spec 86 §2.1) |
| Sampling | One compute pass per class in the existing dispatch | Same encoder, submit, and readback |
| Area tables | One summed-area scan per class texture whose outputs use area sampling | The scan runs per pass today (`gpu_sampling.rs:1308-1320`) |
| Scene projection | One projection layer per output, every frame, for any scene with more than one sampling source | A one-zone scene with a class leaves the lone-zone path and starts paying this (`scene_assembly.rs:91-94`; Spec 86 §11.3) |
| Zone preview | One retained RGBA texture at the zone extent, and one composite of the base class and at most one box per output per frame the preview is due | Only for a zone with more than one class, and only while a subscriber selects that zone |
| Input | One `LayerPlacement` and one scope set per interactive scoped layer | Rebuilt when the layout or the layer changes |

The class count is the number of distinct effective stacks, at most the number
of outputs plus one, and in practice two or three. There is no product cap.
The daemon's frame metrics and the frame log carry `scope_class_count`, and
`gpu_sampling_passes` already counts the passes. No wire type carries the
class count yet.

**Admission.** Class resources go through the paths zone resources use today,
and fail the same way:

- Class textures are admitted with the zone textures. When the GPU refuses
  them, a zone with classes composes each class, and its preview, on the CPU
  from CPU layer frames, each into pixels of its own. It does not take the
  projected-resource fallback (`sparkleflinger/gpu/projected.rs:100-125`):
  that fallback reads back through a latch keyed by size alone, and two
  classes of one size would be handed each other's pixels. When a layer frame
  is a GPU texture there is no way to a CPU canvas that keeps classes apart,
  and the zone fails through the render-failure surface.
- A canvas resize prepares and commits class textures, framed render extents,
  class sampling plans, placements, and retained composition identities in the
  existing two-step resize (`pipeline_runtime.rs:2256-2275`), so a resize never
  exposes a class sized for the old canvas.
- A scene whose classes cannot be admitted reports through the frame's
  existing render-failure and diagnose surfaces. It does not silently widen a
  scope or drop a layer.

**Against today's workaround.** The live scene's three zones run five HTML
renderers (§2.1); one zone with Touch Ripple scoped to the ROLI and Push 2
outputs runs two. Fewer renderers is a fact. A lower frame time is a
hypothesis, because that one zone also leaves the lone-zone path. §9 measures
it against the tier budgets before W2 is planned in detail.

### 5.9 Previews

LED output never reads a preview (Spec 86 doctrine 1). Previews are pictures
for people, built from class textures with an explicit overlap order:

- **Scene canvas.** `append_projection_composition_layers_for_zone` takes one
  frame per zone and adds one layer per output (`projection.rs:205-254`). It
  takes the output's class frame instead. Each box then shows what its LEDs
  show, in the existing order: LED outputs in zone order, then screens
  (`projection.rs:63-88`). Where boxes overlap, the later one wins, in the
  picture only. A lone zone with more than one class lays its base class under
  the boxes (§5.4).
- **Zone preview.** Studio's per-zone preview (Spec 86 §5.2) for a zone with
  more than one class has no single frame that is its picture, so one is
  composed at the zone's extent: the base class, then every output's box from
  its own class in the scene canvas's order, LED outputs and then screens, the
  later box winning. An output of the base class takes its turn like any
  other, and paints again only where it follows a scoped box it overlaps. The
  picture is kept in a texture of its own, made only while a subscriber selects
  that zone, and first shown one frame after the subscription.

Both pictures place a box through `LayerPlacement::window`, on both backends.
The box is turned in normalized units, as the sampler turns the output, so on a
canvas that is not square it is a parallelogram in pixels, and it sits where
the output's LEDs are. The CPU raster steps the same affine across its rows.

### 5.10 Screen backdrops

A screen's backdrop is hardware output, not a preview, and it cannot come from
the flattened scene canvas once classes exist.

Verified today: a screen with no face is cropped from the published scene
canvas by its worker (`hypercolor-daemon/src/display_output/worker.rs:589-597`),
and a face is finalized over the one shared scene frame
(`display_lane.rs:130-150`, `:240-275`). The scene canvas paints screen boxes in
zone order (`projection.rs:228-251`), so where two screens' boxes overlap, the
later one's pixels cover the earlier one's, and both screens crop the same
image (`display_output/mod.rs:798-811`). With one texture per zone, two
overlapping screens in the same zone read the same pixels and nothing shows.
With scope classes they can differ, and across two zones they already can.

Proposed: **each screen's backdrop is resolved from its own source**, which is
the class frame of that screen's output in the zone that owns it, cropped by
that output's placement in that zone's layout.

- **With a face.** Display finalize takes the screen's backdrop frame where it
  takes the shared scene frame today (`display_lane.rs:130-150`). The worker's
  unfinalized blend branch (`worker.rs:614-630`) takes it the same way.
- **Without a face.** The render thread finalizes the backdrop alone, with no
  face, and publishes it on the per-screen channel finalized faces use
  (`worker.rs:598-612`), in place of the worker cropping the scene canvas. The
  screen keeps the cadence cap it has today, so `capped_display_target_fps`
  (`display_output/mod.rs:1029-1051`) gains a backdrop-only source beside its
  scene and face cases.
- **On the CPU backend** the same resolve runs on the class's CPU canvas with
  the existing CPU blend.
- **The viewport** comes from the screen's output in its owning zone's layout,
  the layout the class frame is drawn in. Today it comes from the spatial
  service's layout (`display_output/mod.rs:722`, `:945-1007`). §9 confirms the
  two agree for every screen on the evidence rig before the source changes.

**As built in W1.** Four things the proposal above left open are settled by
the implementation:

- **Placement belongs to the frame.** Display output publishes, per screen,
  only what the device is: its layout ids, its surface, and how the panel is
  mounted. The render thread finds the zone that places each screen in the
  zones of the frame it is rendering. Where the display worker does the crop,
  the zone's canvas travels with its viewport. A placement resolved on another
  thread could describe the previous scene by the time its frame arrived, and
  did: a scene switch could flash a lit screen black.
- **A screen no zone places has no backdrop.** With no face either it is not
  driven, and keeps what it last showed.
- **A backdrop is held by its zone's content.** The zone runtime keeps a
  content revision per zone that stands while the layers drawing the zone are
  the ones that drew its last frame. The revision travels with the zone's
  frame, so frames retained across a failed render keep the revision they
  were rendered with. A still zone costs each screen one finalize pass, or
  one published canvas on the CPU backend, whatever the screen's cadence.
- **Frames that compose no scene still carry the screens.** The idle frame
  gives every placed screen black, and the frame that takes output off gives
  every placed screen its off color, whether output is released or held
  static. Each is made again when the set of screens or the scene's zones
  change, so a screen that connects afterwards is not left on its last
  picture.

The viewport check §9 asks for was run against the evidence rig's live scene.
The owning zone and the spatial service agree for every screen but one. The
scene's primary zone holds the pump LCD's box at (0.75668, 0.59697) and the
spatial layout holds it at (0.75523, 0.60085), the same size: about one pixel
right and two pixels up on a 640 by 480 canvas. The scene has custom zones, so
its primary zone keeps its own placements instead of following the layout, and
this one was moved there. The zone's value is the right one, since the zone's
frame is drawn in the zone's layout.

**Scene transitions.** A screen never reads the composed scene crossfade. That
crossfade is one flattened picture, so a screen that read it would show
whichever zone was painted last over its box for the length of the transition
and snap back to its own zone at the end. Each screen crossfades its own
backdrop instead, on the render thread, the way the composed scene and the
LEDs do (Spec 86 §4.1): from a frozen base to the incoming backdrop by the
transition's eased progress.

- **The base** is the backdrop the screen showed when the transition began,
  kept per device as a picture in panel space, before any face and before the
  panel's brightness and mask. It is taken once, when a transition's key first
  appears, from what the frame before left behind: on the GPU by one
  backdrop-only pass from the zone frames that frame retained, kept on the GPU
  and never read back; on the CPU backend by cropping the backdrop the screen's
  lane last published. Outside a transition nothing is taken, held or drawn.
- **The published frame is the finished picture.** While the transition runs,
  the incoming backdrop is mixed over the base, the face is laid over the mix,
  and brightness and mask come last. On the GPU the finalize pass does all of
  it. On the CPU backend the render thread mixes the two crops and hands the
  display worker a backdrop marked as cropped. The worker encodes what it is
  handed and keeps no history, and nothing about a transition is published
  apart from the frames.
- **Faces.** A face that blends with its backdrop keeps its own pixels through
  the transition while the backdrop under it moves. A face that replaces its
  backdrop shows nothing of the scene and does not fade. A screen that gains or
  loses a face with the scene still fades its backdrop: the base belongs to the
  device, not to the lane.
- **Black and no base.** A screen whose zone renders nothing in the incoming
  scene fades to black, and one whose zone rendered nothing before fades in
  from black. A screen with no base cuts to the incoming picture: one the
  outgoing scene did not place, or one whose panel changed shape since the base
  was taken.
- **Interruption.** A transition that interrupts another starts each screen
  from the mix that was on its panel: the new base is the incoming backdrop
  over the old base, at the weight of the picture that screen last published.
  A screen under its cadence cap has published less of the fade than the
  transition has run, so the weight is kept per screen and travels with each
  finalization through its readback. A screen that published nothing of the
  interrupted transition keeps the base it had. On the GPU the incoming
  backdrop is the zone's frame at the interruption, and the CPU backend starts
  from the mix it retained. A base is a picture and not the outgoing zone's
  frame and viewport for this reason.
- **Output off.** Output that goes off ends every fade. The frame that
  publishes the off color drops the bases, the transition's key and each
  screen's retained backdrop, and a transition that begins before output is
  back starts each screen from that color: black for released output, the
  static color for held output.
- **Cadence.** A fading screen is finalized at its own cadence cap, as it is
  outside a transition. The loop keeps composing while a screen fades into a
  scene that renders nothing, and idles once the fade is over.
- **No special cases.** A lone zone at the scene extent fades like any other
  screen's zone: its screens read the zone's own frame, never the composed
  crossfade, so none fades twice.
- The mix is linear in the backdrop's encoded bytes. A transition's color
  interpolation mode applies to LEDs only.

This changes the display lane, the display output targets, and the workers. It
ships first, as W1, on zone frames: it fixes overlapping screens across zones
on its own, and scope classes then only swap the frame it reads.

### 5.11 As built in W2

What W2 measured, and the limits it ships with.

**Builds without `wgpu`.** In a build without the `wgpu` feature, which no
release artifact is and only CI's CPU test and smoke lanes use, a fade out of a
scene sampled per zone or per class into a single-zone scene samples the
flattened crossfade, so an output outside a scope can pass through a scoped
layer's color on the way.

**Placement.** `layer_placement_tests` holds the property test: over
random frames, canvas aspects and layer transforms, a source point survives
the trip to the canvas and back, and a touch resolves to the source point its
LED samples. In the daemon,
`random_placements_draw_where_the_placement_says_on_both_compositors` holds
the GPU and CPU compositors to the same placements.

**Effects at framed extents.** The nine interactive and audio effects
(Audio Pulse, Frequency Cascade, Keystrike, Touch Ripple, Cymatics,
Hyperbloom, Iris, Spectral Fire, Shockwave) were rendered through
`ServoRenderer` at 640 by 64, 64 by 640 and 640 by 640 with synthetic audio,
keys, pointer clicks and touches, and the frames were reviewed. Each renders
at each extent with no error and no blank frame. Keystrike's key cells and
Touch Ripple's contact points span the frame. Three are worth knowing about,
none a fault: Audio Pulse and Hyperbloom size themselves by the short side, so
on a 10 to 1 frame they light about a tenth of it; Frequency Cascade shows a
slice of its scene; Iris is dim at 64 by 640. The capture harness needs a
running daemon and a request type that carries scope, so the renderer was
driven directly.

**Class cost.** Measured through the render-zone pass in a test
harness, not on a running daemon: the benchmark drives scenes over REST, and
no request type carries scope in W2. One 640 by 480 scene of 62 outputs and
2,537 LEDs shaped like the evidence rig's, every layer handed a fresh frame
every frame, timed from the start of the scene pass until every LED color is
on the CPU and the GPU is idle. 400 frames after 60 of warmup, preview
profile, on an i7-14700K with an RTX 4070 SUPER that other work shared. No
effect renders, no faces and no device writes are in these numbers. The 60 fps
tier allows 16.7 ms for all of that together.

| Scene | GPU, bilinear | GPU, area | CPU, bilinear |
| --- | --- | --- | --- |
| One zone, no scope | 0.10 / 0.70 | 0.36 / 1.66 | 0.05 / 0.08 |
| Three zones, five layers | 1.68 / 2.95 | 2.86 / 4.28 | 23.9 / 32.2 |
| One zone, two classes | 2.14 / 3.50 | 1.62 / 3.92 | 32.7 / 33.7 |
| One zone, two classes, zone preview shown | 1.84 / 3.26 | 2.00 / 4.37 | 36.3 / 48.7 |
| One zone, four classes | 2.34 / 3.67 | 2.96 / 4.44 | 58.2 / 73.8 |
| One zone, scoped layer mid-stack of three | 1.66 / 3.01 | 1.74 / 4.09 | 39.5 / 50.1 |

Median and p95, in milliseconds. On the GPU backend every case is within a
millisecond of the three-zone scene it replaces and far inside the tier. On
the CPU backend the three-zone scene already misses the 60 fps tier, and each
class beyond the base adds 9 to 13 ms: the measured limit is the 30 fps tier
for two classes and below it for four. By reading the code, not by profile,
two things own that time: the CPU scene canvas draws every output's box by
sampling each pixel on its own, and a placed layer walks every zone pixel
through the placement. With area sampling on the CPU backend the three-zone
scene did not finish: the scene canvas builds a summed-area table for every
pixel it draws, at more than 800 ms a frame, before any class exists.

The hardware pass of W2, ten minutes on the evidence rig from a stored scene,
is not in these numbers.

---

## 6. Part B: screens as scoped layers

### 6.1 What the code does today

- A face renders in an HTML renderer sized to the panel, with the panel's
  `DisplayDescriptor` injected (`domain/display.rs:59-75`,
  `hypercolor-core/src/effect/pool.rs:166-177`). The owner's panels are 960 by
  160, 480 by 480, and three at 400 by 400 (`GET /api/v1/displays`).
- A display zone's stack composes at panel size
  (`render_zones/render_pass.rs:63-114`). Each layer keeps its own opacity
  there, the bottom one included (`render_zones/frame_helpers.rs:114-126`,
  `layers.rs:28-31`, `cpu.rs:409-437`).
- The face then meets its backdrop in one of two branches. On the routed path
  the render thread finalizes it: face over scene crop with the zone's
  `display_target` blend mode and opacity, then viewport, mask, brightness, and
  format (`display_lane.rs:240-275`), and the worker encodes the finalized
  frame (`worker.rs:598-612`). Otherwise the worker blends face over scene
  itself (`worker.rs:614-630`). Either way the target's opacity is applied
  after, and in addition to, the layers' own.
- A screen with a face binding takes the face cadence, 30 fps by default. A
  screen without one takes the scene canvas at 15 fps for encoded panels and
  30 for raw ones (`display_output/mod.rs:44-46`, `:786-791`, `:1029-1062`).
- A display zone with no enabled layer renders nothing
  (`render_zones/zone_state.rs:18-20`).

### 6.2 Verdict: the engine keeps a panel-resolution target per screen

A face cannot be a canvas layer. On the owner's rig a 400 by 400 panel's box
covers a few dozen pixels of a 640 by 630 zone canvas; a face composed there
would be unreadable, and scaling the zone canvas up to give every panel its
pixels would multiply every LED zone's cost for nothing. Screen frames also run
at their own cadence and end in a per-device encode.

So the dedicated target stays. What changes is who knows about it. **The scene
format and the UI see one model, a layer scoped to a screen. The engine lowers
that layer into the per-screen target it already has.** No user-facing surface
and no newly written document says "display zone" after this spec.

### 6.3 Target model

- **A screen is an output.** It is the output with segment name `Display` in
  some zone's layout (`hypercolor-core/src/spatial/mod.rs:147-151`). Nothing
  new is stored for it.
- **A face is a screen layer.** A `SceneLayer` in the zone that places the
  screen, with `scope.outputs` naming that screen. Any source is allowed: a
  face, a GIF, a web page, a color.
- **Screen layer, defined.** A layer is a screen layer when its scope has at
  least one live output and every live output in it is a screen. Any other
  layer is a canvas layer and follows Part A. A layer scoped to a keyboard and
  a screen is a canvas layer, and the screen sees it through its backdrop
  (§5.10).
- **Several screens, one layer.** A screen layer scoped to three panels
  renders once per panel, each at that panel's resolution with that panel's
  descriptor, from one set of controls. Setting the clock's hour format once
  sets it on all three.
- **Order.** Screen layers compose above every canvas layer on their screen,
  whatever their position in the zone's stack, and in stack order among
  themselves. Studio presents them as their own group so the rule is visible
  (§7.4).
- **One composition, on the bottom layer.** A screen's layers form one stack.
  The bottom enabled screen layer's `blend` and `opacity` are how that whole
  stack meets the backdrop. They are consumed there and nowhere else: inside
  the stack that layer is the opaque base. Layers above it blend within the
  stack with their own blend and opacity. This replaces
  `DisplayFaceTarget.blend_mode` and `opacity`, and it removes the second,
  inner opacity a display zone's bottom layer has today (§6.1). §6.6 handles
  stored zones that used both.
- **Pausing.** A screen's layers follow their zone's `enabled` flag, so
  pausing a zone pauses the faces of the screens it places. Today a face runs
  on when the zone that places its screen is paused, because it lives in
  another zone. This is a behavior change (§10).
- **Roles.** A scene authors `Primary` and `Custom` zones only. `Display`
  stays in the enum for stored documents and for the views in §6.7.
- **A screen's stack follows its placement.** A layer can only be scoped to an
  output, so where a screen's stack lives is decided by whether some zone of
  the scene holds its `Display` output. This holds at the end of every scene
  write, for every screen the rig knows:
  - **Placed:** the stack is screen layers in the zone that holds the output,
    unless the scene stores a display zone for the screen that cannot convert
    exactly (§6.6), in which case that zone is the whole stack.
  - **Unplaced:** the stack is a stored display zone under the screen's view
    id (§6.7.1), written and rendered as it is today, and its backdrop comes
    from wherever the render thread finds its device (§5.10). No layer scoped
    only to that screen is left dormant in the zone its output left. No zone
    pauses it: a stored zone is enabled whatever the zone its screen left was.

  A layout can hold more than one `Display` output for one device. The screen
  still has one stack, and one output is the screen's: the first that holds a
  stack, else the first. Views, face writes, lowering, and the kept face's
  coverage all read it from one function (`screens::screen_placement`).

  A screen that a layout holds under another output id has not left: its
  layers follow it to the new id (R5).

  Nothing places a screen on its own: a box the daemon chose would move the
  backdrop under a blended face. The two spellings are inverses of each other
  (§6.6), they share one zone id and one set of layer ids, and so a screen
  that leaves a zone and returns keeps its document, its effect slots and its
  running renderer.

### 6.4 Lowering

`SceneManager::refresh_resolved_zones` already builds the zone list the render
thread consumes, and already appends runtime-only display zones to it
(`hypercolor-core/src/scene/mod.rs:1621-1643`). Lowering extends that one
function. For each authored zone it emits:

1. The zone itself, with its screen layers removed and its canvas layers and
   their scopes intact.
2. One **derived display zone** per screen that has at least one enabled
   screen layer, an enabled owning zone, and a known surface.

A derived display zone is built so that scope is fully consumed before any
panel target exists:

- **Scope is resolved, then cleared.** The authored scope decides which
  screens get the layer. Each derived layer is a clone with `scope: None`,
  rendered over the full panel. The panel layout has no outputs
  (`domain/display.rs:64-74`), so a scope left on the clone would make it
  dormant under R3 and it would never get a renderer.
- **Composition is consumed.** `display_target` takes the bottom enabled
  layer's blend and opacity, normalized as today
  (`domain/display.rs:590-596`). That layer's clone is set to `Replace` at
  opacity 1, so the opacity is applied once, at the target.
- **Identity is kept.** Clones keep their authored layer ids, so control
  patches and effect slots address them. Layer health is reported under the
  authored zone's id; a layer on several screens reports its least healthy
  instance.
- **The zone id is stable.** A UUID v5 of the scene id and the screen's device
  id. It survives every refresh, a restart, and a move of the screen between
  zones, so a moved screen's face does not restart. Effect slots are keyed by
  zone and layer (`pool.rs:64`).
- **The rest is today's shape.** `role: Display`, the panel-sized layout
  `display_face_layout` builds, and the owning zone's `enabled`.

The direct-zone render pass and the effect pool consume a derived zone as they
consume a stored display zone now (`render_pass.rs:63-114`, `:332-394`). The
display lane and the workers are not untouched: they change in W1 for
backdrops (§5.10).

Lowering needs each screen's device and panel size, which the scene manager
does not hold today: the daemon writes them into scene zones
(`domain/display.rs:362-377`, `:1020-1034`). That write is replaced by a
**screen surface table** on the scene manager, keyed by the layout device id an
output carries, holding the physical device id, name, width, and height. The
display domain updates it where it calls `sync_connected_surfaces` today. A
screen with no entry (its device is not connected) lowers to nothing; its
layers stay in the scene and return when the device does.

A stored display zone that has not been converted (§6.6) passes through
lowering as it is and is that screen's stack.

### 6.5 The kept face, and what "assigned" means

The kept face is today's per-display default. Its store, file, and record are
unchanged (`display_preferences.rs:39-48`), and it still travels in backups as
`display_faces`.

Today one predicate does two jobs. A scene "has a face" for a screen when its
display zone holds a layer with an effect source, whatever that layer's or
zone's `enabled` state (`core/src/scene/mod.rs:1630-1633`, `:1910-1915`;
`domain/display.rs:1103-1105`). That predicate both suppresses the default and
answers `scene_assigned` on the wire. It ignores layers of every other source.

This spec keeps that predicate under one name and adds the one it was standing
in for:

- **Assigned:** the scene holds a screen layer with an effect source for the
  screen, enabled or not, in a zone enabled or not. Unchanged meaning. It is
  what `scene_assigned` and `live_scope` report (§6.7).
- **Rendering:** the scene holds at least one enabled screen layer for the
  screen in an enabled zone.

The kept face renders when the scene's stack is neither assigned nor
rendering. Row by row:

| Scene's stack for the screen | Today | After | Change |
| --- | --- | --- | --- |
| A face layer, enabled | Scene face renders; default suppressed | Same | None |
| A face layer, disabled | Nothing from the scene; default suppressed; the screen shows its backdrop | Same | None |
| A face layer, in a disabled zone | Nothing from the scene; default suppressed | Same. After conversion the zone in question is the one that places the screen (§6.3) | None to precedence |
| Media, color, or web layers only, enabled, and a default exists | Both the scene's display zone and the default's zone reach the render list; the output picks one binding, preferring a finalized zone and then the newer zone id (`display_output/mod.rs:897-943`) | The scene's stack renders; the kept face is suppressed | **Changed.** Today's outcome depends on zone ids. The new one is defined |
| Media, color, or web layers only, all disabled, and a default exists | Default renders | Same | None |
| No screen layers, and a default exists | Default renders | Same | None |
| No screen layers, no default | The screen shows its backdrop | Same | None |

Disabling a face does not reveal the kept face. That holds today and it holds
after.

What changes for people is that the kept face stops being a place they manage:

- **Studio never creates a kept face.** Adding or changing a face in Studio
  writes a screen layer in the scene.
- **An existing kept face stays visible and editable.** When it is what a
  screen is showing, Studio lists it in that screen's layers as a row with its
  controls, as it does today through the default-face card
  (`face_composition.rs:409-413`). Its controls write the store through the
  face routes, so a change shows in every scene that leaves this screen to it,
  and Studio says so beside the controls (§7.5).
- **"Use in this scene"** copies it into the scene as a screen layer, which is
  then assigned here and independent of the kept face.
- The use case it served, the same face in every scene, becomes a command on a
  screen layer: copy to every scene (§7.5).

The store stays because the frozen routes write it (§6.7) and because deleting
it would mean rewriting every saved scene, including snapshot-locked ones that
refuse runtime mutation (`hypercolor-types/src/scene.rs:411-413`).

### 6.6 Migration

W5 carries one migration: a semantic conversion of stored display zones into
screen layers. It changes no schema number (§4.6 moved those in W2).

Nothing is rewritten at load. A stored display zone is the **legacy spelling**
of a screen's stack: it parses, lowering passes it through, and it renders as
it does today.

**One settle pass, at the end of every scene write.** Each write to the scene
manager, a commit or a layout publication, ends by settling the live scene's
screens against the invariant of §6.3, whatever the write was: an activation,
a membership edit and its undo, a zone-layout save, a layout applied or
published, a zone or device deleted, a screen becoming known. It needs the
binding from a physical device to its output, which a file loader does not
have, and which outputs each scene held before the write, because a departed
output can only be tied to its screen from the state before it left. The pass
is idempotent. It has two halves:

- **Unplaced to placed (conversion).** A stored display zone whose screen a
  zone now holds becomes screen layers in that zone, when the result is exact
  (below).
- **Placed to unplaced (return).** A screen whose output was in a zone before
  the write and is in none after it gets its layers back as a stored display
  zone: the layers of that zone scoped only to it, in order, under their ids,
  with the bottom enabled layer's blend and opacity as the target and that
  layer as the opaque base. That is the zone lowering rendered, so the frames
  do not change. The stored zone is enabled, and carries the version counters
  of the zone its layers left. Which layers were the screen's own is decided
  against the outputs the scene held before the write: a layer that also
  reached an LED output is a canvas layer and stays as it is (R5), even when
  the same write takes that LED output away. A layer the screen shared with
  screens that are still placed stays with them and loses the departed id
  from its scope; the departed screen gets a copy under a new layer id, and
  when it returns that copy converts as a layer of its own and does not
  rejoin the shared one. A screen that already has a stored display zone
  keeps it as its whole stack. A zone that is deleted unplaces its screens,
  and their faces are stored the same way (R6).

The return and the scope rewrite of R5 are internal rewrites and run for a
snapshot-locked scene, whose primary zone a layout publication rewrites like
any other: the lock refuses authored edits at their entry points, and
`SceneManager::sync_primary_zone_layout` is not one of them. Only conversion
honors the lock.

A screen whose output leaves while its device is not known cannot be stored
yet: a stored display zone names its device. This is the startup case, where
the realign runs before discovery. The layers wait in the zone, the scene
manager remembers which screen they belonged to, and the first settle after
the device is known stores them. A clear in between keeps them. That memory
lasts for the run.

A stored zone has one composition, and the bottom enabled layer takes it. When
a disabled layer sits below that one, enabling it later in the stored zone
composes differently than it would have as a screen layer, where it would have
become the base. Such a stack is stored anyway, because the panel keeps its
picture, and the settle pass reports it as approximated.

Return then conversion, and conversion then return, are each the identity on
what the scene stores for a convertible stack, apart from the one field a
display zone never reads, its bottom layer's own blend.

An activation, by scene or by selected fields, aligns the scene's primary zone
with the layout the scene will use, its own or the selected one, before it
settles. So where each stack lives
is decided once, against the outputs the scene will have, and activating a
scene twice changes nothing.

**Conversion is exact or it does not happen.** A legacy zone has two
compositions the new model has one field for (§6.1, §6.3). A zone converts
only when the result renders the same frames:

- the zone is enabled, and its `brightness` is 1;
- its first layer is enabled, and that layer's `opacity` is 1.

Then the legacy target's blend and opacity move onto the first layer, and every
other layer moves unchanged. Lowering rebuilds exactly the zone that was
stored: same target, same opaque base, same upper layers. Any other zone stays
in the legacy spelling, keeps rendering as it does, and is offered for
conversion again whenever it changes.

| Stored today | After conversion | Notes |
| --- | --- | --- |
| Convertible display zone with layers L1 to Ln, target blend *b* and opacity *o*; its screen is output *s* of zone Z | Z gains L1 to Ln at the top, in order, each with `scope.outputs = [s]`. L1 takes blend *b* and opacity *o*. The display zone is removed | Layer ids are preserved. L1's stored blend was never used: a display zone forces its bottom layer to replace (`sparkleflinger/mod.rs:412-416`) |
| Display zone whose first layer has an opacity other than 1 | Left as stored | Its inner and outer opacities both apply today; one field cannot hold both |
| Display zone whose first layer is disabled | Left as stored | The layer that meets the backdrop is not the one the target was authored for |
| Display zone that is disabled, or whose brightness is not 1 | Left as stored | |
| Display zone with no layers | Removed | The daemon created these as bare surfaces (`core/src/scene/mod.rs:711-725`). The screen stays addressable through its view (§6.7) |
| Convertible display zone whose screen is in no zone | Left as stored | Converts in the write that places the screen, so no layer can be written for the screen beside the stored zone |
| Screen layers whose screen's output leaves every zone | Returned to a stored display zone under the view id, in that write | The inverse of the first row. The panel shows the same frames |
| Convertible display zone whose owning zone already holds screen layers for the screen | Left as stored | Those layers are covered while the zone exists and would become the bottom of its stack |
| Display zone whose device is unknown on this rig | Left as stored | Converts when the device first binds |
| Display zone in a snapshot-locked scene | Left as stored | Converts if the scene is unlocked |
| Per-display default face in the preference store | Unchanged | It is the kept face (§6.5) |
| `display_faces` in a backup manifest | Restored into the preference store, as today | |
| Backup scenes with display zones | Restored as stored, converted on activation | |
| The runtime session's default scene | Converted on its first activation after upgrade | |
| `hypercolor.toml` | No change | |

**Known limits.** Four states leave a face that does not render. None has a
mechanism of its own; each is a condition and what follows from it.

- **A waiting face's device stays away for the whole run.** The memory of
  which screen the layers belonged to ends with the run. On the next run they
  are dormant layers in their zone, and they render again only if an output
  comes back to that zone under the id their scope names.
- **A zone is deleted while its screen's device is unknown.** The face is
  deleted with the zone, because a stored display zone has to name a device.
- **One edit takes a screen's output out of one zone and adds an output with a
  different id for the same device to another.** The layers stay dormant in
  the first zone and the screen has no face in the second. A move in Studio
  keeps the output id and is not affected.
- **A whole-document write to a scene that is not live removes a screen's
  output and keeps its layers.** The settle pass runs for the live scene, so
  the layers are dormant when that scene is activated, and activation has no
  earlier state to return them from.

The startup write that rebinds device ids in stored scenes installs its
scenes without settling, because the store has already taken them. It removes
no output, so it leaves nothing to return, and a conversion it makes possible
happens on the scene's next write.

After this wave the daemon writes a display zone into a scene in one case
only: a face or a layer written for a screen no zone places (§6.7), which has
no output to scope a layer to. That zone takes the id of the screen's view, so
the client that addressed the empty view keeps the same zone id, and a later
conversion keeps the face's renderer. `ensure_display_zone_surface`,
`clear_display_zone_assignment`, and `upsert_display_zone`
(`core/src/scene/mod.rs:616-796`) are deleted. A legacy zone that cannot
convert is still edited in place through the same routes as before.

**Downgrade across W5.** A build from W2 to W4 reads a converted scene without
refusal and treats a screen layer as a canvas layer scoped to the screen: the
face shows in that screen's backdrop at canvas resolution. It is degraded, it
is confined to that screen, and no LED is affected. Restoring pre-upgrade data
is the supported way back, as in §4.6.

### 6.7 Compatibility views

One resolver, `screen_stack(device or zone id)`, answers "where is this
screen's stack": a set of screen layers in an owning zone, a legacy zone, or
nothing yet. Every row below goes through it.

#### 6.7.1 The view zone

`GET /scene` keeps listing one `Display` role zone per connected screen,
whether or not the screen has a stack. Today's Studio finds screens through
those zones (`hypercolor-ui/src/pages/studio/screen_list.rs:23-35`), so a screen
with no layers must stay listed or it loses the only place its first face can
be added. A view zone is independent of renderer allocation.

| Field | Value |
| --- | --- |
| `id` | The derived zone id of §6.4. For a legacy zone, its stored id, which is the derived id too when the daemon stored the zone for an unplaced screen |
| `name` | The screen's name |
| `role` | `display` |
| `enabled` | `true`, or the legacy zone's own flag |
| `display_target` | The device, with the bottom screen layer's blend and opacity; `alpha` and 1 for an empty stack, as a fresh target has today (`hypercolor-types/src/scene.rs:216-228`) |
| `layers` | The screen's layers in stack order, as authored, with `scope` omitted. Empty when the screen has none. The kept face is never listed here, as today |
| `members`, `layout` | Empty and absent, as today |
| `view_of` | New and optional: `{ zone, member }` naming the owning zone and the screen's output. Absent for a legacy zone and for a screen no zone places |

A new client reads screens from their owning zone and skips every `Display`
zone. The TUI already skips the role (`hypercolor-tui/src/state.rs:166`). A
view's id differs from the legacy zone it replaces after conversion; a client
that cached one gets the existing not-found error and re-reads. The daemon has
always created and removed these zones itself
(`core/src/scene/mod.rs:1397-1426`).

`ZoneChanged` events with `role: display` keep publishing for views, beside
`LayerStackChanged` for the owning zone.

#### 6.7.2 Layer routes addressed to a view

A compatibility write is always about one screen. It never changes what
another screen shows and never touches a canvas layer. A layer is *sole* when
its scope is exactly this screen, and *shared* when it names others too. Every
operation answers with the view zone. A legacy zone takes these routes directly
as it does today.

| Operation on a view | Effect on the owning zone |
| --- | --- |
| `GET .../layers` | The screen's layers, bottom to top |
| `POST .../layers` | Creates the layer at the top of the owning zone's stack with `scope.outputs = [screen]`. For a screen no zone places, the layer joins the scene's stored display zone for the screen, which is created under the view's id when there is none. A `scope` in the request is a validation error: a view's target is fixed |
| `PUT .../layers/{layer}`, sole | Replaces the layer in place. The scope stays `[screen]` whatever the request says about it, including nothing (§4.5) |
| `PUT .../layers/{layer}`, shared | Splits: the screen is removed from the shared layer's scope, and the replacement is inserted directly above it with `scope.outputs = [screen]`. Other screens keep the original |
| `DELETE .../layers/{layer}`, sole | Deletes the layer |
| `DELETE .../layers/{layer}`, shared | Removes the screen from the layer's scope. Other screens keep it |
| `PATCH .../layers/order` | `order` names every layer of the view exactly once, the existing contract (`hypercolor-types/src/api/scene.rs:486-495`) applied to the view. The view's layers are rewritten, in the requested order, into the positions they occupy in the owning stack. No other layer moves. When the requested order differs from the current one, each shared layer in the view is split first, so other screens keep their order |
| `PATCH .../layers/{layer}/controls`, sole | Patches the layer |
| `PATCH .../layers/{layer}/controls`, shared | Splits as replace does, then patches this screen's copy |

A split mints a new layer id for this screen's copy, as replace does today
(`api/scene.rs:445-449`); the returned view carries it. Shared layers only come
from new-model authoring, so a client written before this spec meets a split
only on a rig someone edited with a newer one.

#### 6.7.3 The face routes

Every route resolves the device first, with today's errors: an unknown device
is not found, and a device with no screen is a validation error
(`hypercolor-daemon/src/api/displays.rs:580-595`). `DisplayFaceResponse` keeps
every field with its type (`hypercolor-types/src/api/displays.rs:62-81`);
`effect` stays required and is never null. It gains one optional field,
`rendering`: `scene`, `default`, or `none`, naming what is on the panel. It
exists because `live_scope` says where the returned assignment lives, which is
not always what is showing.

**`GET /displays/{id}/face`.** "Face layer" means a screen layer with an effect
source; "other layers" means screen layers of any other source.

| Scene's stack | Kept face | Response |
| --- | --- | --- |
| Face layer, enabled | Any | The scene assignment: `effect` of the lowest face layer, `zone` the view, `live_scope: scene`, `scene_assigned: true`, `default_assigned` as stored, `rendering: scene` |
| Face layer, disabled or in a paused zone | Any | The same, with `rendering: none` |
| Other layers only, enabled | Stored | The kept assignment: `effect` and `zone` of the kept face, `live_scope: default`, `scene_assigned: false`, `default_assigned: true`, `rendering: scene` |
| Other layers only, enabled | None | `data: null`, as today (`api/displays.rs:165-179`) |
| Other layers only, disabled; or no screen layers | Stored | The kept assignment, with `rendering: default` |
| No screen layers | None | `data: null` |
| Face layer whose effect is not installed | Any | Not found, effect, as today (`:640-642`) |

Every row but the third returns exactly the fields today's handler returns for
the same state (`:156-180`, `:616-699`). The third is the changed row of §6.5,
and its fields are also today's; only `rendering` says the kept face is
covered.

**`PUT /displays/{id}/face`.** Request fields and their defaults are unchanged
(`hypercolor-types/src/api/displays.rs:84-96`): `effect_id` required;
`controls` empty; `blend_mode` omitted means `alpha`; `opacity` omitted means
1; a `replace` blend forces opacity to 1 (`domain/display.rs:590-596`); `scope`
omitted means `default`. An unknown effect is not found
(`api/displays.rs:203-210`), and a non-face effect is a validation error
(`domain/display.rs:149-163`).

| `scope` | Effect | Response |
| --- | --- | --- |
| `scene` | The screen's stack becomes one layer: this effect, these controls, enabled, `scope.outputs = [screen]`, with the request's blend and opacity. Sole layers of the screen are removed; the screen is removed from shared ones. Input bindings carry over when the effect is the one already assigned, as today (`core/src/scene/mod.rs:629-635`). A legacy zone for a placed screen is replaced by this layer in the same commit. For a screen no zone places, the face is written to the scene's stored display zone for the screen as today: one face layer under a target carrying the request's blend and opacity, created under the view's id when there is none | `effect`, `zone` the view, `live_scope: scene`, `scene_assigned: true`, `default_assigned` as stored, `rendering: scene`. The existing rule that reports a default composition as `alpha` stays (`api/displays.rs:275-279`, `:701-709`) |
| `default` | Unchanged: writes the kept face | Unchanged (`:221-250`): `default_assigned: true`, `zone` the kept face's zone, `live_scope` and `scene_assigned` by the assigned predicate. `rendering` follows §6.5 |

**`DELETE /displays/{id}/face`.** The `scope` query defaults to `default`
(`hypercolor-types/src/api/displays.rs:99-105`).

| `scope` | Effect | Response |
| --- | --- | --- |
| `scene` | Removes the screen's sole screen layers and removes the screen from shared ones. A legacy zone's layers are cleared and the zone removed | `deleted: true`, `scope: scene`, `scene_id` set, as today (`api/displays.rs:414-419`) |
| `default` | Unchanged: clears the kept face, including for a display that is no longer registered (`:366-374`, `:599-614`) | `deleted` says whether one was stored, `scene_id` absent (`:382-387`) |

**`PATCH /displays/{id}/face/composition`.** An empty body is a validation
error (`:298-301`).

| State | Effect | Response |
| --- | --- | --- |
| Not assigned, kept face stored | Unchanged: patches the kept face (`:304-317`) | The kept assignment |
| Assigned | Patches the blend and opacity of the stack's bottom enabled screen layer, normalized as today. A shared layer is split first | The scene assignment |
| Neither | Not found, as today (`:319-322`, `:625-639`), with the view's id where a zone id appears | |

**`PATCH /displays/{id}/face/controls`.** A non-empty `clear_bindings` and an
empty `values` are validation errors (`:438-451`).

| State | Effect | Response |
| --- | --- | --- |
| Not assigned, kept face stored | Unchanged: merges into the kept face (`:456-469`) | The kept assignment |
| Assigned, exactly one face layer | Patches that layer's controls. A shared layer is split first | The scene assignment |
| Assigned, more than one face layer; or neither | Not found, as today (`core/src/scene/mod.rs:1435-1441`; `api/displays.rs:486-489`) | |

`GET /displays` and `GET /displays/{id}/frame` are unchanged.

#### 6.7.4 The MCP tool

`set_display_face` keeps its arguments and their defaults
(`hypercolor-daemon/src/mcp/tools/displays.rs:19-57`): `device` required;
`effect_id` required unless `clear` is true; `scope` omitted means `default`
and any other value than `default` or `scene` is an invalid parameter
(`:72-81`); `controls` optional. Its result keeps every field and gains
optional `rendering`.

| Call | Effect | Result |
| --- | --- | --- |
| `scope: scene`, set | As `PUT` with `scope: scene`, blend `alpha`, opacity 1 (`:112-135`) | `cleared: false`, `scene_id`, `effect`, `zone` the view, `live_scope: scene` |
| `scope: scene`, `clear` | As `DELETE` with `scope: scene` | `cleared: true`, `scene_id`, `effect` absent, `zone` the screen's now empty view, `live_scope` by the assigned predicate. Today this returns the emptied display zone (`:88-110`); the view stands in for it, which is why views exist for empty screens |
| `scope: default`, set | Unchanged (`:165-222`) | `scene_id` absent, `effect`, `zone` the kept face's zone, `live_scope` by the assigned predicate |
| `scope: default`, `clear` | Unchanged | `cleared` says whether one was stored; `scene_id`, `effect`, `zone` absent |

The tool's description stops saying "display zone". `scope: default` is marked
deprecated in the REST and MCP references in favor of screen layers, and keeps
working through 1.x.

### 6.8 As built in W5

Where the build differs from the text above, and what it measured.

**The surface table holds every registered screen.** The face routes accept
any registered display, connected or not, and a stored display zone always
reached the render list. So the table is built from the device registry, each
entry says whether its device can take frames now, and a screen that is away
keeps its view while it has a stack and lowers as before. A screen with no
entry is one the rig does not know. A scene face write reads the registry
itself, because it may be the first thing written for a display the table has
not seen. Deleting a display drops its entry in the commit that prunes it, so
its view goes with a published removal while its layers wait like any layer
whose output was forgotten, and a disconnect re-reads the registry so an empty
screen that left stops being listed. Every writer of the table reads the
registry after its mutation begins. A commit lands only if nothing committed
since it began, so tables are installed in the order they were read and an
activation that waited on its guards cannot put an older one back.

**Nothing places a screen but its owner.** Automatic layout leaves screens
off the canvas on purpose, and the render thread finds an unplaced screen a
backdrop through its device's other outputs. A box minted for it would move
that backdrop under a blended face, so nothing adds one.

**A scope follows a renamed output at the realign.** `follow_outputs` runs
wherever a zone takes a layout whole. A rig whose stored layouts name one
panel under three ids keeps a face written under any of them through all
three, and through a clear.

**The settle pass has two call sites.** Scene state is written in two places:
the commit of a mutation, and a layout publication, which rewrites a primary
zone that follows the layout under the same lock. Both settle the live scene
and announce its display zones. A mutation records which screen outputs each
scene held when it began; a publication reads them from the state it is about
to replace. A service that answers with the scene or a zone settles before it
reads, so its answer is the scene as committed: a layer write that makes a
stored zone convertible answers with the screen's view.

**Frames across the two spellings.** On both compositors, a translucent face
on a placed screen keeps the panel byte for byte through a layout that drops
the screen and one that brings it back, with no other frame in between, and
the stored zone on a cold start is the same picture. The same holds through
the layout transaction's own write, with layouts that hold the screen under
other output ids, and a face that changes every frame keeps changing through
them and never falls back to the bare backdrop. Scenes that follow a
layout with no screen keep their faces through activation and through coming
back, which is the shape of a rig whose stored layouts hold no screens. In
those tests the screen's device holds one box on the canvas placed or not, so
the backdrop is the same and any difference would be the face.

**Conversion has three more guards.** A zone also stays in the legacy spelling
when its stored target is one the renderer clamps (a replacing target with a
partial opacity), when the zone that places its screen is paused (the face
would stop, §6.3), and when that zone already uses one of its layer ids. The
table's row for an owner that already holds screen layers for the screen is
reported the same way. Each
is reported with its reason.

**A stored display zone is the screen's whole stack while it exists.** Screen
layers scoped to the same screen are not lowered beside it, so a screen never
has two bindings.

**A zone's effect is not a face.** Screen layers and scoped overlays sit in
the owning zone's stack, above its lighting. "The effect this zone is running"
is therefore one rule in `hypercolor-types`, `zone_effect_layer`: the top
layer with no scope that has an effect source. The daemon's active effect, the
effect a stop reports, the previous effect an apply reports, the layer whose
input bindings an apply keeps, the CLI's `effects patch` and `effects reset`,
and the TUI all read it there. A client with no primary zone to read falls
back to the first zone that is not a `display` zone. The
error fallback follows the same line: a failed layer that carries a scope is
removed alone, and only a failed whole-zone layer clears its zone.

**A display zone's events come from the write.** A screen is listed as one
`display` zone, its stored zone or its view, and both change through writes
that never address them: a layer added through the owning zone, the owner
paused or deleted, a layout that takes the screen's output away, the settle
pass turning one spelling into the other under one id. Each write compares
what the live scene lists, and whether each stack draws, before and after. A
zone that appears or goes is announced as that; one that is still listed is
announced as changed, so removing the stored zone of a screen that stays
listed as an empty view is a change and not a removal.

**Layer health is published under the view's id.** That is the zone id an
unchanged client holds for a screen's layers. W6 reads screens from their
owning zone and needs the owning zone's id, with a layer on several screens
reporting its least healthy instance.

**`add_layer` takes a face on screens.** The MCP tool accepts a display-face
effect when `outputs` resolve to screens and nothing else, which is what the
REST layer routes accept. Without outputs, or with an LED output among them,
it refuses as before, and it still refuses a view as its `zone`.

**Views are of the live scene.** `GET /scenes/{id}` lists none. `PUT
/scenes/{id}` skips a zone that carries `view_of` or is the view of a screen,
so a document read from `GET /scene` writes back unchanged.

**Spikes.** A face's effect slot is keyed by zone and layer. Conversion moves
it from the stored zone's id to the derived id once, which is one renderer
start, and a second conversion, the surface table arriving again and a move of
the screen between zones all leave the key alone. A zone the daemon stored
for an unplaced screen already carries the derived id, so a screen that is
placed, unplaced and placed again starts no renderer at all. One layer on
three panels asks for three slots, as three display zones do. The captured 0.9.0 store
converts whole: two zones become screen layers, the bare surface is removed,
none is left legacy, and all three panels show the same frames before and
after on both compositors. The evidence rig's stored scenes and the hardware
pass are not in this.

---

## 7. Part C: Studio

Verified today, after the Studio changes that landed during rev 1:

- The page is a zone tab strip (one tab per LED zone, then a Screens tab), the
  device pane, and the Stage (`hypercolor-ui/src/pages/studio/mod.rs:1-7`,
  `zone_strip.rs:1-10`).
- The composition panel opens from the Stage header's chip, which reads
  "Layers" for a zone and "Face settings" for a screen
  (`pages/studio/stage.rs:376`, `:429-462`). At `xl` the panel docks: the Stage
  gives up 420 pixels and stays live and clickable. Below `xl` it overlays the
  Stage behind a scrim (`pages/studio/composition_panel.rs:100-117`,
  `mod.rs:724-729`).
- Under the canvas sits one output properties bar: what is selected, its
  position, size, rotation, and scale, and Identify, Reset, Remove, and Move to
  (`hypercolor-ui/src/components/layout_zone_properties.rs:1-2`, `:13`,
  `:448`, `:482`, `:546`).
- For a screen the Stage header carries the mounting select and a link to the
  full-screen preview, and the Stage shows the panel
  (`stage.rs:191-226`, `:234-260`;
  `hypercolor-ui/src/components/mounting_select.rs:25`;
  `hypercolor-ui/src/display_utils.rs:8`).
- A screen that shows its default face gets a card above the layer stack with
  the face's name, a "Default" chip, its controls, "Use in this scene", and
  "Clear default" (`face_composition.rs:257-258`, `:336-390`, `:409-413`).
- "All screens" in the picker adds one layer to every display zone in the
  scene (`layer_panel/source.rs:95-115`, `:172-190`).

Rules that carry over: the composition panel is never a permanent rail; device
moves stay on device cards and the properties bar; purple is the only chrome
accent, and a zone or category color is identity only. No id, role name, or
the words "scope", "default", or "scene layer" reach the user.

### 7.1 The words

| Concept | What Studio says |
| --- | --- |
| No scope | Whole zone |
| A scope of one output | The device's name: "K70" |
| A scope of several | "K70 and Basilisk", then "3 outputs" |
| A screen layer | Listed under "On this screen" |
| The backdrop | Listed under "Behind it, from Main" |
| A kept face | "Saved with this screen" |
| A dormant layer | "Not in this zone" |

### 7.2 Adding a layer to just this keyboard

Three entry points, one flow.

1. **From the canvas.** Select the keyboard's box. The output properties bar
   gains "+ Layer" beside "Move to". It opens the composition panel's picker
   with the target already set to the selection. Shift-select several boxes
   first and the target is all of them.
2. **From a device card.** The card's menu gains "Add layer to this device",
   beside "Move to". Same picker, same target. A device with several outputs
   targets all of them; one channel row targets that channel.
3. **From the stack.** "+ Add layer" in the composition panel shows its target
   as a line under the button: "Whole zone" by default, or the current
   selection when there is one. Clicking the line opens the target editor.

The **target editor** is a popover: the zone's outputs as a checklist grouped
by device, "Whole zone" at the top, and "Pick on canvas". At `xl` the Stage is
already live beside the docked panel, so picking toggles boxes directly until
"Done". Below `xl` the panel slides away while picking and returns on "Done".
The editor is hidden when the zone has one output.

**Screens are not in the target editor until screen layers exist.** In the
wave that ships this flow (W4), the checklist and canvas picking cover LED
outputs only, and a screen keeps its existing path: the Screens tab and its
face picker. A face authored as a scoped layer before the engine lowers screen
layers would render as a canvas layer, at canvas resolution, under the screen's
existing face. Screens join the target editor in W6, gated on the
`screen-layers` capability.

The picker opens in its effect mode for any target that includes a non-screen
output, and in its face mode when every targeted output is a screen
(`layer_panel/source.rs:46-49`, `:117-128`). The Effects page still leaves
faces out (`hypercolor-ui/src/pages/effects.rs:53`).

### 7.3 Scoped layers in the stack

The wireframe is the `xl` layout, with the composition panel docked.

```
+ Studio ------------------------------------------ Scene  Studio Session v +
| * Main    * Desk    +                                                      |
+-------------------------+--------------------------------------------------+
| / Search devices        | (o) Bubbles  | Layers >      undo redo  Fit |    |
|                         |-------------------------+------------------------+
| MAIN . 19               |  +------+  +========+   | Layers              x  |
|  [] Corsair K70     ... |  | fans |  |  K70   |   | Showing: K70    [All]  |
|  [] ROLI Lightpad   ... |  +------+  +========+   |                        |
|  [] TL LCD 1  Clock ... |                         | Ripples       [ K70 ]  |
|  [] Nollie 32       ... |      (clock)            | Bubbles    Whole zone  |
|                         |     TL LCD 1            | . Glow    [ Strimer ]  |
| UNASSIGNED . 2   Off v  |                         |                        |
|                         +-------------------------+ + Add layer            |
|                         | K70 . 110 LEDs  x y w h |   to K70               |
|                         | rot  + Layer   Move to v|                        |
+-------------------------+-------------------------+------------------------+
```

- **One list, bottom to top**, as today. A scoped row carries a chip after its
  name with the target in the words of §7.1. An unscoped row reads "Whole
  zone" in secondary text, with no chip.
- **Hover a row or its chip**: the scoped boxes take the purple outline on the
  Stage and every other box dims. Hovering a box highlights the rows that apply
  to it. This is the existing rail-to-canvas hover sync, extended to layers.
- **Click the chip**: the target editor (§7.2). Changing it is one undo step.
- **Selecting a box filters emphasis, not content.** With the K70 selected the
  header reads "Showing: K70": rows that apply to the K70 are full strength in
  their true order, rows that do not are dimmed with a leading dot (the
  Strimer's "Glow" above). "All" clears the emphasis. The list always answers
  "what is on this device" without hiding the rest of the zone.
- **A dormant row** shows a warning glyph, "Not in this zone", and a remove
  action. It takes no renderer (R3).
- **Moving a device** carries its layers (R4). The undo toast says so: "Moved
  K70 to Desk with 1 layer."
- **Studio sends the scope on every replace.** Its request builder
  (`hypercolor-ui/src/api/layers.rs:26-39`) gains the field, so a blend change
  restates the scope rather than relying on the keep rule of §4.5.

### 7.4 A screen's face and its settings

A screen's box on the Stage shows the panel's live picture inside it, fed by
the `display_preview` topic, which is keyed by device
(`hypercolor-leptos-ext/src/ws/registry.rs:26`, `stage.rs:150-170`). Its card
in the device pane shows the face's name.

Selecting the box, or the card, makes the Stage header's chip read the face's
name with "Face settings", as it does for a screen today, or "+ Face" when the
screen has none. It opens the composition panel on the screen:

```
+ Layers                              x +
| TL LCD 1                              |
|                                       |
| ON THIS SCREEN                        |
|  v Neon Clock        Alpha    100%    |
|      Hour format    ( 12h | 24h )     |
|      Dial style     [ Orbit      v ]  |
|      Accent         [ #80ffea ]       |
|  + Add layer to this screen           |
|                                       |
| BEHIND IT, FROM MAIN                  |
|    Bubbles                Whole zone  |
+---------------------------------------+
```

- **On this screen** holds the screen layers, top first, with the face's
  controls open in place. The hour format is two clicks from the box: select,
  open.
- **The bottom row's blend and amount are how the stack meets what is behind
  it** (§6.3). They are the controls the separate face-composition section
  holds today (`face_composition.rs:1-7`, `:52-53`), now on the row they
  belong to. Rows above the bottom one blend within the stack.
- **Behind it** lists the zone's canvas layers that reach this screen, dimmed
  and not editable from here; clicking one clears the selection and shows it in
  the zone's list.
- **A stack still in the legacy spelling** (§6.6) looks the same. Studio
  addresses its stored zone; nothing about it is shown differently.
- **Mounting and the standalone preview move with the screen.** With one
  screen selected, the output properties bar carries the mounting select and
  the full-screen preview link that sit in the Screens Stage header today.
  Mounting is a fact about the panel, so it belongs with the output's other
  placement facts.
- **Focus.** With one screen selected, a Stage toolbar toggle fills the Stage
  with that panel at full size, round mask and rotation applied, for close
  work. Its toolbar repeats the mounting select and the preview link. Escape
  returns. It is a view of the selected output, not a tab.
- **Several screens.** Select three LCD boxes and add one face: one row, one
  set of controls, chip "3 screens".

### 7.5 What replaces the Screens tab

Nothing takes its place in the strip. A screen is found where its hardware is:
a box on its zone's canvas and a card in that zone's section of the device
pane. Pane search matches "screen" and the device name, and a screen's card
carries a screen glyph.

- **All screens.** The scene-wide gesture stays, as a command rather than a
  target. "Add to all screens" in the picker, offered whenever the scene has
  screens in more than one place, creates one layer per zone that owns
  screens, each scoped to that zone's screens, in sequence, as one undo step.
  Screens in Main and in Desk get the face together. A failure partway is
  reported with what landed, as the current multi-target add does. Unassigned
  screens are named in the confirmation and skipped.
- **A kept face that is live** appears under "On this screen" as a row with
  its controls and a "Saved with this screen" chip. Beside the controls:
  "Changes apply in every scene using this saved face." Its menu has "Use in
  this scene", which makes an independent copy as a screen layer, and
  "Remove", which clears it everywhere. Studio offers no way to create one.
- **Copy to every scene** sits in a screen layer's row menu. It copies the
  layer, with its controls, onto the same screen in every saved scene that is
  not snapshot-locked, replacing that screen's layers there, after one
  confirmation that names the count: "Use Neon Clock on TL LCD 1 in 6 scenes?"
  The copies are independent afterwards (§11).
- **An unassigned screen** sits in the Unassigned section with "+" like any
  device. It shows no face until a zone claims it.

Deleted with the tab: `screen_list.rs`, `SurfaceKind::Screen` and the code
that branches on it, `DefaultFaceCard` and `ScreenCompositionSection` in
`face_composition.rs` (the kept-face controls move into the row), the Screens
branch of the Stage header, and `AddLayerScope` in `layer_panel/source.rs`.

### 7.6 Phones

The phone summary lists one card per LED zone and lists screens read-only
(`hypercolor-ui/src/pages/studio/phone_summary.rs:1-9`); the spatial editor is
one tap away (`pages/studio/phone_view.rs:1-7`).

- A zone's card lists its screens with its other devices, each with its face's
  name. Tapping one opens a bottom sheet with "On this screen", the face's
  controls, and the mounting select.
- A scoped layer shows its chip in the zone card's layer list. Tapping the
  chip opens the target editor as a bottom-sheet checklist. "Pick on canvas"
  appears only in the spatial editor.
- The zone card's effect button keeps replacing the zone's base effect, and by
  §4.4 it leaves scoped layers and faces alone.
- Hover highlights have no touch twin; the checklist and the "Showing" header
  carry the same information.

---

## 8. Delivery waves

Each wave ships alone and leaves `main` releasable. Each deletes what it
supersedes in the same change. GPU tests run with
`HYPERCOLOR_REQUIRE_GPU_TESTS=1`, and the daemon suite runs serialized.

Migrations, stated plainly: W2 carries one coordinated storage migration across
three containers. W5 carries one semantic conversion. The other four waves
carry none.

### W1. Per-screen backdrops

Section 5.10, on zone frames. No format, wire, or UI change.

- **Migration:** none.
- **Tests:**
  1. Two screens whose boxes overlap, in two zones with different fills: each
     panel shows its own zone, with a face and without one, on both backends.
  2. One zone, no overlap: every panel's frames match the pre-change frames.
  3. A screen without a face holds its cadence cap; a screen with one holds
     the face cadence.
  4. The viewport taken from the owning zone equals the one taken from the
     spatial service for every fixture layout.
- **Hardware:** all five panels on the evidence rig show the same picture
  before and after, at the same cadence, for ten minutes at the 60 fps tier.

### W2. Scope in the engine, and the storage boundary

Sections 4.1 to 4.4, 4.6, and 5.1 to 5.9. `SceneLayer.scope` exists, is
validated, follows the lifecycle rules, persists behind the new schema
numbers, and renders correctly on both backends. No request type carries it
yet and no capability is advertised, so the only way in is a stored document
or a test.

- **Migration:** the coordinated storage migration of §4.6.
- **Tests:**
  1. A zone with no scoped layer allocates one class, one texture, and one
     sampling pass, and the lone-zone path still runs: allocation and pass
     counters equal the pre-change values.
  2. Two overlapping outputs, a layer scoped to one: the other's LEDs match
     the unscoped stack bit for bit, under nearest, bilinear, area, and
     Gaussian sampling.
  3. Every blend mode, scoped layer on top and in the middle of the stack:
     GPU matches CPU within the existing parity tolerance.
  4. A scoped bottom layer with nothing beneath: outputs outside the scope
     are black.
  5. Placement: for an output rotated by an arbitrary angle on a canvas that
     is not square, with a layer transform that moves, scales, and rotates, a
     marker drawn at a source point lands on the LED that samples it, on both
     backends. A two-output scope is continuous across both.
  6. Input: an off-centre touch on that same rotated output, through that same
     layer transform, yields the source point whose pixels the LED under the
     finger samples. A touch on a device outside the scope carries no point.
     An unscoped layer with a transform gets the corrected point.
  7. R1 to R6 as scene manager tests: create and replace with a new id that
     is not a member are refused; replace carrying a dormant id succeeds;
     apply after unassigning a scoped output succeeds and keeps the dormant
     layer; a two-output device moved in one edit carries its layer and undoes
     exactly through `layer_moves`.
  8. Apply keeps scoped layers; clear removes them and keeps screen-only ones.
  9. Scene canvas and zone preview show each box from its class; a screen's
     backdrop shows its class.
  10. Transitions between a one-class and a two-class scene blend in LED
      space.
  11. Storage: schema 2 scenes, a version 0 runtime session, and the shipped
      v1 backup all load; the new v2 backup fixture and the captured 0.9.0
      scene store load; a build from before this wave refuses each new file.
  12. Admission: a refused class texture takes the existing fallback; a canvas
      resize with classes never samples a class at the old extent.
- **Hardware:** on the evidence rig, one Main zone running Bubbles with Touch
  Ripple in `screen` scoped to the ROLI and Push 2 pad outputs, written as a
  stored scene. Ripples appear on those devices only, under the finger, and
  Bubbles is continuous across the rig. The loop holds the 60 fps tier for ten
  minutes with all five faces active, and frame `total_us` p95 is no worse than
  the three-zone "Studio Session" scene measured in the same session.

### W3. Scope on the wire

Section 4.5: the request fields, the replace tri-state, R1 errors over REST,
the `add_layer` MCP tool, the `layer-scope` capability, OpenAPI, the
regenerated Python client, and the reference pages.

- **Migration:** none.
- **Tests:** `layer_api_tests` for create with a scope, a non-member id, and
  an empty list; replace with the scope omitted (kept), `null` (cleared), and
  a value; `openapi_tests` and the REST wire contract fixtures; `mcp_tests` for
  `add_layer`; a Python client round trip.

### W4. Scope in Studio, for LED outputs

Sections 7.1 to 7.3 and the scope parts of 7.6, gated on `layer-scope`. Screens
are excluded from the target editor and keep the Screens tab (§7.2).

- **Migration:** none.
- **Tests:** `just ui-test` for the target editor model (screens absent), chip
  labels, the emphasis filter, dormant detection, the scope on every replace
  request, and undo of a scope change and of a carried layer.
- **Visual, with `agent-browser`, both themes, at `xl`, below `xl`, and at
  phone width:** add a layer to one keyboard from the canvas, from the card,
  and from the stack; retarget it by picking on the canvas with the panel
  docked and with it overlaid; move the keyboard to another zone and undo.
- **Hardware:** the owner rebuilds "Studio Session" as one zone in Studio
  alone.

### W5. Screens in the engine

Sections 6.3 to 6.7: lowering, the screen surface table, conversion on
activation, the resolver, the view zones, the layer-route adapters, the face
route and MCP mappings, and the `screen-layers` capability. Deletes the three
display-zone writers in the scene manager and the surface-sync zone writes,
leaving one writer for a screen no zone places.
Studio is not edited in this wave. Its Screens tab keeps working because every
connected screen still has a view zone, empty or not, and the layer routes it
uses are adapted per operation.

- **Migration:** the semantic conversion of §6.6. No schema number changes.
- **Tests:**
  1. One test per row of the §6.6 table.
  2. Exactness, comparing frames before and after conversion: a stack of three
     layers; a target opacity other than 1 over a multi-layer stack; a target
     blend other than `alpha`. A first layer with an opacity other than 1, and
     a disabled first layer, stay legacy and render the same frames.
  3. Conversion run twice changes nothing the second time.
  4. One test per row of §6.5, including the changed row.
  5. One test per row of every table in §6.7, asserting each response field.
  6. Through the unchanged Studio path: add the first face to a screen that
     has no layers; delete a screen's last layer and add one again; replace a
     face's layer with a request that carries no scope and confirm it stays on
     that screen.
  7. A layer shared by two screens: replace, delete, reorder, and a control
     patch through one screen's view leave the other screen unchanged.
  8. One layer scoped to two screens renders at each panel's size.
  9. The captured 0.9.0 scene store and the shipped v1 backup load, convert,
     and render the same frames as before.
- **Hardware:** upgrade the evidence rig in place. All five screens show the
  same picture before and after, at the same cadence. Then write one Neon
  Clock layer scoped to all three TL panels, change its hour format once, and
  see all three change.

### W6. Screens in Studio

Sections 7.4 to 7.6, and screens joining the target editor of §7.2. Deletes
the Screens tab and everything listed in §7.5. Gated on `screen-layers`.

- **Migration:** none.
- **Tests:** `just ui-test` for the two-group screen panel model, the kept
  face row and its notice, "Add to all screens" planning across two zones as
  one undo step, and "Copy to every scene" planning.
- **Visual, both themes, at `xl`, below `xl`, and at phone width:** select a
  screen's box and change a face setting; add one face to three screens; add a
  face to all screens across two zones and undo it once; change a panel's
  mounting from the properties bar; open the standalone preview; focus a round
  panel; move a screen between zones with its face.
- **Hardware:** the owner finds and flips the clock's hour format from the
  box, cold, with no instruction.

---

## 9. Risks and the spikes that retire them

Run on throwaway branches before the wave named. Each ends in a number or a
yes. Frame-time spikes are measured against the tier budgets the render loop
enforces, on the evidence rig, not only against each other.

| Before | Risk | Spike |
| --- | --- | --- |
| W1 | Finalizing a backdrop in the render thread for a screen with no face costs more than the worker's crop of the scene canvas | Measure both for the two faceless TL panels. Pass: frame `total_us` p95 no worse, and the scene-canvas readback the old path needed is gone |
| W1 | The owning zone's viewport differs from the spatial service's | List both for every screen on the evidence rig, in a one-zone and a multi-zone scene. Pass: equal, or the difference is explained and the owning zone's is the correct one |
| W2 | Leaving the lone-zone path costs more than three zones did | Hard-wire two classes for one zone and compare `total_us` p95 against the live three-zone scene, with no preview subscriber and with one |
| W2 | Class work scales worse than the table says | Measure one zone with four classes; a scoped layer in the middle of a three-layer stack, which cannot extend the base; the same on the CPU backend; and area sampling on every class. Pass: each holds the 60 fps tier on the evidence rig, or the wave ships with the measured limit stated |
| W2 | Effects misbehave at framed extents | Render every interactive and audio effect at 640 by 64, 64 by 640, and 640 by 640 through the capture harness and review the frames. Faces already render at 960 by 160 |
| W2 | The placement affine drifts between CPU, GPU, and input | A property test over random placements, canvas aspects, and layer transforms: the three agree on where a source point lands, within the parity tolerance |
| W2 | Undo of a carried layer drifts | Prototype `layer_moves` through Studio's history for a two-output device and replay it both ways |
| W2 | An older build does something worse than refuse a versioned runtime session | Run the 0.9.0 daemon against a session file with `schema_version`. Record exactly what it does, for the release note |
| W5 | Lowering restarts faces or flaps display workers | Prototype lowering for one screen: confirm one renderer start at conversion, none afterwards, none on a move between zones, and no duplicate face binding in any frame |
| W5 | Conversion cannot find a screen's output | Run conversion against the owner's stored scenes and a backup restored on a clean data directory; list every display zone that fails to resolve, and every one the exactness rule leaves legacy |
| W5 | One layer on several screens multiplies HTML renderers | Measure three panels on one layer against three display zones. Pass: equal |

---

## 10. Decisions taken, owner may overrule

Rev 1 asked three questions. Both reviews answered them the same way, and they
are decided in the text. Three more decisions follow from the reviews.

1. **The compatibility surface is carried.** The kept-face store, the `scope`
   parameter on the face routes and the MCP tool, and the `Display` view zones
   stay through 1.x. The spec chose the additive-only contract in §0; under
   it this is a constraint, not an option to revisit during implementation.
2. **Scoped layers are always framed.** There is no mode switch in this
   delivery. The rationale is not that an unscoped layer covers the other
   case; it does not. A zone-coordinate effect limited to some outputs is a
   separate use case and is deferred (§3.2). If it is wanted later,
   `LayerScope` takes an additive `framing` field and `LayerPlacement` takes a
   whole-canvas frame with the same scope classes; nothing here blocks it.
3. **Applying an effect keeps scoped layers and faces.** Changing the base
   effect preserves the keyboard's interaction layer and every screen's face
   (§4.4). Clear removes lighting and keeps faces, as it does today.
4. **Pausing a zone pauses its screens' faces** (§6.3). Today a face keeps
   running when the zone that places its screen is paused. A single screen is
   paused by disabling its layer.
5. **A screen's media, color, or web layers cover the kept face** (§6.5).
   Today that case is decided by zone ids. Every other precedence case is
   unchanged, including that disabling a face does not reveal the kept one.
6. **A stored display zone converts only when the result is exact** (§6.6).
   The rest stay in the legacy spelling indefinitely and keep working.

---

## 11. Open question for the owner

**Is it acceptable that a face authored in Studio belongs to one scene?**

After this spec, Studio writes faces as scene layers and never creates a kept
face. "Copy to every scene" makes independent copies: change the clock's hour
format in one scene afterwards and the others keep theirs. Kept faces that
exist today keep their one-edit-everywhere behavior, and so does anything
written through `scope: default`.

The alternative is a follow-up spec for linked layers: one layer that several
scenes reference, edited once. It is a real feature with its own storage and
undo questions, and it is the "Surfaces" direction Spec 64 already names. My
pick is to ship per-scene faces now, because new scenes start as a duplicate of
the current one and carry its faces, and to write the linked-layer spec only
if per-scene copies prove to be upkeep in dogfooding. This is a product call
about how the owner uses her own rig, so it stays open.

---

## 12. Appendix: file inventory

**W1**

- `hypercolor-daemon/src/render_thread/display_lane.rs`: per-screen backdrop
  in finalize
- `hypercolor-daemon/src/display_output/mod.rs`, `display_output/worker.rs`:
  the backdrop-only source and its cadence cap
- `hypercolor-core/src/bus/`: the per-screen backdrop route
- `hypercolor-daemon/src/render_thread/render_zones/render_pass.rs`: zone
  frames kept for backdrop resolution

**W2 and W3**

- `hypercolor-types/src/layer.rs`: `LayerScope`, `SceneLayer.scope`,
  normalize, validate
- `hypercolor-types/src/api/scene.rs`: request fields, the replace tri-state,
  `layer_moves`
- `hypercolor-core/src/scene/mod.rs`: R1, R4, R5, apply and clear rules
- `hypercolor-core/src/spatial/layer_placement.rs` (new): `LayerFrame`,
  `LayerPlacement`
- `hypercolor-core/src/spatial/device_surface.rs`: the resolved output
- `hypercolor-core/src/effect/lightscript/payload.rs`, `effect/traits.rs`:
  placement and scope in the touch mapping
- `hypercolor-core/src/effect/pool.rs`: per-slot render extent
- `hypercolor-daemon/src/domain/scene_tree.rs`, `scene_tree/member_edits.rs`,
  `domain/scene.rs`: carry rule, clear rule, replace context
- `hypercolor-daemon/src/render_thread/render_zones/scope_classes.rs` (new)
- `hypercolor-daemon/src/render_thread/render_zones/{layer_rendering,render_pass,scene_assembly,reconcile,projection,zone_state,frame_helpers}.rs`
- `hypercolor-daemon/src/render_thread/{frame_sampling,pipeline_runtime}.rs`
- `hypercolor-daemon/src/render_thread/sparkleflinger/{mod.rs,blend.wgsl,cpu.rs,transform.rs}`,
  `sparkleflinger/gpu/compositor/layers.rs`, `sparkleflinger/gpu/projected.rs`
- `hypercolor-daemon/src/{scene_store,runtime_state,durable_stores}.rs`,
  `hypercolor-types/src/portable_setup/backup.rs`, and the new fixtures
- `hypercolor-daemon/src/api/scene.rs`, `api/system.rs`,
  `mcp/tools/effects.rs`
- `python/`: regenerated client

**W4 and W6**

- `hypercolor-ui/src/components/layer_panel/{mod,row,picker,source}.rs`
- `hypercolor-ui/src/components/layout_zone_properties.rs`: "+ Layer",
  mounting, and the preview link
- `hypercolor-ui/src/api/layers.rs`: scope on every replace
- `hypercolor-ui/src/pages/studio/{composition_panel,stage,device_card,device_pane,history,moves,phone_summary,zone_strip}.rs`
- Deleted in W6: `pages/studio/screen_list.rs`, most of
  `pages/studio/face_composition.rs`, `SurfaceKind::Screen` in
  `zones/surface.rs`

**W5**

- `hypercolor-core/src/scene/mod.rs`: lowering, screen surface table, removal
  of the display-zone writers
- `hypercolor-daemon/src/domain/display.rs`, `api/displays.rs`,
  `mcp/tools/displays.rs`, `mcp/results.rs`: the resolver, the predicates, and
  the mappings
- `hypercolor-types/src/api/displays.rs`: `rendering`
- `hypercolor-daemon/src/domain/scene.rs`: conversion on activation
- `hypercolor-daemon/src/domain/scene_tree.rs`: view zones, `view_of`, the
  layer-route adapters

---

## 13. Review record

Rev 1 was reviewed twice, independently, against `d01dca1c6`. Both verdicts
were "sound with fixes". Each finding was checked against the code it cited
before it changed the text. None was refuted. One was taken with a different
remedy than the one proposed (C2), and two were widened beyond what was asked
(E3, C3). Each says so.

**Engine and rendering**

| # | Finding | Checked against | Disposition |
| --- | --- | --- | --- |
| E1 | The existing transform fields cannot express a rotated frame, and the input mapping left out the layer's own transform | Placements rotate in normalized units (`sampler/mod.rs:72-84`); composition rotates in pixels (`blend.wgsl:203-210`); the touch point is published unmapped (`payload.rs:166-181`) | Accepted. §5.5 defines one source-to-zone affine and `LayerPlacement`; §5.6 inverts the same value; W2 tests 5 and 6 |
| E2 | Lowering applied the bottom layer's opacity twice | Inner composition keeps a base layer's opacity (`frame_helpers.rs:114-126`, `layers.rs:28-31`); finalize applies the target's again (`display_lane.rs:256-257`) | Accepted. §6.3 and §6.4 consume blend and opacity into the target and normalize the clone |
| E3 | Overlapping screens cannot get distinct backdrops from one flattened canvas | Screens are painted in order into one image (`projection.rs:228-251`) that every screen crops (`display_output/mod.rs:798-811`) | Accepted and widened: it already holds across zones. §5.10 resolves a backdrop per screen and ships first as W1. The "downstream is unchanged" claim is gone from §6.4 |
| E4 | Lowering must consume scope before building panel targets | The panel layout has no outputs (`domain/display.rs:64-74`) | Accepted. §6.4 |
| E5 | Strict membership validation contradicts keeping dormant layers through apply; the apply citation was wrong | Apply ends in the private `replace_effect_layer_stack` (`core/src/scene/mod.rs:1482`, `:1893-1908`) | Accepted. R1 validates introduced ids only; §4.4 cites the right function |
| E6 | Kept-face precedence changed despite the claim | Suppression checks for an effect source and ignores `enabled` (`core/src/scene/mod.rs:1630-1633`, `:1910-1915`) | Accepted. §6.5 separates assigned from rendering, preserves every effect-source case, and states the one changed row |
| E7 | The cost model understated producer size and per-class work | A square frame in a 640 by 480 zone is 640 by 640; area scans run per pass (`gpu_sampling.rs:1308-1320`); projection runs every frame (`scene_assembly.rs:91-94`) | Accepted. §5.5 states the bound; §5.8 adds the rows; §9 adds the spikes and stops claiming a frame-time saving |
| E8 | The class bound, the index width, and admission were inconsistent | The base class always exists; one texture binds per pass (`gpu_sampling.rs:908-915`); admission and resize have explicit paths (`projected.rs:100-125`, `pipeline_runtime.rs:2256-2275`) | Accepted. §5.4 bound and index; §5.8 admission |
| E9 | Two descriptions of current code overstated their citations | Faces finalize in the render thread or blend in the worker (`worker.rs:598-630`); opaque replaces copy (`layers.rs:54-75`) | Accepted. §5.1 and §6.1 reworded |

**Compatibility, migration, and UX**

| # | Finding | Checked against | Disposition |
| --- | --- | --- | --- |
| C1 | Downgrade protection arrived after scope became writable and missed the runtime session | `SceneLayer` tolerates unknown fields (`layer.rs:71-73`); the session is unversioned (`runtime_state.rs:206-222`); a shipped fixture is required per backup schema (`portable_backup_tests.rs:123-140`) | Accepted. §4.6 moves the boundary to W2 across all three containers and states the downgrade story. Rev 1's claim of a frozen scene-store fixture was wrong and is corrected |
| C2 | Migration did not preserve face composition | The bottom layer keeps its opacity (`cpu.rs:409-437`) | Accepted, with a different remedy. The model keeps one composition field, and §6.6 converts only when the result is exact and leaves every other zone in the legacy spelling. W5 test 2 holds the cases asked for |
| C3 | Forwarding layer writes to the owning zone was not enough | Studio replaces whole layers with no scope (`hypercolor-ui/src/api/layers.rs:26-39`); reorder must name every layer (`api/scene.rs:486-495`) | Accepted and extended. §6.7.2 gives an adapter per operation, with split and subtract for shared layers. §4.5 also makes replace keep an omitted scope for every zone, which removes the widening at its root |
| C4 | The unchanged precedence changed for disabled assignments | `display_zone_has_face_assignment` ignores `enabled` (`domain/display.rs:1103-1105`) | Accepted. Same change as E6 |
| C5 | W4 made empty screens vanish from an unchanged Studio | The Screens list is built from display zones (`screen_list.rs:23-35`) | Accepted. §6.7.1 keeps a view for every connected screen; W5 test 6 |
| C6 | W3 exposed screen authoring before screen layers existed | Rev 1 shipped the face-mode picker under `layer-scope` alone | Accepted. §7.2 and W4 exclude screens until W6 |
| C7 | The face compatibility table left states and fields unspecified | `effect` is required (`hypercolor-types/src/api/displays.rs:66`); the MCP clear returns an emptied zone (`mcp/tools/displays.rs:88-110`) | Accepted. §6.7.3 and §6.7.4 are normative matrices. `effect` stays required; the new `rendering` field is optional |
| C8 | Removing the Screens tab lost scene-wide "All screens", mounting, and the standalone preview | `resolve_add_layer_targets` spans the scene (`layer_panel/source.rs:172-190`); the Stage header holds mounting and the preview link (`stage.rs:191-226`) | Accepted. §7.5 keeps "Add to all screens" as one undo step; §7.4 homes mounting and the preview link |
| C9 | Kept-face editing had contradictory persistence semantics | Kept-face controls already write the store (`face_composition.rs:409-413`) | Accepted. §6.5 and §7.5 say Studio never creates one, existing ones stay editable, and name the consequence beside the controls |

Both reviews also noted that "one migration per wave" was not literally true.
§8 now states what each wave carries.
