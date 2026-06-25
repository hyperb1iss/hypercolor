+++
title = "REST API reference"
description = "Full /api/v1 HTTP reference for the Hypercolor daemon: the JSON envelope, every route group, and the concurrency model."
weight = 10
template = "page.html"
+++

The Hypercolor daemon serves a REST API over `/api/v1` on port **9420** by
default. Every route group below is enumerated from the daemon's own router
(`build_router()` in `crates/hypercolor-daemon/src/api/mod.rs`), so this page is
the contract, not a curated subset. The same daemon also speaks
[WebSocket](@/api/websocket.md), the [CLI](@/api/cli.md), and an
[MCP server](@/agents/_index.md); this page covers HTTP only.

## Base URL and surfaces 🎯

```
http://localhost:9420
```

Three paths sit outside the `/api/v1` tree:

| Path | Purpose |
| --- | --- |
| `/health` | Liveness check, no auth, returns `200 OK` when the daemon is up. |
| `/preview` | Standalone canvas-preview HTML page. |
| `/mcp` | MCP server (Streamable HTTP), mounted only when `mcp.enabled` is true. |

Everything else lives under `/api/v1`. Axum 0.8 path parameters use brace
syntax, so a device route is `/api/v1/devices/{id}`, not `:id`.

## Response envelope

Every JSON response, success or error, carries a `meta` block. Success
responses put the payload under `data`; errors put it under `error`. The two
keys never both appear.

```json
{
  "data": {},
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

The `meta` fields are fixed by the daemon:

| Field | Shape | Notes |
| --- | --- | --- |
| `api_version` | string `"1.0"` | The literal envelope version. It is unrelated to the `v1` URL segment and never reads `"v1"`. |
| `request_id` | string `req_<uuid-v7>` | A `req_` prefix plus a time-ordered UUID v7. Quote it when filing a bug or correlating logs. |
| `timestamp` | ISO 8601 UTC | Millisecond precision with a trailing `Z`. |

Error bodies replace `data` with `error`:

```json
{
  "error": {
    "code": "validation_error",
    "message": "brightness must be between 0.0 and 1.0",
    "details": null
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

The `code` is a `snake_case` string that maps to an HTTP status. The full set:

| `code` | HTTP status |
| --- | --- |
| `bad_request` | 400 |
| `unauthorized` | 401 |
| `forbidden` | 403 |
| `not_found` | 404 |
| `conflict` | 409 |
| `payload_too_large` | 413 |
| `unsupported_media_type` | 415 |
| `validation_error` | **422** |
| `rate_limited` | 429 |
| `internal_error` | 500 |

{% callout(type="info") %}
`validation_error` is **422 Unprocessable Entity**, not 400. A well-formed
request that fails a business rule (out-of-range brightness, an effect that
isn't runnable) lands here, while a structurally malformed request is
`bad_request` / 400.
{% end %}

## Authentication

Loopback clients are exempt from API keys, which is why the local CLI, TUI, and
web UI work with no configuration. When you bind the daemon to a non-loopback
address or configure a key, send it as a Bearer token:

```
Authorization: Bearer <your-api-key>
```

There are two keys: `HYPERCOLOR_API_KEY` grants control (writes), and
`HYPERCOLOR_READ_API_KEY` grants read-only access. CORS allows loopback origins
unconditionally; configured `cors_origins` are only honored once API auth is
enabled. The auth and rate-limiting model is documented in full on the
[auth and security](@/api/_index.md) overview.

## Concurrency: revisions and `If-Match`

Scene-zone structural edits use optimistic concurrency. A `GET` on a scene's
zones returns a `groups_revision` and an `ETag` header carrying the same
revision. Send that value back as `If-Match` on the mutating request. If the
revision is stale, the daemon rejects the write with `412 Precondition Failed`
rather than clobbering a concurrent edit. The Studio zone editor relies on this
to stay coherent across multiple clients.

---

## System

{% api_endpoint(method="GET", path="/health") %}
Liveness check. Returns `200 OK` when the daemon is running. No authentication,
no envelope. Use this in your reconnect loop and readiness probes.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/status") %}
Aggregate system status: the running effect, connected device count, audio
availability, global brightness, and live render-loop timing.

**Response:**

```json
{
  "data": {
    "running": true,
    "version": "0.1.0",
    "device_count": 3,
    "effect_count": 59,
    "active_effect": "borealis",
    "global_brightness": 85,
    "audio_available": true,
    "render_loop": {
      "state": "running",
      "target_fps": 60,
      "actual_fps": 59.8
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

`effect_count` reflects whatever the registry holds at request time (native
built-ins plus discovered HTML effects); treat it as live, not a fixed product
number.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/server") %}
Stable server identity: instance ID, instance name, and version. This is the
same identity advertised over discovery.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/system/sensors") %}
Latest hardware sensor snapshot: CPU temperature, GPU load, RAM usage, and raw
component readings. These feed sensor-bound effect controls.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/system/sensors/{label}") %}
A single named sensor reading. Common labels: `cpu_temp`, `gpu_load`,
`ram_used`.
{% end %}

## Effects

![Browsing the effect catalog in the web UI](/img/ui/effects.webp)

{% api_endpoint(method="GET", path="/api/v1/effects") %}
List the effect catalog. Returns `data.items` (effect summaries) plus
`data.pagination`. Supports the standard `offset` / `limit` query params.

**Response:**

```json
{
  "data": {
    "items": [
      {
        "id": "borealis",
        "name": "Borealis",
        "description": "Aurora borealis with domain-warped fBm noise",
        "author": "Hypercolor",
        "category": "ambient",
        "source": "html",
        "runnable": true,
        "tags": ["ambient", "shader"],
        "version": "1.0.0",
        "audio_reactive": false
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 59,
      "has_more": false
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

The catalog combines around a dozen native Rust built-ins with the HTML/GLSL
effects discovered on disk. Don't hardcode the count; read `pagination.total`.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}") %}
Full detail for one effect, including its control definitions (types, ranges,
defaults). The `controls` array is what a UI renders into sliders, color
pickers, and dropdowns.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/{id}/apply") %}
Apply an effect to the active output. Optionally override control defaults.

**Request body (optional):**

```json
{
  "controls": {
    "speed": 7,
    "palette": "SilkCircuit"
  }
}
```

**Response:** the applied effect, the resolved control values, any layout
binding, the resolved transition (`cut`, `0` today), and a `warnings` array.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/active") %}
The currently active effect and its live control values.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/effects/current/controls") %}
Patch controls on the running effect. Changes take effect on the next frame.
Note the path segment is `current`, not `active`.

**Request body:**

```json
{
  "controls": {
    "speed": 3,
    "intensity": 90
  }
}
```
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/effects/current/controls/{name}/binding") %}
Bind one named control on the running effect to an input source (audio band,
sensor reading, etc.) so it modulates live instead of holding a fixed value.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/current/reset") %}
Reset every control on the running effect back to its default.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/effects/{id}/controls") %}
Patch controls on a specific effect by ID, whether or not it is the active one.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/stop") %}
Stop the running effect. Output goes dark.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/rescan") %}
Rescan the effects directory and pick up newly built effects without restarting
the daemon. Call this after shipping an effect from the SDK.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/install") %}
Install an effect from an uploaded file via multipart form upload, so a freshly
built HTML bundle reaches the library without a manual file copy.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}/cover") %}
Cover image for one effect.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/active/cover") %}
Cover image for the active effect.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}/layout") %}
Get the layout bound to an effect.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/effects/{id}/layout") %}
Bind an effect to a spatial layout.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/effects/{id}/layout") %}
Clear an effect's layout binding.
{% end %}

Effect screenshots are served statically under
`/api/v1/effects/screenshots/...` from the bundled screenshot root.

## Devices

![The devices panel in the web UI](/img/ui/ui-devices.webp)

{% api_endpoint(method="GET", path="/api/v1/devices") %}
List discovered and connected devices. Returns `data.items` plus
`data.pagination`.

**Response:**

```json
{
  "data": {
    "items": [
      {
        "id": "razer-blackwidow-v4-001",
        "layout_device_id": "razer-blackwidow-v4-001",
        "name": "Razer BlackWidow V4",
        "status": "connected",
        "brightness": 100,
        "total_leds": 126,
        "zones": []
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 1,
      "has_more": false
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}") %}
Full detail for one device: zones, LED layout, firmware version, attachment
configuration.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/devices/{id}") %}
Update device settings (name, brightness, zone assignments).
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/devices/{id}") %}
Remove a device from tracking.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/discover") %}
Trigger a discovery scan across every backend. Returns newly found devices.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/pair") %}
Initiate pairing for a device that requires authentication (Hue link button,
Nanoleaf hold-to-pair token). This is the credential path for network devices;
see the per-vendor hardware guides for the timed pairing windows.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/devices/{id}/pair") %}
Forget a device's stored pairing credentials.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/identify") %}
Flash a device's LEDs so you can spot it physically.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/zones/{zone_id}/identify") %}
Flash one zone on a device to identify it.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/attachments/{slot_id}/identify") %}
Flash one attachment slot's LEDs to identify it.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}/controls") %}
Control surface for a device: fields, types, and current values.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}/attachments") %}
Attachment configuration for a device.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/devices/{id}/attachments") %}
Update a device's attachment configuration.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/devices/{id}/attachments") %}
Clear a device's attachment configuration.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/attachments/preview") %}
Preview attachment placement without persisting it.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/{id}/logical-devices") %}
List logical-device segments carved out of one physical device.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/{id}/logical-devices") %}
Create a logical-device segment on a physical device.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/devices/metrics") %}
Per-device output telemetry snapshot: frame counts, errors, latency.
{% end %}

The router also exposes `/api/v1/devices/debug/queues` and
`/api/v1/devices/debug/routing` for inspecting output queue and routing state
while debugging.

## Logical devices

Logical devices are user-defined LED-range segments carved out of a physical
device so one strip can act as several addressable units.

{% api_endpoint(method="GET", path="/api/v1/logical-devices") %}
List every logical-device segment across all physical devices.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/logical-devices/{id}") %}
Get one logical-device segment.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/logical-devices/{id}") %}
Update a logical-device segment.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/logical-devices/{id}") %}
Delete a logical-device segment.
{% end %}

## Drivers

{% api_endpoint(method="GET", path="/api/v1/drivers") %}
List registered driver modules with their ID, name, and connection state.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/drivers/{id}/config") %}
Configuration for one driver module.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/drivers/{id}/controls") %}
Control surface for one driver module: fields, types, current values.
{% end %}

## Displays and faces

Display devices are physical screens (AIO LCD modules, Ableton Push 2) that show
full-screen HTML faces. See [display faces](@/effects/display-faces.md) for the
authoring contract.

{% api_endpoint(method="GET", path="/api/v1/displays") %}
List connected display devices.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/displays/{id}/preview.jpg") %}
A JPEG preview frame from a display device. Live frame streaming runs over the
`display_preview` WebSocket channel.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/displays/{id}/face") %}
The active face configuration on a display device.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/displays/{id}/face") %}
Set the face effect on a display device. Binds an HTML effect to the device in
the active scene.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/displays/{id}/face") %}
Remove the face assignment from a display device.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/displays/{id}/face/controls") %}
Patch control values on a display's active face.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/displays/{id}/face/composition") %}
Patch composition parameters (blend mode, z-order, opacity) for a face render
group.
{% end %}

## Simulators

Virtual display simulators let you build and test face effects with no physical
display attached.

{% api_endpoint(method="GET", path="/api/v1/simulators/displays") %}
List simulated displays.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/simulators/displays") %}
Create a simulated display.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/simulators/displays/{id}") %}
Get one simulated display.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/simulators/displays/{id}") %}
Update a simulated display's configuration.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/simulators/displays/{id}") %}
Delete a simulated display.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/simulators/displays/{id}/frame") %}
The latest composited frame from a simulated display.
{% end %}

## Attachments

Attachment templates describe physical accessories (keycaps, case panels,
stands) that clip onto device slots and carry their own LED zones.

{% api_endpoint(method="GET", path="/api/v1/attachments/templates") %}
List attachment templates (built-in and user-defined).
{% end %}

{% api_endpoint(method="POST", path="/api/v1/attachments/templates") %}
Create a user-defined attachment template.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/attachments/templates/{id}") %}
Get one attachment template.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/attachments/templates/{id}") %}
Update a user-defined attachment template.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/attachments/templates/{id}") %}
Delete a user-defined attachment template.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/attachments/categories") %}
List attachment categories (keycap-set, case-panel, stand, etc.).
{% end %}

{% api_endpoint(method="GET", path="/api/v1/attachments/vendors") %}
List attachment vendors that have templates available.
{% end %}

## Control surfaces

Control surfaces expose typed fields and actions for dynamic device or driver
configuration (WLED protocol selection, Hue bridge IP, and the like). The web
UI reads these to render device-specific settings panels.

{% api_endpoint(method="GET", path="/api/v1/control-surfaces") %}
List every registered control surface across devices and drivers.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/control-surfaces/{surface_id}") %}
Get one control surface with its current field values.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/control-surfaces/{surface_id}/values") %}
Apply typed field values to a control surface.

**Request body:**

```json
{
  "fields": {
    "protocol": { "type": "enum", "value": "ddp" },
    "ip_address": { "type": "ip", "value": "10.0.0.50" }
  }
}
```
{% end %}

{% api_endpoint(method="POST", path="/api/v1/control-surfaces/{surface_id}/actions/{action_id}") %}
Invoke a typed control-surface action (Discover, Sync, Reset, and so on).
{% end %}

## Scenes

Scenes are whole-rig configurations: the effects, zones, and assignments that
define how your entire setup lights up. Switching scenes swaps the whole rig.

![The scenes panel in the web UI](/img/ui/ui-scenes.webp)

{% api_endpoint(method="GET", path="/api/v1/scenes") %}
List defined scenes.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes") %}
Create a scene with an effect, controls, and an optional transition.

**Request body:**

```json
{
  "name": "Late Night",
  "effect_id": "ambient-glow",
  "controls": { "speed": 2, "brightness": 30 },
  "transition": { "duration_ms": 2000, "easing": "ease_in_out" }
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/active") %}
The currently active scene.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}") %}
One scene's configuration.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}") %}
Update a scene.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}") %}
Delete a scene.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/activate") %}
Activate a scene, applying its effects and controls with the configured
transition.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/deactivate") %}
Deactivate the current scene, returning to the default free-running state.
{% end %}

### Scene zones

Zones are flexible partitions of the scene's canvas. Each zone owns a set of
device outputs and renders its own effect. Zones live **under** a scene; there
is no top-level `/zones` collection.

![Building zones in Studio](/img/ui/ui-studio-zones.webp)

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}/zones") %}
List a scene's zones. The response includes `groups_revision` and an `ETag`
header carrying the same revision for optimistic concurrency.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/zones") %}
Create a zone in a scene. Send `If-Match` with the last seen `groups_revision`;
a stale revision returns `412 Precondition Failed`.

**Request body:**

```json
{
  "name": "Desk",
  "color": "#80ffea"
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}/zones/{zone_id}") %}
Get one scene zone.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/zones/{zone_id}") %}
Update zone metadata. Set `make_primary` to make a zone the default output
zone; that structural edit should carry `If-Match`.

**Request body:**

```json
{
  "name": "Desk halo",
  "description": "Ambient strips behind the monitor",
  "brightness": 0.8,
  "enabled": true
}
```
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}/zones/{zone_id}") %}
Delete a zone. The default and display zones cannot be deleted through this
route.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/zones/{zone_id}/devices") %}
Assign device outputs to a zone. Each item may reference an existing
device-output ID or carry a full payload.

**Request body:**

```json
{
  "device_zones": [
    { "id": "keyboard-left" }
  ]
}
```
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}/zones/{zone_id}/devices/{device_zone_id}") %}
Unassign one device output from a zone.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}/zones/{zone_id}/layout") %}
Update one zone's spatial layout. The route takes a `SpatialLayout` payload and
preserves the zone's existing output roster, so it is for placement edits only;
add or remove outputs through the device routes above. Structural edits should
carry `If-Match`.

**Request body:**

```json
{
  "id": "default-zone-layout",
  "name": "Default zone",
  "canvas_width": 640,
  "canvas_height": 480,
  "zones": [],
  "default_sampling_mode": { "type": "bilinear" },
  "default_edge_behavior": "clamp",
  "spaces": null,
  "version": 1
}
```
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/unassigned-behavior") %}
Set how outputs not claimed by any zone should render.

**Request body:**

```json
{
  "unassigned_behavior": "off"
}
```

Values are `"off"`, `"hold"`, or `{ "fallback": "<zone_uuid>" }`.
{% end %}

### Scene layers

Each zone (render group) stacks layers: effects, faces, and media composited
with a blend mode and opacity.

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}/groups/{group_id}/layers") %}
List the layers in a zone.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/groups/{group_id}/layers") %}
Add a layer to a zone.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/groups/{group_id}/layers/order") %}
Reorder the layers in a zone.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}") %}
Update one layer (blend mode, opacity, transform, color, source binding).
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}") %}
Delete a layer.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}/controls") %}
Patch the control values on one layer's source effect.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/layers/broadcast-media") %}
Broadcast a media layer across the scene's zones in one call.
{% end %}

## Profiles

Profiles save a full state snapshot (effect, controls, brightness, assignments)
that you can restore later.

{% api_endpoint(method="GET", path="/api/v1/profiles") %}
List saved profiles.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/profiles") %}
Create a profile from the current state.

**Request body:**

```json
{
  "name": "Gaming"
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/profiles/{id}") %}
Get one profile's saved state.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/profiles/{id}") %}
Update a profile.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/profiles/{id}") %}
Delete a profile.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/profiles/{id}/apply") %}
Apply a profile, restoring its effect, controls, and assignments.
{% end %}

## Layouts

Layouts define how the effect canvas maps onto physical LED positions, in
normalized `[0.0, 1.0]` coordinates so effects stay resolution-independent.

{% api_endpoint(method="GET", path="/api/v1/layouts") %}
List spatial layouts.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/layouts") %}
Create a spatial layout.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/layouts/active") %}
The active layout.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/layouts/active/preview") %}
Preview a layout without applying it. Returns the zone-to-LED mapping that would
result, so a UI can render it visually.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/layouts/{id}") %}
One layout's configuration: device zones, positions, LED mappings.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/layouts/{id}") %}
Update a layout.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/layouts/{id}") %}
Delete a layout.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/layouts/{id}/apply") %}
Apply a layout as the active spatial mapping.
{% end %}

## Library

The library holds favorites, presets, and playlists.

### Favorites

{% api_endpoint(method="GET", path="/api/v1/library/favorites") %}
List favorited effects.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/favorites") %}
Add an effect to favorites.

**Request body:**

```json
{
  "effect_id": "borealis"
}
```
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/library/favorites/{effect}") %}
Remove an effect from favorites. The path key is the effect ID, not a favorite
ID.
{% end %}

### Presets

{% api_endpoint(method="GET", path="/api/v1/library/presets") %}
List saved presets (effect plus control-value combinations).
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/presets") %}
Save the current effect and controls as a named preset.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/library/presets/{id}") %}
Get one preset.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/library/presets/{id}") %}
Update a preset.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/library/presets/{id}") %}
Delete a preset.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/presets/{id}/apply") %}
Apply a preset.
{% end %}

### Playlists

{% api_endpoint(method="GET", path="/api/v1/library/playlists") %}
List playlists.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/playlists") %}
Create a playlist of effects with transition timing.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/library/playlists/active") %}
The currently running playlist, if any.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/library/playlists/{id}") %}
Get one playlist.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/library/playlists/{id}") %}
Update a playlist.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/library/playlists/{id}") %}
Delete a playlist.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/playlists/{id}/activate") %}
Start a playlist. Effects cycle on the playlist's timing.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/library/playlists/stop") %}
Stop the running playlist.
{% end %}

## Settings and audio

{% api_endpoint(method="GET", path="/api/v1/settings/brightness") %}
The current global brightness level.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/settings/brightness") %}
Set global brightness, `0.0` to `1.0`.

**Request body:**

```json
{
  "brightness": 0.8
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/audio/devices") %}
List available audio capture devices for reactive effects. Pick the **monitor**
of your output, not a microphone, if you want lights to follow what's playing.
{% end %}

## Screen capture

{% api_endpoint(method="POST", path="/api/v1/capture/source/pick") %}
Open the platform picker so the user can choose a screen or window capture
source for screen-reactive effects.
{% end %}

## Configuration

{% api_endpoint(method="GET", path="/api/v1/config") %}
Show the full current configuration.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/config/get?key=path.to.key") %}
Get one configuration value by dotted key path.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/config/set") %}
Set a configuration value.

**Request body:**

```json
{
  "key": "audio.enabled",
  "value": true
}
```
{% end %}

{% api_endpoint(method="POST", path="/api/v1/config/reset") %}
Reset a configuration value to its default.

**Request body:**

```json
{
  "key": "audio.device_name"
}
```
{% end %}

## Diagnostics

{% api_endpoint(method="POST", path="/api/v1/diagnose") %}
Run system diagnostics: device connectivity, audio capture, effect-engine
health, and configuration validity. This is the same check the `diagnose` CLI
command and MCP tool run.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/diagnose/memory") %}
A memory diagnostics snapshot: daemon RSS (which includes the in-process Servo
renderer), canvas buffer size, and allocation counters. Useful when chasing slow
memory growth.
{% end %}

## Assets

User media (images, video) used by media layers.

{% api_endpoint(method="GET", path="/api/v1/assets") %}
List media assets.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/assets") %}
Upload a media asset.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/assets/{id}") %}
Get asset metadata.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/assets/{id}") %}
Update asset metadata.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/assets/{id}") %}
Delete an asset.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/assets/{id}/blob") %}
Fetch the raw asset bytes.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/assets/{id}/thumbnail") %}
Fetch the asset thumbnail.
{% end %}

---

## Where to go next

For the streaming side of the daemon (live frames, spectrum, preview canvases,
and REST-over-WebSocket), see the [WebSocket protocol](@/api/websocket.md). To
drive the same surface from a shell or an agent, see the
[CLI reference](@/api/cli.md) and the
[Agents and MCP guide](@/agents/_index.md). The request and response body shapes
for the devices, effects, scenes, and zones domains are defined once in
`hypercolor-types::api` and shared by the daemon and both UIs.
