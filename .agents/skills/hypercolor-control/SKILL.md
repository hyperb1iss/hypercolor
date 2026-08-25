---
name: hypercolor-control
description: >-
  Use this skill when an agent needs to inspect or control a running
  Hypercolor daemon, browse or activate effects, patch live controls, adjust
  brightness, manage scenes and snapshots, or install built HTML effects from an
  authoring workspace. Triggers on "hypercolor", "list effects", "apply
  effect", "patch controls", "install effect", "rescan effects", "brightness",
  "scene", "snapshot scene", or any request to control the daemon from Claude Code.
---

# Hypercolor Control

Hypercolor exposes three control surfaces. Choose the right one before acting.

- **MCP tools** are the canonical agent surface: seventeen typed tools the daemon serves directly, reachable in Claude Code as `mcp__hypercolor__<tool>`. Prefer them when they are connected.
- **Bare `hypercolor`** is the Rust system CLI for the daemon on `localhost:9420`. It covers everything MCP does not: devices, driver and device control surfaces, layouts, library, audio, permissions, config, and daemon lifecycle.
- **`bunx hypercolor`** inside an effect workspace is the Bun authoring CLI for building, validating, and installing HTML effects.

## MCP tools

The daemon mounts its MCP server over Streamable HTTP at `/mcp`, and only when the
daemon config carries `[mcp] enabled = true`. Until that flag is set the endpoint 404s
and the CLI is the only route in.

Seventeen tools, five resources, three prompt templates. Selector arguments (`query`,
`name`, `zone`, `layer`, `device`, `effect_id`) all resolve in one fixed order: exact
serialized ID, then exact case-insensitive name, then a unique case-insensitive name
substring. An ambiguous substring fails with the candidate list instead of guessing.

Read-only:

- `get_status` (no arguments): active effect, brightness, connected device count, FPS metrics, audio and screen input status, uptime
- `get_devices` (`status` one of `all`, `connected`, `disconnected`; `driver_id`; `backend_id`)
- `list_effects` (`category`, `audio_reactive`, `query`, `limit` default 20, `offset` default 0)
- `list_scenes` (`enabled_only`)
- `get_layout` (no arguments): device positions, zones, topology
- `get_audio_state` (no arguments): levels, beat detection, spectrum
- `get_sensor_data` (`label` optional): CPU, GPU, memory, raw component temperatures
- `diagnose` (no arguments): canonical checks, summary counts, captured status snapshot

Mutating, not destructive:

- `set_brightness` (`brightness` 0-100, required)
- `set_output_power` (`state` one of `running`, `paused`, required) pauses or resumes all output without discarding the active effect, controls, preset provenance, or scene state
- `adjust_controls` (`zone` and `layer` required, plus `values` and `clear_bindings`) atomically patches one live layer
- `create_scene` (`name` required, plus `description`, `enabled`, `mutation_mode` one of `live`, `snapshot`)

Destructive. These five are the ones the tool definitions flag `destructive`, because
each replaces or clears state that no single-field undo restores. Read current state
before calling them:

- `set_effect` (`query` required, plus `controls` and `transition`) replaces the target zone's whole layer stack with one effect. `transition` is an object, `{"type": "cut"}`, never the bare string.
- `set_color` (`color` required, plus `brightness`) sets a solid color across the LED pipeline. `color` accepts CSS names, hex, `rgb()`, `hsl()`, or a natural-language description.
- `clear_zone` (`zone` optional) clears one non-display zone's layer stack. Omitting `zone` clears every non-display zone and quiesces output.
- `activate_scene` (`name` required, plus `transition_ms` default 1000)
- `set_display_face` (`device` required, plus `effect_id`, `clear`, `scope` one of `default`, `scene`, and `controls`)

Resources: `hypercolor://state`, `hypercolor://devices`, `hypercolor://effects`,
`hypercolor://scenes`, `hypercolor://audio`. Prompt templates: `mood_lighting`,
`troubleshoot`, `setup_automation`.

### Control values have two wire forms

They are not interchangeable, and sending the wrong one is a hard parse failure.

`adjust_controls` takes canonical typed control values, adjacently tagged as
`{"kind": ..., "value": ...}`. Its own arguments wrap them alongside the two
selectors it needs:

```json
{
  "zone": "Desk",
  "layer": "1f0a7c2e-4b91-4f0e-9a6d-2c8f5b31d7e4",
  "values": { "speed": { "kind": "float", "value": 0.5 }, "trails": { "kind": "bool", "value": true } },
  "clear_bindings": ["speed"]
}
```

`layer` resolves by ID, exact name, or unique name substring, but layers that
`set_effect`, `hypercolor effects activate`, and `POST /effects/{id}/apply`
create carry no name at all, and the selector admits unnamed candidates only at
the ID stage. So in practice you read `GET /api/v1/scene` and pass the layer's
UUID. A name works only for a layer somebody explicitly named.

The REST route `PATCH /api/v1/scene/zones/{zone}/layers/{layer}/controls` takes
the same control values but a **different body**. `PatchControlsRequest` is
`deny_unknown_fields` over exactly `values` and `clear_bindings`; the zone and
layer live in the path, so including them in the body is a hard parse failure,
not an ignored extra:

```json
{
  "values": { "speed": { "kind": "float", "value": 0.5 } },
  "clear_bindings": ["speed"]
}
```

Kinds are `null`, `bool`, `int`, `float`, `text`, `secret_ref`, `ip`, `mac`, `duration`,
`color_rgb`, `color_rgba`, `color_linear`, `gradient`, `rect`, `enum`, `flags`, `list`,
`map`, and `unknown`. The deserializer denies unknown fields, so a bare number where a
tagged object belongs fails outright rather than falling back.

`set_effect` and `set_display_face` take the other form: raw effect JSON, bare values
admitted against the addressed control's own schema.

```json
{ "query": "Aurora", "controls": { "speed": 0.5, "palette": "Ocean" } }
```

## Scene and zone model

There is no single global "active effect". The live scene is a document of zones, and
each zone owns an ordered bottom-to-top layer stack. A zone carries a role (`primary`,
`display`, or `custom`), its own brightness and enabled flag, the device segments
assigned to it, and an optional zone-scoped layout override. A layer is the addressable
unit: it has a stable ID, an optional name, a source (an effect with its controls and
control bindings, among others), blend mode, opacity, transform, and enabled flag.

That shape drives how the surfaces behave:

- `set_effect` and `hypercolor effects activate` replace the target zone's layer stack. Omitting a zone targets the primary zone, created if the scene has none.
- `adjust_controls` needs both a zone and a layer, because a zone can stack several effect layers.
- `hypercolor effects patch` and `hypercolor effects reset` resolve the zone and layer for you: they read the live scene, pick the primary zone (or the first zone), and take the topmost effect layer in it. When a zone stacks more than one effect, address the layer explicitly through `adjust_controls` or the REST route.
- `hypercolor effects stop` clears the whole live scene, not one zone. `clear_zone` is the scoped version.
- Display faces live in `display`-role zones and are assigned by device, through `set_display_face`.

Read the live document with `hypercolor effects list -j` for the catalog and
`GET /api/v1/scene` for the zone and layer tree, whose layer IDs are what the patch
routes expect.

## Default workflow

Start with state discovery before changing anything:

```bash
hypercolor status
hypercolor effects list
hypercolor scenes active
```

When parsing output in automation, prefer JSON:

```bash
hypercolor status -j
hypercolor effects list -j
hypercolor library presets list -j
```

`--json` and its `-j` shorthand are global flags, so they work on every subcommand.

## Daemon control

Core runtime commands:

```bash
hypercolor status
hypercolor status --watch
hypercolor status --watch --interval 2
hypercolor effects list --search aurora
hypercolor effects info "Aurora"
hypercolor effects activate "Aurora" --param speed=6 --param palette=\"Ocean\"
hypercolor effects patch --param speed=8
hypercolor effects reset
hypercolor effects pause
hypercolor effects resume
hypercolor effects stop
hypercolor effects rescan
hypercolor brightness get
hypercolor brightness set 45
hypercolor scenes list
hypercolor scenes snapshot "Evening"
hypercolor scenes activate "Movie Night"
hypercolor scenes deactivate
hypercolor diagnose --system
```

`hypercolor status --watch` subscribes to the daemon's event WebSocket and re-renders on
state change, rate-limited to one render per `--interval` seconds (default 1). It is the right way to observe a
change landing; do not poll `hypercolor status` in a loop.

`effects pause` and `effects resume` patch global output power and are the
non-destructive alternative to `effects stop`. Pausing holds the active effect, its
controls, and the scene exactly as they are, so resuming needs no re-apply. `effects
stop` clears the live scene and cannot be undone that way.

The nine `effects` subcommands are `list`, `activate`, `stop`, `pause`, `resume`,
`info`, `patch`, `reset`, and `rescan`. Spatial layout work is a separate top-level
command tree:

```bash
hypercolor layouts list
hypercolor layouts active
hypercolor layouts show "Desk"
hypercolor layouts preview "Desk"
hypercolor layouts apply "Desk"
```

Beyond effects, scenes, and brightness, the CLI also carries `devices`, `controls`,
`drivers`, `layouts`, `audio`, `access`, `library`, `server`, `servers`, `service`,
`config`, `diagnose`, `completions`, and `tui`. Run `hypercolor <command> --help` before
guessing at a subcommand or flag.

### Host-input and screen-capture permissions

Screen-reactive and keyboard-reactive effects need OS-level authorization that the
daemon cannot grant itself. When those inputs stay dark, request them explicitly rather
than restarting anything:

```bash
hypercolor access authorize-input-monitoring
hypercolor access authorize-screen-recording
hypercolor access choose-screen-source
```

The three map onto `POST /api/v1/input/authorize`, `POST /api/v1/capture/authorize`, and
`PUT /api/v1/capture/source`. Nothing prompts for them implicitly at startup, so the
request has to be explicit. Each reports the grant owner that actually holds the
permission, which
is the fastest way to see that a grant landed on the app sidecar or a launchd service
rather than the daemon you are talking to. `choose-screen-source` presents the platform
picker, so it needs an owner that can show UI and returns the daemon's typed refusal when
that owner is headless.

## Diagnostics workflow

When diagnosing a running daemon, query telemetry before asking for pasted logs:

```bash
just diagnose
just diagnose -- --json
hypercolor diagnose --system -j
curl -s -X POST http://127.0.0.1:9420/api/v1/diagnose \
  -H 'content-type: application/json' -d '{"system":true}'
```

Use `just windows-diagnose` only when Windows service/PawnIO/SMBus environment state matters; the daemon render/output telemetry itself is cross-platform.

Read these fields first for LED jank:

- `snapshot.render.latest_frame.output_frame_source` is `current_frame`, `published_frame`, or `routed_reuse`
- `gpu_sample_stale`, `gpu_sample_deferred`, `gpu_sample_retry_hit`, `gpu_sample_queue_saturated`, `gpu_sample_wait_blocked`
- `sample_us`, `push_us`, `publish_us`, `devices_written`, `total_leds`
- `snapshot.device_output.items[]`: `backend_id`, `fps_sent`, `fps_queued`, `frames_dropped`, `avg_queue_wait_ms`, `avg_write_ms`, `last_error`
- `snapshot.usb`: USB actor display-lane wait counters

Interpretation:

- Smooth display previews with LED jank usually means LED sampling/output freshness, not effect rendering.
- `gpu_sample_stale=true` with `output_frame_source=published_frame` means LEDs reused older LED data while the visual path may still be smooth.
- `output_frame_source=current_frame` with `gpu_sample_retry_hit=true`, low sample/push times, and `wake_late` warnings usually means the app is ready but the OS woke the render thread late. On Windows, inspect active compiler/linker jobs before changing rendering or device code.
- `fps_queued` above `fps_sent`, rising `frames_dropped`, or high queue/write time points to device-output pressure.
- Drops on queues capped below render FPS are normal latest-frame replacement when `fps_sent` is near that queue's target and write latency/errors are clean.
- Multiple USB devices janking together points upstream or shared queue pressure; one device with errors points at that driver/transport.

## Effect authoring commands

The Bun authoring CLI has exactly four commands: `build`, `validate`, `install`, and
`add`. Anything else prints the help text and exits 1.

```bash
bunx hypercolor build --all
bunx hypercolor build --all --watch
bunx hypercolor build effects/aurora.ts --minify
bunx hypercolor validate dist/aurora.html
bunx hypercolor validate dist/aurora.html --strict --json
bunx hypercolor install dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
bunx hypercolor add ember --template canvas
```

There is no dev server and no watch-mode preview. `build --all --watch` is the iteration
loop: it rebuilds on every `.ts` or `.glsl` change under the entry roots and runs until
interrupted. Preview the result in the real daemon or app after installing it.

Scaffolded TypeScript workspaces expose the same flow through package scripts:

```bash
bun run build
bun run validate
bun run ship
bun run ship:daemon
```

`bun run ship` copies validated artifacts into the user effects directory.
`bun run ship:daemon` uploads through `POST /api/v1/effects/install`. The plain-HTML
workspace template ships only `validate`, `ship`, and `ship:daemon`, since its effects
are hand-authored HTML with nothing to bundle.

## Install workflow

Preferred install sequence for built HTML effects:

```bash
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
hypercolor effects rescan
hypercolor effects activate "Aurora"
```

If no daemon is running yet, local install still works:

```bash
bunx hypercolor install dist/aurora.html
```

That writes into `$XDG_DATA_HOME/hypercolor/effects/user/`, which the daemon
will pick up on boot or via `hypercolor effects rescan`.

## Behavioral guidance

- Inspect first. Do not guess the active scene, its zones, or brightness.
- Reach for the MCP tools when they are connected, and the CLI when they are not or when the work is outside their seventeen.
- Prefer targeted actions over restarting the daemon.
- Prefer `pause` over `stop`, and `clear_zone` over a whole-scene clear, whenever the narrower action does the job.
- Validate HTML artifacts before installing them.
- Use JSON output when another tool or script needs to consume the result.
- After installing a new effect, confirm it appears in `hypercolor effects list` before applying it.
- If a control patch fails, read the control definitions from `hypercolor effects info <name> -j` (the table view omits them) and retry with values that match the declared types and the right wire form.

## Example playbooks

### List and activate an effect

```bash
hypercolor effects list -j
hypercolor effects activate "Aurora" --param speed=7
hypercolor status
```

### Tweak a running effect

```bash
hypercolor effects info "Aurora"
hypercolor effects patch --param speed=4 --param brightness=80
hypercolor status
```

### Tweak one layer in a multi-zone scene

Read the zone and layer IDs first, then patch the layer you actually mean.

```bash
curl -s http://127.0.0.1:9420/api/v1/scene | jq '.data.zones[] | {id, name, role, layers: [.layers[] | {id, name}]}'
```

Then call `adjust_controls` with that zone and layer, or the equivalent REST route with
canonical tagged values.

### Install a fresh artifact from an SDK workspace

```bash
cd /path/to/effect-workspace
bun run build
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
hypercolor effects list --search aurora
```

### Recover after a manual file copy

```bash
cp dist/aurora.html ~/.local/share/hypercolor/effects/user/
hypercolor effects rescan
hypercolor effects info "Aurora"
```
