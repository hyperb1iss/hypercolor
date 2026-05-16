+++
title = "Architecture"
description = "System design, crate structure, and key technical decisions"
sort_by = "weight"
template = "section.html"
+++

Hypercolor is a daemon-first lighting engine. A background service runs the render thread, composes frames through **SparkleFlinger**, manages device connections, and exposes control interfaces. Clients (web UI, TUI, CLI, AI assistants) connect to the daemon — they never talk to hardware directly.

## System Overview

{% mermaid() %}
graph TB
subgraph Input Sources
A1[Audio FFT]
A2[Screen Capture]
A3[MIDI]
end

    subgraph Effect Engine
        B1[HTML/Canvas/WebGL<br/>Servo Path]
        B2[wgpu Native<br/>Shader Path]
    end

    subgraph Daemon Core
        SF[SparkleFlinger<br/>compositor]
        C[Spatial Sampler<br/>composed frame → LEDs]
        D[Event Bus<br/>tokio broadcast/watch]
        E[Device Registry]
    end

    subgraph Device Backends
        F1[Razer USB HID]
        F2[Corsair USB HID]
        F3[ASUS USB/I2C]
        F4[PrismRGB USB HID]
        F5[WLED UDP DDP]
        F6[Hue / Nanoleaf REST]
        F7[QMK USB HID]
        F8[Lian Li USB HID]
        F9[Govee LAN / Cloud]
        F10[Dygma USB Serial<br/>⚠ blocked]
    end

    subgraph Client Interfaces
        G1[Web UI<br/>Leptos WASM]
        G2[TUI<br/>Ratatui]
        G3[CLI<br/>hypercolor]
        G4[MCP Server<br/>AI Assistants]
    end

    A1 --> B1
    A1 --> B2
    A2 --> B1
    A3 --> B1

    B1 -->|surface| SF
    B2 -->|surface| SF
    SF -->|composed frame| C
    C -->|per-zone colors| E
    E --> F1
    E --> F2
    E --> F3
    E --> F4
    E --> F5
    E --> F6
    E --> F7
    E --> F8
    E --> F9
    E --> F10

    D <--> G1
    D <--> G2
    D <--> G3
    D <--> G4

    C -.->|frame data| D
    E -.->|device events| D

{% end %}

## Crate Structure

The project is organized into focused crates with strict dependency boundaries, grouped by layer:

### 💎 Shared Types

| Crate                  | Depends On | Responsibility                                               |
| ---------------------- | ---------- | ------------------------------------------------------------ |
| `hypercolor-types`     | (none)     | Zero-dependency shared data vocabulary — import from here, never sibling internals |

### Engine Core

| Crate                  | Depends On        | Responsibility                                                                      |
| ---------------------- | ----------------- | ----------------------------------------------------------------------------------- |
| `hypercolor-core`      | `types`           | Render loop, device backends, Servo renderer, event bus, spatial sampler, input pipeline, scenes |

### HAL + Platform Interop

| Crate                           | Depends On | Responsibility                                                           |
| ------------------------------- | ---------- | ------------------------------------------------------------------------ |
| `hypercolor-hal`                | `types`    | Hardware abstraction: USB/HID/SMBus protocol encoding and transport       |
| `hypercolor-linux-gpu-interop`  | `types`    | Linux zero-copy GL→wgpu texture import; stubbed on other platforms *(unsafe boundary)* |
| `hypercolor-windows-pawnio`     | `types`    | Windows SMBus via the PawnIO kernel driver; stubbed elsewhere *(unsafe boundary)* |

### Driver Layer

| Crate                      | Depends On            | Responsibility                                                               |
| -------------------------- | --------------------- | ---------------------------------------------------------------------------- |
| `hypercolor-driver-api`    | `types`, `core`       | Stable trait/type boundary between daemon and drivers                        |
| `hypercolor-driver-builtin`| `driver-api`, `hal`, network drivers | Compile-time bundle of HAL + network drivers via feature flags |

### Network Driver Layer

| Crate                       | Depends On   | Responsibility                                           |
| --------------------------- | ------------ | -------------------------------------------------------- |
| `hypercolor-driver-hue`     | `driver-api` | Philips Hue Bridge driver (Entertainment API over DTLS)  |
| `hypercolor-driver-nanoleaf`| `driver-api` | Nanoleaf panels driver (HTTP pairing + UDP control)      |
| `hypercolor-driver-wled`    | `driver-api` | WLED driver (DDP / E1.31 sACN)                           |
| `hypercolor-driver-govee`   | `driver-api` | Govee driver (LAN UDP + Govee Cloud API)                 |
| `hypercolor-network`        | `driver-api` | Network driver registry and orchestration                |

### Daemon + API

| Crate                    | Depends On                                                            | Responsibility                                               |
| ------------------------ | --------------------------------------------------------------------- | ------------------------------------------------------------ |
| `hypercolor-daemon`      | `types`, `core`, `driver-api`, `network`, `leptos-ext`; optionally `driver-builtin` (hal + drivers), `cloud-client` | Daemon binary: render-loop host, REST/WebSocket/MCP server |
| `hypercolor-cloud-api`   | `types`                                                               | Shared data-contract types for the cloud HTTP API            |
| `hypercolor-cloud-client`| `types`, `cloud-api`                                                  | Daemon-side cloud client (OAuth, keyring, identity, sync)    |
| `hypercolor-daemon-link` | `types`                                                               | Daemon↔cloud multiplexed WebSocket tunnel protocol           |

### Clients and UIs

| Crate                        | Depends On       | Responsibility                                                                      |
| ---------------------------- | ---------------- | ----------------------------------------------------------------------------------- |
| `hypercolor-cli`             | `core`           | The `hypercolor` CLI binary — parsing, output formatting, IPC client                |
| `hypercolor-tui`             | `types`          | Ratatui terminal UI library, launched via `hypercolor tui`                          |
| `hypercolor-tray`            | `types`, `core`  | System tray applet binary                                                           |
| `hypercolor-desktop`         | (standalone)     | Tauri 2 native shell — excluded from default CI                                     |
| `hypercolor-app`             | `types`, `core`  | Unified desktop app shell: supervises daemon, owns tray, handles autostart          |
| `hypercolor-leptos-ext`      | (standalone)     | Leptos 0.8 extension helpers for the web UI                                         |
| `hypercolor-leptos-ext-macros`| (standalone)    | Proc macros powering `hypercolor-leptos-ext`                                        |
| `hypercolor-ui`              | (standalone)     | Leptos 0.8 CSR web app, compiled to WASM via Trunk — excluded from the workspace   |

{% callout(type="note", title="Unsafe boundary policy") %}
Application, driver, and domain crates inherit the workspace `unsafe_code = "forbid"` lint.
The only current opt-outs are `hypercolor-linux-gpu-interop` for Linux GPU surface import
and `hypercolor-windows-pawnio` for Windows service/process interop. Those crates isolate
raw platform calls and deny undocumented unsafe blocks.
{% end %}

{% callout(type="warning", title="UI crate exclusion") %}
`hypercolor-ui` is excluded from the Cargo workspace because it targets `wasm32-unknown-unknown`. Running `cargo check --workspace` does NOT cover it. Build the UI separately with `just ui-dev` or `cd crates/hypercolor-ui && trunk build`.
{% end %}

## Render Pipeline

The render thread is the heart of the system. It runs on a dedicated OS thread with adaptive FPS (10–60fps across 5 tiers) and drives the pipeline one frame at a time:

1. **Sample inputs** — Collect audio FFT data, screen capture, MIDI events
2. **Render producers** — The active effect (HTML via Servo or native via wgpu), screen source, and any other producers publish their newest RGBA surface to the queue
3. **Compose with SparkleFlinger** — The render-thread compositor latches one surface per producer at the frame boundary and blends them into a single canonical frame. Blend modes are `Replace`, `Alpha`, `Add`, and `Screen`, with math in premultiplied linear-light sRGB. A single full-opacity layer takes the bypass fast path — the source surface passes through untouched, with no per-pixel work
4. **Spatial mapping** — Sample the composed frame at each LED's physical position using bilinear interpolation (or area averaging / Gaussian, configurable per zone)
5. **Push to devices** — Send per-zone color arrays to hardware backends via their protocol encoders
6. **Publish state** — Broadcast the frame data and canvas preview on the event bus for UI subscribers

SparkleFlinger is the composition boundary that lets render groups, overlapping zones, and mixed-cadence producers (Servo at 30fps, native at 60fps, screen capture at whatever PipeWire hands us) all flow into a single deadline-driven frame. See `docs/design/30-sparkleflinger-implementation.md` for the shipped invariants.

The canvas defaults to 640×480 and is configurable via `daemon.canvas_width` / `daemon.canvas_height`. Effects render in normalized `[0.0, 1.0]` spatial coordinates, so they stay resolution-independent — tune the canvas to match your sampling needs without touching effect code. Readback cost scales with the canvas (≈1.17 MB/frame at 640×480, still trivially fast). Canvas dimensions retune through the scene transaction path at frame boundaries; target FPS can be retuned live too.

## Dual-Path Effect Engine

Hypercolor supports two rendering paths:

**Servo Path (HTML/Canvas/WebGL)** — The primary authoring path. Uses an embedded Servo browser engine to render HTML effects headlessly. This provides full Canvas 2D and WebGL support, letting effect authors use the entire web platform. The `@hypercolor/sdk` compiles effects to self-contained HTML files that Servo loads and renders.

**wgpu Path (Native Shaders)** — For maximum performance. WGSL compute/render pipelines that bypass the browser engine entirely. Used for effects that need every last drop of GPU throughput.

Both paths produce the same output: an RGBA pixel buffer that feeds into the spatial sampler.

## Event Bus

All frontends subscribe to the same event stream. Two channel types serve different semantics:

**`broadcast::Sender<HypercolorEvent>`** — Every subscriber sees every event. Used for device connect/disconnect, profile changes, errors — events where history matters.

**`watch::Sender<FrameData>`** — Only the latest value matters. Subscribers skip stale frames. Used for LED color data and audio spectrum — where freshness matters more than completeness.

```rust
pub enum HypercolorEvent {
    DeviceConnected(DeviceInfo),
    DeviceDisconnected(String),
    EffectChanged(String),
    ProfileLoaded(String),
    InputSourceAdded(String),
    Error(String),
}
```

The daemon runs the core engine. The TUI and CLI connect via HTTP. The web frontend connects via WebSocket. All receive the same events.

## Spatial Layout Engine

The spatial engine bridges the gap between the 2D effect canvas and physical LED positions in 3D space.

Each device zone defines:

- **Position and size** on the canvas (normalized 0-1 coordinates)
- **LED topology** — strip, matrix, ring, or custom positions
- **Rotation** — Allows angled placement
- **LED positions** — Individual LED coordinates within the zone

The sampler uses bilinear interpolation at each LED's canvas position to produce smooth color output even when LEDs are sparse relative to the canvas resolution.

## Key Design Decisions

| Decision          | Choice                    | Rationale                                                                                                                                                         |
| ----------------- | ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Language          | Rust                      | Performance (60fps render thread), safety (USB HID), ecosystem (wgpu, Servo, Ratatui)                                                                             |
| Effect renderer   | wgpu + Servo dual path    | Native performance for new effects + compatibility with existing HTML effects                                                                                     |
| Frame composition | SparkleFlinger compositor | Decouples producer cadence from frame deadlines; enables render groups, overlapping zones, and mixed-rate sources without coupling composition to the render loop |
| Web UI            | Leptos 0.8 (WASM)         | Type-safe, fine-grained reactivity, single Rust ecosystem                                                                                                         |
| Web server        | Axum                      | tokio-native, first-class WebSocket, serves embedded SPA                                                                                                          |
| TUI               | Ratatui                   | Established ecosystem, true-color LED preview                                                                                                                     |
| Audio             | cpal + custom FFT         | Cross-platform capture, low-latency processing                                                                                                                    |
| IPC               | tokio broadcast/watch     | Multi-consumer events + latest-value state for real-time data                                                                                                     |
| Config            | TOML                      | Rust ecosystem standard, human-readable                                                                                                                           |
| Wire format       | zerocopy structs          | Zero-allocation frame encoding at 60fps                                                                                                                           |
| Canvas resolution | 640×480 (configurable)    | Resolution-independent effects render in normalized coords; tune via `daemon.canvas_width` / `canvas_height`                                                      |
| License           | Apache-2.0                | Permissive open source                                                                                                                                            |
