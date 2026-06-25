+++
title = "MCP server"
description = "Hypercolor's Model Context Protocol server: 16 tools, 5 resources, 3 prompts over Streamable HTTP. Canonical docs live in Agents."
weight = 80
+++

# MCP server

Hypercolor ships a [Model Context Protocol](https://modelcontextprotocol.io/)
server so AI agents can drive your lighting through structured tool calls instead
of raw REST. It runs inside the daemon and speaks the MCP **Streamable HTTP**
transport, mounted at `/mcp` on the same `:9420` port as everything else.

{% callout(type="info") %}
This page is the API-reference stub for the MCP surface. The full, worked
documentation — client setup, every tool schema, the resource shapes, and the
prompt templates — lives in the **Agents** section. Start there:

- [Agents & MCP overview](@/agents/_index.md) — MCP vs CLI and the three primitives
- [MCP setup](@/agents/mcp-setup.md) — Claude Code / Desktop / Cursor / Zed config
- [Tools reference](@/agents/tools-reference.md) — all 16 tools, full JSON schema
- [Resources reference](@/agents/resources-reference.md) — the 5 `hypercolor://` resources
- [Prompt templates](@/agents/prompt-templates.md) — the 3 shipped prompts
{% end %}

## The transport at a glance

The server is built on `rmcp`'s `StreamableHttpService`. One endpoint handles the
whole protocol — tool listing and calls, resource reads, prompt fetches — over
HTTP with optional Server-Sent Events for streaming.

| Property | Value |
| --- | --- |
| Transport | Streamable HTTP (`streamable-http`) |
| Default URL | `http://localhost:9420/mcp` |
| Tools | 16 |
| Resources | 5 (`state`, `devices`, `effects`, `audio`, `profiles`) |
| Prompts | 3 (`mood_lighting`, `troubleshoot`, `setup_automation`) |
| Default state | **disabled** |

The server advertises tools, resources, and prompts in its capabilities and ships
`instructions` that tell agents to read `hypercolor://state` or call `get_status`
before making changes.

{% callout(type="warning") %}
**MCP is off by default.** Until you set `enabled = true` in the `[mcp]` config
block, the daemon never mounts the `/mcp` route and the endpoint returns 404. The
[MCP setup](@/agents/mcp-setup.md) page leads with enabling it, then walks the
per-client config. Enable it there first.
{% end %}

## Enable the server

Add an `[mcp]` block to your Hypercolor config and restart the daemon. The
defaults below match the daemon's `McpConfig`:

```toml
[mcp]
enabled = true            # off by default — this is the switch
base_path = "/mcp"        # endpoint path under :9420
stateful_mode = true      # session-tracked transport
json_response = false     # SSE streaming responses (set true for plain JSON)
sse_keep_alive_secs = 15  # SSE keepalive interval; 0 disables keepalive
```

Only `enabled = true` is required. Everything else has a sane default, so the
shortest working config is a single line plus the header.

## Connect a client

Any MCP client that speaks Streamable HTTP can connect at the endpoint above.
For Claude Code, point a server entry at it:

```json
{
  "mcpServers": {
    "hypercolor": {
      "type": "streamable-http",
      "url": "http://localhost:9420/mcp"
    }
  }
}
```

Claude Desktop reaches the same endpoint through an `mcp-remote` bridge, and
Cursor and Zed each have their own config shape. The copy-paste blocks for all of
them live on the [MCP setup](@/agents/mcp-setup.md) page.

## What the server exposes

The three MCP primitives map cleanly onto Hypercolor's engine.

{% mermaid() %}
graph TD
  A[MCP client] -->|tools| T[16 tools: set_effect, get_status, ...]
  A -->|resources| R[5 resources: hypercolor://state, devices, ...]
  A -->|prompts| P[3 prompts: mood_lighting, troubleshoot, setup_automation]
  T --> E[Daemon engine + event bus]
  R --> E
{% end %}

**Tools** are actions and reads. Eleven are listed as `read_only` (`list_effects`,
`get_devices`, `get_status`, `list_scenes`, `get_audio_state`, `get_sensor_data`,
`get_layout`, `diagnose`), and the mutating ones carry `idempotent` annotations so
agents can reason about retries. `create_scene` is the one tool flagged
non-idempotent, because each call writes a new scene from current state.

**Resources** are live read-only snapshots the agent can pull for context. The
`hypercolor://audio` resource updates at roughly 10 Hz when audio is active — not
per render frame — so it is a summary surface, not a spectrum stream.

**Prompts** are guided workflows: `mood_lighting` (vibe to effect), `troubleshoot`
(diagnostics-driven fixes, the only prompt with a required argument: `issue`), and
`setup_automation` (scene and schedule setup).

### Tool catalog

{% api_endpoint(method="POST", path="/mcp") %}
Tool calls and all other MCP traffic flow through this single endpoint. The table
below is a map; the [tools reference](@/agents/tools-reference.md) carries the full
input schemas, defaults, enums, and a worked call plus response for each tool.
{% end %}

| Tool | Read-only | Idempotent |
| --- | --- | --- |
| `set_effect` | No | Yes |
| `list_effects` | Yes | Yes |
| `stop_effect` | No | Yes |
| `set_color` | No | Yes |
| `get_devices` | Yes | Yes |
| `set_brightness` | No | Yes |
| `get_status` | Yes | Yes |
| `activate_scene` | No | Yes |
| `list_scenes` | Yes | Yes |
| `create_scene` | No | No |
| `get_audio_state` | Yes | Yes |
| `get_sensor_data` | Yes | Yes |
| `set_display_face` | No | Yes |
| `set_profile` | No | Yes |
| `get_layout` | Yes | Yes |
| `diagnose` | Yes | Yes |

`set_effect` and `set_color` accept fuzzy input: an exact effect name, a partial
match, or a natural-language description ("calm blue waves", "warm sunset orange").
The daemon resolves it and returns the match with a confidence score, so an agent
does not have to know the catalog by heart. Scenes are whole-rig configs and zones
are flexible canvas partitions; the tools follow that vocabulary exactly.

### Resources

| URI | Updates |
| --- | --- |
| `hypercolor://state` | on every state change |
| `hypercolor://devices` | on device connect/disconnect |
| `hypercolor://effects` | when effects are added or removed |
| `hypercolor://profiles` | when profiles change |
| `hypercolor://audio` | ~10 Hz while audio is active |

## CLI as the scripting alternative

MCP is the structured-agent path. The `hypercolor` CLI is the scripting path for
the same engine, and it carries a few capabilities MCP does not — notably effect
installation and rescan. The CLI exposes three distinct top-level commands that are
easy to confuse:

- `hypercolor server` — operate against the local daemon process
- `hypercolor servers` — manage multiple known daemon endpoints
- `hypercolor service` — manage the daemon as a system service

There is no MCP command in the CLI, and there is no install-or-rescan tool over
MCP, so a build-and-apply workflow crosses from MCP to the CLI. See
[CLI scripting for agents](@/agents/cli-scripting.md) and the
[CLI reference](@/api/cli.md) for the full command tree.

## Where to go next

- [Agents & MCP overview](@/agents/_index.md) — the canonical MCP home
- [MCP setup](@/agents/mcp-setup.md) — enable it, then configure your client
- [Tools reference](@/agents/tools-reference.md) — every tool's schema and a worked example
- [REST API reference](@/api/rest.md) — the surface MCP tools sit on top of
