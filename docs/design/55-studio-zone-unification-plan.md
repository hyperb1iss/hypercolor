# Studio Zone-Model Unification Plan

> Correct the Studio UI to the locked Scene → Zone model, replace the
> standalone layouts library with a scene selector, restore the device
> cards to a working state, make every effect-apply surface zone-aware,
> and lock naming — user-facing strings and internal Rust types alike —
> across the whole codebase.

**Status:** Locked pending owner sign-off — revised through two Codex
(gpt-5.5, high reasoning) adversarial passes
**Author:** Nova
**Date:** 2026-05-19
**Crates:** `hypercolor-ui`, `hypercolor-daemon`, `hypercolor-core`,
`hypercolor-types`, `hypercolor-hal` (rename only)
**Supersedes:** the `idempotent-growing-hanrahan` Studio-rework plan
**Amends:** Spec 64 (Multi-Zone Scenes), Spec 65 (Studio Composition UI)
**Relates to:** Spec 27 (Render Groups), Spec 60 (Layer Stack),
Spec 66 (Component Designer)

---

## 1. Why This Plan Exists

The Studio redesign shipped (the `stef/boops` branch: the original
8-commit redesign plus a 5-wave rework). Reviewed live, it reads as
bolted-together, and the owner identified the root causes:

1. **The vocabulary drifted into smart-home speak.** "Room," "All
   Lights," "Lights," "Zones & Devices" — consumer language for a
   professional tool. This is not Home Assistant.
2. **The device cards are dead.** The old layout-palette device cards
   carried add-to-canvas, remove, and live per-channel / per-component
   data. The Studio tree cards are passive selector tiles.
3. **The Studio has no page header** like every other page, and the
   left column wears an ugly "Zones & Devices" label.
4. **The model itself is inconsistent.** The Studio canvas edits a
   standalone *layouts library* instead of the selected zone's own
   layout — so in a multi-zone scene the canvas saves to nothing.

The owner then locked the conceptual model (§2). This plan is the
comprehensive, reviewed route from where the code is to that model,
with everything — naming, types, API, UI — made consistent.

This plan revises work shipped only days ago. That is expected: the
5-wave rework proved out the two-column workspace and the always-on
canvas; this plan keeps those and corrects the model they sit on (§9).

---

## 2. The Locked Model

Decided with the owner. This is the spine; everything else serves it.

```
Scene  ── top-level object. Exactly one is active. Owns everything.
 └─ Zone  ── a part of a scene. The switchable unit.
     ├─ Layers   ── the zone's inputs: an effect, a face, screen capture, …
     └─ Layout   ── the zone's OWN spatial canvas; device outputs placed on it
         └─ Output ── one device output/segment, positioned on the canvas

Device ── physical hardware.
 └─ Channel  ── an addressable run on the device.
     └─ Component ── what is wired to a channel: a strip, an infinity
                     fan, or an arbitrary LED area. (Code: "attachment".)
```

Load-bearing rules:

- **A scene owns everything.** Pick a scene and you have its zones,
  their layers, their device placement. Nothing meaningful exists
  outside a scene.
- **Each zone owns its own layout.** A zone's layout is its own spatial
  canvas. Switching zones switches the canvas. This is *already* what
  Spec 64 §8.1 and §8.4 specify — the implementation diverged from it.
- **A device output lives in exactly one zone's layout at a time.**
  Adding it to another zone's layout removes it from the first.
- **The default zone** is just a zone. A fresh scene has one, holding
  every device, named "Default zone"; the user can rename it. There is
  no "All Lights."
- **There is no standalone "layouts library."** A saved arrangement is
  not a first-class object; it is part of a scene. What the user picks
  is a **scene**.

### 2.1 What this kills

| Killed concept | Replaced by |
| --- | --- |
| The "layouts library" (`/api/v1/layouts`, the picker) | A **scene selector** |
| "All Lights" / "Lights" as a label | "Default zone"; the section is "Zones" |
| "Room" (picker, "New room", "Rename room") | Scene-level controls |
| The "Zones & Devices" column header | A proper Studio `PageHeader` |
| The Preview/Layout Stage toggle | Already removed in the rework |
| Cross-zone focus dimming (rework Wave 4) | Moot — each zone is its own canvas |

---

## 3. The Locked Vocabulary

The single source of truth for naming. User-facing strings **and**
internal Rust identifiers.

| Concept | User-facing term | Internal Rust type | Today |
| --- | --- | --- | --- |
| Top-level container, one active | **Scene** | `Scene` | `Scene` (unchanged) |
| A device partition with layers + a layout | **Zone** | `Zone` | `RenderGroup` → rename |
| The default zone of a fresh scene | **Default zone** | `ZoneRole::Primary` | "All Lights" string |
| A zone's input | **Layer** | `SceneLayer` | `SceneLayer` (unchanged) |
| A layer's content kind | **Source** | `LayerSource` | `LayerSource` (unchanged) |
| A zone's spatial canvas | **Layout** | `SpatialLayout` | `SpatialLayout` (type kept) |
| One device output placed on a layout | **Output** | `Output` | `DeviceZone` → rename |
| Physical hardware | **Device** | `Device*` | unchanged |
| An addressable run on a device | **Channel** | (`Output.zone_name`) | implicit |
| What is wired to a channel | **Component** | `Component*` | `Attachment*` → rename |
| A component bound to one output | (internal) | `OutputComponent` | `ZoneAttachment` → rename |

Derived type renames (satellites of the three headline types):
`RenderGroupId`→`ZoneId`, `RenderGroupRole`→`ZoneRole`,
`RenderGroupMetaPatch`→`ZoneMetaPatch`, `RenderGroupRuntime`→
`ZoneRuntime`, `RenderGroupResult`/`RenderGroupEffectError`→`Zone*`;
`DeviceZone`→`Output`, `DeviceZoneRef`→`OutputRef`,
`DeviceZoneAssignment`→`OutputAssignment`; the whole `Attachment*`
family → `Component*` (`AttachmentTemplate`→`ComponentTemplate`,
`AttachmentCategory`→`ComponentCategory`, `AttachmentBinding`→
`ComponentBinding`, `AttachmentSlot`→`ComponentSlot`,
`AttachmentRegistry`→`ComponentRegistry`, `AttachmentProfileStore`→
`ComponentProfileStore`, `ZoneAttachment`→`OutputComponent`).

Kept deliberately: `Scene`, `SceneLayer`, `LayerSource`,
`SpatialLayout` (a zone *has a layout*; only the standalone *library of*
layouts dies). REST URL nouns: `/scenes/.../zones` already says "zones";
`/api/v1/layouts` is removed (§5.3), not renamed.

Spec 64's naming section anticipated this exactly: *"If `DeviceZone` is
renamed later … the internal type could become `Zone`."* `DeviceZone` →
`Output` removes the collision that blocked `RenderGroup` → `Zone`. This
plan executes the rename Spec 64 deferred.

### 3.1 The rename is a Rust-identifier rename, not a wire change

**Critical distinction, since the Codex review found the first draft
conflated the two.** The rename (§6, Phase 3) renames **Rust type
identifiers only**. It does **not** rename any serialized name:

- A type name (`struct RenderGroup`) never appears in serde JSON, so
  renaming the struct is wire-invisible.
- A serialized **field** name (`Scene.groups`, `SpatialLayout.zones`,
  `DeviceZone.zone_name`, `DeviceZone.attachment`,
  `SpaceDefinition.zone_ids`) **is** on the wire. The rename does **not
  touch field names.** Where a Rust field identifier would naturally
  follow a type rename, the wire name is pinned with
  `#[serde(rename = "…")]` so the persisted bytes never change.
- An enum **variant** wire string (`ZoneRole`'s `custom`/`primary`/
  `display`, and the variants of `LedTopology`, `ZoneShape`,
  `SamplingMode`, `LayerSource`, the event and runtime-session enums)
  **is** on the wire. The rename freezes every one of them.
- `RenderGroup` has a **hand-written** `Serialize`/`Deserialize`
  (mirroring legacy effect fields). It gets the same freeze audit as a
  derived impl.
- The component templates and device profiles in `attachment.rs` are
  **user-authored / persisted TOML** — `AttachmentTemplateManifest`
  flattens `AttachmentTemplate`. Renaming the structs is fine; their
  TOML field names are pinned exactly as the JSON fields are.

The result of that discipline: a Rust-only rename, no persisted-data
migration, no API-contract break, no version bump. If the team later
wants the *wire* to also read `zone`/`output`/`component` (a cleaner
persisted format), that is a separate, versioned migration — explicitly
**out of scope** for this plan. §6 Phase 3 specifies the audit and the
fixture tests that enforce this.

---

## 4. Gap Analysis — Where the Code Is vs the Model

From a four-sweep investigation (data model, API, naming blast radius,
UI structure) and the Codex review.

### 4.1 The data model is already correct

`Scene` → `RenderGroup` (each owning a `SpatialLayout` and a
`Vec<SceneLayer>`) → `DeviceZone` is exactly the locked model. **No
type-model surgery is needed.** Spec 64 §8.1: *"Each `RenderGroup` owns
a `SpatialLayout`. A device's placement is a `DeviceZone` inside exactly
one zone's `layout.zones`."* This plan adds **zero** new domain types.

### 4.2 Gap A — the canvas edits the wrong object

The Studio canvas (`LayoutWorkspace` / `LayoutEditorState` in
`components/layout_builder.rs`) persists entirely to the standalone
`/api/v1/layouts` **library**. The Stage header's picker
(`StageLayoutBar`, labelled "Room") is the library picker. The canvas
does **not** edit the selected zone's `RenderGroup.layout`. The only
bridge — `apply_layout_update` → `sync_primary_group_layout` — bails out
when the scene has any `Custom` zone and only ever writes `Primary`. So
in a multi-zone scene the canvas saves to nothing, and switching zones
never changes the canvas. Spec 64 §8.4 already specifies the opposite
("the layout editor is zone-scoped"); the implementation never did it.

### 4.3 Gap B — no API to edit a zone's layout

There is no endpoint that mutates a `RenderGroup.layout` directly.
`assign_devices` moves whole `DeviceZone`s between zones (resetting
placement, by design); `unassign_device` removes one. But there is **no
route to edit an `Output`'s position / size / rotation within a zone's
layout, nor to set a zone's canvas dimensions.** Editing device
placement inside a zone — the core thing the canvas must do — has no
API. §5.1 adds it.

### 4.4 Gap C — the layouts-library removal blast radius

`api/layouts.rs` and the `state.layouts` store own more than CRUD
routes: the global "active layout," `layout_auto_exclusions` (hide/show
state, keyed by library layout id), discovery connectivity sync
triggered by layout apply, runtime-session persistence, and global
`SpatialEngine` replacement. Removing the library is not "delete the
routes"; every one of those behaviors needs an explicit replacement or
a compat shim (§5.3).

### 4.5 The UX gaps

- **Naming.** "Room" (5 sites in `stage.rs`), "All Lights" / "Lights"
  (`surface.rs`, `zone_tree.rs`), "Zones & Devices" (`zone_tree.rs`,
  `mod.rs`). Internally: `RenderGroup` (520 refs / 61 files),
  `DeviceZone` (126 / 65), `Attachment` (420 / 44).
- **Dead device cards.** `StudioDeviceCard` is a read-only selector
  tile. The palette card (`layout_palette/devices.rs`) has add-to-canvas,
  remove, hide/show, per-channel rows, and live component (attachment)
  data. None of it is in the tree card. Identify exists on neither card
  (it lives in `device_detail.rs`) and must be added.
- **No scene selector.** Nothing in the UI lists or switches scenes.
- **No Studio page header.** Other pages use `PageHeader`; Studio does
  not.
- **Effect-apply surfaces are not zone-aware.** Dashboard favorites, the
  sidebar command palette, and shell prev/next all call the
  Primary-only `effects/apply`.

---

## 5. The Daemon Additions (Spec 64 Extension)

Spec 65 is "UI-crate-only" and Spec 65 §12.1 states "Studio Waves 1-8
add no daemon endpoints." This plan is **not** UI-only — Gap B and
Gap C force daemon work. It is a focused extension of Spec 64's zone
API, in its established style (`If-Match` on `groups_revision`,
`{data, meta}` envelope, `412` on mismatch). §7 records the Spec 65
amendment this forces.

### 5.1 Per-zone layout editing

```
PUT  /api/v1/scenes/{id}/zones/{zone_id}/layout
        Placement-only update of a zone's SpatialLayout — the position,
        size, rotation, scale of the outputs it ALREADY owns, plus the
        zone's canvas dimensions and sampling defaults.
        Body: SpatialLayout.  If-Match: groups_revision.
```

**Placement-only enforcement (Codex CRITICAL finding).** A
whole-`SpatialLayout` replace could add foreign outputs, drop outputs,
re-bind an output's `device_id`, rewrite `topology` / `led_mapping`, or
move an output between zones — bypassing the exclusivity invariant
`assign_device_zone` maintains. So `update_zone_layout` is **a
placement merge, not a replace**:

- The set of `Output` ids in the request must equal the set the zone
  currently owns, or the daemon rejects with `422`. Adds and drops go
  **only** through `POST/DELETE .../zones/{zone_id}/devices`.
- For each output, only placement / visual fields are taken from the
  request — `position`, `size`, `rotation`, `scale`, `display_order`,
  `orientation`, `shape`, `shape_preset`, `sampling_mode`,
  `edge_behavior`, `brightness`, `name`. The identity fields (`id`,
  `device_id`, `zone_name`) and the hardware-binding fields
  (`topology`, `led_mapping`, `attachment` / `component`) are
  **preserved from the stored output**, never read from the request.
  No request can re-bind hardware or change LED topology — those route
  through device / component config.
- Layout-level canvas dimensions and sampling defaults are mutable;
  the layout's own identity is preserved.

On success it validates scene-wide exclusivity, bumps `groups_revision`,
and calls `refresh_active_render_groups` (Spec 64 §7.1). `SceneManager`
gains `update_zone_layout(scene_id, zone_id, layout)` enforcing all of
the above, with a test that a request mutating `device_id` or the
output-id set is rejected or ignored.

**Live drag preview (Codex CRITICAL finding).** The canvas needs a
live, non-persisting preview while a device is dragged. It **cannot**
reuse `/layouts/active/preview`: that path mutates the *global*
`SpatialEngine`, calls `sync_primary_group_layout`, and queues global
`ReplaceLayout`/`ResizeCanvas` — it is not zone-scoped and not
non-destructive. Instead, B1 adds a **per-zone preview override** in the
render runtime: a transient layout the render thread uses for one zone
only, for the duration of a drag, with explicit commit / cancel and
**no** mutation of the global engine or any other zone. The render
thread already holds a per-zone `SpatialEngine` (Spec 64 §12.1), so the
override is a per-zone slot, not a new engine.

The transport is **locked to the inbound WS protocol**, not a REST
route: a drag fires many updates per second, and the daemon already
has a client-to-server `ClientMessage` enum (`api/ws/protocol.rs`)
decoded in `api/ws/session.rs`. B1 adds two variants —
`ZoneLayoutPreview { scene_id, zone_id, layout }` sets the per-zone
override, `ZoneLayoutPreviewClear { scene_id, zone_id }` drops it.
Commit is the ordinary `PUT .../zones/{zone_id}/layout`, which persists
and clears the override; cancel is a clear message; a socket
disconnect mid-drag auto-clears every override that session held, so a
dropped connection never strands a zone on a transient layout. A REST
`.../layout/preview` route is explicitly rejected — a per-mouse-move
HTTP request is the wrong shape for a hot path.

A new capability, `zone-layout-edit`, is advertised once these are live.

### 5.2 The canonical output-roster builder

With no layouts library, a fresh `Primary` zone (a new default scene,
or `effects/apply` constructing `Primary`) can no longer "resolve the
full active layout" from the library. `install_default_scene` starts
with `groups: Vec::new()` and `upsert_primary_group` takes a
`full_scope_layout`. B1 defines an explicit **output-roster builder**:
given the device registry and component (attachment) profiles, it
enumerates every discovered device output into a `SpatialLayout` with
default placement. This single builder feeds **every** path that needs
a fresh `Primary`: `install_default_scene`; `POST /scenes`
(`create_scene` today writes `groups: Vec::new()`, so a user-created
scene currently has no zone for Studio to select — B1 fixes this by
seeding a Default zone); the first `effects/apply` on a scene with no
`Primary`; and recovery when `Primary` is missing. It replaces every
"resolve the active library layout" call.

### 5.3 Removing the layouts library — staged, side effects mapped

`api/layouts.rs` + `state.layouts` own more than CRUD. Each behavior
gets an explicit replacement before any route is deleted:

| Library behavior | Replacement |
| --- | --- |
| list / get / create / update / delete a layout | Per-zone layout API (§5.1) + scene CRUD; a saved arrangement is a scene |
| `GET /layouts/active`, the global "active layout" | No global active layout — each zone owns one; the render thread already samples per-zone (Spec 64 §12.1) |
| `POST /layouts/{id}/apply` → `apply_layout_update` → `sync_primary_group_layout` | `update_zone_layout` targets a named zone; the Primary-only single-zone special case is removed |
| `layout_auto_exclusions` — discovery-reconciliation memory (whole-device "do not auto-re-add", keyed by library layout id) | Re-keyed to the zone `(scene_id, zone_id)`; stays whole-device, since discovery reconciles devices, not outputs. A real migration, not a drop. **Not** the device-card hide/show (§8) |
| discovery connectivity sync (triggered by layout apply) | Triggered by `update_zone_layout` and device assignment instead |
| runtime-session persistence of the active layout | The default scene's zones already persist via the scene store; the library's separate persistence is dropped |
| global `SpatialEngine` replacement on apply | Per-zone layout updates feed the per-zone engines |
| `upsert_primary_group`'s `full_scope_layout` source | The §5.2 output-roster builder |

**Removal is staged and the daemon routes are soak-gated.** The Studio
canvas stops *consuming* the library in Phase 2 (B2). But the standalone
`/layout` page also consumes it, and `/layout`-page deletion is
soak-gated by existing project policy (Spec 65 Wave 8). So the daemon
`/api/v1/layouts` routes and the UI `api/layouts.rs` client are removed
**only** in that soak-gated cleanup — folded into Spec 65 Wave 8, not
done in this plan's active waves. Until then the routes sit unused by
Studio and harmless. This keeps the soak's rollback path intact (Codex
finding) — the user-facing "layouts list" is gone the moment the scene
selector ships; the dead plumbing is swept on the existing soak
schedule.

`profiles::apply_profile_snapshot`, the MCP `set_effect`/`set_color`
tools, and `api/openapi.rs` all route through the apply path; Spec 64
§9.6 already moved them to the shared zone-preserving helper. B1
enumerates the exact change to each — the helper's layout *source*
becomes §5.2's builder — with a test per path (this plan does not
assume them "unaffected").

### 5.4 Ownership

Spec 65 records the division "Codex implements the backend (Spec 64),
Claude implements the frontend (Spec 65)." §5.1–5.3 are backend work.
The plan specifies them fully so either track can execute them; **who**
executes the daemon waves is a coordination call for the owner.

---

## 6. The Plan — Phases and Waves

Each wave is one commit (or a tight commit series), independently sound:
it compiles, tests pass, and the UI is never left half-wired. Per-wave
gates: `just verify` (workspace) or wasm `cargo check` + `just ui-test`
+ `just ui-build` (UI), and `agent-browser` at `:9430` for visible
waves.

### Phase 1 — Lock the user-facing naming

**Wave A1 — User-facing strings + Studio chrome.** UI-crate only, ~1
day, zero risk.
- "All Lights" → "Default zone" (`surface.rs`); the default zone is
  renameable like any zone.
- The tree's "Lights" section header → "Zones"; "No lights in this
  scene" → "No zones."
- Drop the "Zones & Devices" column label; add a real
  `<PageHeader title="Studio">` matching every other page.
- The narrow-viewport drawer button relabels accordingly.
- *Not* touched here: the "Room" strings in `StageLayoutBar` — that
  component is replaced wholesale by the scene selector in Wave B2.

### Phase 2 — Correct the architecture

**Wave B1 — Daemon: per-zone layout API.** §5.1 + §5.2 + the §5.3
apply-path rework. The `PUT .../zones/{zone_id}/layout` placement-merge
route, the two `ClientMessage` preview variants and the per-zone
preview override, `SceneManager::update_zone_layout`, the output-roster
builder wired into `install_default_scene` **and** `create_scene` (so
every scene is born with a Default zone), the `zone-layout-edit`
capability, and the enumerated profile / MCP / OpenAPI changes.
Backend-testable headless; no UI change. Does **not** delete
`/api/v1/layouts` (§5.3 — soak-gated).

**Wave B2 — UI: scene selector + zone-scoped canvas (one wave).** These
land **together** — a scene selector that switched scenes while the
canvas still edited the library would present scene ownership the writes
do not follow (Codex finding). Together:
- Add the missing scene-CRUD client wrappers (`api/scenes.rs` has only
  `fetch_active_scene` today). Replace `StageLayoutBar`'s "Room" picker
  with a **scene selector** in the Studio `PageHeader` — list scenes,
  activate via `POST /scenes/{id}/activate`, new / rename / delete. The
  "Room" vocabulary dies here.
- Rewire `LayoutEditorState` / `LayoutWorkspace` from the layouts-library
  client to the B1 per-zone API, scoped to `selected_surface_id`.
  Selecting a zone loads *that zone's* `RenderGroup.layout`; switching
  zones switches the canvas. Re-instate per-zone preview consumption on
  the `zone_preview` channel (the rework's Wave 5 deleted this WS
  plumbing — it returns here). Remove the cross-zone focus dimming
  (rework Wave 4): with each zone its own canvas, nothing to dim.

**Wave B3 — UI: zone-aware effect apply.** Today the dashboard
favorites, the sidebar command palette, and shell prev/next call the
Primary-only `effects/apply` through the global `EffectsContext`, and
Studio's selected zone is *local* state — there is no app-wide notion
of which zone an apply targets (Codex finding). B3 locks one: a shared
**apply-target** signal in a global context, defaulting to the active
scene's Default (`Primary`) zone. Studio's zone selection writes it;
the Effects page apply-target selector reads and writes it; the
dashboard, sidebar, and shell quick-apply surfaces read it. A quick
apply then always has a defined, visible target and never silently
hits Primary by accident. Sequenced here, right after the zone-scoped
canvas, so the whole app shares the selected-zone concept *before*
multi-zone editing is exercised in earnest (moved up from the original
"Phase 3"). The Devices page surfaces each device's zone membership.

**Wave B4 — UI: functional device cards.** Restore to `StudioDeviceCard`
the actions the palette card already implements: add output(s) to the
zone's layout (`assign_devices` with `New(Output)`), remove
(`unassign_device`), per-channel rows with live in-layout / hidden
state, and live component data. Add **identify** (the
`/devices/{id}/.../identify` route). The data contract is explicit
(§8): the full output roster comes from §5.2's builder, channel
metadata and component-binding state from the device registry and
component profiles. The per-output hide/show is the palette card's
`hidden_zones` model — a set of `Output` ids — re-keyed per zone (§8);
it is client UI state, **not** the daemon's `layout_auto_exclusions`
(that store is discovery reconciliation, §5.3). The palette card's
logic is the reference; the tree card becomes the single device
surface.

**Wave B5 — UI: zone create / switch UX.** Make zone creation and
switching unmistakable and Luminary-grade; fold in whatever the device-
card and canvas waves revealed as rough.

### Phase 3 — Lock the internal naming (the hardcore sweep)

**Wave P3 — Codebase-wide type rename.** Runs **last**, over the settled
structure (Codex finding: renaming ~150 files before the architecture is
proven churns code Phase 2 reshapes, and a mid-flight 150-file daemon
rename is a coordination hazard). By now Phase 2 has proven the model;
P3 is the final consistency pass.

The renames of §3. The method:
1. **Freeze audit.** Enumerate every `#[derive(Serialize/Deserialize)]`
   and hand-written serde impl in the rename set — `RenderGroup`'s
   hand-written impl, the `attachment.rs` TOML manifest structs, every
   enum's variant strings. Pin every serialized field name and variant
   string that a Rust rename would otherwise change, with
   `#[serde(rename)]`, **before** renaming.
2. **Rename types crate by crate** in dependency order (`hypercolor-types`
   → `core` → `hal` → `daemon` → `hypercolor-ui`), compiler-driven.
3. Update `tests/` and doc-comment prose in the same commits.
4. **Wire round-trip fixtures** — the gate. A pre-rename JSON scene
   file **and** a pre-rename component-template / device-profile TOML
   file load byte-identical and render unchanged after P3. Add these
   fixtures if absent.
5. `just verify` workspace-wide; `just ui-test` + `just ui-build`.

Coordination: P3 wants a quiet daemon window and lands as a tight,
rebase-friendly commit series.

### Deferred — folds into the soak-gated Spec 65 Wave 8 cleanup

Delete `pages/layout.rs`, the UI `api/layouts.rs` client, and the daemon
`/api/v1/layouts` routes + `state.layouts`. Soak-gated by existing
policy (§5.3). Studio stops using the library in B2; this is only the
dead-plumbing sweep.

---

## 7. Spec Amendments

This plan amends two shipped specs; amendments land with the waves that
implement them, never pre-emptively.

**Spec 64:**
- §8.4 — strike "the standalone layout library … is unchanged." Replace
  with the per-zone layout API (§5.1), the output-roster builder (§5.2),
  and the staged removal of `/api/v1/layouts` (§5.3).
- §9.5 — add the `zone-layout-edit` capability.
- §3.2 / Naming — record that `DeviceZone`→`Output` and
  `RenderGroup`→`Zone` are now executed (the section anticipated it).

**Spec 65 (the larger amendment — Codex finding: the first draft
understated this):**
- §4 — replace the vocabulary table with §3 of this plan. Strike
  "All Lights"; the default zone is "Default zone" always.
- §3.3 minimal-baseline — this plan **overrides** Spec 65 §3.3/§9.2's
  "single-zone users see *All Lights* and no zone vocabulary." The
  owner's decision is that "Zone" / "Default zone" is the vocabulary at
  every scale. Record it as an explicit product decision and update the
  §15.2 minimal-baseline checks.
- §6.3 — strike the Output/Layout Stage toggle (already removed); the
  Stage canvas edits the selected zone's layout via §5.1.
- §12.1 — strike "Studio Waves 1-8 add no daemon endpoints." This plan
  adds a daemon zone-layout API; §12.2's backend-dependency list gains
  `zone-layout-edit` and the §5.1–5.3 daemon work.
- §14 — the delivery-wave table is superseded for the Studio canvas by
  Phase 2 of this plan.

---

## 8. The Component / Channel Model and the Device-Card Data Contract

The owner flagged the code's "attachment" should be "component," and
described the device hierarchy: a device has **channels**; **components**
(strips, infinity fans, arbitrary LED areas) attach to channels.

Today this is implicit: a `DeviceZone` carries a `zone_name` (the
channel/segment id) and an optional `ZoneAttachment` (template id, slot,
LED range). There is no first-class `Channel` or `Component` type. Spec
66 (Component Designer) authors components and already uses the word.

**Scope here:** rename `Attachment*`→`Component*` (Phase 3) so code,
Spec 66, and the UI agree on the word, and surface live component data
on the device cards (B4). Promoting `Channel`/`Component` to first-class
types is a larger data-model change owned by Spec 66 — **out of scope**;
flagged so the boundary is explicit.

**The B4 device-card data contract (Codex finding — the first draft
hand-waved this).** A working device card needs, after the library is
gone:
- *All possible outputs for a device* — from §5.2's output-roster
  builder (device registry + component profiles), not the library.
- *Channel / segment metadata* — `Output.zone_name` and the device
  registry topology.
- *Component binding state* — the component (attachment) registry /
  profile store, fetched as the palette card's `attachment_cache` does.
- *In-zone state* — the zone's `layout.zones` membership.
- *Hidden state* — per-`Output`-id, the palette card's `hidden_zones`
  model. Today it persists in `LayoutPageState` localStorage keyed by
  library layout id (`components/layout_builder.rs`); B4 re-keys it to
  `(scene_id, zone_id)`. Whole-device hide is derived (every output of
  the device hidden), exactly as the palette card derives it. Client UI
  state — it does **not** touch the daemon's `layout_auto_exclusions`.
- *Identify* — the `/devices/{id}/.../identify` route.
B1 confirms each source exists or adds it; B4 consumes them.

---

## 9. What Changes From the Recent 5-Wave Rework

| Rework wave | Fate under this plan |
| --- | --- |
| W1 — rich device cards | Kept; made functional (B4). |
| W2 — `LayoutBuilder` split | Kept; `LayoutEditorState` rewired off the library (B2). |
| W3 — always-on canvas, one header | Kept. The `StageLayoutBar` "Room" picker is replaced by the scene selector (B2). |
| W4 — cross-zone focus dimming | Removed (B2) — moot once each zone is its own canvas. |
| W5 — collapsible assignment strip; zone-preview WS deletion | Assignment kept; the deleted `zone_preview` WS plumbing is re-instated for the per-zone canvas preview (B2). |

The two-column workspace and the always-on spatial canvas — the
structural wins of the rework — are kept. What changes is the *object
the canvas edits* and the *vocabulary*.

---

## 10. Risks and Coordination

- **The daemon work is real.** B1 (§5.1–5.3) is substantial backend
  work, not a UI tweak. §5.4 — the owner decides which track executes
  it.
- **P3 touches the daemon broadly.** It wants a quiet daemon window and
  lands as a tight, rebase-friendly commit series.
- **Wire-safety is P3's whole risk surface.** §3.1 + §6 Phase 3's
  freeze audit and JSON-and-TOML round-trip fixtures are the mitigation.
- **`layout_auto_exclusions` is a migration, not a drop** — it is
  discovery-reconciliation memory; the layouts-library removal must
  re-key it per zone, not delete it (§5.3). It is distinct from the
  device-card hide/show (§8).
- **The apply-target contract is a small product decision** — B3
  introduces an app-wide selected-zone signal defaulting to the Default
  zone, so a quick apply is never a silent guess.
- **The soak gate is respected.** The `/layout` page and the daemon
  `/api/v1/layouts` routes are removed only in the existing soak-gated
  Spec 65 Wave 8 cleanup, keeping the rollback path intact.

---

## 11. Verification

- **Per wave:** the gates of §6. Visible waves get `agent-browser`
  verification against the Luminary bar (Spec 65 §3.4).
- **Wave P3:** the §6 wire round-trip is mandatory and is the gate — a
  pre-rename JSON scene and a pre-rename component TOML load and render
  byte-identical after.
- **Phase 2:** a genuine multi-zone scene end to end — create a second
  zone, place devices in each zone's canvas, switch zones and confirm
  the canvas follows, drag-edit positions and confirm the preview is
  zone-scoped and the save persists, add / remove / identify from a
  device card, confirm `update_zone_layout` rejects an output-set
  change and ignores an attempted `device_id` rewrite.
- **Final pass:** an independent Codex review of the full branch diff,
  then the owner's intent checklist — no smart-home vocabulary anywhere,
  the canvas edits the selected zone, the scene selector works, device
  cards functional, a real page header, every effect-apply surface
  zone-aware.

---

## 12. Sequencing and Recommendation

Recommended order: **A1 → B1 → B2 → B3 → B4 → B5 → P3.** The
layouts-library plumbing removal is deferred to the soak-gated Spec 65
Wave 8 cleanup.

Rationale:
- **A1 first** — the smart-home vocabulary is what the owner sees daily;
  one low-risk day buys immediate relief.
- **B1 before B2** — the UI cannot rewire to an API that does not exist.
- **B2 is one wave** — scene selector and zone-scoped canvas land
  together; a scene selector over a library-bound canvas is half-wired.
- **B3 right after B2** — every effect-apply surface agrees with the
  selected-zone concept before multi-zone editing is exercised.
- **P3 last** — the rename is mechanical and wire-safe; running it over
  the *settled* Phase-2 structure avoids churning code mid-restructure
  and avoids a mid-flight 150-file daemon rename. Phase 2's new code is
  written in the current names and renamed by P3 by definition — purely
  mechanical, no judgment lost.
- **Library plumbing removal deferred** — soak-gated by existing policy;
  Studio is already off the library after B2.

This is a multi-week program, honestly — and B1 is genuine daemon work.
It is also the plan that makes the Studio one coherent thing: one model
(Scene → Zone → Layers + Layout), one vocabulary, one place a device's
placement lives, and a UI that edits what it claims to.

Build it, in the order above, after the confirmation review and owner
sign-off.
