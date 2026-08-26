+++
title = "OpenAPI / generated spec"
description = "Reach the utoipa-backed OpenAPI document, the bundled Swagger UI, and the hypercolor-openapi export binary."
weight = 70
+++

The daemon ships a machine-readable OpenAPI 3.1 document describing its `/api/v1`
surface, plus a bundled Swagger UI to browse it. The spec is generated at compile
time by [utoipa](https://github.com/juhaku/utoipa) from the same Rust types the
handlers serialize, so it never drifts from the wire. This page covers where to
reach it, how to export it for codegen, and the boundary of what it covers.

{% <callout type="info"> %}
The OpenAPI document and Swagger UI are always mounted. They are not gated
behind the MCP feature flag or an API key on loopback, so if the daemon is up
on `:9420`, the spec is reachable.
{% </callout> %}

## Where to reach it

The Swagger UI router is merged into the daemon's Axum app unconditionally. Three
entry points, all relative to the daemon base URL (default `http://localhost:9420`):

| Path | What it serves |
|---|---|
| `/api/v1/docs` | Swagger UI, the interactive browser for the spec |
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

{% <callout type="tip"> %}
Loopback clients are exempt from API-key auth, so the local `curl` above works
with no token. Over the network, send `Authorization: Bearer <token>` when a read
key is configured. The daemon uses a dual-key model (a control key,
`HYPERCOLOR_API_KEY`, and a read-only key, `HYPERCOLOR_READ_API_KEY`) with
loopback always exempt.
{% </callout> %}

## Export the spec without a running daemon

You do not need the daemon listening to get the document. The `hypercolor-daemon`
crate ships a dedicated binary, `hypercolor-openapi`, that serializes the same
runtime-registered document to pretty-printed JSON on stdout:

```bash
cargo run -p hypercolor-daemon --bin hypercolor-openapi --no-default-features > openapi.json
```

The `--no-default-features` flag keeps the export lean and feature-independent:
the complete API router and its document are assembled together without starting
the server. The emitted resource surface is identical regardless of which driver
features are compiled in. This is the form used by tooling and CI.

## How the document is assembled

Each domain registers its Axum handlers and operation contract together under
`crates/hypercolor-daemon/src/api/routes/`. The top-level router merges those
domain registrations and nests the result at `/api/v1`.

{% <mermaid> %}
graph TD
    A["Domain route modules"] --> B["documented_route"]
    C["Axum method router"] --> B
    D["Typed OperationDoc"] --> B
    B --> E["OpenApiRouter"]
    F["Base metadata, tags, errors, security"] --> E
    E --> G["Runtime Axum router"]
    E --> H["OpenAPI document"]
    H --> I["/api/v1/openapi.json"]
    H --> J["hypercolor-openapi binary"]
{% </mermaid> %}

`documented_route` gives Axum and utoipa the same path and method registration.
`OperationDoc` supplies the operation id, resource tag, summary, typed request,
typed success payload, real success statuses, and shared error responses. Schema
components are discovered from those operation types at registration time, so
there is no second route table or component inventory to keep synchronized.

The parity tests lock the runtime document to the Spec 78 inventory of 83 paths
and 118 operations. They also verify unique operation ids, required string path
parameters, resolvable schema references, real success bodies and media types,
and the shared error contract on every operation. A new REST route therefore
cannot silently expand the public surface or disappear from OpenAPI.

The schema components are drawn from `hypercolor-types`, the shared contract crate.

## hypercolor-types is the contract source

Request and response bodies for the core domains live in one place:
`hypercolor-types::api`, grouped by resource domains such as `assets`, `capture`,
`devices`, `effects`, `layouts`, `scene`, and `scenes`. The daemon serializes
these exact types and both UIs
deserialize them, so a wire change is a compile error rather than a runtime
surprise. When the OpenAPI document references a schema like `CaptureMonitor`,
`EffectSummary`, or `CreateZoneRequest`, it is referencing those shared
definitions.

{% <callout type="info"> %}
Diagnostic telemetry (system status internals and metrics payloads) deliberately
stays daemon-local and is not part of `hypercolor-types::api`. Those shapes move
fast with performance work, and clients consume tolerant subsets of them by
design. Treat the OpenAPI schemas for status as descriptive, not a frozen contract.
{% </callout> %}

## Coverage and limits

The document describes the REST surface only. A few things are out of scope:

- **WebSocket** at `/api/v1/ws` is listed as a path but its message protocol and
  binary frame format are not OpenAPI-describable. See
  [WebSocket protocol](@/api/websocket.md) and
  [Binary frame format](@/api/websocket-binary-frames.md).
- **MCP** at `/mcp` is a separate Streamable HTTP surface with its own tool,
  resource, and prompt schemas. See the [Agents & MCP](@/agents/_index.md) section.
- **Diagnostic telemetry** remains daemon-local, but its REST responses still
  carry concrete OpenAPI schemas. The [REST reference](@/api/rest.md) is the
  human-readable companion.

The security scheme advertised in the document is HTTP `Bearer` (`bearer_auth`,
bearer format "API key"), matching the daemon's `Authorization: Bearer <token>`
auth.

## Generated clients

The repository's Python client is generated directly from this document. The
`just python-generate` recipe runs `cargo run -p hypercolor-daemon --bin
hypercolor-openapi`, writes the JSON to a temp file, and feeds it to the codegen
script; `just python-generate-check` verifies the committed client is current. The
same export binary is the right starting point for any other language client:
point your generator of choice at the emitted `openapi.json`.

## Try this

Confirm the spec is live and count the documented operations:

```bash
curl -s http://localhost:9420/api/v1/openapi.json \
  | jq '[.paths[] | keys[]] | length'
```

The count is `118`. A matching result means the daemon is serving the locked
OpenAPI surface and you are ready to browse it at
[`/api/v1/docs`](http://localhost:9420/api/v1/docs) or wire it into a client
generator.

## Related

- [API overview](@/api/_index.md): the four daemon surfaces and the response envelope.
- [REST reference](@/api/rest.md): every `/api/v1` endpoint, grouped by domain.
- [Envelope & errors](@/api/rest-envelope-and-errors.md): the `{ data, meta }` and `{ error, meta }` shapes and the error-code-to-HTTP-status table.
- [Auth & security](@/api/auth-and-security.md): the dual-key Bearer model (`HYPERCOLOR_API_KEY` for control, `HYPERCOLOR_READ_API_KEY` for read) and the loopback exemption.
