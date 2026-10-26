+++
title = "Studio architecture"
description = "Developer view of Studio: StudioContext, the one target zone, the one Studio history, live layout commit, and the reused LayerPanel and LayoutWorkspace contracts."
weight = 110
+++

Studio is a Leptos workspace built from shared app-wide state plus a thin layer of page-local UI state. This page maps that split for developers working in `crates/hypercolor-ui/`: which context owns which signal, how every Studio write goes through one history, how canvas edits commit live, and how the reused `LayerPanel` and `LayoutWorkspace` contracts mount.

If you want the runtime wire protocol and the daemon REST surface behind these contracts, read the [zone API and concurrency](@/studio/zone-api-and-concurrency.md) page next. For the user-facing tour, start at the [Studio overview](@/studio/overview.md).

{% <callout type="info"> %}
This is a developer reference. The canonical wire type is `hypercolor_types::api::scene::SceneDocument`. The UI stores that document directly in its shared scene memo. Local surface helpers derive presentation state without creating another REST contract.
{% </callout> %}

## The state map

Studio reads from three app-root contexts and provides its own. The rule of thumb: anything that must survive navigation, stay fresh across clients, or be addressable from another page lives at the app root; anything that is purely Studio's view of the moment lives in a context `StudioPage` provides, or in a provider scoped to the Stage.

| Context | Defined in | Lifetime | Owns |
| --- | --- | --- | --- |
| `ZonesContext` | `zones.rs` | App root | The shared active scene, the zone lists, the target zone (`focused_zone`) |
| `ScenesContext` | `zones.rs` | App root | The scene library, switching and activation state |
| `EffectsContext` | `app.rs` | App root | Per-zone effect state, active-effect signals, and the apply path that lands in the target zone |
| `StudioContext` | `pages/studio/mod.rs` | Studio page | Tab selection, slide-over state, pane state, rail highlight and hidden-output UI state, the rig layout |
| `StudioHistory` | `pages/studio/history.rs`, `history/zones.rs` | Studio page | The undo journal and the queue every membership, zone-structure, and layout write goes through |
| `DragController` | `pages/studio/drag.rs` | Studio page | The one pointer drag between the pane, the tabs, and the canvas, the **Move to** menu, and the move announcements |
| `LayoutEditorContext` / `ZoneCanvasActions` | `components/layout_builder.rs` | The Stage | The in-flight `SpatialLayout`, selection, undo and redo, Fit |

### Shared state at the app root

The live scene is one resource for the whole app. `api::fetch_active_scene` reads the canonical `SceneDocument` from `GET /api/v1/scene`. The writer's and history's recovery reads go through `fetch_active_scene_promptly`, which gives up after `SCENE_READ_DEADLINE` (10 s) so a stalled daemon cannot hold a recovery up; the shared resource waits, keeping the document it has. `provide_scene_contexts()` in `zones.rs` exposes that document as one shared memo on both `ZonesContext` and `ScenesContext`. Derived memos build surface lists and presentation state from the same document and its single `revision`.

WebSocket scene events are freshness hints for the REST snapshot. Structural scene events refetch `GET /scene`; control-only events do not force a whole-document fetch at slider frequency. `EffectControlChanged` identifies the affected zone and real layer for consumers that update control state directly. `daemon_resource` also reads `connection_generation`, so a socket reconnect performs a fresh REST read instead of assuming missed events will be replayed.

Because the scene projection is shared and WebSocket-fresh, a structural change made from another page, another client, or the CLI lands in Studio with no page-local scene resource. There are no Studio-local active-scene snapshots to drift from `GET /scene`.

`ZonesContext` derives the rest with memos, so a refetch returning identical state does not wake every zone-aware surface in the app:

- `zones`: every surface of the active scene in scene order (LED zones and display Screens).
- `led_zones`: LED-role zones only; what effect application targets.
- `multi_zone`: whether the scene has more than one LED zone. This is the trigger for every per-zone affordance outside Studio.
- `scene_resolved`: whether the active scene has been read for this connection, so `multi_zone` describes the scene rather than an empty page still loading.
- `focused_zone: RwSignal<Option<String>>`: the target zone.

### One target zone

`ZonesContext::focused_zone` is the only record of which LED zone quick actions target. `ZonesContext::target_zone()` resolves it: the focused LED zone when it is still in the scene, otherwise the primary zone. `None`, a stale id, and a Screen id all resolve to the primary zone. `surface::apply_zone_id` turns the target into the zone an effect apply names; the primary zone goes unnamed and keeps the daemon's default apply path.

Three writers move it:

- Studio's selection effect writes it whenever the open tab is an LED zone. Opening the Screens tab leaves it alone.
- `CompactZoneTabs` (`components/zone_tabs.rs`) writes it from the preview cabinet on the Dashboard and Effects page. The tabs hide while `multi_zone` is false.
- Nothing else. There is no "all zones" target and no second signal to keep in step.

`EffectsContext::apply_effect` applies to the target zone. The command palette names the target zone next to its search field in a multi-zone scene.

### StudioContext: the page-local view

`StudioContext` (`pages/studio/mod.rs`) holds what is specifically Studio's, provided once from `StudioPage` so the tab strip, the pane, and the Stage read one source of truth. The shared active scene is re-exposed here as `active_scene`, held through a failed read (`history::held_document`): a failed read is no news about the scene, so a drag under way and the selection go on as they were. Everything else is page-local:

| Field | Type | Purpose |
| --- | --- | --- |
| `history` | `StudioHistory` | The page's undo journal and write queue (also provided as its own context) |
| `selected_surface_id` | `RwSignal<Option<String>>` | The open tab's zone, or the selected Screen; the single selection source |
| `active_scene` | `Signal<Option<SceneDocument>>` | Re-exposed shared active-scene document, the last one read through a failed read |
| `refresh_scene` | `Callback<()>` | Re-fetch the shared active scene after a zone mutation |
| `composition_open` | `RwSignal<bool>` | Whether the composition slide-over is open |
| `screen_face`, `refresh_screen_face` | `Signal<Option<DisplayFaceResponse>>`, `Callback<()>` | The selected Screen's live face and its refetch |
| `hidden_outputs` | `RwSignal<HashMap<String, HashSet<String>>>` | Per-`(scene, zone)` hidden-output sets, client UI state only |
| `selected_output_ids` | `RwSignal<HashSet<String>>` | Pane click selection bridged into the canvas |
| `hovered_output_ids` | `RwSignal<HashSet<String>>` | Pane hover highlight bridged into the canvas |
| `pointer_output_id` | `RwSignal<Option<String>>` | The canvas box under the pointer, mirrored back so the pane can highlight the owning card |
| `attachment_cache` | `RwSignal<HashMap<String, Vec<ComponentBindingSummary>>>` | Per-device component-binding cache the cards fill lazily and search reads |
| `screen_labels` | `Memo<HashMap<String, String>>` | Screen labels that tell identical device names apart ("TL LCD 2") |
| `render_canvas_size` | `Memo<(u32, u32)>` | The daemon's render canvas extent, read by anything fitting a footprint to the canvas |
| `rig_layout`, `refresh_rig_layout` | `Signal<Option<SpatialLayout>>`, `Callback<()>` | The active named layout, read once for Studio; arriving outputs are placed from it |
| `rail_disclosure` | `RwSignal<Option<String>>` | The single open-disclosure slot shared by the device cards' menus |
| `zone_rename_draft` | `RwSignal<Option<(String, String)>>` | An in-flight tab rename as `(zone id, draft text)` |
| `pane_expanded` | `RwSignal<HashSet<String>>` | Which Other zones rows are expanded |
| `pane_auto_expanded` | `RwSignal<Option<(String, HashSet<String>)>>` | The rows a new zone opened on its own, closed again when it gains a device or its tab is left |
| `undo_notice` | `RwSignal<Option<UndoNotice>>` | The Undo bar's offer to restore a deleted zone |
| `pane_drawer` | `RwSignal<bool>` | Whether the narrow-viewport device drawer is open |

`rail_disclosure` and `zone_rename_draft` exist because the pane and the tab strip rebuild on every scene event and every search keystroke. Disclosure state and in-flight text have to outlive the DOM that renders them, or a menu snaps shut and a half-typed name vanishes mid-edit. One shared disclosure slot also means opening a menu closes the previous one.

Two more details are easy to get wrong. The hidden-output state is keyed `(scene_id, zone_id)` through `hidden_outputs_storage_key` and persisted to `localStorage`. It is purely client UI state and is never mirrored to the daemon's `layout_auto_exclusions`, which is discovery-reconciliation memory and a different concept entirely. And the highlight signals (`selected_output_ids`, `hovered_output_ids`) clear on every real tab switch, so a stale highlight from the previous zone never lingers on the new one.

## How selection drives everything

Tab selection is the spine of the page. `selected_surface_id` lives in `StudioContext`, the tab strip and the Screens list write it, and the rest of the page reacts to it.

{% <mermaid> %}
flowchart TD
    A[active_scene memo] --> B[Selection-guard effect]
    B --> C[selected_surface_id]
    C --> D[target-zone effect]
    C --> E[layers_resource]
    C --> F[ZoneLayoutProvider]
    C --> P[DevicePane sections]
    D --> G[ZonesContext.focused_zone]
    E --> H[LayerPanel in the slide-over]
    F --> I[Stage canvas]
{% </mermaid> %}

The selection-guard effect keeps `selected_surface_id` pointing at a still-present zone. A fresh selection opens the target zone when it is an LED zone of the scene, otherwise the first LED zone, so Studio always opens on a Light and opens on the zone the rest of the app was pointed at:

```rust
let next = led_zones()
    .find(|zone| focused.as_deref() == Some(zone.id.to_string().as_str()))
    .or_else(|| led_zones().next())
    .or_else(|| scene.zones.first())
    .map(|zone| zone.id.to_string());
selected_surface_id.set(next);
```

`layers_resource` projects the open zone's stack straight out of the shared document instead of fetching it again.

## Leptos-free models

The rules behind the tab strip, the pane, placement, and moves live in Leptos-free modules, exercised from `crates/hypercolor-ui/tests/` without a WASM toolchain. Change the rule there and the test there.

| Module | Rules |
| --- | --- |
| `pages/studio/tab_model.rs` | Which tabs fit and which overflow, reorder targets and the full-order `index`, keyboard stepping, delete eligibility and prompts, Screen labels |
| `pages/studio/pane_model.rs` | The pane's sections, split-device hints, search and hit counts, when Unassigned shows, the policy select |
| `pages/studio/placement.rs` | Where arrivals land from the rig, the first-arrival fit, and Fit |
| `pages/studio/drag_model.rs` | What a drag carries, what each drop target does with it, spring-loaded tabs, announcements, names for zones made from a device |
| `pages/studio/history.rs`, `history/zones.rs` | The journal, replay preparation, zone structure steps, and the order waiting operations run in |
| `components/layout_builder/ledger.rs` | The per-zone layout ledger: confirmed state, local intents, and how each write outcome settles them |
| `components/layout_builder/gesture.rs` | Which placement fields a finished drag or resize carries into the editor |
| `components/layout_canvas/{picking,labels}.rs` | Smallest-box hit testing with Alt cycling, and when a box names itself |
| `zones/surface.rs` | `zone_display_name`, layer labels, the target-zone rule |

## One history for every Studio write

Every write that changes membership, changes zone structure, or replays a layout goes through `StudioHistory`. The scene revision fences each write, so one operation runs at a time:

- `busy` marks a membership write, a zone operation, or a layout replay in flight. The Stage canvas is `inert` while it holds.
- `committing` marks the layout writer holding the scene for a write. Editing carries on.
- An operation asked for while either holds waits its turn in order instead of being dropped. Pending layout writes always go first, so an operation acts on a scene with every finished edit already written.

The journal records five kinds of step:

- `StudioEdit::Layout`: a zone's editor snapshot before and after.
- `StudioEdit::Members`: the canonical `MemberEdit` receipts `POST /scene/members/edit` returns.
- `StudioEdit::ZoneCreated`: a zone Studio made, together with the move that filled it when it came from **Move to** ▸ **A new zone**, so the pair undoes as one step.
- `StudioEdit::ZonePatched`: a rename, recolor, pause or resume, or reorder, as the changed fields were and became.
- `StudioEdit::ZoneDeleted`: the `DeletedZone` record of a delete.

Undo replays a members step by swapping each receipt's `before` and `after`, rebased on the live scene; an output still where the step left it returns to the placement the step recorded. Undoing a `ZoneCreated` moves its devices back and deletes the zone, unless the zone has gained devices or layers since: then it keeps the zone, toasts "Kept" and the name, and records only the move it reversed. A restore or redo that makes a zone again under a new id remaps every journal entry that named the old one. Each undo and redo announces what it did through the drag controller's live region (`history/announce.rs`), for example "Undid: moved X back to Main". An undo or redo waits for a canvas gesture to end, and one that finds a newer edit recorded since it was asked for stands down rather than reversing the wrong thing. The journal resets when the active scene changes.

A delete releases the zone's outputs through one membership edit, keeping their complete snapshots, then deletes the empty zone. Its `DeletedZone` record also keeps the zone's name, color, paused state, brightness, `index`, and layer stack (not its description). A restore rebuilds the zone, puts back the outputs that are still unclaimed, recreates the layers bottom to top, re-pauses or re-dims it, and moves it back to its `index`. The Undo bar (`undo_notice.rs`) and the journal entry share the record's `token`: the bar undoes normally when the delete is the latest step, otherwise it restores and drops the entry, and a deletion restored once is never restored again. Zone create and patches go through `StudioHistory::create_zone`, `create_zone_filled`, and `patch_zone`, called from `zone_controls.rs` and `moves.rs`.

## The reused contracts

Two pieces of Studio are shared components mounted with a fixed contract, so the editor for a thing exists exactly once and cannot drift between pages.

### LayerPanel

`LayerPanel` (`components/layer_panel/mod.rs`) is the single layer-stack editor. Studio's composition slide-over is its only mount today; the mount contract keeps it host-agnostic, small, and deliberate:

- **Zone identity**: the shared active-scene projection plus the selected zone id identify the live stack. Structural routes need the zone id, and layer-specific routes use the real layer id embedded by `GET /scene`.
- **Scene revision**: `layers_resource` extracts the selected zone's stack and the enclosing document `revision`. Structural layer writes may send that value as `If-Match`.
- **One mutation callback**: `on_layers_mutated: Callback<()>` fires after every applied or rejected mutation; the host refetches the active scene in response. There is exactly one.
- **Internal content selection**: the asset list and effect-name resolution are owned inside the panel, so it is decoupled from any host page's selection state.

Studio passes a `surface_label`, which tells the panel to show the open tab's name in its header and drop its own zone selector. The tab strip already owns selection.

New layers land on the open tab. The only multi-target add is the Screens tab's **All screens**, offered when the scene has more than one screen.

One display detail worth knowing when you read the code: layers are authored bottom-to-top, but the row list is reversed for display so "Top" reads first. The Top and Bottom stack markers only show with more than one layer.

### LayoutWorkspace and its provider

`LayoutWorkspace` and `LayoutCanvas` (`components/layout_builder.rs`, `components/layout_canvas.rs`) form the single spatial editor. Studio's Stage is its only mount.

`ZoneLayoutProvider` is the ownership seam. Studio's Stage wraps the editor in this provider, which shows the open zone from its layout ledger and turns each finished edit into ledger intents for the layout writer.

```rust
<ZoneLayoutProvider
    selected_zone_id=selected_surface_id
    refresh_scene=refresh_scene
>
    <Stage />
</ZoneLayoutProvider>
```

`ZoneLayoutProvider` provides three contexts down to the Stage:

- `LayoutEditorContext`: the editor's working state: the in-flight `Signal<Option<SpatialLayout>>`, selection sets, hover sets, compound depth, the `LayoutWriteHandle`, and `can_undo` and `can_redo` (read from `StudioHistory`).
- `LayoutZoneDisplayContext`: the per-device attachment profiles resource.
- `ZoneCanvasActions`: `fit`, `can_fit`, and `has_layout`, consumed by the Stage header's **Fit** button.

The provider feeds every new scene read to the ledgers, so a refetch never clobbers an edit in flight. It also lends the writer `WriterHooks` and connects the gesture state to `StudioHistory`, which is how a write takes its turn with Studio's other operations and how a waiting undo knows to hold for a drag.

{% <callout type="tip"> %}
The drag and resize hot path is deliberately non-reactive. A single requestAnimationFrame scheduler paints positions directly to cached DOM elements, and the layout signal is written once on `pointerup`. Live drag preview goes to the daemon over the outbound WebSocket as JSON messages typed `zone_layout_preview` and `zone_layout_preview_clear` (sent by `send_zone_layout_preview` and `send_zone_layout_preview_clear` in `ws/preview.rs`), throttled to `PREVIEW_PUSH_INTERVAL_MS = 75.0`. It is not a REST route and does not touch the global `SpatialEngine`. See [zone API and concurrency](@/studio/zone-api-and-concurrency.md) for the full hot path.
{% </callout> %}

## Live layout commit

There is no Save. `LayoutWriteHandle` records every finished edit (a released drag, resize, or slider, a property field's `change` event on Enter or blur, Fit) in `StudioHistory`. A drag or resize carries only the placement fields it changed into the editor as the editor stands then (`gesture.rs`), so a change that arrived mid-gesture is never put back. The edit then becomes intents in the zone's ledger.

The ledger (`components/layout_builder/ledger.rs`) keeps two things per zone, and its module doc is the contract:

- **Confirmed**: the zone as the daemon last reported it, with that report's scene revision. A scene read replaces it when its revision is at least as new; a write's reply only when it is newer.
- **Pending intents**: the local edits the daemon has not confirmed, in order. Each names one output, one placement field, and the value the user gave it. An edit adds intents only for the fields it changed, so a field nobody touched never holds one.

The canvas shows confirmed with every pending intent applied in order. A write carries every pending intent at its latest value, fenced at confirmed's revision; fields without intents go out as the daemon stores them, which under the fence changes nothing. One write is out at a time, and its outcome settles it:

- **Applied**: the intents it carried retire, and the reply's zone is adopted when its revision is newer.
- **Stale**: nothing retires. The zone is read afresh, any field whose fresh value already equals its latest intent drops its intents, and the rest goes out again.
- **Unknown** (no answer came back): the same write goes out again under the same `Idempotency-Key`, with the same body and headers, after 250 ms, 1 s, and 3 s, without holding the scene while it waits. A write with no answer within `KEYED_WRITE_DEADLINE` (10 s) is given up on and its browser request aborted, and a `503` with code `service_unavailable` (the daemon cannot take the write right now, as when too many keyed writes are in flight, and kept nothing under its key) counts as no answer too. The daemon applies a key at most once and answers a repeat with its first answer, waiting for the first request when that one is still under way, so a lost request that commits late is never applied twice and never followed by a resend that would undo a later remote write. Whatever comes back settles the write as above. When four tries get no answer the writer pauses with the edit still pending, and the next socket opening or edit sends it again under the same key.

Between clients the daemon's order decides: a remote write that lands after ours wins, and a local intent that reaches the daemon later wins. Undo and redo are edits like any other and make intents the same way.

Every read and reply names the daemon run it came from: scene documents carry `run_epoch`, write replies carry the `hypercolor-run-epoch` header, and the socket's `hello` names the run it reached (exposed as `WsContext::run_epoch`). The ledgers belong to one run. A socket that opens again on the same run changes nothing, and a write still waiting for an answer keeps going out under its key. When the hello names a new run, the daemon restarted, and every ledger re-anchors. A read or document of the old run is never taken in again, not even the shared document Studio still holds while its refetch is out. The first read of the new run becomes confirmed whatever its revision, and pending intents stay. No write goes out fenced at an old revision until that read arrives, and when an edit is waiting on it the writer reads the scene itself. A read of a run the socket has not named yet holds the writer until it does.

Keys mean nothing to a run that never saw them, so a write the old run left without an answer, or one that another run answered, is settled from a fresh read of the new run instead. The old run commits nothing more, so the read shows what the write did; the write is taken as landed, since resending it could undo a remote write that followed it, and a toast says so when the fresh zone does not hold what it carried.

Studio's history sends its own scene writes (membership edits, zone create, patch and delete, layer restores, the unassigned-device policy) the same way, each under a key of its own that its retries reuse. A membership edit, zone create, zone patch, or zone delete (the release of its devices included) that still gets no answer, or that a new run answers, may have landed or not, so it is never reported as failed. It holds its place in the journal, and undo and redo wait for it, until the connection is back. In the same run it goes out again under its key, and the answer says what it did and carries its receipt, which becomes the journal entry. In a new run the scene read decides what it can, since the old run can no longer commit it. A step inside a larger operation that gets no answer is reported as possibly saved, with a prompt to check the scene. History prefers a document it adopted over the shared one within one run, and across runs the one of the run the socket names.

The ledgers and the writer that sends them (`components/layout_builder/writer.rs`) live for the page, not for a Studio mount, in one queue for the active scene. An edit made just before Studio closes still lands, and a Studio opened again reads the ledgers its predecessor left. Selecting another scene drops them. While a Studio canvas is mounted it lends the writer `WriterHooks`, so each write claims the scene through `StudioHistory::begin_commit`, records what landed, and redraws the canvas. When a write is refused, the scene cannot be read to settle one, three stale rejections come in a row, or four tries get no answer, the writer pauses with a toast instead of retrying in a loop; a new edit or a reopened socket resumes it. The ledger's rules are tested from `tests/studio_layout_ledger_tests.rs`.

The wire target is `PUT /scene/zones/{zone}/layout`. The write updates member placements while the daemon preserves member identity and topology; membership changes go through `POST /scene/members/edit` instead. Full route semantics are on the [zone API and concurrency](@/studio/zone-api-and-concurrency.md) page.

## Optimistic concurrency

Studio has one optimistic-concurrency token: `SceneDocument.revision`. Zone, membership, layout, and structural layer mutations send it in `If-Match`; the atomic membership edit requires it. A stale structural write returns the canonical 412 response, which the client maps to a `Stale` outcome before reloading the scene. Control-value patches never send `If-Match`; they apply in commit order against the real layer id.

### Layer mutations

`LayerPanel` threads the scene revision through structural layer writes. The outcome type is the discriminator:

```rust
match api::update_layer(&zone_id, &layer_id, &request, Some(revision)).await {
    Ok(api::LayerStackOutcome::Applied(_)) => on_layers_mutated.run(()),
    Ok(api::LayerStackOutcome::Stale { .. }) => {
        on_layers_mutated.run(());
        toasts::toast_error("Layer stack changed elsewhere; reloaded");
    }
    Err(error) => toasts::toast_error(&format!("Layer update failed: {error}")),
}
```

`LayerStackOutcome::Stale` is a refetch-and-retry signal, not an error and not a clobber. The same pattern covers `delete_layer` and `reorder_layer`.

Adding a layer to every screen carries the document revision across the targets. Each applied structural write returns a projected stack carrying the next revision, which becomes the precondition for the next target. The loop stops on the first stale or failed write.

Live control editing is separate. `patch_layer_controls` sends `PatchControlsRequest` to `/scene/zones/{zone}/layers/{layer}/controls` without a revision header. The layer id comes from the current document; replacing a layer retires that id, so a late control patch cannot land on its replacement.

### Membership and zone writes

Device adds, moves, unassigns, and their undo and redo all go through `POST /scene/members/edit`. An add from Unassigned sends the `assignment` form with `placements` the UI computed from the rig (`placement::assignment_hints`); a move sends explicit `changes` whose `after` states already carry the arrival geometry from `placement.rs`. Zone create posts to `/scene/zones`, and rename, color, pause, and reorder `PATCH /scene/zones/{zone}`, reordering through `index` against the scene's full zone order (`tab_model::zone_move_index`). A delete's restore also recreates the zone's layers through the layer routes. While Studio is open it lends `ZonesContext::studio_zone_patch` its history's `patch_zone`, so the sidebar's per-zone pause is an undo step there; on other pages the sidebar writes directly. The Unassigned policy waits in the same queue but records no undo step.

## Capability gating

Multi-zone affordances do not probe. They gate on named capabilities the daemon advertises in `GET /api/v1/system` under `status.capabilities`, exposed through `CapabilitiesContext` (`app.rs`). An absent advertisement means the affordance stays hidden, with no fallback.

```rust
pub fn zone_crud_ready(&self) -> bool {
    self.has("zone-crud")
        && self.has("multi-zone-sampling")
        && self.has("zone-device-assignment")
}
```

The **+** after the tabs and the **A new zone** move target need all three, because a user who can create a zone but cannot render it or move outputs into it would have an unusable zone. The Unassigned policy select gates separately on `scene-unassigned-behavior-write`.

{{< img path="img/ui/studio.webp" alt="The Hypercolor Studio workspace" />}}

## Where to read next

- [Zone API and concurrency](@/studio/zone-api-and-concurrency.md): the REST routes, the WebSocket preview protocol, and the full `If-Match` story.
- [Vocabulary and naming](@/studio/vocabulary-and-naming.md): the locked type names and the never-rooms rule.
- [Render pipeline](@/architecture/render-pipeline.md): how the composited canvas becomes LED color downstream of Studio.
