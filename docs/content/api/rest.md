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

Two paths sit outside the `/api/v1` tree:

| Path | Purpose |
| --- | --- |
| `/health` | Liveness check, no auth, returns `200 OK` when the daemon is up. |
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
    "message": "brightness must be between 0 and 100"
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
| `malformed_request` | 400 |
| `unauthorized` | 401 |
| `forbidden` | 403 |
| `not_found` | 404 |
| `conflict` | 409 |
| `precondition_failed` | 412 |
| `payload_too_large` | 413 |
| `unsupported_media_type` | 415 |
| `validation_error` | **422** |
| `rate_limited` | 429 |
| `internal_error` | 500 |
| `device_unavailable` | 503 |

{% callout(type="info") %}
`validation_error` is **422 Unprocessable Entity**, not 400. A well-formed
request that fails a business rule (out-of-range brightness, an effect that
isn't runnable) lands here, while a structurally malformed request is
`malformed_request` / 400.
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
[auth and security](@/api/auth-and-security.md) page.

## Concurrency: revisions and `If-Match`

The live scene document has one concurrency token: `revision`. `GET /scene`
returns it in the document and in the `ETag` header. Structural mutations may
send that value as `If-Match`; a stale value returns `412 Precondition Failed`
with the current revision instead of overwriting a concurrent edit.

Control-value patches never use `If-Match`. They address a real layer id read
from the live document. Replacing a layer mints a fresh id, so a stale control
write returns `404 layer_not_found` rather than landing on the replacement.

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
    "screen_capture_capacity": {
      "admission_enforced": true,
      "physical_transition_byte_capacity": 268435456,
      "physical_transition_backend_capacity": 4,
      "physical_reserved_bytes": 33177600,
      "physical_available_bytes": 235257856,
      "steady_total_byte_budget": 134217728,
      "steady_total_backend_capacity": 2,
      "steady_publication_byte_budget": 134217728,
      "transition_publication_backend_capacity": 2
    },
    "input": {
      "enabled": true,
      "host_capture_registered": true,
      "host_capturing": true,
      "devices_opened": 3,
      "devices_denied": 1,
      "degraded": "access_denied",
      "backends": ["evdev"],
      "source_graph_generation": 2,
      "sources": []
    },
    "render_loop": {
      "state": "running",
      "target_fps": 60,
      "capacity_fps": 60.0,
      "delivered_fps": 59.8,
      "actual_fps": 60.0
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

`screen_capture_capacity` reports the byte fences that gate screen-capture
publication admission. The fences are installed on Linux and Windows, where
`admission_enforced` is `true` and the capacity fields are populated; on other
platforms the object collapses to `{ "admission_enforced": false }` with every
fence field omitted. When an analysis plan is active, additional
`analysis_*` fields describe its resolution, byte budgets, and compute
capacity.

`input` is the host keyboard/mouse capture health snapshot. `enabled` is the
consent gate from config, `host_capturing` reports whether a host backend is
actively reading input, and `devices_opened` versus `devices_denied` separates
"input is off" from "input is on but blocked". The denied counter counts
device nodes that are present but unreadable, a Linux-specific failure (udev
rules missing); Windows has no per-node denial, so its session-level failure
arrives through `degraded` instead, as one of `no_interactive_session`,
`access_denied`, or `unavailable`. Each entry in `sources` carries per-source
lifecycle, freshness, and issue detail.
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

{% api_endpoint(method="GET", path="/api/v1/system/audio-devices") %}
List available audio capture devices for reactive effects. Pick the **monitor**
of your output, not a microphone, if you want lights to follow what's playing.
{% end %}

## Effects

{{ img(path="img/ui/effects.webp", alt="Browsing the effect catalog in the web UI") }}

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
Replace the target zone's layer stack with one new layer running this effect.
The server validates the effect, zone, and controls before committing, mints a
fresh layer id, then wakes paused output.

**Request body (optional):**

```json
{
  "zone": "84b20af9-0700-4b82-8488-88314b87fb5c",
  "controls": {
    "speed": { "float": 7.0 },
    "palette": { "enum": "SilkCircuit" }
  },
  "transition": { "type": "cut" }
}
```

Omit `zone` to target the primary zone, which the daemon creates if needed.
The response contains the updated zone resource, including the new layer id,
the applied transition, and the output-wake outcome. A post-commit wake failure
is reported inside a `200` response. Repair output through `PATCH /output`
instead of retrying apply, because every apply creates another layer id.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/effects/{id}/presets") %}
List bundled and saved presets available for one effect.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/effects/{id}/presets/{preset_id}/apply") %}
Apply one effect-scoped preset through the same stack-replacement contract as
`POST /effects/{id}/apply`. Preset CRUD remains under `/library/presets`, but
the library does not expose a second apply route.
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

Live effect state belongs to `GET /scene`. Patch controls through the real
layer id embedded in that document, and clear the show through
`POST /scene/clear`. Spatial layout selection belongs to `scene.layout_id`;
effects do not carry layout associations.

Effect screenshots are served statically under
`/api/v1/effects/screenshots/...` from the bundled screenshot root.

## Devices

{{ img(path="img/ui/ui-devices.webp", alt="The devices panel in the web UI") }}

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

{% api_endpoint(method="GET", path="/api/v1/devices/bindings") %}
New in 0.3.0. Surfaces the two halves of a re-bind decision: layout bindings
that no attached device currently resolves, and attached devices that no
layout references.

**Response:**

```json
{
  "unresolved": [
    {
      "layout_device_id": "wled-desk-strip",
      "layout_ids": ["desk-ring"],
      "rebindable": true
    }
  ],
  "candidates": [
    {
      "device_id": "0197a2f4-6c1e-7d3a-9b02-4f8e1c5a7d90",
      "name": "WLED Desk Strip",
      "layout_device_id": "wled-desk-strip-2",
      "status": "connected",
      "portable_key": "net:aabbccddeeff"
    }
  ]
}
```

`rebindable` reports whether a recorded identity exists for the binding, which
is what a durable re-bind needs to inherit. A device that is reconnecting
(hardware that vanished) is surfaced as an orphaned binding rather than a
candidate, but its recorded identity remains inheritable. Candidates without a
`portable_key` can only be re-bound by editing the layout.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/devices/rebind") %}
New in 0.3.0. Executes a re-bind: re-pins the chosen device's portable key
onto the orphaned binding's recorded identity, which heals the layouts without
editing them and holds across restarts.

**Request body:**

```json
{
  "layout_device_id": "wled-desk-strip",
  "device_id": "0197a2f4-6c1e-7d3a-9b02-4f8e1c5a7d90"
}
```

**Response:** the `device_id`, the `layout_device_id` the device now resolves
to, and the `portable_key` that was re-pinned.

A binding can only be inherited within its driver; a cross-driver re-bind is
rejected with `422` before anything mutates. When the binding's current device
is still renderable, the call returns `409 Conflict` and nothing is replaced.
An unknown device or a binding with no recorded identity returns `404`.
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

{{ img(path="img/ui/ui-scenes.webp", alt="The scenes panel in the web UI") }}

{% api_endpoint(method="GET", path="/api/v1/scenes") %}
List defined scenes.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes") %}
Create a named scene. New scenes are born with a default Primary zone, live
mutation mode, and the engine's default scene transition.

**Request body:**

```json
{
  "name": "Late Night",
  "description": "Dim amber for late sessions",
  "enabled": true,
  "mutation_mode": "live"
}
```
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/snapshot") %}
Capture the complete live scene as a new snapshot-mode scene. The snapshot
keeps the active scene's zones, members, layers, controls, display faces, and
current named layout reference. Global output brightness is not captured.

**Request body:**

```json
{
  "name": "Current Rig",
  "description": "Captured after tuning the desk"
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scene") %}
Read the complete live scene document. The response always exists and embeds
every authored zone, each zone's member device segments, and every layer with
its real id. The document's `revision` is also returned as `ETag`.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scenes/{id}") %}
Read one stored scene as a complete document, including its zones, members,
layouts, and layer stacks. The response carries the document's `revision` and
the same value as an `ETag` header.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scenes/{id}") %}
Replace one stored scene in full. Read the current document first, remove the
server-owned `revision` and `is_default` fields, apply the intended edits, and
send the result with the previous revision in `If-Match`.

The route id is authoritative. If the body includes `id`, it must match the
route or the daemon returns `422 Unprocessable Entity`. Existing zone and
layer ids must already belong to this scene. Omit either id only when creating
that resource, and the daemon mints it. Omitted optional fields are cleared,
so partial update bodies are not accepted. A stale `If-Match` returns `412
Precondition Failed` with the current revision.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scenes/{id}") %}
Delete a scene.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scenes/{id}/activate") %}
Activate a scene, applying its effects and controls with the configured
transition. The response reports the post-commit layout and brightness
outcomes separately, because either side effect may fail after the scene switch
has committed.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene") %}
Patch the live scene's name or `unassigned_behavior`. The default scene cannot
be renamed. This structural write optionally accepts `If-Match`.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/deactivate") %}
Return to the default scene and receive the new live scene document.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/clear") %}
Clear every non-display layer stack, or pass `{ "zone": "<zone_uuid>" }` to
clear one non-display zone. Display zones remain owned by the display API, and
a targeted display clear is rejected. This is the canonical stop gesture and
optionally accepts `If-Match`.
{% end %}

### Scene zones

Zones are flexible partitions of the live scene's canvas. Each zone owns member
device segments and a layer stack. Fine-grained editing is live-tree-only under
`/scene`; stored scenes use whole-document `PUT /scenes/{id}`.

{{ img(path="img/ui/ui-studio-zones.webp", alt="Building zones in Studio") }}

{% api_endpoint(method="POST", path="/api/v1/scene/zones") %}
Create a custom zone. Send `If-Match` with the last seen scene `revision` when
you need optimistic concurrency; a stale revision returns
`412 Precondition Failed`.

**Request body:**

```json
{
  "name": "Desk",
  "color": "#80ffea"
}
```
{% end %}

{% api_endpoint(method="GET", path="/api/v1/scene/zones/{zone}") %}
Get one live zone resource.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene/zones/{zone}") %}
Update a zone's name, enabled state, brightness, or color. The structural write
optionally accepts `If-Match`.

**Request body:**

```json
{
  "name": "Desk halo",
  "brightness": 0.8,
  "enabled": true
}
```
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scene/zones/{zone}") %}
Delete a zone. The default and display zones cannot be deleted through this
route.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/zones/{zone}/members") %}
Assign one device's segments to a zone. The response carries the minted member
ids, which are the resource identities for later removal.

**Request body:**

```json
{
  "device_id": "razer:huntsman-v3",
  "segments": ["left", "right"]
}
```
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scene/zones/{zone}/members/{member}") %}
Remove one membership by the member id returned in the live zone document.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scene/zones/{zone}/layout") %}
Replace the zone-scoped spatial placement override. The compact body contains
`placements`, keyed by member id. Add or remove members through the member
routes. This structural write optionally accepts `If-Match`.
{% end %}

### Scene layers

Each zone stacks layers bottom to top. Clients use the layer ids returned by
`GET /scene`; they never derive an id from the zone.

{% api_endpoint(method="GET", path="/api/v1/scene/zones/{zone}/layers") %}
List the layers in a zone.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/scene/zones/{zone}/layers") %}
Append a layer to a zone. The server mints its id. This structural write
optionally accepts `If-Match`.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene/zones/{zone}/layers/order") %}
Reorder the stack with every layer id exactly once, from bottom to top.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/scene/zones/{zone}/layers/{layer}") %}
Replace a whole layer. Every successful replacement mints a fresh layer id,
even when the effect is unchanged.
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/scene/zones/{zone}/layers/{layer}") %}
Delete a layer.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/scene/zones/{zone}/layers/{layer}/controls") %}
Patch an effect layer with `{ "values": {...}, "clear_bindings": [...] }`.
Control patches never use `If-Match`. A vanished layer returns
`404 layer_not_found`.
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

Apply a preset through
`POST /api/v1/effects/{effect}/presets/{preset}/apply`. The effect-scoped route
is the only apply contract; the library owns storage and CRUD.

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

## Output

Global output has one resource and two knobs. Pausing preserves the live
scene, its effects, and their controls: devices hold the configured static
off color until you set power back to `running`.

{% api_endpoint(method="GET", path="/api/v1/output") %}
Read global output power and brightness.

**Response:**

```json
{
  "power": "running",
  "brightness": 0.8
}
```

`power` is `running` or `paused`. A destructive stop leaves outputs dark, so
it reads as `paused` here; the stop's other consequences are visible on the
effect surface. `brightness` is a float on `0.0` to `1.0`.
{% end %}

{% api_endpoint(method="PATCH", path="/api/v1/output") %}
Set power, brightness, or both. Every field is optional, but a document that
sets neither returns `422` rather than quietly succeeding, so a client that
drops its payload hears about it. Use `GET` to read.

**Request body:**

```json
{
  "power": "paused",
  "brightness": 0.35
}
```

A brightness outside `0.0` to `1.0` returns `422` naming the offending field,
and it is refused before power moves, so a rejected patch changes nothing.
{% end %}

## Screen capture

The four protected capture operations below only accept local requests. A
remote client receives `403 Forbidden` even when it presents a valid control
key. The locality decision uses the socket peer and only trusts forwarded
addresses from a loopback proxy.

The system status response keeps the selected session source identifier for a
local request. Any remote response replaces application and window selection
identifiers with `session_scoped`; stable display UUIDs remain available for
diagnostics.

{% api_endpoint(method="POST", path="/api/v1/input/authorize") %}
Request Input Monitoring authorization from the process that owns host keyboard
capture. The response reports whether access is currently authorized and names
the process topology that owns the grant.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/capture/authorize") %}
Request Screen Recording authorization from the process that owns screen
capture. The response reports whether access is currently authorized and names
the process topology that owns the grant.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/capture/source/pick") %}
Open the platform picker so the user can choose a display, window, or application
for screen-reactive effects. An accepted display persists by its stable display
UUID. Window and application choices persist as `session_scoped`, so Hypercolor
remembers the privacy boundary without writing the selected window ID or bundle
ID to configuration. Cancelling the picker leaves the current source unchanged.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/capture/monitors") %}
New in 0.3.0. List the display outputs the capture backend can address, for
building a monitor picker. Each entry carries a ready-to-store `value` for the
`capture.source` config key.

**Response:**

```json
[
  {
    "index": 0,
    "id": "DP-1",
    "name": "\\\\.\\DISPLAY1",
    "width": 2560,
    "height": 1440,
    "primary": true,
    "value": "monitor:DP-1"
  }
]
```

The list is empty on platforms where the backend picks its own source (the
XDG portal on Linux); a UI uses that emptiness to decide between a monitor
dropdown and the portal picker button.
{% end %}

## Configuration

{% api_endpoint(method="GET", path="/api/v1/config") %}
Show the full current configuration.

Secret-classified sections render masked as `{"redacted": true}`: every
`drivers` entry, plus any top-level section this build does not model. Driver
settings are read and edited through `/api/v1/drivers/{id}/config`.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/config/keys/{key}") %}
Read one configuration value. The dotted key is a single path segment.
{% end %}

{% api_endpoint(method="PUT", path="/api/v1/config/keys/{key}") %}
Write one configuration value and persist it. The request body is the value
itself:

```json
true
```

Add `?live=false` to persist without re-applying the change to the running
daemon; the default re-applies every live-classified key.

The response carries the effective value, whether the daemon applied it live,
whether the key is boot-frozen (`requires_restart`), and which sections are
currently waiting on a restart (`pending_restart`).
{% end %}

{% api_endpoint(method="DELETE", path="/api/v1/config/keys/{key}") %}
Restore one configuration value to its default. Takes the same `?live=` query
parameter as the write.
{% end %}

{% api_endpoint(method="POST", path="/api/v1/config/reset") %}
Restore the whole configuration to defaults. The `drivers` map, unmodeled
extension sections, and the include list survive the reset.
{% end %}

{% api_endpoint(method="GET", path="/api/v1/config/schema") %}
Describe every configuration key: how a change applies (`live` with a section,
`live_on_read`, `next_scan`, `restart`, or `inert`), how it renders on read
surfaces, and whether the daemon validates it beyond type checking. Clients
derive their live and restart affordances from this table.
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
