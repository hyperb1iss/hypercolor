+++
title = "API overview"
description = "REST, WebSocket, CLI, and MCP — the four interfaces for driving the Hypercolor daemon on :9420."
sort_by = "weight"
weight = 0
template = "section.html"
+++

Hypercolor is a daemon with four front doors. Every interface — REST, WebSocket,
the `hypercolor` CLI, and the MCP server — talks to the same engine state through
the same event bus, so a brightness change made over the CLI shows up instantly
in the web UI's preview and in any subscribed WebSocket client. There is one
source of truth and four ways to reach it.

![The Hypercolor dashboard, the same engine the API drives](/img/ui/dashboard.webp)

## Pick your interface

| Interface | Transport | Reach for it when |
| --- | --- | --- |
| [REST API](@/api/rest.md) | HTTP on `:9420` | Scripting, automation, one-shot reads and writes |
| [WebSocket](@/api/websocket.md) | WS on `:9420` | Live state, canvas previews, spectrum streaming, low-latency UIs |
| [CLI](@/api/cli.md) | HTTP client to the daemon | Terminal workflows, shell scripts, agent tooling |
| [MCP server](@/api/mcp.md) | Streamable HTTP at `/mcp` | AI assistants and agents (16 tools, 5 resources, 3 prompts) |

REST and WebSocket share the same port and the same `AppState`. The CLI is a thin
HTTP client over the REST surface with table/JSON/plain rendering on top. MCP is a
separate protocol mounted at `/mcp`, and it is the canonical AI-control path — the
[Agents & MCP](@/agents/_index.md) section owns its full reference.

## Base URL and the surface map

Everything the daemon serves lives at one of three places:

- `/api/v1/...` — the REST and WebSocket surface (the bulk of the contract)
- `/health` and `/preview` — top-level, not under `/api/v1`
- `/mcp` — the MCP server, top-level, mounted only when MCP is enabled

The REST router groups its routes by domain. The full set, enumerated straight
from the daemon's `build_router()`, is assets, attachments, capture, controls,
control-surfaces (`control_values`), devices, drivers, displays, layers, layouts,
profiles, scenes, scene zones (`scenes_zones`), settings, simulators, system,
diagnose, access log, preview, and the WebSocket upgrade at `/api/v1/ws`. The
[REST reference](@/api/rest.md) documents every one.

{% callout(type="info") %}
**Zones live under scenes.** There is no top-level `/api/v1/zones` collection.
Scenes are whole-rig configurations; zones are flexible canvas partitions inside
a scene, addressed at `/api/v1/scenes/{id}/zones/...`. Scene and zone semantics
are explained in the [Studio docs](@/studio/_index.md).
{% end %}

## The response envelope

Every JSON response wraps its payload in a consistent envelope, so clients can
read `data` (or `error`) and `meta` the same way on every endpoint.

```json
{
  "data": { "...": "endpoint-specific payload" },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_0190f3c2-7a4e-7b21-9c83-2f6e1a4d5b90",
    "timestamp": "2026-06-25T18:03:11.482Z"
  }
}
```

Errors swap the top-level key from `data` to `error` and keep the same `meta`:

```json
{
  "error": {
    "code": "validation_error",
    "message": "canvas_width must be positive",
    "details": null
  },
  "meta": { "api_version": "1.0", "request_id": "req_...", "timestamp": "..." }
}
```

A few details worth pinning down. The `api_version` field is the literal string
`"1.0"` and is unrelated to the `v1` URL segment. The `request_id` is `req_`
followed by a time-ordered UUID v7, not a bare UUID. Error codes serialize in
`snake_case`, and `validation_error` maps to HTTP **422** (Unprocessable Entity),
not 400. The [REST reference](@/api/rest.md) carries the complete error-code to
HTTP-status table.

## Authentication and access

Local clients work with zero configuration. Requests from loopback addresses
(`localhost`, `127.0.0.1`, `::1`) are exempt from API-key checks, which is why
the CLI, TUI, and web UI all talk to a local daemon out of the box.

Once you expose the daemon beyond loopback, two-tier bearer auth applies:

- `HYPERCOLOR_API_KEY` — full control (read and write)
- `HYPERCOLOR_READ_API_KEY` — read-only access

Both are sent as `Authorization: Bearer <token>`. The CLI's `--api-key` flag sets
the control key for you. CORS follows the same logic: loopback origins are always
allowed, and configured `cors_origins` are honored only when API auth is enabled.

## MCP transport

The MCP server speaks **Streamable HTTP** (not stdio, not plain SSE), mounted at
`/mcp` by default and configurable through `McpConfig::base_path`. It is **off by
default** — enable it before any agent can connect.

{% callout(type="warning") %}
MCP must be turned on. Set `mcp.enabled = true` in your config (or use the
config endpoints), then restart the daemon. The [MCP setup
guide](@/agents/mcp-setup.md) leads with enabling it and provides copy-paste
client config for Claude Code, Claude Desktop, Cursor, and Zed.
{% end %}

## How the surfaces relate

{% mermaid() %}
graph TD
    Engine[Engine + AppState] --> Bus[HypercolorBus event bus]
    Bus --> REST["REST /api/v1"]
    Bus --> WS["WebSocket /api/v1/ws"]
    Bus --> MCP["MCP /mcp"]
    REST --> CLI[hypercolor CLI]
    REST --> UI[Web UI + TUI]
    WS --> UI
    MCP --> Agents[AI agents]
{% end %}

## Where to go next

- [REST API reference](@/api/rest.md) — every `/api/v1` endpoint, grouped by domain, with the envelope and error model.
- [WebSocket protocol](@/api/websocket.md) — the `hypercolor-v1` subprotocol, JSON channels, and binary canvas/spectrum frames.
- [CLI reference](@/api/cli.md) — the command tree, global flags, and environment variables. Note that `server`, `servers`, and `service` are three distinct commands: `server` targets this daemon, `servers` is the multi-daemon registry, and `service` manages the system service.
- [MCP server](@/api/mcp.md) — the pointer into the canonical [Agents & MCP](@/agents/_index.md) reference for tools, resources, and prompts.
