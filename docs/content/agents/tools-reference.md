+++
title = "Tools reference"
description = "All 17 Hypercolor MCP tools: arguments, defaults, enums, read-only and idempotency flags, and a worked call for each."
weight = 20
template = "page.html"
+++

The Hypercolor MCP server exposes **17 tools**, the verbs an agent uses to read and reshape the lighting state. This page is the authoritative reference: every tool's arguments, defaults, enums, annotations, and a worked call. All facts here are pulled from the daemon source in `crates/hypercolor-daemon/src/mcp/tools/`, not paraphrased.

{% callout(type="warning") %}
The MCP server is **off by default**. Until you enable it in config, `http://127.0.0.1:9420/mcp` returns 404. Turn it on first in [MCP setup](@/agents/mcp-setup.md), then come back here.
{% end %}

If you have not met the three-primitive model yet, start at the [Agents & MCP overview](@/agents/_index.md). Tools are one of the three primitives; the others are [resources](@/agents/resources-reference.md) (browsable state) and [prompt templates](@/agents/prompt-templates.md) (guided flows).

## How tools are annotated 🎯

Every tool carries two annotations the daemon reports to MCP clients, plus two constants that never change.

| Annotation | Meaning |
| --- | --- |
| `read_only` | The tool only reads state and never mutates it. Most clients skip the confirmation dialog for read-only tools. |
| `idempotent` | Repeating the call with the same arguments lands on the same state. Safe to retry. |
| `destructive` | The tool overwrites state you cannot get back. Reported per tool, not as a blanket value. |
| `open_world` | Always `false`; the tool set is closed and known. |

Of the 17 tools, **8 are read-only**: `get_status`, `list_effects`, `get_devices`, `get_audio_state`, `get_sensor_data`, `list_scenes`, `get_layout`, and `diagnose`. The other 9 mutate state. Every mutating tool advertises `idempotent: true` except one: `create_scene` is the only non-idempotent tool, since each call mints a new scene.

Six of the nine mutating tools are destructive: `set_effect`, `set_color`, `stop_effect`, `activate_scene`, `set_profile`, and `set_display_face` each discard something the caller did not supply and cannot recover, such as the running effect's live control values or a display's assigned face. The other three are not: `set_brightness` and `set_output_power` are reversible value writes, and `create_scene` only adds.

{% callout(type="tip") %}
Read-then-act is the through-line. The server's own instructions tell every client to call `get_status` or read `hypercolor://state` before making changes, and to call `list_effects` before applying visuals. Follow that order and your calls land predictably.
{% end %}

## Errors and the call envelope

Tool calls return a structured JSON payload on success. On failure they return a structured error object with a JSON-RPC code and a message:

```json
{ "code": -32602, "message": "invalid parameter 'brightness': must be between 0 and 100" }
```

The code maps from the daemon's `ToolError` type:

| Code | Condition |
| --- | --- |
| `-32601` | Tool name not found |
| `-32602` | Missing or invalid parameter |
| `-32000` | State conflict (the current state rejects the mutation) |
| `-32603` | Internal execution error |

{% callout(type="info") %}
Tool output schemas are intentionally broad right now. The shapes shown below are what the live handlers actually return, read straight from the source, not from the declared output schema, which is a placeholder that will tighten as the surface stabilizes.
{% end %}

---

## Effects

### set_effect

Apply a lighting effect. The `query` argument is fuzzy-matched against effect names, partial names, and natural-language descriptions of the desired visual. It returns the matched effect, a confidence score, and alternatives.

- **Mutates state.** `read_only: false`, `idempotent: true`.
- **Required:** `query` (string): effect name or natural-language description.
- **Optional:** `controls` (object): parameter overrides as key-value pairs.

The schema is closed: `query` and `controls` are the only arguments. Effect switches are immediate cuts, and the tool takes no transition argument rather than accepting one it would ignore. The response still reports the `transition_ms` the daemon applied, which is always `0` today.

A display-face effect cannot be applied through `set_effect`; it returns an invalid-parameter error pointing you at [`set_display_face`](#set-display-face).

```json
{
  "name": "set_effect",
  "arguments": {
    "query": "calm blue waves",
    "controls": { "speed": 2 }
  }
}
```

Response (abridged):

```json
{
  "matched_effect": { "id": "...", "name": "Deep Current", "category": "ambient" },
  "confidence": 0.82,
  "alternatives": [ { "name": "Ink Tide", "score": 0.61 } ],
  "applied": true,
  "applied_controls": { "speed": 2 },
  "rejected_controls": {},
  "transition_ms": 0
}
```

### list_effects

Browse the effect catalog with optional filters. Read-only, idempotent. Returns effect names, descriptions, categories, tags, and each effect's control schema.

- **Optional:** `category` (enum): one of `ambient`, `audio`, `generative`, `particle`, `scenic`, `interactive`, `fun`, `source`, `utility`, `display`; `audio_reactive` (boolean): filter to audio-reactive effects; `query` (string): full-text search across names, descriptions, and tags; `limit` (integer, default `20`, range 1-100); `offset` (integer, default `0`).

```json
{ "name": "list_effects", "arguments": { "category": "audio", "limit": 10 } }
```

The response carries `effects`, `total`, `has_more`, `limit`, and `offset`. The catalog is large and growing, so always page rather than hardcoding a count; browse [the effects gallery](@/effects/_index.md) for the visual side.

{{ img(path="img/ui/effects.webp", alt="Effect gallery in the Hypercolor UI") }}

### stop_effect

Stop the currently running effect, clearing its live controls and preset provenance and releasing network-device ownership. LEDs go dark unless a fallback is configured. Mutates state, destructive, idempotent. For a reversible blackout use [`set_output_power`](#set-output-power) instead.

- **Arguments:** none. The stop is immediate, so the tool takes no fade duration.

```json
{ "name": "stop_effect", "arguments": {} }
```

### set_color

Set a solid color globally. Under the hood this applies the `solid_color` effect; it is not a separate device mode. Mutates state, idempotent.

The `color` argument accepts CSS color names (`coral`, `dodgerblue`), hex (`#ff6ac1`), `rgb()`, `hsl()`, and natural-language descriptions (`warm sunset orange`, `deep ocean blue`), all resolved by the daemon's fuzzy color resolver.

- **Required:** `color` (string).
- **Optional:** `brightness` (integer, range 0-100); `transition_ms` (integer, default `0`): the daemon performs immediate cuts, so `0` is the only value it accepts and anything else is refused rather than silently ignored.

```json
{ "name": "set_color", "arguments": { "color": "#e135ff", "brightness": 70 } }
```

The response includes `resolved_color` (with `hex`, `name`, and `rgb`), `applied`, `applied_controls`, and `device_count`.

### set_output_power

Pause or resume all output without discarding the active effect, its controls, preset provenance, or scene state. Mutates state, idempotent, and **not** destructive: it is the reversible counterpart to [`stop_effect`](#stop-effect).

- **Required:** `state` (enum): `running` or `paused`.

```json
{ "name": "set_output_power", "arguments": { "state": "paused" } }
```

The response echoes the resulting `state`. Pausing blacks out the rig while the render pipeline keeps its place, so resuming picks up exactly where it left off.

---

## Devices

### get_devices

Enumerate known RGB devices with connection status, driver origin, output backend, LED count, and zone configuration. Read-only, idempotent.

- **Optional:** `status` (enum, default `all`): one of `all`, `connected`, `disconnected`; `driver_id` (string): filter by driver module id; `backend_id` (string): filter by output backend id.

```json
{ "name": "get_devices", "arguments": { "status": "disconnected" } }
```

The response carries a `devices` array plus a `summary` with `total`, `connected`, and `total_leds`.

{{ img(path="img/ui/ui-devices.webp", alt="Connected devices in the Hypercolor UI") }}

### set_brightness

Set the global brightness level. Brightness is a **percentage from 0 to 100** (not a 0.0-1.0 float); the daemon normalizes it internally. Mutates state, idempotent.

- **Required:** `brightness` (integer, range 0-100).
- **Arguments:** `brightness` only. Brightness is global and the change is immediate, so the tool exposes neither a device target nor a fade duration.

```json
{ "name": "set_brightness", "arguments": { "brightness": 35 } }
```

The response reports the applied `brightness`, its `scope` (always `global`), and the previous global brightness.

---

## Scenes

Scenes are whole-rig configurations: a scene bundles effects, device assignments, brightness, and transitions into one preset. (Within a scene, [zones](@/studio/_index.md) are flexible canvas partitions; they are not the same thing.)

### activate_scene

Activate a named scene by exact name or fuzzy query. Mutates state, idempotent.

- **Required:** `name` (string).
- **Optional:** `transition_ms` (integer, default `1000`, range 0-10000).

```json
{ "name": "activate_scene", "arguments": { "name": "Evening Calm" } }
```

If no scene matches, the call succeeds with `"activated": false` and a message suggesting `list_scenes`, rather than erroring.

{{ img(path="img/ui/ui-scenes.webp", alt="Scenes in the Hypercolor UI") }}

### list_scenes

List available scenes with names, descriptions, and trigger configuration. Read-only, idempotent. Ephemeral scenes are excluded.

- **Optional:** `enabled_only` (boolean, default `false`).

```json
{ "name": "list_scenes", "arguments": { "enabled_only": true } }
```

Each entry includes `id`, `name`, `description`, `enabled`, `mutation_mode`, and an `active` flag.

### create_scene

Create a new scene. This is the **only non-idempotent tool**: `read_only: false`, `idempotent: false`. It is more constrained than "save the current state" implies, requiring three arguments.

- **Required:** `name` (string); `profile_id` (string): must reference an existing profile, or the call fails with an invalid-parameter error; `trigger` (object): its `type` is one of `schedule`, `sunset`, `sunrise`, `device_connect`, `device_disconnect`, `audio_beat`, `webhook`. The type is recorded on the scene; the daemon has no scheduler behind it yet, so a trigger does not fire on its own and the tool takes no cron expression.
- **Optional:** `description` (string); `enabled` (boolean, default `true`); `mutation_mode` (enum, default `live`): `live` lets runtime effect and display-face actions rewrite the scene, `snapshot` freezes it. There is no transition argument: creating a scene renders nothing.

```json
{
  "name": "create_scene",
  "arguments": {
    "name": "Sunset Warmth",
    "profile_id": "prof_01J...",
    "trigger": { "type": "sunset" }
  }
}
```

The response returns `scene_id`, `name`, `enabled`, and `mutation_mode`.

---

## System

### get_status

Get the current daemon state: active effect, global brightness, connected device count, effect and scene counts, FPS metrics, audio and screen input status, and uptime. Read-only, idempotent. Takes no arguments.

This is the tool to call first. The reported `fps.target` is the current adaptive tier (the render loop shifts between 10/20/30/45/60 Hz). `fps.capacity` is a capacity estimate: the theoretical throughput derived from smoothed frame time, capped at the tier. `fps.actual` mirrors `fps.capacity`, so it is **not** measured delivery; the real delivery rate is the separate `fps.delivered` field. Never read any of them as a fixed ceiling.

```json
{ "name": "get_status", "arguments": {} }
```

Response (abridged):

```json
{
  "running": true,
  "paused": false,
  "brightness": 70,
  "fps": { "target": 60, "capacity": 59.4, "delivered": 58.7, "actual": 59.4 },
  "effect": { "id": "...", "name": "Borealis" },
  "effect_count": 59,
  "scene_count": 4,
  "devices": { "connected": 3, "total": 4, "total_leds": 412 },
  "inputs": { "audio": "enabled", "screen": "disabled" },
  "uptime_seconds": 8123,
  "version": "..."
}
```

### get_audio_state

Get the current audio analysis: overall level, bass/mid/treble energy, beat detection, beat confidence, and a BPM estimate. Read-only, idempotent. Takes no arguments.

```json
{ "name": "get_audio_state", "arguments": {} }
```

The response carries `enabled`, a `levels` object (`overall`, `bass`, `mid`, `treble`), a `beat` object (`detected`, `confidence`, `bpm_estimate`), and `spectrum_bins`. For a streaming view of the same data, read the [`hypercolor://audio` resource](@/agents/resources-reference.md), which updates at roughly 10 Hz.

### get_layout

Get the current spatial layout: device positions, zones, and topology. Read-only, idempotent. Takes no arguments.

```json
{ "name": "get_layout", "arguments": {} }
```

The response carries a `layout` object (`id`, `name`, `description`, `canvas_width`, `canvas_height`, `zone_count`), a `zones` array, and `total_devices` plus `total_leds`. The canvas defaults to 640×480 but is configurable, so read the reported dimensions rather than assuming them.

### get_sensor_data

Get the latest system telemetry snapshot, or one named sensor reading: CPU, GPU, memory, and raw component temperatures. Read-only, idempotent.

- **Optional:** `label` (string): a sensor label such as `cpu_temp`, `gpu_load`, `ram_used`, or a normalized raw component label. Omit for the full snapshot.

```json
{ "name": "get_sensor_data", "arguments": { "label": "gpu_load" } }
```

The response returns a `snapshot` object and a `reading` field (populated only when a matching `label` was requested).

### diagnose

Run live system diagnostics: connectivity, frame delivery, latency, and error rates. Read-only, idempotent. This returns rich, real metrics, not a placeholder.

- **Arguments:** none. Every call runs the full-system pass across every device.

```json
{ "name": "diagnose", "arguments": {} }
```

The response carries `overall_status` (`healthy`, `warning`, or `unhealthy`; any finding with `severity: "error"` makes it `unhealthy`), a `findings` array (each with a `severity` and `message`), and a deep `metrics` object: capacity and delivered FPS, frame timing, a per-frame `latest_frame` block, a `render_window` block, a `device_output` block with accepted and delivered rates, coalescing reasons, transport terminal counters, and actor latency, plus a `usb_actor` block. This is the backbone of the diagnose flow in [agent workflows](@/agents/workflows.md).

---

## Displays

### set_display_face

Assign or clear an HTML display-face effect on a display device (an LCD or similar). Mutates state, idempotent.

The target effect must be in the `Display` category **and** be an HTML source; anything else returns an invalid-parameter error. This is the only path to drive a display face; `set_effect` will refuse a display effect.

- **Required:** `device` (string): display device ID or exact display name.
- **Optional:** `effect_id` (string): display-face effect UUID, exact name, or source stem; omit when clearing. `clear` (boolean): when true, removes the assignment on the chosen scope. `scope` (enum): `default` (the default) persists the face across scenes; `scene` writes the active scene's display zone and wins while that scene is active. `controls` (object): control overrides stored on the display-face zone.

```json
{
  "name": "set_display_face",
  "arguments": {
    "device": "AX Display",
    "effect_id": "clock-face",
    "scope": "default"
  }
}
```

To clear a face, pass `"clear": true` and the same `scope`. See [display faces](@/effects/display-faces.md) for authoring the HTML side.

---

## Library

### set_profile

Activate a saved profile by name or fuzzy query. A profile captures the complete lighting state: effect, control parameters, device selection, and brightness. Mutates state, idempotent.

- **Required:** `query` (string): profile name or description.
- **Arguments:** `query` only. Profile apply is immediate, so the tool exposes no fade duration.

```json
{ "name": "set_profile", "arguments": { "query": "Focus" } }
```

If no profile matches, the call succeeds with `"applied": false` and an explanatory message rather than erroring. On success the response returns the profile (`id`, `name`, `description`, `primary`, `displays`, `layout_id`), `applied: true`, and any `warnings`.

---

## A note on installing effects

There is no MCP tool to install or rescan effects. Agents can apply, browse, and stop effects, but installing a freshly built effect crosses transports: the SDK authoring CLI uploads it, then `hypercolor effects rescan` (or a daemon restart) makes it visible, after which `set_effect` can apply it. That cross-transport pattern is walked end to end in [agent workflows](@/agents/workflows.md) and [CLI scripting for agents](@/agents/cli-scripting.md).

## Where to go next

- **[Resources reference](@/agents/resources-reference.md)**: The 5 `hypercolor://` resources an agent reads to orient itself.
- **[Prompt templates](@/agents/prompt-templates.md)**: The 3 guided flows that compose these tools.
- **[Agent workflows](@/agents/workflows.md)**: Worked playbooks with real call-and-response pairs.
- **[MCP server reference](@/api/mcp.md)**: Transport, config keys, and the raw protocol surface.
