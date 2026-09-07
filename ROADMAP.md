# 🌊 Hypercolor Roadmap

This document describes where Hypercolor is headed. It is **indicative and non-binding**: priorities shift based on contributor interest, hardware availability, and what the community needs most. No dates are committed here.

If you want to help with anything on this list, check [CONTRIBUTING.md](CONTRIBUTING.md) and open an issue or PR.

---

## 💎 Shipped

The foundation works on Linux, Windows, and macOS today.

- Core render pipeline: SparkleFlinger compositor with CPU and GPU lanes, spatial sampler, adaptive FPS (10-60)
- Effect system: Servo HTML renderer with GPU framebuffer import on all three platforms, 57 built-in effects (46 SDK, 11 native Rust), curated cover art and presets
- Effect SDK: TypeScript + GLSL authoring, watch-rebuild workflow, bundled HTML output, published to npm as `hypercolor` with the `create-hypercolor` scaffolder
- Face SDK: 7 display faces for LCD panels (clocks, sensors, now-playing, spectrum) with the `face()` declarative API
- Interactive input pipeline: consent-gated keyboard/mouse capture (evdev on Linux, Raw Input on Windows, CGEventTap on macOS) driving interactive effects like Keystrike
- Screen capture: Desktop Duplication on Windows (GPU path, on by default), Wayland portal + PipeWire on Linux (opt-in), ScreenCaptureKit on macOS (opt-in), with byte-admission capacity control
- Hardware: 179 supported devices across 12 driver families (Razer, Corsair, ASUS, Lian Li, Nollie, PrismRGB, QMK, Ableton Push 2, Hue, Nanoleaf, WLED, Govee), plus a thirteenth family, Dygma, whose driver ships but stays dark until firmware allows it, and two opt-in bridges: the OpenRGB SDK bridge for anything OpenRGB drives and the `blocksd` bridge for ROLI Blocks
- Portable device identity: devices survive cable moves, IP churn, and BIOS renumbering; layouts rebind after hardware swaps
- Web UI: effects browser, live canvas preview, Studio multi-zone workspace, viewport designer, spatial layout editor, scene management, mobile-responsive shell
- Terminal UI (TUI): true-color LED preview, audio spectrum, fullscreen mode
- Audio pipeline: FFT, beat detection, mel bands, chromagram; cpal on every platform with a native PulseAudio/PipeWire monitor path on Linux
- REST API + WebSocket (binary preview transport v2) + MCP server (17 tools, 5 resources, 3 prompts) on `:9420`
- CLI (`hypercolor`) with shell completions
- Installers everywhere: Linux tarball/`.deb`/AUR/Homebrew formula, per-machine Windows NSIS with PawnIO hardware setup, macOS DMGs (both architectures) + Homebrew cask, all published automatically on tag
- Per-PR CI lanes for Linux, Windows, and macOS (both Apple Silicon and Intel runners)
- Python client on PyPI as `hypercolor` (sdist + wheel, trusted publishing)
- Virtual display simulator for developing effects and faces without physical hardware
- Scene engine with Oklab cross-fades and priority stacking
- Session and power integration: D-Bus on Linux (logind, screensaver) and native session/system-power monitors on macOS

---

## ⚡ Near-Term

Things actively in progress or next in the queue.

### More Hardware

The biggest gap is still hardware coverage. Prioritized work:

- **NZXT**: spec written, covers seven distinct protocol families (Smart Device 3, HUE 3, Kraken fans/LCD, lighting controller variants). Needs USB captures for final gaps.
- **Cooler Master**: large researched catalog (20 devices), needs a clean-room driver.
- **Logitech**: 9 researched devices, USB analysis work underway.
- **Aqua Computer**: HID status reports and fixed-channel fan RGB, spec complete.
- **Wooting**: analog keyboards with per-key RGB, spec in progress.
- **Roccat**: 14 researched devices, protocol under review.
- **Remaining Razer SKUs**: 38 researched devices, most share existing protocol variants.

If you own hardware in the `researched` column of the [compatibility matrix](docs/content/hardware/compatibility.md), your USB captures and testing are the fastest path to a working driver.

### SDK + Effect Ecosystem

- `hypercolor install <effect>`: install community effects from the CLI
- Wasmtime-based plugin system for community-authored backends, enabling drivers without a daemon rebuild

### Platform

- **Windows**: deeper session/power integration, and compiling the Servo lane in the per-PR Windows CI job (it currently builds only in the tag-gated release job)
- **Code signing**: Windows Authenticode and macOS Developer ID + notarization, once signing credentials exist; until then first launches hit SmartScreen and Gatekeeper speed bumps

### Python Ecosystem

- Home Assistant integration (`hypercolor-homeassistant`) and a Lovelace card

---

## 🔮 Later

Larger features that are designed but not yet in active implementation.

### Native GPU Shader Effects

The compositor and area sampling already run on the GPU, and Servo frames import as GPU textures on all three platforms. The remaining lane is a native wgpu shader-effect renderer: effects authored as WGSL that render without a browser in the loop. The `EffectRenderer` trait boundary was designed for it.

### Effect Marketplace

A community effect gallery: browse, install, and share effects without leaving the UI. The infrastructure depends on the Wasmtime plugin system being stable first.

---

## 🧪 Exploratory / Under Research

Things we want but haven't committed to a shape for yet.

- **ROLI Blocks**: expressive pressure/tilt MIDI instruments with LED output. Support is a `DeviceBackend` that bridges to a separate `blocksd` daemon over a Unix socket, not a HAL protocol encoder, and no ROLI device appears in the compatibility database. End-to-end device support is unproven.
- **X11 screen capture**: Linux capture is Wayland-portal-only today; an XShm path would cover legacy sessions.
- **SMBus / I2C on more silicon**: motherboard and DRAM RGB beyond ASUS Aura (MSI Mystic Light and friends). The Windows PawnIO path and the Linux i2c-dev path both exist; each new controller family needs a hardened probe model.
- **Wired / wireless headset RGB**: most headset protocols are closed; community reverse-engineering is the prerequisite.

---

Corrections, additions, or contributions welcome on the [issue tracker](https://github.com/hyperb1iss/hypercolor/issues) and [Discussions](https://github.com/hyperb1iss/hypercolor/discussions).
