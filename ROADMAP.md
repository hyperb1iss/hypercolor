# 🌊 Hypercolor Roadmap

This document describes where Hypercolor is headed. It is **indicative and non-binding** — priorities shift based on contributor interest, hardware availability, and what the community needs most. No dates are committed here.

If you want to help with anything on this list, check [CONTRIBUTING.md](CONTRIBUTING.md) and open an issue or PR.

---

## 💎 v0.1 — Shipped

The foundation is working on Linux today.

- Core render pipeline: SparkleFlinger compositor, spatial sampler, adaptive FPS (10–60)
- Effect system: Servo HTML renderer, wgpu native shaders, 44 built-in effects
- Effect SDK: TypeScript + GLSL authoring, hot-reload dev server, bundled HTML output
- Hardware: 175 supported devices across 11 driver families (Razer, Corsair, ASUS, Lian Li, Nollie, QMK, Ableton Push 2, Hue, Nanoleaf, WLED, Govee, and more)
- Web UI: effects browser, live canvas preview, spatial layout editor, scene management
- Terminal UI (TUI): true-color LED preview, audio spectrum, fullscreen mode
- Audio pipeline: FFT, beat detection, mel bands, chromagram
- REST API + WebSocket + MCP server on `:9420`
- CLI (`hypercolor`) with shell completions
- Screen capture input for ambient backlighting
- Linux session integration via D-Bus (logind, screensaver)
- Scene engine with Oklab cross-fades and priority stacking

---

## ⚡ Near-Term

Things actively in progress or next in the queue.

### More Hardware

The biggest gap in v0.1 is hardware coverage. Prioritized work:

- **NZXT** — spec written, covers seven distinct protocol families (Smart Device 3, HUE 3, Kraken fans/LCD, lighting controller variants). Needs USB captures for final gaps.
- **Cooler Master** — large researched catalog (20 devices), needs a clean-room driver.
- **Logitech** — 9 researched devices, USB analysis work underway.
- **Aqua Computer** — HID status reports and fixed-channel fan RGB, spec complete.
- **Wooting** — analog keyboards with per-key RGB, spec in progress.
- **Roccat** — 14 researched devices, protocol under review.
- **Remaining Razer SKUs** — 38 researched devices, most share existing protocol variants.

If you own hardware in the `researched` column of the [compatibility matrix](docs/content/hardware/compatibility.md), your USB captures and testing are the fastest path to a working driver.

### SDK + Effect Ecosystem

- Publish `@hypercolor/sdk` and `@hypercolor/create-effect` to npm so authoring works without a local checkout
- `hypercolor install <effect>` — install community effects from the CLI
- Wasmtime-based plugin system for community-authored backends, enabling drivers without a daemon rebuild
- Face SDK for LCD display panels: TypeScript gauge components, sensor formatters, and the `face()` declarative API mirroring the existing `effect()` pattern

### Platform

- **macOS**: arm64 builds exist today; CI gates and the installer need to match Linux
- **Windows**: USB peripheral support and the PawnIO SMBus path are implemented; completing audio-reactive input, session integration, and an MSI installer is the remaining work

### Python Ecosystem

- `hypercolor-python` async client published to PyPI (source-only at launch)
- Home Assistant integration (`hypercolor-homeassistant`) and a Lovelace card

---

## 🔮 Later

Larger features that are designed but not yet in active implementation.

### GPU Render Pipeline

Servo effects currently read back through CPU (`glReadPixels`). The Linux GPU surface interop work (spec written, opt-in soak passed) imports Servo's GL framebuffer into the wgpu texture pipeline directly, eliminating the readback. macOS and Windows equivalents are specced; Linux default-on is deferred pending broader soak.

### Effect Marketplace

A community effect gallery — browse, install, and share effects without leaving the UI. The infrastructure depends on the Wasmtime plugin system and npm publication being stable first.

### Cloud Sync

Profile, scene, and layout sync across machines. The architecture (cloud-api contract types, OAuth client, daemon-link WebSocket tunnel) is designed and partially stubbed in the codebase. No deployment timeline.

### Interactive Viewport Designer

A live 2D canvas editor where you draw zones, masks, and spatial regions and see them mapped to LEDs in real time. The spec exists; it builds on the existing spatial engine.

### Virtual Display Simulator

A headless device simulator for developing and testing drivers and effects without physical hardware. Spec written.

---

## 🧪 Exploratory / Under Research

Things we want but haven't committed to a shape for yet.

- **ROLI Blocks** — expressive pressure/tilt MIDI instruments with LED output
- **GPU spatial sampling** — sample LED positions directly from a GPU texture at render time, eliminating the CPU sampling stage for wgpu effects
- **SMBus / I2C** — motherboard and DRAM RGB (ASUS AURA, MSI Mystic Light) on Linux; the Windows PawnIO path exists but the Linux hwmon/i2c-dev path needs a hardened driver model
- **Wired / wireless headset RGB** — most headset protocols are closed; community reverse-engineering is the prerequisite

---

Corrections, additions, or contributions welcome on the [issue tracker](https://github.com/hyperb1iss/hypercolor/issues) and [Discussions](https://github.com/hyperb1iss/hypercolor/discussions).
