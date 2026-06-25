+++
title = "MCP setup"
description = "Enable the Hypercolor MCP server and connect it to Claude Code, Claude Desktop, Cursor, Zed, and generic MCP clients."
weight = 10
template = "page.html"
+++

The Hypercolor daemon ships a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server that exposes 16 tools, 5 resources, and 3 prompts over Streamable HTTP. This page gets it running and wired into your assistant. There is one thing to do before anything else: turn it on.

{% callout(type="warning") %}
The MCP server is **off by default**. Until you enable it in config, `http://127.0.0.1:9420/mcp` returns 404 and no client can connect. Enabling it is step one below.
{% end %}

## Step 1: Enable the server 🔮

The daemon only mounts the MCP router when `mcp.enabled` is `true`. Add this to your Hypercolor config:

```toml
[mcp]
enabled = true
```

That single flag is the difference between a live server and a 404. The remaining `[mcp]` keys all have sensible defaults, so `enabled = true` on its own is a complete configuration.

| Key | Default | What it does |
| --- | --- | --- |
| `enabled` | `false` | Mounts the MCP router. Must be `true` to connect. |
| `base_path` | `"/mcp"` | The mount path. Empty or `"/"` normalizes back to `/mcp`; a leading slash is added and trailing slashes are trimmed, so `base_path = "mcp"` still serves at `/mcp`. |
| `stateful_mode` | `true` | Keeps per-session state across requests (the standard mode for conversational clients). |
| `json_response` | `false` | When `false`, responses stream over SSE. Set `true` for minimal HTTP clients that want single-shot JSON. |
| `sse_keep_alive_secs` | `30` | SSE keep-alive interval. Set to `0` to disable keep-alive pings. |

Restart the daemon after editing config so the router picks up the change.

## Step 2: Know your URL

The MCP server lives on the same Axum router as the REST API, at the daemon's listen address. With the shipped defaults (`listen_address = "127.0.0.1"`, `port = 9420`, `base_path = "/mcp"`) the endpoint is:

```text
http://127.0.0.1:9420/mcp
```

The transport is **Streamable HTTP** (an `rmcp` `StreamableHttpService`). There is no stdio binary and no `command`/`args` launch form. Clients connect to the URL directly, or bridge to it when they only speak stdio (see Claude Desktop below).

## Step 3: Verify it is up

Before configuring a client, confirm the daemon is running and the route is mounted.

{% api_endpoint(method="GET", path="/health") %}
Returns the daemon's health snapshot. A 200 here proves the daemon is up. Note that `/health` is a top-level route, not under `/api/v1`.
{% end %}

```bash
curl -s http://127.0.0.1:9420/health
```

If that succeeds but a later MCP connection 404s, the cause is almost always `mcp.enabled` still being `false`, or the daemon not having been restarted after the config edit.

## Connect Claude Code

Claude Code speaks Streamable HTTP natively and adds servers from the CLI, so there is no JSON to hand-edit:

```bash
claude mcp add --transport http hypercolor http://127.0.0.1:9420/mcp
```

For a server you want checked into a project, write a `.mcp.json` instead:

```json
{
  "mcpServers": {
    "hypercolor": {
      "type": "http",
      "url": "http://127.0.0.1:9420/mcp"
    }
  }
}
```

For a user-global server rather than a project one, add `--scope user` to the `claude mcp add` command.

## Connect Claude Desktop

Claude Desktop's classic `claude_desktop_config.json` historically launches **stdio** servers (a `command` plus `args`), so it cannot POST to a Streamable HTTP endpoint directly through that file. Bridge to the HTTP server with [`mcp-remote`](https://www.npmjs.com/package/mcp-remote):

```json
{
  "mcpServers": {
    "hypercolor": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://127.0.0.1:9420/mcp"]
    }
  }
}
```

The config file lives at:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

Restart Claude Desktop after editing. Newer Desktop builds with native remote-connector support can target the URL without the bridge; if yours offers that, use it. The `mcp-remote` bridge is the portable default that works regardless of version.

## Connect Cursor and Zed

Both editors support remote MCP servers over HTTP. Point them at the same URL.

For Cursor, add the server to `.cursor/mcp.json` (project) or your global Cursor MCP settings:

```json
{
  "mcpServers": {
    "hypercolor": {
      "url": "http://127.0.0.1:9420/mcp"
    }
  }
}
```

For Zed, add an HTTP MCP server entry to `settings.json` referencing the same `http://127.0.0.1:9420/mcp` URL. Consult your editor's current MCP documentation for the exact key names, since these evolve; the connection target is always the daemon's `/mcp` endpoint.

## Connect a generic MCP client

Any MCP client that can speak Streamable HTTP connects with just the URL. Two daemon settings matter for thinner clients:

- If your client cannot consume SSE streams, set `json_response = true` in the `[mcp]` config so responses come back as single-shot JSON.
- The default base path is `/mcp`. If you changed `base_path`, append your value to the daemon address instead.

{% callout(type="info") %}
The server advertises three capabilities to every client on connect: tools, resources, and prompts. It also ships operating instructions that tell the agent to read `get_status` or the `hypercolor://state` resource first, browse the catalog with `list_effects` before applying visuals, and prefer structured arguments over guessing. A well-behaved client surfaces all three primitives automatically.
{% end %}

## Authentication

Local agents need no credentials. Loopback requests bypass auth entirely, so a client talking to `127.0.0.1:9420` connects with no key. This is why everything above works with a bare URL.

A bearer token is only required when the daemon is reached from a **non-loopback** address. In that case the client must send:

```text
Authorization: Bearer <token>
```

Tokens come from two environment variables read by the daemon at startup:

- `HYPERCOLOR_API_KEY` — the control tier, allowed to mutate state.
- `HYPERCOLOR_READ_API_KEY` — the optional read-only tier.

{% callout(type="warning") %}
The `?token=` query-string fallback is accepted only on the `/api/v1/ws` WebSocket route, never on `/mcp`. Remote MCP clients must use the `Authorization: Bearer` header. Reaching a non-loopback daemon also requires that the client's address pass the daemon's network-access policy, so an exposed rig needs both a valid key and an allowed origin.
{% end %}

## Remote rigs

If the agent and the daemon are on different machines, first locate the daemon. The CLI discovers Hypercolor daemons advertised over mDNS on the local network:

```bash
hypercolor servers discover
```

Point the MCP URL at the discovered host and port (`http://<host>:9420/mcp`), bind the daemon to a reachable interface via `listen_address`, set `HYPERCOLOR_API_KEY` on the daemon, and send the matching bearer token from the client. For the full security model, including remote-client allowlisting and the dual-key tiers, see the API reference for auth and security if that page is present in your build; otherwise the `security` module in the daemon is the source of truth.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| `/mcp` returns 404 | `mcp.enabled` is still `false`, or the daemon was not restarted | Set `enabled = true` under `[mcp]` and restart the daemon. |
| Connection works but no tools appear | Client connected to the wrong path | Confirm the URL ends in your `base_path` (default `/mcp`). |
| 401 / 403 from a remote client | Non-loopback request without a valid key or allowed origin | Set `HYPERCOLOR_API_KEY` on the daemon and send `Authorization: Bearer <token>`; confirm the client address is allowed. |
| Minimal HTTP client hangs on the response | Client cannot read the SSE stream | Set `json_response = true` in `[mcp]` for single-shot JSON. |
| Claude Desktop cannot reach the URL | Classic config only launches stdio servers | Bridge with `mcp-remote` as shown above. |

## Where to go next

Once the server is connected, learn what it can do:

- **[Agents & MCP](@/agents/_index.md)** — The three-primitive model and how MCP and the CLI complement each other.
- **[Tools reference](@/agents/tools-reference.md)** — All 16 tools with arguments, defaults, enums, and worked calls.
- **[Resources reference](@/agents/resources-reference.md)** — The 5 `hypercolor://` resources and their payload shapes.
- **[Prompt templates](@/agents/prompt-templates.md)** — The 3 shipped prompts and when each fits.
- **[CLI scripting for agents](@/agents/cli-scripting.md)** — Drive the daemon from a shell when an agent cannot speak MCP.
