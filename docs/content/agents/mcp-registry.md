+++
title = "Publishing to the MCP Registry"
description = "Publish the Hypercolor MCP server to the official registry: server.json, the io.github.hyperb1iss/hypercolor namespace, and submission."
weight = 70
template = "page.html"
+++

The [MCP Registry](https://registry.modelcontextprotocol.io/) is the public discovery index for Model Context Protocol servers. Publishing Hypercolor there puts it in front of every MCP-aware client that browses the registry, so an assistant can find and connect to the lighting engine without anyone hand-editing a config file. This page walks through the `server.json` manifest, the namespace convention, and the submission flow.

{% callout(type="info") %}
This is a maintainer task, not a user setup step. To connect your own assistant to a daemon you already run, you do not need the registry at all. Head to [MCP setup](@/agents/mcp-setup.md) for the copy-paste config. The registry matters when you want the world to discover Hypercolor's MCP server.
{% end %}

## What the registry is for 🔮

A user who already knows about Hypercolor configures it the manual way, with a one-line `claude mcp add` or a `.mcp.json` block (see [MCP setup](@/agents/mcp-setup.md)). The registry serves the other direction: a client that does not yet know Hypercolor exists can search the index, read the manifest, and learn how to reach the server. Think of it as the npm of MCP servers, a single namespaced catalog that every compatible tool can query.

The unit of publication is a `server.json` manifest. It describes who owns the server, how to reach it, and which transport it speaks. For Hypercolor that transport is Streamable HTTP, the same one the daemon mounts at `/mcp`.

## Before you publish

Confirm the live server first. The manifest you submit must match what the daemon actually serves, or clients will hit a dead connection on their first call.

- The daemon exposes the MCP server over **Streamable HTTP** at the base path `/mcp`. It is **off by default**, mounted only when `mcp.enabled = true`. See [MCP setup](@/agents/mcp-setup.md).
- The surface clients discover through it is **16 tools, 5 resources, and 3 prompts**. The [tools reference](@/agents/tools-reference.md), [resources reference](@/agents/resources-reference.md), and [prompt templates](@/agents/prompt-templates.md) are the authoritative lists.
- The server identifies itself as `hypercolor`, titled "Hypercolor RGB Lighting Controller", with its website set to the GitHub repository. The daemon reports its crate version (`CARGO_PKG_VERSION`) as the server version, so the manifest version should track the release you are publishing.

{% callout(type="warning") %}
Hypercolor binds to loopback (`127.0.0.1:9420`) by default and local clients need no credentials. A registry listing is a public pointer, so think carefully about what URL you advertise. A `127.0.0.1` endpoint only resolves on the same machine; a remote endpoint requires a reachable host and a bearer token from `HYPERCOLOR_API_KEY`. Do not publish a manifest that points at a private address as if it were globally reachable.
{% end %}

## The namespace

The MCP Registry namespaces servers by their source of authority. Hypercolor lives under the maintainer's GitHub identity, so the canonical server name is:

```
io.github.hyperb1iss/hypercolor
```

The `io.github.<owner>/<repo>` form ties the listing to the GitHub account that owns the project, which is how the registry proves you control the namespace you are publishing under. Authentication during submission is performed against that same GitHub identity.

## The server.json manifest

`server.json` is the manifest the registry stores and clients read. For Hypercolor's HTTP transport it advertises a remote endpoint rather than a runnable package. A minimal manifest looks like this:

```json
{
  "$schema": "https://static.modelcontextprotocol.io/schemas/2025-07-09/server.schema.json",
  "name": "io.github.hyperb1iss/hypercolor",
  "description": "AI-powered RGB lighting control for Linux with effects, devices, layouts, profiles, scenes, and diagnostics.",
  "version": "0.1.0",
  "websiteUrl": "https://github.com/hyperb1iss/hypercolor",
  "repository": {
    "url": "https://github.com/hyperb1iss/hypercolor",
    "source": "github"
  },
  "remotes": [
    {
      "type": "streamable-http",
      "url": "http://127.0.0.1:9420/mcp"
    }
  ]
}
```

A few fields are load-bearing and worth getting right.

- **`name`** must be the namespaced `io.github.hyperb1iss/hypercolor`, not a bare `hypercolor`. The registry rejects unnamespaced submissions.
- **`description`** mirrors the description string the daemon already reports in its `ServerInfo`, so the catalog entry and the live handshake agree.
- **`version`** should track the daemon release you are publishing. The running server reports its crate version through the MCP handshake; keep the manifest in step so a client comparing the two does not see a mismatch.
- **`remotes[].type`** is `streamable-http`. Hypercolor has no stdio binary and no separate bridge process, so there is no `packages` block with a `command` and `args`. The whole point is that a client POSTs to the daemon's HTTP endpoint directly.
- **`remotes[].url`** is the endpoint clients connect to. The loopback URL above is correct for a same-machine listing; a publicly discoverable server needs a routable host and the bearer-token note below.

{% callout(type="tip") %}
Validate the manifest against the published schema before submitting. The `$schema` URL points at the version the registry expects, and most JSON tooling can lint against it locally so you catch a malformed `remotes` block before the registry does.
{% end %}

### Remote endpoints and auth

If you advertise anything other than a loopback URL, the daemon is being reached from a non-loopback address, which means auth is enforced. Remote clients must send `Authorization: Bearer <token>`, where the token comes from `HYPERCOLOR_API_KEY` (control) or `HYPERCOLOR_READ_API_KEY` (read-only). The registry manifest itself does not carry secrets; it points at the endpoint, and the connecting client supplies the credential. The full auth model, including the loopback exemption and remote-client allowlisting, is documented in [MCP setup](@/agents/mcp-setup.md).

## Submitting to the registry

Publication uses the registry's own publisher CLI, which authenticates you against the GitHub namespace and pushes the manifest.

```bash
# Authenticate against the io.github.hyperb1iss namespace
mcp-publisher login github

# Publish the manifest from the repository root
mcp-publisher publish
```

The flow, in order:

1. Author and validate `server.json` against the schema referenced in its `$schema` field.
2. Authenticate with `mcp-publisher login github`. This proves you control the `io.github.hyperb1iss` namespace.
3. Run `mcp-publisher publish` to push the manifest. The registry verifies the namespace ownership and stores the listing.
4. Confirm the entry resolves by searching the registry for `io.github.hyperb1iss/hypercolor`.

Once published, any MCP client that browses the registry can discover Hypercolor and learn that it speaks Streamable HTTP at the advertised endpoint.

{% mermaid() %}
graph TD
  A[Author server.json] --> B[Validate against schema]
  B --> C[mcp-publisher login github]
  C --> D[mcp-publisher publish]
  D --> E[Registry verifies namespace]
  E --> F[Listed under io.github.hyperb1iss/hypercolor]
  F --> G[MCP clients discover the server]
{% end %}

## Keeping the listing honest

A registry entry is a promise about a live server, so it has to stay true as Hypercolor evolves.

- **Re-publish on release.** When the daemon version moves, bump the manifest `version` and re-run `mcp-publisher publish` so the catalog and the handshake report the same number.
- **Track the surface.** The listing advertises a tool, resource, and prompt count by reputation, not by field, but a client that reads the description expects what it finds. When the [tools reference](@/agents/tools-reference.md) grows, refresh the description so the catalog stays accurate.
- **Watch the schema.** The registry schema is versioned in the `$schema` URL. If the registry adopts a newer schema, update the reference and re-validate before the next publish.

## Where this fits

This page is the publish-side counterpart to the connect-side setup. If you are wiring an assistant to a daemon you already run, you do not need the registry at all.

- **[MCP setup](@/agents/mcp-setup.md)** — Enable the server and connect a client by hand, the path most users actually take.
- **[Tools reference](@/agents/tools-reference.md)** — The 16 tools a registry client discovers once connected.
- **[Resources reference](@/agents/resources-reference.md)** — The 5 `hypercolor://` resources, with payload shapes and freshness notes.
- **[Prompt templates](@/agents/prompt-templates.md)** — The 3 shipped prompts a connected client can surface as slash commands.
- **[Agents & MCP overview](@/agents/_index.md)** — The three-primitive model and how MCP and the CLI fit together.
