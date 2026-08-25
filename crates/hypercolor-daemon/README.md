# hypercolor-daemon

_The beating heart of Hypercolor — render loop, hardware orchestration, and API server._

This crate is the Hypercolor daemon binary. It owns the full runtime: device discovery and
management, effect composition via SparkleFlinger (up to 60 fps, adaptive across five tiers),
scene management, spatial LED layout, and user configuration. Everything is exposed
outward as a REST + WebSocket API on port 9420 (Axum) with a Swagger UI at `/api/v1/docs`, an
MCP server for AI integration, and mDNS advertisement for LAN discovery. On Linux the daemon
integrates with systemd via sd-notify; on Windows it can run as a Windows Service.

## Role in the Workspace

Leaf binary — the top of the dependency stack. Consumes hypercolor-core, hypercolor-hal (via
hypercolor-driver-builtin), hypercolor-driver-api, hypercolor-driver-support,
hypercolor-network, hypercolor-types, hypercolor-color, hypercolor-gpu-frame,
hypercolor-platform-fs, hypercolor-leptos-ext, and one platform crate per capability seam:
session monitors on all three platforms, hypercolor-macos-owner, macOS capture, input, and
GPU interop (the last behind `screen-capture`), and Windows capture and GPU interop (both
behind `wgpu`). No crate depends on this one at build time; hypercolor-cli dev-depends on it
for integration tests.

## Binary

| Binary              | Command                                        |
| ------------------- | ---------------------------------------------- |
| `hypercolor-daemon` | `just daemon` (preview profile, debug logging) |

Notable secondary binaries built from `src/bin/`: `hypercolor-debug` (diagnostics),
`hypercolor-openapi` (dumps the OpenAPI spec), and `hypercolor-ws-manifest` (regenerates the
WebSocket protocol manifest, driven by `just ws-manifest`).

## Cargo Features

| Feature            | Default | Description                                                  |
| ------------------ | ------- | ------------------------------------------------------------ |
| `builtin-drivers`  | yes     | Bundles all HAL device drivers via hypercolor-driver-builtin |
| `wgpu`             | yes     | GPU-accelerated compositing; pulls in the Windows capture and GPU-interop crates |
| `screen-capture`   | yes     | macOS ScreenCaptureKit GPU path; implies `wgpu`              |
| `servo`            | yes     | Servo HTML effect rendering                                  |
| `servo-gpu-import` | yes     | Zero-copy Servo-to-wgpu texture import on Linux, macOS, and Windows |
| `media-lottie`     | no      | Lottie playback in the media-player renderer                 |
| `media-video`      | no      | GStreamer video playback in the media-player renderer        |

## API Surface

The daemon serves on `:9420`:

- `GET /api/v1/effects` — list all effects
- `POST /api/v1/effects/{id}/apply`: apply an effect to a scene zone
- `GET /api/v1/scene`: read the complete live scene tree
- `PATCH /api/v1/scene/zones/{zone}/layers/{layer}/controls`: update live controls
- `POST /api/v1/scene/clear`: clear one zone or the whole live scene
- `GET /api/v1/devices` — connected devices
- `GET|POST|DELETE /api/v1/library/favorites` — favorites CRUD
- `GET|POST /api/v1/scenes` + `POST /api/v1/scenes/snapshot`: scene management
- `POST /api/v1/scenes/{id}/activate`: scene activation
- `GET|POST /api/v1/layouts` — spatial layout CRUD
- `WebSocket /api/v1/ws` — real-time events, canvas frames, metrics, spectrum
- `GET /api/v1/docs` — interactive API docs; the raw spec is at `GET /api/v1/openapi.json`
- MCP server: 17 tools, 5 resources for AI integration

## Usage

```bash
just daemon          # Run daemon with preview profile and debug logging
just daemon-servo    # Run daemon with Servo HTML effect rendering enabled
```

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB lighting
orchestration for Linux, Windows, and macOS. Apache-2.0 licensed.
