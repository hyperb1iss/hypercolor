+++
title = "OpenAPI / generated spec"
description = "Reach the utoipa-backed OpenAPI document, the bundled Swagger UI, and the hypercolor-openapi export binary."
weight = 70
+++

# OpenAPI / generated spec

The daemon ships a machine-readable OpenAPI 3.1 document describing its `/api/v1`
surface, plus a bundled Swagger UI to browse it. The spec is generated at compile
time by [utoipa](https://github.com/juhaku/utoipa) from the same Rust types the
handlers serialize, so it never drifts from the wire. This page covers where to
reach it, how to export it for codegen, and the boundary of what it covers.

{% callout(type="info") %}
The OpenAPI document and Swagger UI are always mounted — they are not gated
behind the MCP feature flag or an API key on loopback. If the daemon is up on
`:9420`, the spec is reachable.
{% end %}

## Where to reach it

The Swagger UI router is merged into the daemon's Axum app unconditionally. Three
entry points, all relative to the daemon base URL (default `http://localhost:9420`):

| Path | What it serves |
|---|---|
| `/api/v1/docs` | Swagger UI — interactive browser for the spec |
| `/api/v1/openapi.json` | The raw OpenAPI 3.1 document as JSON |
| `/api/v1/docs/openapi.json` | Same document, resolved by the Swagger UI bundle |

Open the interactive UI in a browser:

```bash
xdg-open http://localhost:9420/api/v1/docs
```

Or pull the raw document straight from the running daemon:

```bash
curl -s http://localhost:9420/api/v1/openapi.json | jq '.info'
```

{% callout(type="tip") %}
Loopback clients are exempt from API-key auth, so the local `curl` above works
with no token. Over the network, send `Authorization: Bearer <token>` when a read
key is configured. The daemon uses a dual-key model — a control key
(`HYPERCOLOR_API_KEY`) and a read-only key (`HYPERCOLOR_READ_API_KEY`) — with
loopback always exempt.
{% end %}

## Export the spec without a running daemon

You do not need the daemon listening to get the document. The `hypercolor-daemon`
crate ships a dedicated binary, `hypercolor-openapi`, that serializes the same
`ApiDoc` to pretty-printed JSON on stdout:

```bash
cargo run -p hypercolor-daemon --bin hypercolor-openapi --no-default-features > openapi.json
```

The `--no-default-features` flag keeps the export lean and feature-independent:
the route catalog is assembled from a static `ROUTES` table plus utoipa's derived
operations, so the emitted document is identical regardless of which driver
features are compiled in. This is the form used by tooling and CI.

## How the document is assembled

The spec is built from two layers in `crates/hypercolor-daemon/src/api/openapi.rs`:

{% mermaid() %}
graph TD
    A["#[derive(OpenApi)] ApiDoc"] --> B["utoipa::path operations<br/>(fully-annotated endpoints)"]
    A --> C["components::schemas<br/>(shared request/response types)"]
    D["RouteCatalogAddon modifier"] --> E["static ROUTES table<br/>(every /api/v1 path + method)"]
    B --> F["ApiDoc::openapi()"]
    C --> F
    E --> F
    F --> G["/api/v1/openapi.json"]
    F --> H["hypercolor-openapi binary"]
{% end %}

A handful of endpoints (system, drivers, devices, effects) carry full utoipa
`#[utoipa::path]` annotations with request and response schemas. Every other route
is registered through the static `ROUTES` catalog, so the document lists the
complete path-and-method surface even where a per-operation body schema is not yet
annotated. The catalog mirrors the live router; the two stay in lockstep because
both describe `/api/v1`.

The schema components are drawn from `hypercolor-types`, the shared contract crate.

## hypercolor-types is the contract source

Request and response bodies for the core domains live in one place:
`hypercolor-types::api`, with submodules `common`, `devices`, `effects`, `scenes`,
and `zones`. The daemon serializes these exact types and both UIs deserialize them,
so a wire change is a compile error rather than a runtime surprise. When the
OpenAPI document references a schema like `EffectSummary` or `CreateZoneRequest`,
it is referencing those shared definitions.

{% callout(type="info") %}
Diagnostic telemetry — system status internals and metrics payloads — deliberately
stays daemon-local and is not part of `hypercolor-types::api`. Those shapes move
fast with performance work, and clients consume tolerant subsets of them by
design. Treat the OpenAPI schemas for status as descriptive, not a frozen contract.
{% end %}

## Coverage and limits

The document describes the REST surface only. A few things are out of scope:

- **WebSocket** at `/api/v1/ws` is listed as a path but its message protocol and
  binary frame format are not OpenAPI-describable. See
  [WebSocket protocol](@/api/websocket.md) and
  [Binary frame format](@/api/websocket-binary-frames.md).
- **MCP** at `/mcp` is a separate Streamable HTTP surface with its own tool,
  resource, and prompt schemas. See the [Agents & MCP](@/agents/_index.md) section.
- **Per-operation request bodies** are fully annotated for the core endpoints and
  catalogued (path + method + standard responses) for the rest. The
  [REST reference](@/api/rest.md) is the human-readable companion enumerated from
  the same router.

The security scheme advertised in the document is HTTP `Bearer` (`bearer_auth`,
bearer format "API key"), matching the daemon's `Authorization: Bearer <token>`
auth.

## Generated clients

The repository's Python client is generated directly from this document. The
`just python-generate` recipe runs `cargo run -p hypercolor-daemon --bin
hypercolor-openapi`, writes the JSON to a temp file, and feeds it to the codegen
script; `just python-generate-check` verifies the committed client is current. The
same export binary is the right starting point for any other language client —
point your generator of choice at the emitted `openapi.json`.

## Try this

Confirm the spec is live and count the documented operations:

```bash
curl -s http://localhost:9420/api/v1/openapi.json \
  | jq '[.paths[] | keys[]] | length'
```

A non-zero count means the daemon is serving the OpenAPI document and you are
ready to browse it at [`/api/v1/docs`](http://localhost:9420/api/v1/docs) or wire
it into a client generator.

## Related

- [API overview](@/api/_index.md) — the four daemon surfaces and the response envelope.
- [REST reference](@/api/rest.md) — every `/api/v1` endpoint, grouped by domain.
- [Envelope & errors](@/api/rest-envelope-and-errors.md) — the `{ data, meta }` and `{ error, meta }` shapes and the `ErrorCode`-to-HTTP-status table.
- Auth & security — the dual-key Bearer model (`HYPERCOLOR_API_KEY` for control, `HYPERCOLOR_READ_API_KEY` for read) and the loopback exemption.
