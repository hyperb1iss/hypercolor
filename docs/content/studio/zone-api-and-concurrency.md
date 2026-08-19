+++
title = "Zone API & concurrency"
description = "The live scene tree, real zone and layer identity, one revision token, and the per-zone WebSocket preview protocol."
weight = 120
+++

Studio edits one live resource tree. `GET /api/v1/scene` returns the active
scene, its zones, each zone's member device segments, and every authored layer.
Fine-grained writes stay under that same root. Stored scenes remain a collection
under `/api/v1/scenes`, but they do not expose a second nested zone API.

The live document also carries the only concurrency token on the REST wire:
`revision`. Structural writes may guard themselves with `If-Match`. Control
values are unguarded and target a real layer id from the document. A replaced
layer has a new id, so a stale control write cannot reach the replacement.

This page is the developer reference for those contracts. For the user-facing
model, see [Zones](@/studio/zones.md), [Layers](@/studio/layers.md), and
[Layouts](@/studio/layouts.md). The shared response and error envelopes live in
the [REST API](@/api/rest.md) reference.

{% callout(type="info") %}
**Vocabulary.** A scene is a whole-rig configuration. A zone is a flexible
partition of its canvas. A member is one device segment assignment inside a
zone. A layer is one authored source in that zone's stack.
{% end %}

## Route map 🎯

All routes below are mounted under `/api/v1`.

### Stored scenes

Stored scenes use collection and whole-document operations:

```text
GET    /scenes
POST   /scenes
GET    /scenes/{id}
PUT    /scenes/{id}
DELETE /scenes/{id}
POST   /scenes/{id}/activate
```

`POST /scenes` seeds a Default zone server-side. Activating a stored scene
makes it the resource returned by `GET /scene`. To edit stored scene structure,
activate it and edit the live tree, or replace the stored scene document with
`PUT /scenes/{id}`.

### Live scene

{% api_endpoint(method="GET", path="/api/v1/scene") %}
Return the complete live scene document. An active scene always exists, so this
route always returns `200`. The JSON `revision` is also served as `ETag`.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene") %}
Patch `name` or `unassigned_behavior`. The default scene cannot be renamed.
This structural write optionally accepts `If-Match`.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/deactivate") %}
Return to the Default scene and receive the new live document.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/clear") %}
Clear every non-display zone's layer stack. Pass
`{ "zone": "<zone_uuid>" }` to clear one non-display stack. Display zones stay
owned by the display API, and a targeted display clear is rejected. This is the
canonical stop gesture and optionally accepts `If-Match`.
{% end %}

### Zones and members

{% api_endpoint(method="POST", path="/api/v1/scene/zones") %}
Create a custom zone. Primary and display zones are created by their owning
engine flows.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scene/zones/{zone}") %}
Read one live zone resource.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene/zones/{zone}") %}
Patch the zone's name, enabled state, brightness, or color.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scene/zones/{zone}") %}
Delete a custom zone.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/zones/{zone}/members") %}
Assign device segments with `{ "device_id": "...", "segments": [...] }`.
The response carries minted member ids.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scene/zones/{zone}/members/{member}") %}
Remove one membership by its member id. Segment names are not resource ids.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scene/zones/{zone}/layout") %}
Write a compact zone layout containing member placements. Every placement names
a member id from the live zone document.
{% end %}

All mutating routes in this section are structural. Each may carry the scene
document's current `revision` in `If-Match`.

### Layers

{% api_endpoint(method="GET", path="/api/v1/scene/zones/{zone}/layers") %}
List the zone's authored layer stack from bottom to top.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/zones/{zone}/layers") %}
Append a layer. The server mints its `SceneLayerId`.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene/zones/{zone}/layers/order") %}
Reorder the stack with every current layer id exactly once, bottom to top.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scene/zones/{zone}/layers/{layer}") %}
Replace a whole layer. Every successful replacement mints a fresh id, including
a replacement with the same effect and controls.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scene/zones/{zone}/layers/{layer}") %}
Delete a layer.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene/zones/{zone}/layers/{layer}/controls") %}
Patch effect controls with the shared shape:

```json
{
  "values": {
    "speed": { "float": 45.0 },
    "palette": { "enum": "Midnight" }
  },
  "clear_bindings": ["speed"]
}
```

This value write never takes `If-Match`. If the layer was replaced, the old id
returns `404 layer_not_found`. A value targeting a bound control returns
`409 control_bound` unless the same request names that key in `clear_bindings`.
Binding removal and the new values commit atomically.
{% end %}

Layer create, replace, delete, and reorder are structural writes and may carry
`If-Match`. The control route is the only in-place layer value mutation.

### Layout library

The `/layouts` collection manages named spatial layouts. A live scene may point
at one through `scene.layout_id`, while a zone may hold its own placement
override through `PUT /scene/zones/{zone}/layout`.

```text
GET    /layouts
POST   /layouts
GET    /layouts/active
PUT    /layouts/active/preview
GET    /layouts/{id}
PUT    /layouts/{id}
DELETE /layouts/{id}
POST   /layouts/{id}/apply
```

Effects do not have layout associations. The old effect-layout API and its
store are removed; `scene.layout_id` is the deliberate successor.

## The live document and real identity

The live tree embeds the ids a client needs for every follow-up:

```json
{
  "data": {
    "id": "832c4b7f-9f4d-49d2-b37a-674c76bc2a80",
    "name": "Desk rig",
    "kind": "named",
    "is_default": false,
    "unassigned_behavior": "off",
    "layout_id": null,
    "revision": 42,
    "zones": [
      {
        "id": "84b20af9-0700-4b82-8488-88314b87fb5c",
        "name": "Primary",
        "role": "primary",
        "enabled": true,
        "brightness": 1.0,
        "color": null,
        "display_target": null,
        "members": [
          {
            "id": "keyboard-left",
            "device_id": "razer:huntsman-v3",
            "segment": "left",
            "name": "Keyboard left"
          }
        ],
        "layers": [
          {
            "id": "d6cf26a0-2c54-47e1-9eab-65dd8c4021fe",
            "source": {
              "type": "effect",
              "effect_id": "0198c5b6-1111-7000-8000-000000000004",
              "controls": { "speed": { "float": 45.0 } }
            },
            "blend": "replace",
            "opacity": 1.0
          }
        ]
      }
    ]
  }
}
```

Clients never derive a layer id from the zone id. They read the id from
`GET /scene` or from an effect-apply response. Layer create and apply mint an
id. Whole-layer `PUT` replaces that identity with a new one. Control patches
and reorder operations keep existing ids.

Persisted scene layer and zone ids survive activation, deactivation, restart,
and snapshot. Ids in the auto-managed Default scene are stable only for the
current daemon run, so a client must not persist them.

## Optimistic concurrency

### One token: `revision`

The live scene's `revision` is the commit generation. A successful read or
mutation returns it in the resource and quotes it in the response header:

```http
ETag: "42"
```

No resource-specific version counters exist on the REST wire. Internal
bookkeeping may use more detail, but clients coordinate through the one
document revision.

### Structural writes use optional `If-Match`

Send the last revision when overwriting a stale structure would be harmful:

```http
PATCH /api/v1/scene/zones/84b20af9-0700-4b82-8488-88314b87fb5c HTTP/1.1
If-Match: "42"
Content-Type: application/json

{ "name": "Desk halo", "color": "#7c5cff" }
```

The daemon accepts a quoted integer, a bare integer, or `*`. Omitting the
header, or sending `*`, applies without a precondition. A stale integer returns
the canonical `412 Precondition Failed` envelope:

```json
{
  "error": {
    "code": "precondition_failed",
    "message": "scene revision does not match",
    "details": { "current": 43 }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_...",
    "timestamp": "..."
  }
}
```

Structural writes include scene metadata, zone create, patch, and delete, zone
layout replacement, member assignment and removal, layer create, replace,
delete, and reorder, scene clear, stored-scene replacement, and both effect
apply forms. After a `412`, read `/scene`, rebase the intended edit, and retry
with the new revision.

### Control values never use `If-Match`

A slider would invalidate its own revision on every frame if control writes
were guarded. The control route therefore commits values in arrival order. Its
real layer id is the stale-write fence: a layer replacement removes the old id,
and later writes to it return `404`.

## Zone layout is member placement

`PUT /scene/zones/{zone}/layout` writes the zone-scoped placement override. The
request is compact: each placement names a member and supplies normalized
position, size, rotation, scale, optional orientation, and topology. Membership
itself changes only through the member routes.

The `member` field is the identity returned in the zone's `members` list. A
device-scoped segment name is not unique across devices and cannot identify a
membership on its own.

## Per-zone WebSocket preview

While the user drags a member on the Studio Stage, the editor pushes a
transient preview so the live render reflects the in-progress placement. The
preview is not a REST mutation, does not persist the scene, and does not change
the global spatial layout.

{% callout(type="warning") %}
The `zone_layout_preview` text command is distinct from the `zone_preview`
binary channel. The command sends an in-progress placement to the daemon. The
binary channel streams rendered preview pixels back to subscribers.
{% end %}

### Pushing and clearing a preview

The editor throttles pushes to one every `75ms` (`PREVIEW_PUSH_INTERVAL_MS`).
The inbound command is active-scene-only and keyed by zone id:

```json
{
  "type": "zone_layout_preview",
  "zone_id": "84b20af9-0700-4b82-8488-88314b87fb5c",
  "layout": {
    "canvas_width": 640,
    "canvas_height": 480,
    "zones": []
  }
}
```

There is no caller-selected `scene_id`. The daemon rejects that stale field
instead of silently applying it to another scene. The daemon resolves the live
scene and applies the preview to its named zone. Clear it with:

```json
{
  "type": "zone_layout_preview_clear",
  "zone_id": "84b20af9-0700-4b82-8488-88314b87fb5c"
}
```

The daemon clears a connection's previews when that socket closes. A committed
layout write retires only the preview version it replaced, so it cannot erase a
newer drag. Saving, reverting, and unmounting also send the explicit clear
command.

### The binary preview frame

The rendered `zone_preview` frame still carries both scene and zone UUIDs so a
subscriber can route interleaved frames. The shared codec in
`hypercolor-leptos-ext::ws` owns both layouts:

```text
legacy tag 0x08:
tag u8 | frame u32 | timestamp u32 | scene_id 16B | zone_id 16B |
width u16 | height u16 | format u8 | payload

wide tag 0x0C:
tag u8 | frame u32 | timestamp u32 | scene_id 16B | zone_id 16B |
width u32 | height u32 | format u8 | payload
```

The wide layout is used when either dimension exceeds `u16::MAX`.

## How a structural mutation flows

{% mermaid() %}
sequenceDiagram
    participant UI as Studio
    participant API as Daemon REST
    participant Bus as Event bus
    participant Other as Other clients

    UI->>API: PATCH /scene/zones/{zone} with If-Match "42"
    alt revision is 42
        API->>API: commit scene revision 43
        API-->>UI: 200 with ETag "43"
        API->>Bus: publish scene event
        Bus-->>Other: refetch live scene
    else revision changed
        API-->>UI: 412 with details.current
        UI->>API: GET /scene
        UI->>UI: rebase and retry
    end
{% end %}

## Related

- [Studio architecture](@/studio/architecture.md): the client-side contexts
  that drive these calls.
- [REST API](@/api/rest.md): the complete daemon REST surface.
- [WebSocket API](@/api/websocket.md): subscriptions, events, and binary frame
  layouts.
