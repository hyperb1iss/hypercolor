+++
title = "Architecture"
description = "System design, crate structure, and key technical decisions"
sort_by = "weight"
template = "section.html"
+++

Hypercolor is a daemon-first lighting engine. A background service runs the render loop, manages device connections, and exposes control interfaces. Clients (web UI, TUI, CLI, AI assistants) connect to the daemon — they never talk to hardware directly.

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
        C[Spatial Sampler<br/>320x200 canvas]
        D[Event Bus<br/>tokio broadcast/watch]
        E[Device Registry]
    end

    subgraph Device Backends
        F1[Razer USB HID]
        F2[PrismRGB USB HID]
        F3[WLED UDP DDP]
        F4[ASUS USB/I2C]
        F5[Push 2 USB Bulk]
    end

    subgraph Client Interfaces
        G1[Web UI<br/>Leptos WASM]
        G2[TUI<br/>Ratatui]
        G3[CLI<br/>hyper]
        G4[MCP Server<br/>AI Assistants]
    end

    A1 --> B1
    A1 --> B2
    A2 --> B1
    A3 --> B1

    B1 -->|RGBA pixels| C
    B2 -->|RGBA pixels| C
    C -->|per-zone colors| E
    E --> F1
    E --> F2
    E --> F3
    E --> F4
    E --> F5

    D <--> G1
    D <--> G2
    D <--> G3
    D <--> G4

    C -.->|frame data| D
    E -.->|device events| D
{% end %}

## Crate Structure

The project is organized into focused crates with strict dependency boundaries:

```
hypercolor-types     Pure data types — zero deps, no logic, no I/O
    |
hypercolor-core      Engine: traits, bus, sampler, config, render loop
    |
hypercolor-hal       Hardware abstraction — USB/HID drivers
    |
hypercolor-daemon    Binary: daemon + REST API + WebSocket + MCP
    |
    +-- hypercolor-cli   Binary: `hyper` CLI tool
    +-- hypercolor-tui   Binary: terminal UI (Ratatui)
    +-- hypercolor-ui    Leptos WASM web UI (separate from workspace)
```

| Crate | Depends On | Responsibility |
|---|---|---|
| `hypercolor-types` | (none) | Shared vocabulary types — import from here, never sibling internals |
| `hypercolor-core` | `types` | Traits, engine logic, effect registry, audio pipeline, spatial mapping |
| `hypercolor-hal` | `types`, `core` | USB/HID device drivers, protocol implementations |
| `hypercolor-daemon` | `core`, `hal` | HTTP/WS server, REST API, MCP server, daemon lifecycle |
| `hypercolor-cli` | `core` | CLI parsing, output formatting, IPC client |
| `hypercolor-tui` | `core` | Terminal UI with LED preview and spectrum visualizer |
| `hypercolor-ui` | (standalone) | Leptos 0.8 CSR web app, compiled to WASM via Trunk |

{% callout(type="warning", title="UI crate exclusion") %}
`hypercolor-ui` is excluded from the Cargo workspace because it targets `wasm32-unknown-unknown`. Running `cargo check --workspace` does NOT cover it. Build the UI separately with `just ui-dev` or `cd crates/hypercolor-ui && trunk build`.
{% end %}

## Render Pipeline

The render loop is the heart of the system. It runs at 60fps on the daemon's async runtime:

1. **Sample inputs** — Collect audio FFT data, screen capture, MIDI events
2. **Render effect** — Execute the active effect (HTML via Servo or native via wgpu) to produce an RGBA canvas buffer (320x200)
3. **Spatial mapping** — Sample the canvas at each LED's physical position using bilinear interpolation
4. **Push to devices** — Send per-zone color arrays to hardware backends via their protocol encoders
5. **Publish state** — Broadcast the frame data on the event bus for UI preview

The 320x200 canvas resolution is intentional. It matches the spatial mapping granularity, keeps pixel readback fast (256KB/frame), and is the standard used by the HTML effect ecosystem.

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

| Decision | Choice | Rationale |
|---|---|---|
| Language | Rust | Performance (60fps render loop), safety (USB HID), ecosystem (wgpu, Servo, Ratatui) |
| Effect renderer | wgpu + Servo dual path | Native performance for new effects + compatibility with existing HTML effects |
| Web UI | Leptos 0.8 (WASM) | Type-safe, fine-grained reactivity, single Rust ecosystem |
| Web server | Axum | tokio-native, first-class WebSocket, serves embedded SPA |
| TUI | Ratatui | Established ecosystem, true-color LED preview |
| Audio | cpal + custom FFT | Cross-platform capture, low-latency processing |
| IPC | tokio broadcast/watch | Multi-consumer events + latest-value state for real-time data |
| Config | TOML | Rust ecosystem standard, human-readable |
| Wire format | zerocopy structs | Zero-allocation frame encoding at 60fps |
| Canvas resolution | 320x200 | Matches LED spatial mapping granularity, fast readback |
| License | Apache-2.0 | Permissive open source |
