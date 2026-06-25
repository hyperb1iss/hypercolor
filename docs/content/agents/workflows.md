+++
title = "Agent workflows"
description = "Three worked agent playbooks for Hypercolor: set a calm scene, build and apply an effect, and diagnose a sick device."
weight = 60
+++

This page turns the tools, resources, and CLI commands from the rest of the Agents section into three end-to-end playbooks an agent can follow verbatim. Each one is a real sequence of calls with the responses an agent should expect, drawn straight from the daemon's MCP and CLI contracts. Every workflow opens the same way: read state before you touch it.

{% callout(type="tip") %}
The MCP server ships this exact instruction to every client: start with `get_status` or the `hypercolor://state` resource, browse with `list_effects` before applying visuals, and prefer structured arguments and resource reads over guessing. The playbooks below are that discipline made concrete.
{% end %}

![The Hypercolor dashboard an agent reads through hypercolor://state](/img/ui/dashboard.webp)

## Before you start

Two surfaces drive Hypercolor, and a complete workflow often uses both. The [MCP server](@/agents/mcp-setup.md) gives a model 16 structured tools, 5 browsable resources, and 3 prompts over Streamable HTTP at `http://127.0.0.1:9420/mcp`. The [`hypercolor` CLI](@/agents/cli-scripting.md) gives any agent that can run a shell command a machine-readable contract through `--json` output and exit codes. Local agents need no credentials, since loopback requests bypass auth entirely.

One thing trips agents up constantly, so fix it in your head now. There are two different `hypercolor` command-line tools:

- Bare `hypercolor` is the Rust daemon CLI. It talks REST to the daemon on `:9420` and controls live lighting.
- `bunx hypercolor`, run inside an SDK effect workspace, is the Bun authoring CLI. It builds, validates, and installs HTML effects.

Workflow B below uses both, and crossing that line in the right order is the whole point of the playbook.

## Workflow A: set a calm scene

The most common request: "make the lights calm." This is a read-then-act loop entirely over MCP. It mirrors the shipped [`mood_lighting` prompt](@/agents/prompt-templates.md), which front-loads the same resource reads before recommending anything.

### 1. Orient

Read the live state before changing anything. Either call the `get_status` tool or read the `hypercolor://state` resource.

```json
// get_status → returns
{
  "running": true,
  "active_effect": { "name": "Hyperspace", "category": "ambient" },
  "device_count": 4,
  "fps": { "actual": 60, "target": 60 },
  "brightness": 80
}
```

Four devices, the engine is running, an effect is already live. Good baseline.

### 2. Find a candidate

Browse the catalog instead of guessing an effect name. Filter `list_effects` to the `ambient` category, or pass a `query`.

```json
// list_effects with { "category": "ambient", "limit": 10 }
{
  "items": [
    { "name": "Borealis", "category": "ambient", "audio_reactive": false },
    { "name": "Deep Current", "category": "ambient", "audio_reactive": false },
    { "name": "Nebula Drift", "category": "ambient", "audio_reactive": false }
  ]
}
```

![A calm ambient effect in the gallery](/img/effects/borealis.webp)

### 3. Apply it

Call `set_effect`. The required argument is `query`, which does fuzzy and natural-language matching, so a description works as well as an exact name. Pass `controls` to tune it and `transition_ms` for a gentle crossfade (default 500, max 10000).

```json
// set_effect with
{
  "query": "calm blue borealis",
  "controls": { "speed": 2 },
  "transition_ms": 1500
}
// → returns
{
  "matched_effect": { "name": "Borealis" },
  "confidence": 0.94,
  "alternatives": ["Deep Current", "Nebula Drift"]
}
```

Read `confidence` and `alternatives` back to the user when the match is uncertain. A display-face effect cannot be applied through `set_effect`; for LCD faces use `set_display_face` instead.

### 4. Settle the brightness

`set_brightness` takes an integer percentage from 0 to 100, not a 0.0 to 1.0 float.

```json
// set_brightness with { "brightness": 35 }
```

### 5. Persist it (optional)

To make the calm look reusable, persist it as a scene. The MCP `create_scene` tool is more constrained than "save the current state": it requires `name`, an existing `profile_id`, and a `trigger` object whose `type` is one of `schedule`, `sunset`, `sunrise`, `device_connect`, `device_disconnect`, `audio_beat`, or `webhook`. It is the only non-idempotent tool, so call it once.

```json
// create_scene with
{
  "name": "Evening Calm",
  "profile_id": "prof_a1b2c3",
  "trigger": { "type": "sunset" }
}
```

{% callout(type="info") %}
Scenes are whole-rig configurations, not per-room settings. A scene captures the entire setup and the trigger that activates it. Flexible canvas partitions inside a scene are zones, covered in the [Studio docs](@/studio/_index.md).
{% end %}

The same loop in the CLI, for an agent that shells out rather than speaking MCP:

```bash
hypercolor status -j
hypercolor effects list --category ambient -j
hypercolor effects activate "calm blue borealis" --speed 2 --transition 1500
hypercolor brightness set 35
```

Note the CLI surface differs from the MCP tool. The activate verb uses `--speed`, `--intensity`, and repeatable `--param key=value` shorthands, and `--transition` is the crossfade duration in milliseconds. The CLI's `scenes create` is lighter than the MCP tool, taking just a `name` and an optional `--mutation-mode`.

## Workflow B: build an effect, then apply it

This is the playbook that crosses both CLIs. You author an HTML effect with the SDK, install it into the daemon, then apply it. There is no MCP tool to install or rescan effects, so this path has to cross from the authoring CLI to the daemon CLI or an MCP `set_effect` call.

{% callout(type="warning") %}
The SDK is pre-release and not published to npm yet. Inside a scaffolded effect workspace it is wired in through a `file:` dependency, so `bunx hypercolor` resolves to the local build of the authoring CLI rather than a registry version. These instructions assume you are in such a workspace.
{% end %}

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

### 4. Rescan, then apply

After installing, the daemon picks up the new effect through a rescan. There is no MCP rescan tool, so an agent uses the daemon CLI.

```bash
hypercolor effects rescan
# → Rescanned: 12 effects found
hypercolor effects list --search aurora -j
```

Then apply it through whichever surface the agent already speaks. Over the daemon CLI:

```bash
hypercolor effects activate "Aurora" --param speed=7 --transition 800
```

Or over MCP, with `set_effect`:

```json
// set_effect with { "query": "Aurora", "controls": { "speed": 7 } }
```

![An effect applied and rendering on the canvas](/img/ui/effects.webp)

{% callout(type="info") %}
The install-and-apply path is the clearest case where a single agent job spans both CLIs. The SDK authoring CLI gets the effect onto the daemon; the daemon CLI or an MCP tool makes it live. Building [HTML effects](@/effects/_index.md) is its own topic with its own section.
{% end %}

## Workflow C: diagnose a sick device

A device stops responding, or the frame rate drops. This playbook narrows from the whole system to the offending device, reading live metrics at each step. It mirrors the shipped [`troubleshoot` prompt](@/agents/prompt-templates.md), which runs the same diagnostic descent.

### 1. Check the whole system

Start broad. `get_status` shows whether the engine is running and whether the actual frame rate is tracking the target.

```json
// get_status → returns
{
  "running": true,
  "fps": { "actual": 22, "target": 60 },
  "device_count": 4,
  "connected_count": 3
}
```

Actual FPS well below target and one device missing from the connected count. Two threads to pull.

### 2. Find the offender

Filter `get_devices` by connection status to surface the disconnected device.

```json
// get_devices with { "status": "disconnected" } → returns
{
  "items": [
    { "id": "dev_wled_a4cf21", "name": "Desk Strip", "status": "disconnected", "transport": "network" }
  ]
}
```

The same query over the CLI:

```bash
hypercolor devices list --status disconnected -j
```

### 3. Run diagnostics

Call the `diagnose` tool. Omit `device_id` for a full-system pass, or pass it to scope the checks to one device. The live tool returns an `overall_status`, a `findings[]` array with per-finding `severity`, and a deep `metrics` object covering frame rate, render-window timing, and per-device output queues.

```json
// diagnose with { "device_id": "dev_wled_a4cf21" } → returns
{
  "overall_status": "warning",
  "findings": [
    {
      "severity": "warning",
      "message": "Device unreachable: no ACK for 3.2s on udp/4048"
    }
  ],
  "metrics": {
    "fps": 22,
    "target_fps": 60,
    "consecutive_misses": 2,
    "device_output": {
      "items": [
        { "id": "dev_wled_a4cf21", "fps_sent": 0, "fps_queued": 60, "frames_dropped": 184, "errors_total": 41 }
      ]
    }
  }
}
```

The `metrics.device_output.items[]` block is the signal: `fps_queued` of 60 against `fps_sent` of 0, with a climbing `frames_dropped` and `errors_total`, means the render loop is producing frames the transport cannot deliver. That is a connectivity failure, not a rendering one.

The CLI equivalent runs named checks and can write a full report file for a bug report:

```bash
hypercolor diagnose --check devices --check render -j
hypercolor diagnose --report ./hypercolor-report.json --system
```

### 4. Interpret and act

Read the findings before acting. A network device that drops to `fps_sent: 0` with rising errors is almost always off the network: powered down, on a different VLAN, or behind AP isolation. The fix lives in [Network devices](@/hardware/_index.md), not in Hypercolor. A device that is connected but rendering wrong colors is a different class of problem, covered in [Color science for LEDs](@/effects/color-science.md). Distinguishing the two is exactly what the metrics let an agent do.

{% callout(type="success") %}
The pattern repeats across all three workflows: orient on shared state, narrow with a filtered query, act with a structured call, and verify by reading state back. An agent that follows it never operates blind.
{% end %}

## Where to go next

- **[Tools reference](@/agents/tools-reference.md)** — Every tool's full argument schema, defaults, enums, and read-only and idempotency flags.
- **[Resources reference](@/agents/resources-reference.md)** — The `hypercolor://` resources these workflows read, with payload shapes and freshness notes.
- **[Prompt templates](@/agents/prompt-templates.md)** — The `mood_lighting`, `troubleshoot`, and `setup_automation` prompts these playbooks mirror.
- **[CLI scripting for agents](@/agents/cli-scripting.md)** — The full agent-facing CLI contract: JSON output, exit codes, env vars, and jq recipes.
- **[CLI reference](@/api/cli.md)** — The complete command tree behind every shell example above.
