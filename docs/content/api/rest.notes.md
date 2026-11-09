<!--
Authored notes for the generated REST reference (docs/content/api/rest.md).

scripts/gen-rest-reference.ts renders every operation from the daemon's
OpenAPI document and merges in the prose below. Zola skips this file
(`ignored_content` in docs/config.toml); run `just rest-reference` after
editing it, and `just rest-reference-check` to confirm the page is current.

Each `# ` line opens one note:

  # @intro                          prose at the top of the page
  # @outro                          prose at the bottom of the page
  # @tag <tag> [Title]              a resource heading's title and intro prose
  # @section <tag> <Title> </path>  a subsection holding that tag's operations
                                    whose path starts with a listed prefix
                                    (the longest matching prefix wins)
  # <METHOD> </path>                prose inside that operation's card

Notes carry what the document cannot: behavior, examples, and caveats. The
card already shows the summary, parameters, request and response types, and
statuses, so do not restate them. A key that names no current operation,
tag, or path prefix fails generation.
-->

# @intro

The Hypercolor daemon serves a REST API over `/api/v1` on port **9420** by
default. The same daemon also speaks [WebSocket](@/api/websocket.md), the
[CLI](@/api/cli.md), and an [MCP server](@/agents/_index.md); this page
covers HTTP only.

## Base URL and surfaces 🎯

```
http://localhost:9420
```

Two paths sit outside the `/api/v1` tree:

| Path | Purpose |
| --- | --- |
| `/health` | Liveness and readiness check. No authentication and no envelope. |
| `/mcp` | MCP server (Streamable HTTP), mounted only when `mcp.enabled` is true. |

Everything else lives under `/api/v1`. Path parameters use brace syntax, so a
device route is `/api/v1/devices/{id}`, not `:id`.

## Conventions

- **Envelope.** A JSON success wraps its payload as `{ data, meta }` and an
  error as `{ error, meta }`. [Envelope & errors](@/api/rest-envelope-and-errors.md)
  documents both shapes, the `meta` fields, and every error code. A card whose
  body reads "no envelope" answers the bare type instead.
- **Collections.** List routes put `items` and `total` under `data`, plus a
  `page` block on the routes that genuinely page. See
  [list responses](@/api/rest-envelope-and-errors.md#list-responses).
- **Authentication.** Loopback clients need no key. Remote clients send
  `Authorization: Bearer <key>`: `HYPERCOLOR_API_KEY` grants control and
  `HYPERCOLOR_READ_API_KEY` grants reads. See
  [auth & security](@/api/auth-and-security.md).
- **Concurrency.** Structural scene writes accept the scene fence as
  `If-Match`, so a stale write returns `412 Precondition Failed` instead of
  overwriting a concurrent edit. Control-value patches never use `If-Match`.
  See [Zone API and concurrency](@/studio/zone-api-and-concurrency.md#optimistic-concurrency).

# @outro

## Where to go next

For the streaming side of the daemon (live frames, spectrum, preview canvases,
and REST-over-WebSocket), see the [WebSocket protocol](@/api/websocket.md). To
drive the same surface from a shell or an agent, see the
[CLI reference](@/api/cli.md) and the [Agents and MCP guide](@/agents/_index.md).
The request and response types are defined once in `hypercolor-types::api` and
shared by the daemon, both UIs, the CLI, and the generated Python client.

# @tag system

# GET /health

Use this in your reconnect loop and readiness probes.

The daemon answers as soon as its port is bound, before startup finishes. While
it is still starting, `/health` returns `503` with `"status": "starting"` and a
`startup` block naming the current `phase`, a `sequence` counter that advances
each time startup completes a unit of work (a phase, or a step inside one such
as loading one effect file or compiling one compositor pipeline), and, while a
step is running, a `detail` naming it. Every other route returns
`503 service_unavailable`. Only `200` means the daemon is ready.

```json
{
  "status": "starting",
  "version": "0.6.2",
  "uptime_seconds": 4,
  "checks": { "render_loop": "starting", "device_backends": "starting", "event_bus": "starting" },
  "startup": {
    "phase": "starting_render_thread",
    "sequence": 17,
    "detail": "SparkleFlinger GPU area horizontal tile scan"
  }
}
```

# GET /api/v1/system

`identity` is always present so discovery probes can verify the daemon.
`status` is present for loopback clients and requests with a valid read or
control key. Anonymous remote requests to a keyed daemon receive the public
identity without the status block.

```json
{
  "data": {
    "identity": {
      "instance_id": "studio-daemon",
      "instance_name": "Studio",
      "version": "0.3.2",
      "device_count": 3,
      "auth_required": true
    },
    "status": {
      "running": true,
      "version": "0.3.2",
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
        "actual_fps": 60.0,
        "output_demand": {
          "parked": false,
          "output_devices": 3,
          "stream_receivers": 2,
          "input_event_streams": 0,
          "demand_holds": 0,
          "parks_total": 1,
          "last_wake_reason": "output_consumer"
        }
      }
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

`render_loop.output_demand` says what consumes rendered frames. With no device,
stream, preview, or capture attached the loop parks: `parked` turns true,
`idle_reason` reads `no_output_consumers`, `parked_ms` counts up, and
`delivered_fps` and `actual_fps` read zero until a consumer appears and the loop
resumes at its current tier.

`status.effect_count` reflects whatever the registry holds at request time
(native built-ins plus discovered HTML effects); treat it as live, not a fixed
product number.

`status.screen_capture_capacity` reports the byte fences that gate
screen-capture publication admission. The fences are installed on Linux and
Windows, where `admission_enforced` is `true` and the capacity fields are
populated; on other platforms the object collapses to
`{ "admission_enforced": false }` with every fence field omitted. When an
analysis plan is active, additional `analysis_*` fields describe its
resolution, byte budgets, and compute capacity.

`status.input` is the host keyboard and mouse capture health snapshot.
`enabled` is the consent gate from config, `host_capturing` reports whether a
host backend is actively reading input, and `devices_opened` versus
`devices_denied` separates "input is off" from "input is on but blocked". The
denied counter counts device nodes that are present but unreadable, a
Linux-specific failure (udev rules missing); Windows has no per-node denial, so
its session-level failure arrives through `degraded` instead, as one of
`no_interactive_session`, `access_denied`, or `unavailable`. Each entry in
`sources` carries per-source lifecycle, freshness, and issue detail.

# GET /api/v1/system/sensors

CPU temperature, GPU load, RAM usage, and raw component readings. These feed
sensor-bound effect controls.

# GET /api/v1/system/audio-devices

Capture devices for reactive effects. Pick the **monitor** of your output, not
a microphone, if you want lights to follow what's playing.

# GET /api/v1/system/audit

Entries come from the daemon's audit trail. Each has the time, transport
(`http`, `websocket`, or `mcp`), method, path without its query string, MCP
tool name, status, the socket peer's address, any `X-Forwarded-For` or
`X-Real-IP` claim (as `forwarded_for`, never trusted for `remote`), user agent,
and the durable stores the request changed. A request whose client disconnects
before the response is recorded with status `499`. Request bodies, query
strings, credentials, and tool arguments are never recorded. The same entries
live on disk in
`${XDG_STATE_HOME:-~/.local/state}/hypercolor/logs/api-audit.jsonl`; see
[Undoing an unwanted change](@/troubleshooting/common-issues.md#undoing-an-unwanted-change).

# GET /api/v1/system/openrgb

Reports installation path and version, configured SDK endpoints, negotiated
protocol versions, controller counts, permission checks, and installation
hints, plus native and bridge coverage and the output-disabled route count.
Endpoints are probed even when the bridge is disabled, so setup can check
readiness before enabling output.

# GET /api/v1/system/conflicts

Competing software includes SignalRGB, L-Connect, and Razer Synapse. The
response carries `supported` (false where the host can't list running
software, macOS today), `scanned`, `scan_failed` (the latest scan failed, so
`conflicts` holds the last successful result), and `conflicts`: each entry has
an `id`, a display `name`, the process and service names it `matched`, the
`driver_ids` it competes with (or `all_drivers`), whether it drives SMBus
lighting, and a `remedy` for the user. The daemon rescans every 30 seconds,
after discovery, and when a device fails, and publishes
`software_conflicts_changed` whenever that status changes. See
[Conflicting software](@/hardware/conflicting-software.md).

# POST /api/v1/system/conflicts/scan

Returns the same body as `GET /api/v1/system/conflicts`.

# @tag drivers

# GET /api/v1/drivers/{id}/config

Driver settings are read here rather than through `GET /api/v1/config`, which
masks every `drivers` entry.

# @tag devices

{{< img path="img/ui/ui-devices.webp" alt="The devices panel in the web UI" />}}

# GET /api/v1/devices

This route genuinely pages: `limit` defaults to 50 and anything above 200 is
rejected with `validation_error`. Follow `page.has_more`, advancing `offset` by
the number of items you received, until it reads `false`. Add
`?include=attachments` to embed each device's attachment profile in the same
response.

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
        "segments": []
      }
    ],
    "total": 1,
    "page": {
      "offset": 0,
      "limit": 50,
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

# GET /api/v1/devices/unclaimed

Each item includes vendor and product IDs, identifying strings, bus path,
interface classes, and a `claimable_by` driver when native support exists but
is disabled. Subscribe to `unclaimed_devices_changed` to refresh after arrivals
and removals.

# GET /api/v1/devices/coverage

Joins native devices, OpenRGB routes, and unclaimed hardware by physical
identity. Each row reports its native and bridge matches and active owner:
`native`, `bridge`, `none`, or `conflict`. A bridge match includes its
effective output state and disabled reason.

# GET /api/v1/devices/{id}

Segments, LED layout, firmware version, and attachment configuration.

# PUT /api/v1/devices/{id}

Updates `name`, `enabled`, `brightness` (0 to 100), and, for display-capable
devices, `display_rotation` (`deg0`, `deg90`, `deg180`, `deg270`), which is how
the panel is mounted so everything drawn on it turns to read upright. Sending
`display_rotation` for hardware without a panel answers 422.

# DELETE /api/v1/devices/{id}

Removes a device from tracking and deletes its saved outputs, attachments, and
logical segments from all layouts and scenes, including the default scene.

# POST /api/v1/devices/forget

Deletes a saved controller by its layout identity, even when the hardware is
absent from device tracking. Removal clears its outputs from all layouts and
scenes, including the default scene. When retained ownership identifies the
physical controller, removal also clears its attachments and logical segments.
An empty identity returns 422; repeating a completed removal succeeds.
Temporary disconnection preserves saved content.

# POST /api/v1/devices/discover

Runs a discovery scan across every backend and returns the newly found devices.

Each entry in `scanners` has a `status` of `ok`, `partial`, or `error`. A
`partial` scanner may have missed devices and lists why in `gaps`. What it did
see still applies. A gap that names one device holds only that device, and any
other gap keeps all of that scanner's devices from being reported vanished.

# POST /api/v1/devices/{id}/pair

Starts pairing for a device that requires authentication (Hue link button,
Nanoleaf hold-to-pair token). This is the credential path for network devices;
see the per-vendor hardware guides for the timed pairing windows.

# DELETE /api/v1/devices/{id}/pair

Forgets the device's stored pairing credentials.

# POST /api/v1/devices/{id}/identify

Flashes the device's LEDs so you can spot it physically.

# PUT /api/v1/devices/{id}/attachments

Send `"validate_only": true` to return the computed profile without persisting
it, publishing events, or changing the live device.

# @tag controls Control surfaces

Control surfaces expose typed fields and actions for dynamic device or driver
configuration (WLED protocol selection, Hue bridge IP, and the like). The web
UI reads these to render device-specific settings panels.

# PATCH /api/v1/control-surfaces/{id}/values

The body is the same control-patch shape the layer-control route takes, keyed
by field id. Control surfaces have no input bindings, so a non-empty
`clear_bindings` is rejected here.

```json
{
  "values": {
    "protocol": { "kind": "enum", "value": "ddp" },
    "ip": { "kind": "ip", "value": "10.0.0.50" }
  }
}
```

# POST /api/v1/control-surfaces/{id}/actions/{action}

Actions are typed operations such as Discover, Sync, and Reset.

# @tag effects

{{< img path="img/ui/effects.webp" alt="Browsing the effect catalog in the web UI" />}}

Live effect state belongs to `GET /scene`. Patch controls through the real
layer id embedded in that document, and clear the show through
`POST /scene/clear`. Spatial layout selection belongs to `scene.layout_id`;
effects do not carry layout associations.

# GET /api/v1/effects

The catalog combines around a dozen native Rust built-ins with the HTML and
GLSL effects discovered on disk. Don't hardcode the count; read `data.total`.
The catalog answers complete, so it carries no `data.page` block.

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
    "total": 59
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

# GET /api/v1/effects/{id}

The `controls` array (types, ranges, defaults) is what a UI renders into
sliders, color pickers, and dropdowns.

# POST /api/v1/effects/{id}/apply

Replaces the target zone's layer stack with one new layer running this effect.
The server validates the effect, zone, and controls before committing, mints a
fresh layer id, then wakes paused output.

```json
{
  "zone": "84b20af9-0700-4b82-8488-88314b87fb5c",
  "controls": {
    "speed": { "kind": "float", "value": 7.0 },
    "palette": { "kind": "enum", "value": "SilkCircuit" }
  },
  "transition": { "type": "cut" }
}
```

Omit `zone` to target the primary zone, which the daemon creates if needed. The
response contains the updated zone resource, including the new layer id, the
applied transition, and the output-wake outcome. A post-commit wake failure is
reported inside a `200` response. Repair output through `PATCH /output` instead
of retrying apply, because every apply creates another layer id.

# GET /api/v1/effects/{id}/presets

Lists the bundled and saved presets available for this effect.

# POST /api/v1/effects/{id}/presets/{preset}/apply

Applies the preset through the same stack-replacement contract as
`POST /effects/{id}/apply`. Preset CRUD remains under `/library/presets`, but
the library does not expose a second apply route.

# POST /api/v1/effects/rescan

Picks up newly built effects without restarting the daemon. Call this after
shipping an effect from the SDK.

# POST /api/v1/effects/install

Gets a freshly built HTML bundle into the library without a manual file copy.
Uploading a file name that is already installed replaces that effect in place
and keeps its id, so a scene already running it reloads the new page on its
next frame.

# GET /api/v1/effects/console

Holds what HTML effect pages printed: `console.*` output and uncaught script
errors, oldest first. Locations in an effect's own page point at the author's
source file and line, not the engine's runtime copy with its injected preamble.
The daemon keeps the most recent 512 messages.

```json
{
  "entries": [
    {
      "seq": 42,
      "timestamp_ms": 1760300000000,
      "level": "error",
      "message": "TypeError: ctx is null at file:///home/me/fx/ember.html:57:9",
      "effect_name": "Ember",
      "effect_id": "0199c1f2-3a4b-7c5d-8e6f-0a1b2c3d4e5f",
      "source_file": "ember.html",
      "line": 57
    }
  ],
  "next_seq": 42,
  "truncated": false
}
```

This is a cursor window, not a paged list. Pass `next_seq` back as `after` to
read only newer output. `truncated: true` means messages after your cursor
were evicted before the read.

# GET /api/v1/effects/{id}/cover

Serves curated art when present, else the image the effect ships in
`<meta cover>`, else a frame the daemon captured once the effect had the canvas
to itself for a few seconds. Effects with none of these advertise no
`cover_image_url`.

Covers are named by the SHA-256 of their bytes. `cover_image_url` carries it as
`?v=<sha256>`, and the `ETag` is `"sha256-<sha256>"`. A request whose `v` names
the cover served gets `Cache-Control: public, max-age=31536000, immutable`; any
other request gets `no-cache`. `If-None-Match` with the current `ETag` answers
`304 Not Modified`. A cache can check what it holds by hashing it.

# @tag canvas

The canvas is the composed image every effect layer draws into before the
spatial sampler maps it onto LEDs. It is rendered at full brightness, so it
shows what an effect looks like rather than how bright the hardware is set.

# GET /api/v1/canvas/frame

The request waits up to 1.5 seconds for a frame rendered after it arrives, so
the image reflects the scene as it is now, then returns the raw image bytes
rather than a JSON envelope. Use `png` for exact pixel values.

Captures never upscale past the canvas size. The response carries
`X-Canvas-Frame-Number`, `X-Canvas-Timestamp-Ms`, `X-Canvas-Width`,
`X-Canvas-Height`, `X-Canvas-Source-Width`, and `X-Canvas-Source-Height`.
`X-Canvas-Fresh: 0` means nothing rendered during the wait and the image is the
last frame published before rendering went quiet. When the newest published
canvas is empty, as before anything has rendered, the route answers `503`.

```bash
curl -s 'http://127.0.0.1:9420/api/v1/canvas/frame?format=png&width=320' -o canvas.png
```

# @tag assets

# @tag media

# POST /api/v1/media/authorize

Requests macOS Automation access for an already-running media application. The
endpoint requires a control credential and accepts either Apple Music or
Spotify:

```json
{
  "adapter": "music"
}
```

The response reports the selected adapter and whether access was authorized.
The daemon returns `422` when the application is not running and `409` when
Automation access is unavailable or denied.

# @tag displays Displays and faces

Display devices are physical screens (AIO LCD modules, Ableton Push 2) that
show full-screen HTML faces. See [display faces](@/effects/display-faces.md)
for the authoring contract.

# GET /api/v1/displays/{id}/frame

A JPEG preview frame from the display. Live frame streaming runs over the
`display_preview` WebSocket channel.

# PUT /api/v1/displays/{id}/face

Binds an HTML effect to the device in the active scene.

# PATCH /api/v1/displays/{id}/face/composition

Composition parameters are the blend mode, z-order, and opacity of the face's
render group.

# @section displays Simulators /api/v1/simulators

Virtual display simulators let you build and test face effects with no physical
display attached.

# GET /api/v1/simulators/displays/{id}/frame

The latest composited frame from the simulated display.

# @tag attachments

Attachment templates describe physical accessories (keycaps, case panels,
stands) that clip onto device slots and carry their own LED zones.

# GET /api/v1/attachments/templates

Lists built-in and user-defined templates. This route pages: `limit` defaults
to 50 and anything above 200 is rejected with `validation_error`.

# @tag output

Global output has one resource and two knobs. Pausing preserves the live scene,
its effects, and their controls: devices hold the configured static off color
until you set power back to `running`.

# GET /api/v1/output

```json
{
  "power": "running",
  "brightness": 0.8
}
```

`power` is `running` or `paused`. A destructive stop leaves outputs dark, so it
reads as `paused` here; the stop's other consequences are visible on the effect
surface. `brightness` is a float on `0.0` to `1.0`.

# PATCH /api/v1/output

Every field is optional, but a document that sets neither returns `422` rather
than quietly succeeding, so a client that drops its payload hears about it.

```json
{
  "power": "paused",
  "brightness": 0.35
}
```

A brightness outside `0.0` to `1.0` returns `422` naming the offending field,
and it is refused before power moves, so a rejected patch changes nothing.

# @tag scenes

Scenes are whole-rig configurations: the effects, zones, and assignments that
define how your entire setup lights up. Switching scenes swaps the whole rig.

{{< img path="img/ui/ui-scenes.webp" alt="The scenes panel in the web UI" />}}

# @section scenes Stored scenes /api/v1/scenes

Stored scenes are edited as whole documents with `PUT /scenes/{id}`; the
fine-grained editing routes below address the live scene only.

# POST /api/v1/scenes

New scenes are born with a default Primary zone, live mutation mode, and the
engine's default scene transition.

```json
{
  "name": "Late Night",
  "description": "Dim amber for late sessions",
  "enabled": true,
  "mutation_mode": "live"
}
```

# POST /api/v1/scenes/snapshot

Captures the complete live scene as a new snapshot-mode scene. The snapshot
keeps the active scene's zones, members, layers, controls, display faces, and
current named layout reference. Global output brightness is not captured.

```json
{
  "name": "Current Rig",
  "description": "Captured after tuning the desk"
}
```

# GET /api/v1/scenes/{id}

Returns the stored scene as a complete document, including its zones, members,
layouts, and layer stacks. The response carries the document's `revision` and
`run_epoch`, and their fence, `"<run_epoch>:<revision>"`, as the `ETag` header.

# PUT /api/v1/scenes/{id}

Replaces the stored scene in full. Read the current document first, apply the
intended edits, and send the whole document back. The read-only fields the
read carried are checked, not written: the body's `revision` and `run_epoch`
together act exactly like `If-Match`, so a document read before someone else's
write, or in an earlier daemon run, is refused with `412 Precondition Failed`.
`If-Match` remains accepted, and when both are sent each must be current.

The route id is authoritative. If the body includes `id`, it must match the
route or the daemon returns `422 Unprocessable Entity`; `kind` and `is_default`
likewise must match the stored scene. Existing zone and layer ids must already
belong to this scene. Omit either id only when creating that resource, and the
daemon mints it. Omitted optional fields are cleared, so partial update bodies
are not accepted. A stale `If-Match` returns `412 Precondition Failed` with the
current revision.

# POST /api/v1/scenes/{id}/activate

Applies the scene's effects and controls with the configured transition. The
response reports the post-commit layout and brightness outcomes separately,
because either side effect may fail after the scene switch has committed. Send
`{}` to use the scene's authored transition, or pass `{ "transition_ms": 250 }`
to override its duration for this activation.

# @section scenes Live scene /api/v1/scene

# GET /api/v1/scene

The response always exists and embeds every authored zone, each zone's member
device segments, and every layer with its real id. The response's `ETag` is the
document's fence, `"<run_epoch>:<revision>"`, ready to send back as `If-Match`.

# PATCH /api/v1/scene

Changes the live scene's name or `unassigned_behavior`. The default scene
cannot be renamed.

# POST /api/v1/scene/deactivate

Returns to the default scene and answers with the new live scene document.

# POST /api/v1/scene/clear

Clears every non-display layer stack, or pass `{ "zone": "<zone_uuid>" }` to
clear one non-display zone. Display zones remain owned by the display API, and
a targeted display clear is rejected. This is the canonical stop gesture.

# @section scenes Scene zones /api/v1/scene/zones /api/v1/scene/members

Zones are flexible partitions of the live scene's canvas. Each zone owns member
device segments and a layer stack. Fine-grained editing is live-tree-only under
`/scene`; stored scenes use whole-document `PUT /scenes/{id}`.

{{< img path="img/ui/ui-studio-zones.webp" alt="Building zones in Studio" />}}

# POST /api/v1/scene/zones

```json
{
  "name": "Desk",
  "color": "#80ffea"
}
```

# PATCH /api/v1/scene/zones/{zone}

Updates a zone's name, enabled state, brightness, or color.

```json
{
  "name": "Desk halo",
  "brightness": 0.8,
  "enabled": true
}
```

# DELETE /api/v1/scene/zones/{zone}

The default and display zones cannot be deleted through this route.

# POST /api/v1/scene/zones/{zone}/members

Assigns one device's segments to the zone. The response carries the minted
member ids, which are the resource identities for later removal.

```json
{
  "device_id": "razer:huntsman-v3",
  "segments": ["left", "right"]
}
```

# DELETE /api/v1/scene/zones/{zone}/members/{member}

Removes one membership by the member id returned in the live zone document.

# POST /api/v1/scene/members/edit

Adds, moves, or removes outputs in one atomic transaction. Supply the active
`scene_id` and the scene fence in `If-Match`, then choose one request form:

- **Explicit changes:** a `changes` array containing `before` and `after`
  states. Each state contains `zone_id`, the complete `output`, and its ordered
  `index`. A null `before` adds an output; a null `after` removes it.
- **Device assignment:** an `assignment` object containing the target
  `zone_id`, `device_id`, optional `segments`, and optional `placements`.
  Leave `changes` empty. The daemon creates canonical outputs for the device;
  placement hints preserve prepared geometry without choosing device bindings.

All changes validate before committing. A failed request leaves membership
unchanged. The response contains the committed `document` and canonical
`changes` receipts with complete output snapshots for undo and redo. The
committed revision is available in `document.revision` and the response `ETag`.

Omitting `If-Match` or sending `*` is rejected. A stale revision returns `412`.
Explicit edits also validate their `before` states, so rebasing requires fresh
member states from the current document.

# PUT /api/v1/scene/zones/{zone}/layout

Replaces the zone-scoped spatial placement override. The compact body contains
`placements`, keyed by member id. Add or remove members through the member
routes.

# @section scenes Scene layers /api/v1/scene/zones/{zone}/layers

Each zone stacks layers bottom to top. Clients use the layer ids returned by
`GET /scene`; they never derive an id from the zone.

# POST /api/v1/scene/zones/{zone}/layers

Appends a layer to the zone. The server mints its id.

# PATCH /api/v1/scene/zones/{zone}/layers/order

Send every layer id exactly once, from bottom to top.

# PUT /api/v1/scene/zones/{zone}/layers/{layer}

Every successful replacement mints a fresh layer id, even when the effect is
unchanged.

# PATCH /api/v1/scene/zones/{zone}/layers/{layer}/controls

Send `{ "values": {...}, "clear_bindings": [...] }`. Control patches never use
`If-Match`. A vanished layer returns `404 layer_not_found`.

# @tag layouts

Layouts define how the effect canvas maps onto physical LED positions, in
normalized `[0.0, 1.0]` coordinates so effects stay resolution-independent.

# GET /api/v1/layouts

This route pages: `limit` defaults to 50 and anything above 200 is rejected
with `validation_error`.

# PUT /api/v1/layouts/active/preview

Returns the zone-to-LED mapping the layout would produce without applying it,
so a UI can render it visually.

# GET /api/v1/layouts/{id}

Device outputs, positions, and LED mappings.

# POST /api/v1/layouts/{id}/apply

Makes the layout the active spatial mapping.

# @tag library

# @section library Favorites /api/v1/library/favorites

A fresh install starts with a few curated favorites, which the daemon seeds
once on its very first run. An existing install never gets them, even one whose
favorites list is empty.

# POST /api/v1/library/favorites

```json
{
  "effect_id": "borealis"
}
```

# DELETE /api/v1/library/favorites/{effect}

The path key is the effect ID, not a favorite ID.

# @section library Presets /api/v1/library/presets

A preset is an effect plus a set of control values. Apply one through
`POST /api/v1/effects/{effect}/presets/{preset}/apply`: the effect-scoped route
is the only apply contract, and the library owns storage and CRUD.

# POST /api/v1/library/presets

Saves the current effect and controls as a named preset.

# @section library Playlists /api/v1/library/playlists

# POST /api/v1/library/playlists

A playlist is a sequence of effects with transition timing.

# GET /api/v1/library/playlists/active

Answers with the running playlist, if any.

# POST /api/v1/library/playlists/{id}/activate

Effects cycle on the playlist's timing.

# @tag capture Capture and input

The four protected capture operations below require protected control: the
control-tier `HYPERCOLOR_API_KEY`, the desktop app's session credential, or a
trusted in-process connection. Every other caller receives `403 Forbidden`,
including a keyless loopback client, a read key, and a credential from a
downstream credential authority. Where the request comes from plays no part.

The system status response keeps the selected session source identifier only
for a caller with protected control. Every other response replaces application
and window selection identifiers with `session_scoped`; stable display UUIDs
remain available for diagnostics.

# POST /api/v1/input/authorize

Asks the process that owns host keyboard capture to request Input Monitoring
access. The response reports whether access is currently authorized and names
the process topology that owns the grant.

# POST /api/v1/capture/authorize

Asks the process that owns screen capture to request Screen Recording access.
The response reports whether access is currently authorized and names the
process topology that owns the grant.

# PUT /api/v1/capture/source

Lets the user choose a display, window, or application for screen-reactive
effects. An accepted display persists by its stable display UUID. Window and
application choices persist as `session_scoped`, so Hypercolor remembers the
privacy boundary without writing the selected window ID or bundle ID to
configuration. Cancelling the picker leaves the current source unchanged.

# GET /api/v1/capture/monitors

Builds a monitor picker. Each entry carries a ready-to-store `value` for the
`capture.source` config key.

```json
{
  "items": [
    {
      "index": 0,
      "id": "DP-1",
      "name": "\\\\.\\DISPLAY1",
      "width": 2560,
      "height": 1440,
      "primary": true,
      "value": "monitor:DP-1"
    }
  ],
  "total": 1
}
```

The list is empty on platforms where the backend picks its own source (the XDG
portal on Linux); a UI uses that emptiness to decide between a monitor dropdown
and the portal picker button.

# @tag config Configuration

# GET /api/v1/config

Secret-classified sections render masked as `{"redacted": true}`: every
`drivers` entry, plus any top-level section this build does not model. Driver
settings are read through `GET /api/v1/drivers/{id}/config`.

# GET /api/v1/config/keys/{key}

The dotted key is a single path segment.

# PUT /api/v1/config/keys/{key}

The request body is the value itself:

```json
true
```

Add `?live=false` to persist without re-applying the change to the running
daemon; the default re-applies every live-classified key.

The response carries the effective value, whether the daemon applied it live,
whether the key is boot-frozen (`requires_restart`), and which sections are
currently waiting on a restart (`pending_restart`).

The response also says whether the config file holds the change. When the file
write fails (a read-only config directory, a full disk), the change still takes
effect in the running daemon, which keeps retrying the file, and the response
reports `"persisted": false` with the failure in `persistence_error`. A restart
before a retry lands loads the older file. The `config` diagnose check reports
the same lag until it clears. The reset routes report it the same way.

Writes that rebuild the live screen-capture source are the exception: a key in
the `capture` section, or a whole-config reset, with live apply on. They stage
the new file before touching the running source, so a config directory that
refuses the staged file fails the request with `500` and leaves the live value
unchanged.

# PATCH /api/v1/config/keys/{key}

Unmentioned entries survive; `null` removes an entry. The daemon merges and
validates against the current config under its write lock, so concurrent
patches to different OpenRGB controllers or zones preserve each other's sizes.
The response and `?live=` option match PUT.

# DELETE /api/v1/config/keys/{key}

Takes the same `?live=` query parameter as the write.

# POST /api/v1/config/reset

The `drivers` map, unmodeled extension sections, and the include list survive
the reset.

# GET /api/v1/config/schema

Describes every configuration key: how a change applies (`live` with a section,
`live_on_read`, `next_scan`, `restart`, or `inert`), how it renders on read
surfaces, whether the daemon validates it beyond type checking, and which
writes need a protected-control credential (`protection`: `open`,
`section_root`, or `tree`). Clients derive their live and restart affordances
from this table.

# @tag backups

Backups transfer authored lighting configuration, including scenes, layouts,
presets, favorites, and portable preferences. Self-installed effect files and
local media stay on their respective machines. Importing an older archive
verifies and discards its file payloads; it does not install them or remove
existing local resources.

# GET /api/v1/backups/export

The archive downloads with attachment filename `hypercolor.hypercolor-backup`.
Newly exported archives contain no effect or media blobs.

# POST /api/v1/backups/import

Send the raw archive bytes to validate them and stage a restore preview. The
response includes an `operation_id`, configuration counts, and resource
warnings. The preview does not apply the uploaded settings. It captures the
destination revision and relevant preferences for the subsequent apply.

# POST /api/v1/backups/apply

Send the staged preview's operation ID:

```json
{"operation_id": "019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25"}
```

The restore merges configuration and favorites and applies the saved
selection. Reusing the same staged operation ID replays its durable result
after a lost response. If the destination changed after preview, the apply
returns `409 Conflict`; import the archive again to obtain a fresh preview.

Both preview and apply responses include `unresolved_devices` (device names)
and `missing_resources` (effect, media, and preset ID arrays under `effects`,
`media`, and `presets`). Missing resources keep their authored references;
unavailable render sources remain inactive while other configuration restores.
Install effects or reconnect media separately where needed.

# @tag diagnostics

# POST /api/v1/diagnose

Checks device connectivity, audio capture, effect-engine health, memory, and
configuration validity. Memory failures are reported as the named `memory`
check in the same response. The `conflicts` check scans for competing RGB
software before it reports, as `devices.competing_software`. On Windows the
`devices` checks include `devices.smbus_broker`, which fails when the
`HypercolorSmBus` broker is installed but not running. The `diagnose` CLI
command and MCP tool use this exact check vocabulary.

# @tag websocket WebSocket

# GET /api/v1/ws

Upgrades to the `hypercolor-v1` WebSocket subprotocol. Its channels, JSON
messages, and binary frames are outside what OpenAPI can describe; the
[WebSocket protocol](@/api/websocket.md) and
[binary frame format](@/api/websocket-binary-frames.md) pages document them.
