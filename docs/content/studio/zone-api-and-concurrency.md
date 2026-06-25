+++
title = "Zone API & concurrency"
description = "Zone, scene, layer, and layout REST routes plus the optimistic-concurrency machinery and the per-zone WebSocket preview protocol."
weight = 120
+++

Studio composes a scene by issuing scoped REST mutations against the daemon's
zone, scene, layer, and layout routes, and every structural mutation is guarded
by an optimistic-concurrency token. A zone or scene write carries the active
scene's `groups_revision`; a layer write carries the target zone's
`layers_version`. The token rides as an `If-Match` precondition, the daemon
replies `412 Precondition Failed` with the authoritative current value when it
no longer matches, and the client surfaces that as a `Stale` outcome it can
rebase and retry rather than clobber a concurrent edit. Live drag-to-reposition
preview is a separate, transient path: it travels over the inbound WebSocket as
a `zone_layout_preview` message, never through REST, and never touches the
global spatial engine.

This page is the developer reference for those contracts. For the user-facing
walkthrough of the same surfaces, see [Zones](@/studio/zones.md),
[Layers](@/studio/layers.md), and [Layouts](@/studio/layouts.md). The shared
REST envelope and error shapes live in the [REST API](@/api/rest.md) reference.

{% callout(type="info") %}
**Vocabulary.** A scene is a whole-rig config; a zone is a flexible partition of
the canvas. The daemon's Rust type for a zone is `Zone` and its handlers live in
`scenes_zones.rs` and `layers.rs`. A device output placed on a zone's canvas is
an `Output`. Never call a zone a "room."
{% end %}

## Route map 🎯

All routes are mounted under `/api/v1`. Scene-scoped paths accept either the
scene UUID or its name as `{id}`; the daemon resolves the name to an id.

### Scenes

{% api_endpoint(method="GET", path="/api/v1/scenes") %}
List user-created scenes. The ephemeral Default scene is omitted from the list.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes") %}
Create a scene. The new scene is seeded server-side with a Primary Default zone
holding the current device selection.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/active") %}
Get the active scene, including the Default scene when it is active.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}") %}
Replace the scene's `name` and `description`.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}") %}
Delete a scene. The Default scene cannot be deleted.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/activate") %}
Activate a scene.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/deactivate") %}
Deactivate the active scene and fall back to Default.
{% end %}

`POST /scenes` seeds the new scene with a Default zone server-side, so a freshly
created scene already partitions the whole rig. `PUT /scenes/{id}` replaces the
scene's `name` and `description` wholesale, which is why the client always sends
`description` back verbatim on a rename to avoid clearing it. `GET /scenes` omits
the ephemeral Default scene; only user-created scenes appear in the list.

### Zones

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}/zones") %}
List the scene's zones with the current `groups_revision`.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/zones") %}
Create a zone. `If-Match` enforces `groups_revision`.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}/zones/{zone_id}") %}
Get a single zone.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/zones/{zone_id}") %}
Patch a zone's name, color, brightness, enabled, or `make_primary`. The
precondition is enforced only for the structural `make_primary` edit.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}/zones/{zone_id}") %}
Delete a zone. `If-Match` enforces `groups_revision`.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}/zones/{zone_id}/layout") %}
Reposition the outputs the zone already owns. Placement-only; see below.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/zones/{zone_id}/devices") %}
Assign outputs into the zone, by id or as a new `Output`.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}/zones/{zone_id}/devices/{device_zone_id}") %}
Remove an output from the zone.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/unassigned-behavior") %}
Set the scene-level policy for outputs not assigned to a zone.
{% end %}

### Layers

Layers are scoped to a zone via the `groups` path segment (the daemon's render
group is the zone). The layer stack carries its own version, `layers_version`,
independent of `groups_revision`.

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}/groups/{group_id}/layers") %}
List the zone's layer stack with its current `layers_version`.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/groups/{group_id}/layers") %}
Add a layer. `If-Match` enforces `layers_version`. An optional `index` query
sets the insertion position.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}") %}
Replace a layer. `If-Match` enforces `layers_version`.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}") %}
Remove a layer. `If-Match` enforces `layers_version`.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/groups/{group_id}/layers/order") %}
Reorder the stack with an exact permutation of layer ids.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}/controls") %}
Patch an effect layer's live controls.
{% end %}

There is also a batch route that fans a single media layer across several zones
in one call, each target carrying its own placement and its own
`expected_layers_version`:

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/layers/broadcast-media") %}
Fan one media layer across multiple zones, each target versioned independently.
{% end %}

### Layouts

The `/layouts` routes manage the standalone layout library, which is separate
from a zone's own `Zone.layout`. Studio edits the zone's layout through
`PUT /scenes/{id}/zones/{zone_id}/layout`, not these routes. The library is
soak-gated for retirement; treat it as legacy and prefer the per-zone path.

{% api_endpoint(method="GET", path="/api/v1/layouts") %}
List saved layouts.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/layouts") %}
Create a layout.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/layouts/active") %}
Get the active layout.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/layouts/{id}") %}
Get a single layout.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/layouts/{id}") %}
Replace a layout.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/layouts/{id}") %}
Delete a layout.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/layouts/{id}/apply") %}
Apply a saved layout.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/layouts/active/preview") %}
Preview a layout against the active layout without persisting it.
{% end %}

## Optimistic concurrency

Studio is a multi-client surface. The same scene can be edited from another
browser, the TUI, or the CLI, so a blind write would risk overwriting an edit
the client never saw. Every structural mutation instead names the version it
believes it is editing, and the daemon refuses the write if reality has moved on.

### The token: `groups_revision` and `layers_version`

Each scene carries a monotonic `groups_revision`. Any change to the set of zones
or their structure (create, delete, make-primary, device assignment, zone layout
placement, unassigned-behavior) bumps it. Each zone separately carries a
`layers_version` that bumps on any layer-stack change (add, update, remove,
reorder, controls patch).

Both numbers ride out in two places on every successful response: in the JSON
body (`groups_revision` / `layers_version`) and in the HTTP `ETag` header,
quoted, for example `ETag: "7"`. The client may read either.

### The precondition: `If-Match`

To guard a mutation, send the version you are editing as an `If-Match` header.
The daemon accepts the quoted form, the bare integer, or `*` (which is treated as
"no precondition"). The header is parsed by `parse_if_match_groups_revision` for
zone and scene routes and `parse_if_match_layers_version` for layer routes. A
non-integer that is not `*` is a `400 Bad Request`.

```http
PATCH /api/v1/scenes/living-room/zones/4f1c.../  HTTP/1.1
If-Match: "7"
Content-Type: application/json

{ "name": "Desk", "color": "#7c5cff" }
```

When the precondition still matches, the mutation applies, persists, and the
daemon publishes a change event on the bus so every connected client converges.
When it does not match, the daemon replies `412 Precondition Failed` with a body
carrying the authoritative current value, plus an `ETag` of that value:

```json
{ "error": "groups_revision mismatch", "current": 9 }
```

Layer routes use the same shape with `"error": "layers_version mismatch"`.

{% callout(type="warning") %}
`If-Match` is optional on the wire — a request with no `If-Match` header skips
the precondition entirely and applies unconditionally. Studio always sends one
for structural edits. Send `*` only when you deliberately want a last-writer-wins
overwrite.
{% end %}

### The client outcome: `MutationOutcome::Stale`

The web UI client wraps every versioned mutation in `send_json_versioned`, which
classifies the reply into `MutationOutcome<T>`:

```rust
pub enum MutationOutcome<T> {
    Applied(T),
    Stale { current: u64 },
}
```

A `2xx` becomes `Applied(payload)`; a `412` becomes `Stale { current }`, reading
`current` from the response body. The zone client aliases this as
`ZoneOutcome<T>`. A `Stale` is explicitly **not** a failure to log and forget:
the caller refetches the active scene (or the layer stack), rebases its edit on
the authoritative `current`, and retries, so a concurrent edit from another
client is never silently clobbered.

```rust
match api::zones::update_zone(&scene_id, &zone_id, &request, Some(revision)).await? {
    ZoneOutcome::Applied(zone) => apply_locally(zone),
    ZoneOutcome::Stale { current } => {
        // refetch the scene, rebase on `current`, retry
    }
}
```

The daemon's matching error type for the layer stack is
`LayerMutationError::Stale { current }`, which the handler renders as the same
`412` body. On the zone side, `check_groups_revision` performs the comparison and
returns the `412` response before any mutation runs.

### What gets versioned

| Route family | Precondition | Notes |
| --- | --- | --- |
| `POST/DELETE /zones/...` | `groups_revision` | Create and delete enforce the precondition. |
| `PATCH /zones/{id}` | `groups_revision` | Enforced only when the edit is structural (`make_primary`); name/color/brightness/enabled patches skip it. |
| `PUT /zones/{id}/layout` | `groups_revision` | Placement-only; see below. |
| `POST/DELETE /zones/{id}/devices...` | `groups_revision` | Reassigning or removing an output. |
| `PATCH /unassigned-behavior` | `groups_revision` | Scene-level policy. |
| `POST/PUT/DELETE/PATCH /groups/{id}/layers...` | `layers_version` | Per-zone stack version. |
| `POST /layers/broadcast-media` | per-target `expected_layers_version` | Each fan-out target versions independently; a target may pass `null` to apply unconditionally. |

{% callout(type="tip") %}
Bulk add-layer that fans across multiple zones sends `If-Match` only for the zone
currently on screen; the additional fan-out targets apply unconditionally
(`null` version). That keeps a multi-zone broadcast from failing because of an
unrelated concurrent edit in a zone the user is not looking at.
{% end %}

## Zone layout is a placement merge, not a replace

`PUT /scenes/{id}/zones/{zone_id}/layout` updates only the placement of the
outputs a zone already owns. The request body is a full `SpatialLayout`, but the
daemon requires its output-id set to match the zone's current outputs exactly. It
may reposition those outputs, reorder them, and retune the canvas dimensions, but
it preserves the server-side identity and topology fields. Adding or dropping an
output is rejected here with `422` and a `LayoutOutputMismatch` validation error:

```json
{
  "error": "Zone layout must carry exactly the zone's current outputs; add or remove outputs through the device endpoints"
}
```

Adds and drops route through the device sub-routes instead:
`POST /zones/{zone_id}/devices` moves an existing output into the zone (referenced
by id) or places a brand-new one (carrying a full `Output`), and
`DELETE /zones/{zone_id}/devices/{device_zone_id}` removes one. Both return the new
`groups_revision` so a sequence of assignments can chain without a refetch.

In the web UI, `ZoneLayoutProvider` reads the active scene's `groups_revision`,
sends it as the save's `If-Match`, and treats a `ZoneOutcome::Stale` as a signal
to reload before saving again rather than overwriting.

## Per-zone WebSocket preview

While the user drags an output on the Studio Stage, the editor pushes a transient
preview so the live render reflects the in-progress placement before any save.
This is **not** a REST route and it does **not** mutate the persisted scene or the
global `SpatialEngine`. It is a `ClientMessage` sent over the inbound WebSocket.

{% callout(type="warning") %}
The drag-preview push (the `zone_layout_preview` text message described here) is
distinct from the `zone_preview` binary frame channel that streams the rendered
per-zone preview image back to the client. The push is the client telling the
daemon "render this placement"; the binary frame is the daemon streaming the
resulting pixels. The binary wire format is owned by
`hypercolor-leptos-ext::ws`; see the [WebSocket API](@/api/websocket.md) for the
channel catalog and binary-frame conventions.
{% end %}

### Pushing a preview

The editor throttles pushes to one every `75ms` (`PREVIEW_PUSH_INTERVAL_MS`) and
sends a text frame tagged `zone_layout_preview` carrying the scene id, the zone
id, and the full in-progress `SpatialLayout`:

```json
{
  "type": "zone_layout_preview",
  "scene_id": "living-room",
  "zone_id": "4f1c0e2a-...",
  "layout": { "canvas_width": 640, "canvas_height": 480, "zones": [ ... ] }
}
```

On the wire, `ClientMessage` is internally tagged (`#[serde(tag = "type")]`,
snake_case), so the `type` discriminator selects the variant. The daemon applies
the preview as an override for that one zone only.

### Clearing a preview

When the drag ends, or the editor saves, reverts, or unmounts, it clears the
override with a companion message:

```json
{
  "type": "zone_layout_preview_clear",
  "scene_id": "living-room",
  "zone_id": "4f1c0e2a-..."
}
```

A socket disconnect mid-drag auto-clears the override daemon-side, so a dropped
connection never leaves a zone stuck on a half-placed preview. After a successful
save, `update_zone_layout` clears the preview for that zone explicitly.

### The binary preview frame

The rendered preview streams back on the `zone_preview` channel as a
`ZonePreviewFrame`, tag `0x08`, with a 46-byte header. Subscribe and configure it
through the standard channel-config mechanism; the daemon caps it at 60 fps. The
encoded header, little-endian, is:

```text
[0]      u8    tag = 0x08
[1..5]   u32   frame_number
[5..9]   u32   timestamp_ms
[9..25]  16B   scene_id   (UUID bytes)
[25..41] 16B   zone_id    (UUID bytes)
[41..43] u16   width
[43..45] u16   height
[45]     u8    pixel format  (0 = rgb, 1 = rgba, 2 = jpeg)
[46..]         payload
```

Because both the scene id and the zone id travel in the header, a client
subscribed to `zone_preview` can route each frame to the correct zone tile with
no ambiguity, even when several zones preview at once.

## How a mutation flows end to end

{% mermaid() %}
sequenceDiagram
    participant UI as Studio (client)
    participant API as Daemon REST
    participant Bus as Event bus
    participant Other as Other clients

    UI->>API: PATCH /zones/{id} (If-Match: "7")
    alt revision still 7
        API->>API: apply + persist (groups_revision -> 8)
        API-->>UI: 200 + ETag "8"
        API->>Bus: publish render-group changed
        Bus-->>Other: converge to revision 8
    else revision moved to 9
        API-->>UI: 412 { current: 9 }
        UI->>UI: ZoneOutcome::Stale -> refetch, rebase, retry
    end
{% end %}

## Related

- [Studio architecture](@/studio/architecture.md) — the client-side context and
  provider map that drives these calls.
- [REST API](@/api/rest.md) — the full daemon REST surface.
- [WebSocket API](@/api/websocket.md) — channels, subscription, the text
  control protocol, and the binary frame layouts including `zone_preview`.
