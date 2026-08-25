# Client Generation

Hypercolor has two client contracts:

- REST is generated from the daemon OpenAPI document.
- WebSocket helpers are generated from `protocol/websocket-v1.json`, which is
  itself generated from the daemon's topic registry.

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
pixel formats, and subscription config bounds.

**That file is generated output, not input. Do not hand-edit it.** Every fact
about a topic comes from `define_ws_topics!`, the event vocabulary comes from
`HypercolorEvent`, config bounds come from the compiled registry metadata, and
binary layouts come from the compiled codecs
(`crates/hypercolor-daemon/src/api/ws/manifest.rs`). An edit made directly to
the JSON is overwritten by the next regeneration and fails the drift check in
the meantime.

The authored half is `protocol/websocket-v1.descriptions.json`, which holds the
human-readable topic and transport descriptions. Change wording there.

Regenerate the manifest and verify it against the registry:

```bash
just ws-manifest
just ws-manifest-check
```

The `Python Generated Client` job in `ci.yml` runs the same `--check` pass, so a
stale manifest fails CI.

Python generates protocol constants from the manifest:

```bash
just python-ws-protocol-generate
just python-ws-protocol-check
```

## CI Gates

The Python job runs the hand-written client checks as individual `uv run`
steps, the same set `just python-verify` runs locally: Ruff check, Ruff
format check, ty, WebSocket protocol drift, and pytest.
Generated OpenAPI drift runs in the separate `Python Generated Client` job
because it compiles the Rust daemon exporter.

The Python client is published to PyPI as `hypercolor`. Publishing is
automated: stable release tags build sdist and wheel artifacts, and the
`publish-pypi` job in `ci.yml` uploads them through PyPI trusted publishing
(OIDC, no token).

## TypeScript Client Path

When the TypeScript client lands, it should use the same two inputs:

- OpenAPI JSON from `hypercolor-openapi` for REST types and endpoint helpers.
- `protocol/websocket-v1.json` for WebSocket channels, binary tags, preview
  frame decoding, and subscription config types.

Keep generated TypeScript output isolated from hand-written ergonomic wrappers,
matching the Python split between private `hypercolor._generated` plumbing and
the public `HypercolorClient`.
