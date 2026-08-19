+++
title = "Studio issues"
description = "Scene revision conflicts, live-tree recovery, zone layouts, layer identity, and Studio rendering issues."
weight = 40
+++

Studio edits the always-present live tree under `/api/v1/scene`. Structural
writes may carry the scene document's `revision` in `If-Match`; the daemon
returns HTTP 412 when that revision is stale. Control values follow a different
rule: they never use `If-Match` and instead address a real layer id from the
live document.

{{ img(path="img/ui/studio.webp", alt="Studio workspace with zone tree and layout canvas") }}

## Save rejected: "Scene changed elsewhere, reloaded, try again"

The daemon returned `412 Precondition Failed` because the `If-Match` value no
longer matched the live scene's `revision`. The response carries the
authoritative value in `error.details.current`. Studio reloads `GET /scene` so
you can reapply the edit without overwriting another client's change.

If the rejection repeats, another browser tab, CLI command, or MCP agent is
actively changing the scene. Run the daemon with debug logging and watch the
live-tree routes:

```bash
journalctl --user -u hypercolor -f | grep '/scene'
```

A missing `If-Match` is an unconditional structural write. Send a revision when
you need conflict detection, and update it from each successful response's
`ETag` before issuing the next guarded mutation.

## A control change returns `layer_not_found`

The layer was replaced after the client read the scene. Whole-layer
replacement and effect apply both mint a fresh layer id, so the old identity
cannot absorb a late control write.

Read `GET /scene` again, locate the current effect layer, and retry against its
real id:

```bash
scene=$(curl -s http://localhost:9420/api/v1/scene)
zone_id=$(printf '%s' "$scene" | jq -r '.data.zones[0].id')
layer_id=$(printf '%s' "$scene" | jq -r '.data.zones[0].layers[-1].id')

curl -s -X PATCH \
  "http://localhost:9420/api/v1/scene/zones/$zone_id/layers/$layer_id/controls" \
  -H 'Content-Type: application/json' \
  -d '{"values":{"speed":{"float":45.0}}}'
```

Do not add `If-Match` to that control request. Layer identity is its stale-write
fence.

## A control change returns `control_bound`

The named control has an active input binding. A manual value would be replaced
by the next sensor update, so the daemon rejects it. Clear the binding and set
the value atomically:

```json
{
  "values": { "speed": { "float": 45.0 } },
  "clear_bindings": ["speed"]
}
```

## All LEDs show the same color

The effect is running, but every LED receives one flat sample. Check the zone's
member placements in Studio. If blocks overlap at the origin, or have no useful
spatial separation, the sampler reads nearly the same canvas location for every
output.

Move each member to its physical position, then save. Some effects intentionally
render one color per zone. Switch to a spatial effect such as Borealis, Color
Wave, or Gradient to distinguish an effect choice from a placement problem.

## Zone went stale mid-edit

An incoming scene event invalidated Studio's local projection. Common causes
include activating another scene, changing membership from another client, or
reconnecting after a daemon restart. Studio refetches the live document because
events are hints, not a replayable source of truth.

If refreshes repeat, inspect mutation traffic:

```bash
journalctl --user -u hypercolor -f \
  | grep -E '(POST|PATCH|PUT|DELETE).*/scene'
```

## "Snapshot scene cannot be structurally edited"

A stored snapshot scene refuses structural live mutations by design. Activate
an editable scene or replace the stored scene document through
`PUT /scenes/{id}`. The live route does not create a second editable copy behind
the snapshot.

## Changes vanish after a daemon restart

Successful scene mutations persist through the scene commit path. If a change
survives an in-session reload but disappears after restart, check for a 5xx at
the original write and confirm the daemon's config directory is writable:

```bash
ls -la ~/.config/hypercolor/
```

Also verify that the daemon is starting with the same config path that received
the write. `hypercolor service status` and the startup log show the active path.

## Studio shows a different layout than `/layout`

The two views edit related but distinct resources. Studio writes a zone's
member placement override through
`PUT /api/v1/scene/zones/{zone}/layout`. The `/layout` page manages the named
layout collection under `/api/v1/layouts`.

A scene can deliberately reference a named layout through `scene.layout_id`.
The daemon does not infer that link from the current effect. Effect-layout
associations no longer exist.

## Zone canvas edits are not visible in the live render

Studio pushes an in-memory `zone_layout_preview` WebSocket command while you
drag. The command is keyed by `zone_id` and always targets the active scene. A
disconnected socket cannot deliver it, so check the WebSocket indicator first.

The preview is temporary. Saving writes the member placements through the live
zone layout route and clears the preview. Reverting, unmounting, or disconnecting
also clears it.

## Layer ordering looks reversed

The stored stack is bottom to top, while Studio presents the visual top layer
first. Composition starts with the bottom layer and blends each layer above it.
If the top layer is not visible, inspect its blend mode and opacity before
changing the order.

## Zone enable or brightness changes do not land

Both fields are patched at `/api/v1/scene/zones/{zone}`. The route accepts the
same optional scene `If-Match` as other structural writes. A stale header
returns 412; a request without the header applies unconditionally. Check the
daemon log for the exact response instead of retrying a stale revision.

## Unassigned devices do not turn off

The live scene's `unassigned_behavior` controls device segments that are not
members of any zone. Patch it through `PATCH /api/v1/scene`. Studio exposes the
same setting in the Unassigned row for multi-zone scenes.

## Debug a revision conflict directly

Read the live scene and its `ETag`, then use that one revision on a structural
write:

```bash
curl -s -D /tmp/hypercolor-scene-headers \
  http://localhost:9420/api/v1/scene \
  | jq '{id: .data.id, revision: .data.revision}'

curl -s -X POST http://localhost:9420/api/v1/scene/zones \
  -H 'Content-Type: application/json' \
  -H 'If-Match: "42"' \
  -d '{"name":"Desk"}'
```

A successful mutation returns the new `ETag`. A stale write returns
`precondition_failed` with `error.details.current`. There are no separate zone,
layer, or control version tokens.

For the complete contract, see
[Zone API and concurrency](@/studio/zone-api-and-concurrency.md).
