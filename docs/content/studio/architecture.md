+++
title = "Studio architecture"
description = "Developer view of Studio: StudioContext, shared-vs-local state, the reused LayerPanel and LayoutWorkspace contracts, and optimistic concurrency."
weight = 110
+++

Studio is a two-column Leptos workspace built from shared app-wide state plus a thin layer of page-local UI state. This page maps that split for developers working in `crates/hypercolor-ui/`: which context owns which signal, how the reused `LayerPanel` and `LayoutWorkspace` contracts mount, and how every mutation threads an optimistic-concurrency token so concurrent edits never clobber each other.

If you want the runtime wire protocol and the daemon REST surface behind these contracts, read the [zone API and concurrency](@/studio/zone-api-and-concurrency.md) page next. For the user-facing tour, start at the [Studio overview](@/studio/overview.md).

{% callout(type="info") %}
This is a developer reference. It names Rust types, signals, and file paths verbatim from `crates/hypercolor-ui/`. The client code already uses zone and `Output` vocabulary; some daemon-side types still carry the older `RenderGroup`/`DeviceZone` identifiers pending the Plan 55 Phase 3 rename. Check the crate you are editing before naming a type.
{% end %}

## The state map

Studio reads from three app-root contexts and owns one of its own. The rule of thumb: anything that must survive navigation, stay fresh across clients, or be addressable from another page lives at the app root; anything that is purely Studio's view of the moment lives in `StudioContext` or in a provider scoped to the Stage.

| Context | Defined in | Lifetime | Owns |
| --- | --- | --- | --- |
| `ZonesContext` | `zones.rs` | App root | The shared active scene, the zone lists, the focused zone |
| `ScenesContext` | `zones.rs` | App root | The scene library, switching/activation state |
| `EffectsContext` | `app.rs` | App root | The apply-target, per-zone effect state, active-effect signals |
| `StudioContext` | `pages/studio/mod.rs` | Studio page | Surface selection, slide-over state, rail highlight + hidden-output UI state |
| `LayoutEditorContext` / `ZoneCanvasActions` | `components/layout_builder.rs` | The Stage | The in-flight `SpatialLayout`, selection, undo history, save/revert |

### Shared state at the app root

The active scene is one resource for the whole app. `provide_scene_contexts()` in `zones.rs` builds a `LocalResource` over `api::fetch_active_scene` and exposes it as a `Memo<Option<ActiveSceneResponse>>` on both `ZonesContext` and `ScenesContext`. A WebSocket effect refetches it on every scene event except pure control patches, which arrive at slider-drag rate and never change scene structure:

```rust
let controls_only = hint.event_type == "render_group_changed"
    && hint.render_group_change_kind
        == Some(hypercolor_types::event::ZoneChangeKind::ControlsPatched);
if !controls_only {
    active_scene_resource.refetch();
}
```

Because the scene is shared and WebSocket-fresh, a zone change made from another page, another client, or the CLI lands in Studio with no Studio-local refetch. There are no page-local `fetch_active_scene` snapshots to go stale.

`ZonesContext` derives the rest with memos, not derives, so a refetch returning identical state does not wake every zone-aware surface in the app:

- `zones` — every surface of the active scene in scene order (LED zones and display Screens).
- `led_zones` — LED-role zones only; what effect application targets.
- `multi_zone` — whether `led_zone_count(&scene.groups) > 1`. This is the trigger for every per-zone affordance.
- `focused_zone: RwSignal<Option<String>>` — the zone that quick-applies and the controls panel target. `None` means the primary zone.

### EffectsContext and the apply-target

`EffectsContext` (`app.rs`) is the cross-page composition seam. Its `apply_target: RwSignal<ApplyTarget>` is what makes a quick-apply from the dashboard, sidebar command palette, or shell land in the zone you are composing in Studio.

`ApplyTarget` (`apply_target.rs`) is a three-variant enum:

```rust
pub enum ApplyTarget {
    Primary,
    Zone(String),
    AllZones,
}
```

It defaults to `Primary`. Studio's selection effect writes it from the selected LED zone, so a quick-apply always has a visible, defined target. A Screen or the synthetic Unassigned entry is not an apply target, so when one of those is selected Studio falls back to `Primary`:

```rust
if let Some(zone_id) = selected_led_zone {
    zones_ctx.focused_zone.set(Some(zone_id.clone()));
    effects_ctx.apply_target.set(ApplyTarget::Zone(zone_id));
} else if matches!(/* a stale Zone(target) no longer in the scene */) {
    zones_ctx.focused_zone.set(None);
    effects_ctx.apply_target.set(ApplyTarget::Primary);
}
```

### StudioContext: the page-local view

`StudioContext` (`pages/studio/mod.rs`) holds what is specifically Studio's, provided once from `StudioPage` so both columns read one source of truth. The shared active scene is re-exposed here as `active_scene` for convenience, but everything else is page-local:

| Field | Type | Purpose |
| --- | --- | --- |
| `selected_surface_id` | `RwSignal<Option<String>>` | The selected surface; the single selection source the tree owns and the Stage reads |
| `active_scene` | `Signal<Option<ActiveSceneResponse>>` | Re-exposed from `ZonesContext` |
| `refresh_scene` | `Callback<()>` | Re-fetch the shared active scene after a zone mutation |
| `composition_open` | `RwSignal<bool>` | Whether the composition slide-over (effect and layer editing) is open |
| `hidden_outputs` | `RwSignal<HashMap<String, HashSet<String>>>` | Per-`(scene, zone)` hidden-output sets, client UI state only |
| `selected_output_ids` | `RwSignal<HashSet<String>>` | Rail click selection bridged into the canvas |
| `hovered_output_ids` | `RwSignal<HashSet<String>>` | Rail hover highlight bridged into the canvas |
| `attachment_cache` | `RwSignal<HashMap<String, Vec<ComponentBindingSummary>>>` | Per-device component-binding cache the cards fill lazily |
| `device_search` | `Signal<String>` | Header search term, filters the tree's device rows |

Two details are easy to get wrong. The hidden-output state is keyed `(scene_id, zone_id)` through `hidden_outputs_storage_key` and persisted to `localStorage`. It is purely client UI state and is never mirrored to the daemon's `layout_auto_exclusions`, which is discovery-reconciliation memory and a different concept entirely. And the rail highlight signals (`selected_output_ids`, `hovered_output_ids`) clear on every surface switch, so a stale highlight from the previous zone never lingers on the new one.

## How selection drives everything

Surface selection is the spine of the page. `selected_surface_id` lives in `StudioContext`, the zone tree writes it, and three effects in `StudioPage` react to it.

{% mermaid() %}
flowchart TD
    A[active_scene memo] --> B[Selection-guard effect]
    B --> C[selected_surface_id]
    C --> D[apply-target effect]
    C --> E[layers_resource]
    C --> F[ZoneLayoutProvider zone_signature]
    D --> G[EffectsContext.apply_target + focused_zone]
    E --> H[LayerPanel in the slide-over]
    F --> I[Stage canvas]
{% end %}

The selection-guard effect keeps `selected_surface_id` pointing at a still-present surface. When the active scene changes it defaults to the first non-Display group, so Studio always opens on a Light:

```rust
let next = scene
    .groups
    .iter()
    .find(|group| group.role != ZoneRole::Display)
    .or_else(|| scene.groups.first())
    .map(|group| group.id.to_string());
selected_surface_id.set(next);
```

The synthetic Unassigned entry is a special case throughout. `UNASSIGNED_SURFACE_ID` is the sentinel `"__unassigned__"`, deliberately not a UUID so it never collides with a real zone id. It is "present" only while the scene is genuinely multi-zone, it is not an apply target, and it has no layer stack, so `layers_resource` short-circuits to an empty stack at version 0 rather than hitting the per-group layer endpoint.

## The reused contracts

Two pieces of Studio are not Studio's at all. They are shared singletons mounted with a fixed contract, so the editor for a thing exists exactly once and cannot drift between pages.

### LayerPanel

`LayerPanel` (`components/layer_panel/mod.rs`) is the single layer-stack editor. Studio, `/assets`, and any future surface mount the same component. The mount contract is small and deliberate:

- **Surface identity** — `active_scene` plus `selected_group_id` name the `(scene id, group id)` pair every mutation is addressed to. The panel never displays the ids.
- **`layers_version`** — read from `layers_resource` and threaded as the `If-Match` precondition on every mutation.
- **One mutation callback** — `on_layers_mutated: Callback<()>` fires after every applied or rejected mutation; the host refetches the stack and the active scene in response. There is exactly one.
- **Internal content selection** — the asset list and effect-name resolution are owned inside the panel, so it is decoupled from any host page's selection state.

Studio passes a `surface_label`, which tells the panel to show the selected surface's name in its header and drop its own redundant group selector. The Studio zone tree already owns selection, so that selector would be dead weight.

One display detail worth knowing when you read the code: layers are authored bottom-to-top, but the row list is reversed for display so "Top" reads first. The Top/Bottom stack markers only show with more than one layer.

### LayoutWorkspace and the two providers

`LayoutWorkspace`/`LayoutCanvas` (`components/layout_builder.rs`, `components/layout_canvas.rs`) is the single spatial editor. The standalone `/layout` page and Studio's Stage drive one shared editor; only the header chrome and the scoping provider differ.

The provider is the seam. Studio's Stage wraps the editor in `ZoneLayoutProvider`, which loads the selected zone's own `Zone.layout` and persists it through the per-zone layout API. The standalone `/layout` page wraps it in `LayoutEditorProvider`, which edits the standalone layouts library that Plan 55 is retiring.

```rust
<ZoneLayoutProvider
    active_scene=active_scene
    selected_zone_id=selected_surface_id
    refresh_scene=refresh_scene
>
    <Stage />
</ZoneLayoutProvider>
```

`ZoneLayoutProvider` provides three contexts down to the Stage:

- `LayoutEditorContext` — the editor's working state: the in-flight `Signal<Option<SpatialLayout>>`, selection sets, hover sets, compound depth, the `LayoutWriteHandle`, and `can_undo`/`can_redo`.
- `LayoutZoneDisplayContext` — the per-device attachment profiles resource.
- `ZoneCanvasActions` — `save`, `revert`, `is_dirty`, and `has_layout`, consumed by the Stage header so the header drives Save and Revert off the same provider state.

The provider reloads the canvas on a **zone signature**, not on every scene refetch. The signature is the zone id plus its sorted output-id set, so a placement-only change (including this canvas's own saved edits) leaves the signature unchanged. That is what stops an unrelated scene refetch from clobbering in-flight canvas edits.

{% callout(type="tip") %}
The drag and resize hot path is deliberately non-reactive. A single requestAnimationFrame scheduler paints positions directly to cached DOM elements, and the layout signal is written once on `mouseup`. Live drag preview goes to the daemon over the outbound WebSocket as JSON messages typed `zone_layout_preview` and `zone_layout_preview_clear` (sent by `send_zone_layout_preview` / `send_zone_layout_preview_clear` in `ws/preview.rs`), throttled to `PREVIEW_PUSH_INTERVAL_MS = 75.0`. It is not a REST route and does not touch the global `SpatialEngine`. See [zone API and concurrency](@/studio/zone-api-and-concurrency.md) for the full hot path.
{% end %}

## Optimistic concurrency

Every Studio mutation is optimistic and guarded. Two preconditions cover the whole surface: zone and scene mutations carry the active scene's `groups_revision`, and layer mutations carry `layers_version`. Both ride as the `If-Match` header. A stale write is never silently lost. The daemon reports a `Stale` outcome, the client reloads, and the user retries.

### Layer mutations

`LayerPanel` threads `layers_version` through every layer write. The outcome type is the discriminator:

```rust
match api::update_layer(&scene_id, &group_id, &layer_id, &request, Some(layers_version)).await {
    Ok(api::LayerStackOutcome::Applied(_)) => on_layers_mutated.run(()),
    Ok(api::LayerStackOutcome::Stale { .. }) => {
        on_layers_mutated.run(());
        toasts::toast_error("Layer stack changed elsewhere — reloaded");
    }
    Err(error) => toasts::toast_error(&format!("Layer update failed: {error}")),
}
```

`LayerStackOutcome::Stale` is a refetch-and-retry signal, not an error and not a clobber. The same pattern covers `delete_layer` and `reorder_layer`.

Bulk add-layer is the one nuance. When the add-layer scope targets multiple surfaces, only the surface on screen is being watched, so only it sends its `If-Match` version. The bulk targets add unconditionally with `None`:

```rust
let version = if *target == group_id {
    expected_version
} else {
    None
};
```

### Layout saves

`ZoneLayoutProvider::save` carries `groups_revision` as its precondition and handles `ZoneOutcome::Stale` the same way: clear the preview, tell the user the scene changed, and refetch.

```rust
let Some((scene_id, revision)) = active_scene
    .get_untracked()
    .map(|scene| (scene.id, scene.groups_revision))
else { return; };
match api::zones::update_zone_layout(&scene_id, &zone_id, &current, Some(revision)).await {
    Ok(api::zones::ZoneOutcome::Applied(_)) => { /* mark clean, clear preview, refresh */ }
    Ok(api::zones::ZoneOutcome::Stale { .. }) => { /* clear preview, toast, refresh */ }
    Err(error) => toasts::toast_error(&format!("Save failed: {error}")),
}
```

The save is a placement merge, not a replace: the output-id set must match, and identity and topology fields are preserved server-side. Adds and drops route through the device sub-routes, not the layout PUT. Full route semantics are on the [zone API and concurrency](@/studio/zone-api-and-concurrency.md) page.

## Capability gating

Multi-zone affordances do not probe. They gate on named capabilities the daemon advertises in `GET /api/v1/status`, exposed through `CapabilitiesContext` (`app.rs`). An absent advertisement means the affordance stays hidden, with no fallback.

```rust
pub fn zone_crud_ready(&self) -> bool {
    self.has("zone-crud")
        && self.has("multi-zone-sampling")
        && self.has("zone-device-assignment")
}
```

`+ New zone` and the zone rows need all three, because a user who can create a zone but cannot render it or move outputs into it would have an unusable zone. The unassigned-lights policy editor gates separately on `scene-unassigned-behavior-write`.

Studio and Media replace Assets/Layout/Displays in the nav only when the browser-local `studio_ui_beta` flag is on (`StudioFlag` in `app.rs`, persisted under `hc-studio-ui-beta`). It is never daemon config, so it flips against a live daemon without a rebuild.

![The Hypercolor Studio workspace](/img/ui/studio.webp)

## Where to read next

- [Zone API and concurrency](@/studio/zone-api-and-concurrency.md) — the REST routes, the WebSocket preview protocol, and the full `If-Match` story.
- [Vocabulary and naming](@/studio/vocabulary-and-naming.md) — the locked type names and the never-rooms rule.
- [Render pipeline](@/architecture/render-pipeline.md) — how the composited canvas becomes LED color downstream of Studio.
