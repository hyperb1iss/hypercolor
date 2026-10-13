+++
title = "Agent workflows"
description = "Four worked agent playbooks for Hypercolor: set a calm scene, build and apply an effect, write an effect live, and diagnose a sick device."
weight = 60
+++

This page turns the tools, resources, and CLI commands from the rest of the Agents section into four end-to-end playbooks an agent can follow verbatim. Each one is a real sequence of calls with the responses an agent should expect, drawn straight from the daemon's MCP and CLI contracts. Every workflow opens the same way: read state before you touch it.

{% <callout type="tip"> %}
The MCP server ships this exact instruction to every client: start with `get_status` or the `hypercolor://state` resource, browse with `list_effects` before applying visuals, check the result with `capture_canvas` instead of assuming it looks right, and prefer structured arguments and resource reads over guessing. The playbooks below are that discipline made concrete.
{% </callout> %}

{{< img path="img/ui/dashboard.webp" alt="The Hypercolor dashboard an agent reads through hypercolor://state" />}}

## Before you start

Two surfaces drive Hypercolor, and a complete workflow often uses both. The [MCP server](@/agents/mcp-setup.md) gives a model 21 structured tools, 5 browsable resources, and 4 prompts over Streamable HTTP at `http://127.0.0.1:9420/mcp`. The [`hypercolor` CLI](@/agents/cli-scripting.md) gives any agent that can run a shell command a machine-readable contract through `--json` output and exit codes. Local agents need no credentials, since loopback requests bypass auth entirely.

One thing trips agents up constantly, so fix it in your head now. There are two different `hypercolor` command-line tools:

- Bare `hypercolor` is the Rust daemon CLI. It talks REST to the daemon on `:9420` and controls live lighting.
- `bunx hypercolor`, run inside an SDK effect workspace, is the Bun authoring CLI. It builds, validates, and installs HTML effects.

Workflow B below uses both, and crossing that line in the right order is the whole point of the playbook.

## Workflow A: set a calm scene

The most common request: "make the lights calm." This is a read-then-act loop entirely over MCP. It mirrors the shipped [`mood_lighting` prompt](@/agents/prompt-templates.md), which front-loads the same resource reads before recommending anything.

### 1. Orient

Read the live state before changing anything. Either call the `get_status` tool or read the `hypercolor://state` resource.

```json
// get_status → returns (abridged)
{
  "running": true,
  "paused": false,
  "brightness": 80,
  "fps": { "target": 60, "capacity": 60.0, "delivered": 59.8, "actual": 60.0 },
  "effect": { "id": "hyperspace", "name": "Hyperspace" },
  "effect_count": 59,
  "scene_count": 4,
  "devices": { "connected": 4, "total": 4, "total_leds": 432 }
}
```

Four devices, the engine is running, an effect is already live. Good baseline.

### 2. Find a candidate

Browse the catalog instead of guessing an effect name. Filter `list_effects` to the `ambient` category, or pass a `query`.

```json
// list_effects with { "category": "ambient", "limit": 10 }
{
  "effects": [
    {
      "id": "borealis",
      "name": "Borealis",
      "category": "ambient",
      "audio_reactive": false
    },
    {
      "id": "deep-current",
      "name": "Deep Current",
      "category": "ambient",
      "audio_reactive": false
    },
    {
      "id": "nebula-drift",
      "name": "Nebula Drift",
      "category": "ambient",
      "audio_reactive": false
    }
  ],
  "total": 3,
  "has_more": false,
  "limit": 10,
  "offset": 0
}
```

{{< img path="img/effects/borealis.webp" alt="A calm ambient effect in the gallery" />}}

### 3. Apply it

Call `set_effect` with one catalog identity. The `query` resolves by exact ID, exact case-insensitive name, or a unique case-insensitive name substring. The optional `transition` object accepts only `{ "type": "cut" }`, and omission also means cut. Choose once, apply once, then tune the returned layer instead of reapplying candidates.

```json
// set_effect with
{
  "query": "Borealis",
  "transition": { "type": "cut" }
}
// → returns (abridged)
{
  "zone": {
    "id": "0199...",
    "name": "Primary",
    "layers": [{ "id": "0199...", "source": { "type": "effect", "effect_id": "borealis", "controls": {} }]
  },
  "transition": { "type": "cut" },
  "output": { "applied": true }
}
```

Use the returned zone and layer IDs to tune the running layer atomically:

```json
// adjust_controls with
{
  "zone": "0199...",
  "layer": "0199...",
  "values": { "speed": { "kind": "float", "value": 0.2 } }
}
```

An ambiguous substring returns structured candidates rather than selecting one silently. A display-face effect cannot be applied through `set_effect`; for LCD faces use `set_display_face` instead.

### 4. Settle the brightness

`set_brightness` takes an integer percentage from 0 to 100, not a 0.0 to 1.0 float.

```json
// set_brightness with { "brightness": 35 }
```

### 5. Prepare it for automation (optional)

The MCP `create_scene` tool creates an empty reusable scene with a seeded Primary zone. It takes a name plus optional description, enabled state, and mutation mode. Creating a scene does not capture the current output. Configure the new scene explicitly, then let an external automation system call `activate_scene` when its own conditions match.

```json
// create_scene with
{
  "name": "Evening Calm",
  "description": "Calm lighting for an external sunset automation"
}
// activate_scene with { "name": "Evening Calm" }
// set_effect with { "query": "Borealis" }
// adjust_controls with the returned zone and layer identities
```

{% <callout type="info"> %}
Scenes are whole-rig configurations, not per-room settings. Flexible canvas partitions inside a scene are zones, covered in the [Studio docs](@/studio/_index.md). To capture the live runtime state exactly, use `hypercolor scenes snapshot "Evening Calm"` through the CLI.
{% </callout> %}

The same loop in the CLI, for an agent that shells out rather than speaking MCP:

```bash
hypercolor status -j
hypercolor effects list --category ambient -j
hypercolor effects activate "calm blue borealis" --speed 2
hypercolor brightness set 35
```

Note the CLI surface differs from the MCP tool. The activate verb uses `--speed`, `--intensity`, and repeatable `--param key=value` shorthands. The CLI's `scenes create` is lighter than the MCP tool, taking a `name` plus optional `--description`, `--enabled`, and `--mutation-mode` flags.

## Workflow B: build an effect, then apply it

This is the playbook that crosses both CLIs. You author an HTML effect with the SDK, install it into the daemon, then apply it. The SDK path suits a workspace with a build step and several effects; to iterate on one page over MCP alone, use [Workflow C](#workflow-c-write-an-effect-live).

{% <callout type="info"> %}
Inside a scaffolded effect workspace, `bunx hypercolor` resolves the workspace's `hypercolor` dependency: the npm release by default, or a local build when the workspace was scaffolded with a `file:` SDK spec. These instructions assume you are in such a workspace.
{% </callout> %}

### 1. Build in the SDK workspace

Run the authoring CLI from your effect project. This is `bunx hypercolor`, the Bun tool, not the daemon CLI.

```bash
bunx hypercolor build --all
```

The build emits self-contained HTML into the workspace `dist/` directory.

### 2. Validate

Validate the built artifact before it goes near the daemon. Validation catches the common authoring errors, like a missing audio declaration or a shader uniform mismatch, at build time rather than at render time.

```bash
bunx hypercolor validate dist/aurora.html
```

### 3. Install into the daemon

Install uploads the validated effect to the running daemon through `POST /api/v1/effects/install`.

```bash
bunx hypercolor install dist/aurora.html --daemon
```

### 4. Apply

The install route adds the effect to the catalog as soon as it answers, so no rescan is needed. Confirm it landed, then apply it through whichever surface the agent already speaks.

```bash
hypercolor effects list --search aurora -j
```

Over the daemon CLI:

```bash
hypercolor effects activate "Aurora" --param speed=7
```

Or over MCP, with `set_effect`:

```json
// set_effect with { "query": "Aurora", "controls": { "speed": 7 } }
```

{{< img path="img/ui/effects.webp" alt="An effect applied and rendering on the canvas" />}}

{% <callout type="info"> %}
The install-and-apply path is the clearest case where a single agent job spans both CLIs. The SDK authoring CLI gets the effect onto the daemon; the daemon CLI or an MCP tool makes it live. Building [HTML effects](@/effects/_index.md) is its own topic with its own section.
{% </callout> %}

## Workflow C: write an effect live

An agent that writes effect code itself does not need an SDK workspace. Over MCP alone it can install a page, read what the page printed, look at what it drew, and go again. Every iteration reuses one `file_name`, so the effect keeps its ID and the running layer picks up each new version without another `set_effect`.

### 1. Install the page

Send the complete HTML document. The page needs a `<title>`, a `<canvas id="exCanvas">`, and a `<script>`. Validation reports every problem at once as an invalid-parameter error, so one failed call lists everything to fix.

```json
// install_effect with
{
  "html": "<!DOCTYPE html><html><head><title>Livecode Probe</title>...</html>",
  "file_name": "probe.html"
}
// → returns
{
  "id": "e77d0339-0967-495f-bf5c-b57fcfbc5faf",
  "name": "Livecode Probe",
  "path": "/home/you/.local/share/hypercolor/effects/user/probe.html",
  "controls": 2,
  "presets": 0,
  "replaced_existing": false,
  "console_after": 6
}
```

Keep `console_after`. It is the console cursor from just before the install, so it marks where this version's output begins.

### 2. Apply it once

A first install only adds the effect to the catalog. Put it on a layer with one `set_effect` call, and keep the returned zone and layer IDs for tuning.

```json
// set_effect with { "query": "e77d0339-0967-495f-bf5c-b57fcfbc5faf" }
```

### 3. Read the console

Ask for warnings and errors printed since the install. Locations point at your own file and line, not at the runtime copy the engine actually loads.

```json
// get_effect_console with { "after": 6, "level": "warn" }
// → returns (abridged)
{
  "entries": [
    {
      "seq": 7,
      "level": "error",
      "message": "Error at file:///home/you/.local/share/hypercolor/effects/user/probe.html:19:5 undefinedFunctionCall is not defined",
      "effect_name": "Livecode Probe",
      "source_file": "probe.html",
      "line": 19
    }
  ],
  "next_seq": 7,
  "truncated": false
}
```

An empty `entries` array means nothing at warning level or above has printed yet. Pass `next_seq` as `after` on the next read.

### 4. Look at it

A quiet console does not mean the effect looks right. Capture a short burst to judge color and motion.

```json
// capture_canvas with { "frames": 4, "interval_ms": 250 }
// → returns four images plus metadata (abridged to the first frame)
{
  "frames": [
    {
      "frame_number": 258,
      "timestamp_ms": 9036,
      "width": 320,
      "height": 240,
      "fresh": true,
      "mime_type": "image/jpeg"
    }
  ],
  "source_width": 640,
  "source_height": 480
}
```

The canvas is the full-brightness image before LED sampling. A frame with `fresh: false` means nothing rendered during the wait, so the effect is not drawing at all; the burst stops there.

### 5. Tune without a reload

For value changes, patch the live layer with `adjust_controls` instead of reinstalling. The page keeps running rather than reloading.

```json
// adjust_controls with
{
  "zone": "0199...",
  "layer": "0199...",
  "values": { "speed": { "kind": "float", "value": 0.6 } }
}
```

### 6. Hot swap the code

For code changes, call `install_effect` again with the same `file_name`. The response carries the same `id` and `replaced_existing: true`, and the running layer reloads the new page on its own. Read the console from the new `console_after`, capture again, and repeat.

Over the daemon CLI, the same loop is `hypercolor effects install probe.html --activate`, then `hypercolor effects console --after 6 --level warn`, then `hypercolor effects capture -o probe.png --frames 4`.

## Workflow D: diagnose a sick device

A device stops responding, or the frame rate drops. This playbook narrows from the whole system to the offending device, reading live metrics at each step. It mirrors the shipped [`troubleshoot` prompt](@/agents/prompt-templates.md), which runs the same diagnostic descent.

### 1. Check the whole system

Start broad. `get_status` shows whether the engine is running and whether the delivered frame rate is tracking the target.

```json
// get_status → returns (abridged)
{
  "running": true,
  "fps": { "target": 60, "capacity": 22.4, "delivered": 21.8, "actual": 22.4 },
  "devices": { "connected": 3, "total": 4, "total_leds": 432 }
}
```

Capacity well below target and one device missing from the connected count. Two threads to pull.

### 2. Find the offender

Filter `get_devices` by connection status to surface the disconnected device.

```json
// get_devices with { "status": "disconnected" } → returns (abridged)
{
  "devices": [
    {
      "id": "0197a2f4-6c1e-7d3a-9b02-4f8e1c5a7d90",
      "name": "Desk Strip",
      "state": "disconnected",
      "transport": "network",
      "led_count": 90
    }
  ],
  "summary": { "total": 1, "connected": 0, "total_leds": 90 }
}
```

The same query over the CLI:

```bash
hypercolor devices list --status disconnected -j
```

### 3. Run diagnostics

Call the `diagnose` tool. It takes no arguments and runs the canonical default-safe diagnostic pass. The tool returns the same `checks`, `summary`, and `snapshot` payload as the REST diagnostic collector, including render timing and per-device output queues.

```json
// diagnose with {} → returns (abridged)
{
  "checks": [
    {
      "category": "devices",
      "name": "output_queues",
      "status": "warning",
      "detail": "queues=4, lagging=1, dropped_total=184, errors_total=41"
    }
  ],
  "summary": { "passed": 8, "warnings": 1, "failed": 0 },
  "snapshot": {
    "device_output": {
      "items": [
        {
          "id": "0197a2f4-6c1e-7d3a-9b02-4f8e1c5a7d90",
          "delivered_fps": 0,
          "accepted_fps": 60,
          "coalesced_backend_overrun": 184,
          "transport_failed": 41
        }
      ]
    }
  }
}
```

The `snapshot.device_output.items[]` block is the signal: `accepted_fps` of 60 against `delivered_fps` of 0, with climbing backend-overrun coalescing and transport failures, means the render loop is producing frames the transport cannot deliver. That is a connectivity failure, not a rendering one.

The CLI equivalent runs named checks and can write a full report file for a bug report:

```bash
hypercolor diagnose --check devices --check render -j
hypercolor diagnose --report ./hypercolor-report.json --system
```

### 4. Interpret and act

Read the checks before acting. A network device that drops to `delivered_fps: 0` with rising transport failures is usually outside the daemon's control: powered down, on a different VLAN, or behind AP isolation. The fix lives in [Network devices](@/hardware/_index.md), not in a nonexistent reconnect tool. A device that is connected but rendering wrong colors is a different class of problem, covered in [Color science for LEDs](@/effects/color-science.md). Distinguishing the two is exactly what the diagnostic snapshot lets an agent do.

{% <callout type="success"> %}
The pattern repeats across all four workflows: orient on shared state, narrow with a filtered query, act with a structured call, and verify by reading state back. An agent that follows it never operates blind.
{% </callout> %}

## Where to go next

- **[Tools reference](@/agents/tools-reference.md)**: Every tool's full argument schema, defaults, enums, and read-only and idempotency flags.
- **[Resources reference](@/agents/resources-reference.md)**: The `hypercolor://` resources these workflows read, with payload shapes and freshness notes.
- **[Prompt templates](@/agents/prompt-templates.md)**: The `mood_lighting`, `troubleshoot`, and `setup_automation` prompts these playbooks mirror.
- **[CLI scripting for agents](@/agents/cli-scripting.md)**: The full agent-facing CLI contract: JSON output, exit codes, env vars, and jq recipes.
- **[CLI reference](@/api/cli.md)**: The complete command tree behind every shell example above.
