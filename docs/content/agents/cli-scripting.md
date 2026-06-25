+++
title = "CLI scripting for agents"
description = "Use the hypercolor CLI as an agent tool: JSON output, exit codes, env vars, a state-first workflow, and jq recipes."
weight = 50
template = "page.html"
+++

The `hypercolor` CLI is the second way an agent drives the daemon, alongside the [MCP server](@/agents/_index.md). Where MCP gives a model structured tools and resources, the CLI gives a model a shell. Anything an agent can run in a terminal, it can use to read live lighting state and change it: list effects, apply one, tune brightness, manage scenes, and pull diagnostics. This page covers the pieces that make the CLI a reliable tool inside an automation loop: machine-readable JSON, exit codes you can branch on, connection environment variables, and a state-first workflow with copy-paste `jq` recipes.

The CLI talks to the daemon over its REST API on `:9420`. It does not touch hardware directly, so every command an agent runs goes through the same shared engine state as the web UI and MCP. A change made on the command line is instantly visible everywhere else.

{% callout(type="info") %}
This is the command-driven sibling of the MCP path. If your agent runtime supports MCP, the [tool reference](@/agents/tools-reference.md) gives a typed, schema-validated surface. Reach for the CLI when you want shell scripting, piping into `jq`, exit-code branching, or coverage of commands MCP does not expose (effect rescan, profiles, layouts, driver controls). For the full human-facing command tree, see the [CLI reference](@/api/cli.md).
{% end %}

## The agent contract: JSON in, exit codes out 🎯

Two flags turn the CLI from a human tool into an agent tool. Pass `-j` (or the long form `--format json`) on any command and it emits machine-readable JSON to stdout instead of a styled table. On failure the process exits non-zero, so an agent can branch on the exit code without parsing prose.

```bash
hypercolor status -j
hypercolor effects list -j
hypercolor devices list --format json
```

The default output format is `table`, which is built for humans and is not stable to parse. Always pass `-j` from a script. `-j` is global, so it works on every subcommand.

### Exit codes

Every command exits `0` on success and `1` on any error: the daemon being unreachable, a bad request, an effect that does not exist, or a connection timeout. There is no partial-success exit code, so the check is binary.

```bash
if hypercolor status -j > /tmp/state.json; then
  echo "daemon reachable"
else
  echo "daemon down or unreachable" >&2
  exit 1
fi
```

When the daemon is not running, the error message names the URL it tried and asks "Is the daemon running?", which is a useful signal to surface back to a user or to trigger `hypercolor service start`.

### What the JSON looks like

The CLI unwraps the daemon's response envelope before printing. The REST API wraps every payload as `{ "data": ..., "meta": ... }`; the CLI strips that and prints the inner `data` value, pretty-printed. So you parse the payload directly, not the envelope.

For list commands the payload is an **object with an `items` array**, not a bare array. This is the single most common scripting mistake, so the `jq` recipes below all reach through `.items[]`.

```bash
hypercolor effects list -j
```

```json
{
  "items": [
    {
      "name": "Borealis",
      "category": "ambient",
      "author": "Hypercolor",
      "version": "1.0.0"
    }
  ]
}
```

## Connection environment variables

The CLI reads connection settings from flags or environment variables, with the flag winning when both are set. For an agent that runs many commands, exporting the environment once is cleaner than threading flags through every call.

| Variable | Flag | Default | Purpose |
| --- | --- | --- | --- |
| `HYPERCOLOR_HOST` | `--host` | `localhost` | Daemon hostname or IP. |
| `HYPERCOLOR_PORT` | `--port` | `9420` | Daemon port. |
| `HYPERCOLOR_API_KEY` | `--api-key` | _(none)_ | Bearer token for authenticated requests. |
| `HYPERCOLOR_PROFILE` | `--profile` | _(none)_ | Named connection profile from `cli.toml`. |
| `HYPERCOLOR_THEME` | `--theme` | _(none)_ | Color theme for table output (irrelevant under `-j`). |

```bash
export HYPERCOLOR_HOST=127.0.0.1
export HYPERCOLOR_PORT=9420
hypercolor status -j
```

{% callout(type="tip") %}
A local agent on the same machine as the daemon needs **no** API key. Loopback requests bypass authentication entirely. A token is only required when the daemon is reached from a non-loopback address, in which case set `HYPERCOLOR_API_KEY` and the CLI sends it as `Authorization: Bearer <token>`. The [REST API reference](@/api/rest.md) covers the remote-access auth model.
{% end %}

## State-first workflow

The reliable pattern for an agent is read, then act, then verify. Never guess the current state; query it. Never assume an effect name; search for it. After a mutation, read back to confirm the daemon applied what you intended.

{% mermaid() %}
graph TD
    A[Read state: hypercolor status -j] --> B[Discover: effects list / devices list]
    B --> C[Act: effects activate / brightness set / scenes activate]
    C --> D[Verify: status -j, branch on exit code]
    D -->|drifted| B
    D -->|matches intent| E[Done]
{% end %}

### 1. Read the live state

`status` is the cheapest read and the right first call in any session. It returns the running effect, connected device count, audio state, and the render loop's actual FPS.

```bash
hypercolor status -j
```

### 2. Discover before you act

Effect names are fuzzy-matched, but searching first means an agent applies a known effect instead of a guess. The catalog ships **11 native built-in effects** compiled into the engine plus roughly four dozen HTML effects from the SDK, so browse rather than hardcode names. Filter the list server-side with `--search`, `--category`, `--engine`, or `--audio`.

```bash
hypercolor effects list --search aurora -j
hypercolor effects list --category ambient -j
hypercolor effects list --audio -j          # audio-reactive effects only
```

```bash
hypercolor devices list -j
```

### 3. Act

Activating an effect takes a fuzzy name or slug. Pass control values with repeatable `--param key=value` flags, or use the `--speed` and `--intensity` shorthands. `--transition` sets a crossfade duration in milliseconds (default `0`, which cuts instantly).

```bash
hypercolor effects activate borealis --param speed=7 --param palette=SilkCircuit
hypercolor effects activate "calm waves" --speed 3 --transition 1500
```

To retune the running effect without re-applying it, use `effects patch`. This is the cheap path for live adjustments inside a control loop.

```bash
hypercolor effects patch --param speed=3 --param intensity=90
```

Brightness is a global percentage from 0 to 100. Scene activation takes a scene name or ID and switches the whole rig to that configuration.

```bash
hypercolor brightness set 35
hypercolor scenes activate "evening" --transition 800
```

{% callout(type="info") %}
Scenes are whole-rig configurations, not per-device groupings. A scene captures the full lighting setup and swaps it in atomically. Zones are flexible partitions of the render canvas within a scene. Keep the two distinct when an agent reasons about "change the lighting": a scene switch changes everything, a zone change is scoped.
{% end %}

### 4. Verify

Read back and compare. Because mutations exit non-zero on failure, the simplest verification is the exit code, but reading `status` confirms the daemon actually settled on the intended effect.

```bash
hypercolor effects activate borealis -j > /dev/null \
  && hypercolor status -j | jq -r '.effect.name'
```

## jq recipes

These pipe `-j` output through `jq`. The recurring shape to remember: lists arrive as `{ "items": [...] }`, so list queries reach through `.items[]`.

Count available effects:

```bash
hypercolor effects list -j | jq '.items | length'
```

Names of every connected device:

```bash
hypercolor devices list -j | jq -r '.items[] | select(.status == "connected") | .name'
```

First effect name in a search, ready to feed straight back into `activate`:

```bash
EFFECT=$(hypercolor effects list --search nebula -j | jq -r '.items[0].name')
hypercolor effects activate "$EFFECT" --transition 1000
```

The current effect and live FPS from `status`:

```bash
hypercolor status -j | jq '{ effect: .effect.name, fps: .fps }'
```

Audio-reactive effect names only, sorted:

```bash
hypercolor effects list --audio -j | jq -r '.items[].name' | sort
```

## Diagnostics for agents

When something looks wrong, `diagnose` runs the daemon's health checks and returns a structured report: per-check pass/warn/fail entries grouped by category, plus a summary. Under `-j` an agent gets the full structure to reason over.

```bash
hypercolor diagnose -j
hypercolor diagnose --check devices --check audio -j   # scope to specific checks
hypercolor diagnose --report /tmp/hypercolor-report.json
```

The report covers daemon health, device connectivity, audio capture, render-engine status, configuration validity, and USB permissions. The `--report` flag writes the full diagnostic JSON to a file, which is the right artifact to attach when an agent files a bug on a user's behalf. A typical "diagnose a sick device" loop reads `status`, lists `devices` to find the disconnected one, then runs `diagnose --check devices` and reasons over the failing entries.

For a quick liveness probe that does not need the full diagnostic pass:

```bash
hypercolor server health -j
```

## Three commands that look alike

The CLI has three top-level commands whose names overlap. Picking the wrong one is a frequent agent error, so keep them straight:

| Command | Scope | Use it to |
| --- | --- | --- |
| `server` | The daemon you are connected to | Read its identity, version, and health (`server info`, `server health`). |
| `servers` | The local network | Discover other Hypercolor daemons via mDNS (`servers discover`). |
| `service` | The OS service manager | Manage the daemon process lifecycle through systemd or launchd (`service start`, `service stop`, `service status`, `service logs`). |

In short: `server` queries the connected daemon, `servers` finds daemons on the LAN, and `service` controls the local daemon process.

## Two CLIs named "hypercolor"

The `hypercolor` binary documented here is the Rust daemon CLI. Inside an SDK effect-authoring workspace there is a **second, separate** CLI invoked as `bunx hypercolor` that builds, validates, and installs HTML effects. They are not the same tool: one drives a running daemon over REST, the other is a Bun-based authoring toolchain.

An agent that builds an effect and then applies it crosses both. Build and install with the SDK CLI, then use the daemon CLI to make the new effect visible and apply it. MCP has no install or rescan tool, so this step is CLI-only.

```bash
# In the SDK workspace: build, validate, and install the effect
bunx hypercolor build --all
bunx hypercolor install dist/aurora.html --daemon

# Back on the daemon CLI: pick up the new effect and apply it
hypercolor effects rescan
hypercolor effects activate aurora --transition 1000
```

The SDK is pre-release and not yet on npm, so the exact `bunx hypercolor` invocation depends on the workspace's local `file:` spec until it is published. See [effects setup](@/effects/setup.md) for the authoring side.

## Where to go next

- [MCP setup](@/agents/mcp-setup.md) — the typed, schema-validated alternative to shell scripting.
- [Tools reference](@/agents/tools-reference.md) — all 16 MCP tools with arguments and return shapes.
- [Prompt templates](@/agents/prompt-templates.md) — the three shipped prompts agents can invoke.
- [CLI reference](@/api/cli.md) — the complete command tree, human-facing.
- [REST API reference](@/api/rest.md) — the daemon contract the CLI sits on top of.
