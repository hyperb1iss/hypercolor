# Client Generation

Hypercolor has two client contracts:

- REST is generated from the daemon OpenAPI document.
- WebSocket helpers are generated from `protocol/websocket-v1.json`.

The Rust daemon is the source of truth for both. Generated clients stay in this
repository so Home Assistant, Python packaging, and the future TypeScript client
all evolve with the daemon API.

## REST Contract

The daemon exports OpenAPI without starting the HTTP server:

```bash
cargo run -p hypercolor-daemon --bin hypercolor-openapi --quiet
```

Python vendors the generated REST client under the private package
`python/src/hypercolor/_generated/`:

```bash
just python-generate
just python-generate-check
```

`python/scripts/generate_openapi_client.py` uses
`openapi-python-client` through the `generate` dependency group. It fails on
generator warnings, validates the JSON document, and compares generated output
directly during `--check`.

## WebSocket Contract

The shared WebSocket manifest lives at:

```text
protocol/websocket-v1.json
```

It records channel names, advertised capabilities, binary frame tags, preview
pixel formats, and subscription config bounds. The daemon has a regression test
that compares the manifest with `WsChannel::SUPPORTED`, `ws_capabilities()`,
and the binary tag constants.

Python generates protocol constants from the manifest:

```bash
just python-ws-protocol-generate
just python-ws-protocol-check
```

## CI Gates

The Python job runs hand-written client checks:

```bash
just python-verify
```

That includes Ruff, Ruff format, ty, WebSocket protocol drift, and pytest.
Generated OpenAPI drift runs in the separate `Python Generated Client` job
because it compiles the Rust daemon exporter.

PyPI publishing depends on both Python jobs, so releases cannot ship with stale
generated clients.

## TypeScript Client Path

When the TypeScript client lands, it should use the same two inputs:

- OpenAPI JSON from `hypercolor-openapi` for REST types and endpoint helpers.
- `protocol/websocket-v1.json` for WebSocket channels, binary tags, preview
  frame decoding, and subscription config types.

Keep generated TypeScript output isolated from hand-written ergonomic wrappers,
matching the Python split between private `hypercolor._generated` plumbing and
the public `HypercolorClient`.
