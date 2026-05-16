# hypercolor-daemon

*The beating heart of Hypercolor — render loop, hardware orchestration, and API server.*

This crate is the Hypercolor daemon binary. It owns the full runtime: device discovery and
management, effect composition via SparkleFlinger (up to 60 fps, adaptive across five tiers),
scene and profile management, spatial LED layout, and user configuration. Everything is exposed
outward as a REST + WebSocket API on port 9420 (Axum) with a Swagger UI at `/swagger-ui`, an
MCP server for AI integration, and mDNS advertisement for LAN discovery. On Linux the daemon
integrates with systemd via sd-notify; on Windows it can run as a Windows Service.

## Role in the Workspace

Leaf binary — the top of the dependency stack. Consumes hypercolor-core, hypercolor-hal (via
hypercolor-driver-builtin), hypercolor-driver-api, hypercolor-network, hypercolor-types, and
hypercolor-leptos-ext. Optionally pulls in hypercolor-cloud-client under the `cloud` feature.
Nothing in the workspace depends on this crate.

## Binary

| Binary | Command |
|--------|---------|
| `hypercolor-daemon` | `just daemon` (preview profile, debug logging) |

Notable secondary binaries built from `src/bin/`: `hypercolor-debug` (diagnostics) and
`hypercolor-openapi` (dumps the OpenAPI spec).

## Cargo Features

| Feature | Default | Description |
|---------|---------|-------------|
| `builtin-drivers` | yes | Bundles all HAL device drivers via hypercolor-driver-builtin |
| `wgpu` | yes | GPU-accelerated effect rendering |
| `servo` | yes | Servo HTML effect rendering |
| `servo-gpu-import` | no | Servo/wgpu GPU texture sharing on Linux |
| `cloud` | no | Enables cloud connectivity via hypercolor-cloud-client |
| `official-cloud` | no | Alias for `cloud` used in official builds |
| `nvidia` | no | Propagates the NVIDIA display capture path from hypercolor-core |

## API Surface

The daemon serves on `:9420`:

- `GET /api/v1/effects` — list all effects
- `POST /api/v1/effects/{id}/apply` — apply effect to devices
- `PATCH /api/v1/effects/current/controls` — update live controls
- `GET /api/v1/devices` — connected devices
- `GET|POST|DELETE /api/v1/library/favorites` — favorites CRUD
- `GET|POST /api/v1/scenes` + `POST /api/v1/scenes/{id}/activate` — scene management
- `GET|POST /api/v1/layouts` — spatial layout CRUD
- `GET|POST /api/v1/profiles` — profile save/load
- `WebSocket /api/v1/ws` — real-time events, canvas frames, metrics, spectrum
- `GET /swagger-ui` — interactive API docs
- MCP server — 16 tools, 5 resources for AI integration

## Usage

```bash
just daemon          # Run daemon with preview profile and debug logging
just daemon-servo    # Run daemon with Servo HTML effect rendering enabled
```

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux. Apache-2.0 licensed.
