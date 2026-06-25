+++
title = "Studio issues"
description = "Stale-zone and save-rejected conflicts: optimistic concurrency, zone-revision mismatches, and Studio vs /layout."
weight = 40
+++

Studio writes are guarded by optimistic concurrency — every zone, layer, and layout mutation sends a revision token with the request, and the daemon rejects the write with HTTP 412 if anything changed the scene between when the UI loaded it and when the save arrived. The Studio UI handles this automatically: a toast fires, the scene reloads, and you retry. This page explains when that happens, what triggers it, and how to recover from the cases the UI cannot resolve automatically.

![Studio workspace with zone tree and layout canvas](/img/ui/studio.webp)

## Save rejected: "Scene changed elsewhere — reloaded, try again"

This toast means the daemon returned a `412 Precondition Failed` on the zone layout save. The daemon replied with a `groups_revision mismatch` error and the current revision number; the UI cleared the live preview, reloaded the scene, and showed the toast.

**Why it happens.** Every zone mutation — create, rename, delete, assign device, remove device, change unassigned behavior — increments the scene's `groups_revision`. The UI sends the revision it loaded as an `If-Match` header. If the revision on the server no longer matches, the write is rejected. This is not a bug; it prevents two simultaneous edits from silently clobbering each other.

**What to do.** Wait for the scene to reload (it happens automatically after the toast), then repeat the action. The conflict was transient. If the same rejection loops more than twice, a background process (another browser tab, a CLI call, an MCP agent) is actively modifying the scene. Identify and stop it before continuing. The daemon's access-log middleware writes every request to the daemon log, so if you run the daemon with debug logging (`just daemon`) you can watch which client keeps issuing zone mutations:

```bash
# Watch the daemon log for zone-mutation traffic
journalctl --user -u hypercolor -f | grep '/zones'
```

**In the zone layout canvas specifically.** The zone canvas save path (`PUT /api/v1/scenes/{id}/zones/{zone_id}/layout`) sends the active `groups_revision` as a precondition. A stale revision produces `ZoneOutcome::Stale` in the client, which triggers the "reloaded, try again" toast. The layout you had in the canvas is discarded on reload; redo the placement adjustments and save again.

{% callout(type="tip") %}
The zone canvas only accepts placement changes through the layout PUT. Adding or removing a device from a zone always goes through the device assignment endpoints. If you see a `422 Unprocessable Entity` (not a 412), you are hitting the output-set mismatch guard: the layout you are saving contains a different set of device outputs than the zone currently owns. Go back to device assignment, add the device there, then re-open the canvas.
{% end %}

## All LEDs show the same color (no spatial gradient)

The effect is running but every LED gets the same flat color instead of a position-mapped gradient. This is almost always a zone layout problem: the device outputs exist in the zone but have no spatial positions on the canvas, so the sampler maps them all to the same canvas coordinate.

**Check 1: open the zone canvas.** In Studio, select the zone from the left tree. The Stage shows the spatial layout editor with device blocks on the canvas. If all blocks are stacked at the origin (top-left) or none are visible at all, the layout has no placed positions.

Drag each device block to a meaningful position that reflects its physical location — left side of the desk, bottom of the case, right monitor. The live preview updates as you drag. Save when done.

**Check 2: verify the layout is applied.** After saving, check that the effect now shows a gradient. If it still shows flat, the effect may not be spatially-driven. Open the effect controls and look for `mapping_mode` or similar: some effects have an explicit `Static` / `Spatial` mode. Set it to spatial.

**Check 3: the effect itself is not spatial.** Breathing, Color Zones, and a handful of palette-only effects do not sample the canvas spatially — they apply a uniform color or a per-zone palette that is independent of position. Switch to an effect like Borealis, Color Wave, or Gradient to confirm the layout is working, then switch back to your preferred effect.

## Zone went stale mid-edit

You are editing a zone and the tree or Stage refreshes unexpectedly, discarding your in-progress changes. This happens when an incoming WebSocket scene event marks the local state as stale and triggers a reload.

**Common causes:**

- You or another client activated a different scene (scene switches replace the zone set entirely).
- A CLI call or MCP tool modified zone membership while you were composing.
- The daemon restarted mid-session and re-broadcasted the current scene state.

**Recovery.** After the reload, the scene reflects the daemon's authoritative state. Re-apply the changes you lost. If the stale state keeps recurring, check whether a scheduled agent or service is calling zone mutation endpoints on a timer. The daemon log (run with `just daemon` for debug logging) shows every mutating request, so tail it and watch for writes you did not make:

```bash
# Tail the daemon log for scene/zone mutations from other clients
journalctl --user -u hypercolor -f | grep -E '(POST|PATCH|PUT|DELETE).*/scenes/'
```

## "Snapshot scene cannot be structurally edited"

This conflict error appears when you try to create, delete, or reassign zones in a scene that is in snapshot mode. Snapshot scenes are read-only for structural mutations — zone rename, enable/disable, and brightness still work, but you cannot add zones, delete zones, or move devices between zones.

To edit the structure, create a new scene, make your changes there, and activate it. Or duplicate the snapshot scene (if that option is available in your build) to get an editable copy.

## Zones not persisting after daemon restart

Zone changes are persisted to disk as part of every successful zone mutation — the save path in the daemon calls `save_scene_store_snapshot` and `persist_runtime_session` before returning the response. If zone changes survive a reload in the same session but vanish after a daemon restart, one of these is true:

1. The save returned an error (5xx) that the UI dismissed silently. Check the daemon log for `Failed to persist zones` around the time of the mutation.
2. You are running the daemon without a writable config directory. Verify the scene store path is accessible:

```bash
# Default config path on Linux
ls -la ~/.config/hypercolor/
```

3. The daemon is starting from a `--config` path that does not match where the store was written. Check `hypercolor service status` or the daemon startup log for the config path in use.

## Studio shows a different layout than /layout

Studio and the `/layout` page edit different objects. Studio's canvas edits the **selected zone's own layout** — it saves through `PUT /api/v1/scenes/{id}/zones/{zone_id}/layout` and is specific to that scene and zone. The `/layout` page edits the **standalone layouts library** (`PUT /api/v1/layouts/{id}`), which is a separate, scene-independent collection of named layout templates.

Changes made in `/layout` do not automatically flow into Studio's zone canvas. If you built a device arrangement in `/layout` and want to use it in a zone, you need to re-create it in the zone canvas (or, via the API, copy the zone entries into the zone's layout and save through the zone PUT).

{% callout(type="info") %}
The standalone `/layout` page and library are on a retirement path. Studio's zone canvas is the canonical way to arrange devices spatially. New work belongs in Studio, not in the library.
{% end %}

## Zone canvas edits are not visible in the live render

You moved device blocks in the zone canvas but the lighting output is not changing position-wise. Possible causes:

**The live preview was not pushed.** The canvas sends position changes to the daemon over the WebSocket as a `zone_layout_preview` message while you drag, throttled to 75 ms intervals. This preview is in-memory on the daemon and does not touch the persisted layout or the global spatial engine. If the WebSocket connection dropped mid-drag, the daemon never received the preview positions.

Check the WebSocket connection indicator in the web UI. If it shows disconnected, reconnect (reload the page) and retry. The drag preview will resume on the next drag.

**The layout was not saved.** The drag preview is temporary. It is cleared as soon as you save or revert, and it does not survive a page reload. The Save button in the zone canvas bar glows when there are unsaved changes. Save after positioning to persist the layout through the zone API and have it take effect in the render engine.

**The effect sampling mode.** See "All LEDs show the same color" above — some effects ignore position.

## Layer ordering is reversed from what I expect

The layer stack is authored bottom-to-top internally, but the UI displays it top-to-bottom (most recent / topmost layer at the top of the list). The "Top" marker shows at the top of the list, "Bottom" at the bottom. Blend modes are applied from bottom to top: the bottom layer is the base, each layer above blends into it using its blend mode.

If an effect at the top of the list is not visible, check its blend mode. The default blend for an effect added to a non-empty stack is Screen; Screen on a very dark base may appear invisible. Switch to Alpha or Replace to see it.

## Zone enable/disable not taking effect

A zone that is disabled (`enabled: false`) still exists in the scene but its outputs receive no colors from the render loop — they hold last colors or go dark depending on the zone's shutdown behavior. The enabled toggle is a zone metadata field and goes through the zone PATCH endpoint at `/api/v1/scenes/{id}/zones/{zone_id}`. Metadata edits like enable/disable, rename, color, and brightness are not structural, so they do not enforce the `groups_revision` precondition — only a structural change (promoting a zone to primary) does. If the toggle fails silently, check the daemon log for a 5xx on the PATCH.

## Unassigned devices are not turning off between zones

The unassigned-lights policy controls what happens to device outputs that are not assigned to any zone in the current scene. The options are:

- **Turn off** — outputs go dark.
- **Hold last colors** — outputs freeze at whatever color they last received.
- **Follow zone** — outputs mirror a specific zone.

If unassigned devices are staying lit when you expect them to turn off, the policy is set to Hold or Follow. Open Studio, select the Unassigned entry in the zone tree (it appears only in scenes with more than one LED zone), and change the policy. The change takes effect immediately.

{% callout(type="info") %}
The Unassigned entry is only visible in genuinely multi-zone scenes. In a single-zone scene, all devices belong to the default zone and there is no Unassigned bucket.
{% end %}

## REST API: debugging zone conflicts directly

If you are scripting zone mutations or building an MCP workflow and hitting 412s, the daemon returns the current `groups_revision` in both the response body and the `ETag` header of any failed precondition response:

```bash
# Fetch the active scene to get its id and current groups_revision
curl -s http://localhost:9420/api/v1/scenes/active | jq '{id: .data.id, rev: .data.groups_revision}'

# Send a structural mutation with the correct revision. The zone
# subroutes resolve the scene by id or name, not by the literal
# "active", so use the id from the call above (or "default").
curl -s -X POST http://localhost:9420/api/v1/scenes/<scene_id>/zones \
  -H 'Content-Type: application/json' \
  -H 'If-Match: "42"' \
  -d '{"name": "Desk"}'
```

The `If-Match` precondition is enforced for structural mutations — creating, deleting, or reassigning zones, promoting a zone to primary, and the zone layout PUT. Pure metadata edits (rename, color, brightness, enable/disable) skip the check, so an `If-Match` header on those is accepted but not validated.

A successful mutation returns a new `groups_revision` in the response body and an updated `ETag` header. Chain sequential mutations by reading the new revision from each response rather than refetching the scene.

For more detail on the zone API contract, see [Zone API and concurrency](@/studio/zone-api-and-concurrency.md).
