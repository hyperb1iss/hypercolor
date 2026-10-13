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
- Device input: driver-owned hardware publishes touch and button events into the daemon with device attribution, starting with ROLI Blocks over `blocksd` (Spec 82 phase 1)
- Screen capture: Desktop Duplication on Windows (GPU path, on by default), Wayland portal + PipeWire on Linux (opt-in), ScreenCaptureKit on macOS (opt-in), with byte-admission capacity control
- Hardware: 184 supported devices across 12 driver families (Razer, Corsair, ASUS, Lian Li, Nollie, PrismRGB, QMK, Ableton Push 2, Hue, Nanoleaf, WLED, Govee), plus a thirteenth family, Dygma, whose driver ships but stays dark until firmware allows it, and two opt-in bridges: the OpenRGB SDK bridge for anything OpenRGB drives and the `blocksd` bridge for ROLI Blocks
- Portable device identity: devices survive cable moves, IP churn, and BIOS renumbering; layouts rebind after hardware swaps
- Web UI: effects browser, live canvas preview, Studio multi-zone workspace, viewport designer, spatial layout editor, scene management, mobile-responsive shell
- Terminal UI (TUI): true-color LED preview, audio spectrum, fullscreen mode
- Audio pipeline: FFT, beat detection, mel bands, chromagram; cpal on every platform with a native PulseAudio/PipeWire monitor path on Linux
- REST API + WebSocket (binary preview transport v2) + MCP server (21 tools, 5 resources, 4 prompts) on `:9420`
- CLI (`hypercolor`) with shell completions
- Installers everywhere: Linux tarball/`.deb`/AUR/Homebrew formula, managed Linux installs with probation and rollback, a Nix flake with a NixOS module, per-machine Windows NSIS with PawnIO hardware setup that upgrades in place, signed and notarized macOS DMGs (Apple silicon) + Homebrew cask, and headless Docker images for amd64 and arm64, all published automatically on tag
- Per-PR CI lanes for Linux, Windows, and macOS (Apple silicon runners), plus a Windows DX12 WARP lane that runs the SparkleFlinger GPU tests
- Python client on PyPI as `hypercolor` (sdist + wheel, trusted publishing)
- Home Assistant integration through [hypercolor-hass](https://github.com/hyperb1iss/hypercolor-hass) (installable from HACS) with the [hyper-light-card](https://github.com/hyperb1iss/hyper-light-card) Lovelace card
- Virtual display simulator for developing effects and faces without physical hardware
- Scene engine with Oklab cross-fades and priority stacking
- Session and power integration: D-Bus on Linux (logind, screensaver) and native session/system-power monitors on Windows and macOS

### Recent Releases

- **0.6.3**: device input from ROLI Blocks, drivers for the Razer Kraken Ultimate and the Lian Li Universal Screen 8.8" LCD, daemon startup progress on `/health` and in the desktop app, and Windows installs that upgrade in place
- **0.6.2**: fixes the Windows startup stall that broke fresh 0.6.1 installs, ships signed and notarized macOS builds for Apple silicon only, and renames the CLI package to `hypercolor` ahead of a crates.io publish
- **0.6.1**: ships the VC++ runtime beside every Windows binary, adds the headless Docker runtime, and drives the Servo worker from wakeups instead of polling

See the [changelog](CHANGELOG.md) for the full lists.

---

## ⚡ Near-Term

Things actively in progress or next in the queue.

### 0.7.0: Safe on the Network, Honest About What's Wrong

The [0.7.0 milestone](https://github.com/hyperb1iss/hypercolor/milestone/1) finishes what makes the LAN-mode change safe, closes the diagnostics loop, and lets device input reach effects.

- **Host header validation**: check the `Host` header on API requests, so a web page that points its own hostname at your machine (DNS rebinding) cannot drive the loopback API.
- **Reverse-proxy locality**: a request that a reverse proxy on the same machine relays from the network is never treated as a loopback one.
- **Windows SMBus broker start** ([#396](https://github.com/hyperb1iss/hypercolor/issues/396)): find out why the `HypercolorSmBus` broker installs but never starts, fix it, and make a stopped broker visible in the installer and in `hypercolor diagnose`.
- **Config persistence honesty** ([#397](https://github.com/hyperb1iss/hypercolor/issues/397)): a config change that never reaches disk says so instead of reporting success, and `hypercolor diagnose` flags a config file that lags the live config.
- **Device input reaches effects** ([#398](https://github.com/hyperb1iss/hypercolor/issues/398), Spec 82 phase 2): touches and device buttons flow through the LightScript payload and the SDK, touches get canvas coordinates from the active layout, and a touch-ripple effect proves the path on a Lightpad.
- **Memory growth investigation** ([#399](https://github.com/hyperb1iss/hypercolor/issues/399)): reproduce the daemon's growth over long runs, name the allocation site, and fix it or show that a release candidate holds steady through a long soak.

### After 0.7.0

- **Mobile UI**: [Spec 75](docs/specs/75-mobile-responsive-ui.md) waves 2 to 4 (per-page audits, touch polish, PWA affordances), plus the real-device pass that wave 1 still owes.
- **First crates.io publish**: the workspace has been publishable since 0.6.2 and the CLI package is already named `hypercolor`, but nothing is on crates.io yet.
- **Faces packaging**: the face packaging format from [Spec 69](docs/specs/69-world-class-faces.md) wave 5, with a versioned manifest contract and a `hypercolor face install` verb that validates faces on install.
- **Servo resilience**: run HTML rendering in supervised worker processes so a runaway script or a Servo crash degrades only its own sessions while the daemon keeps rendering ([design 70](docs/design/70-servo-subprocess-isolation.md)).
- **Windows code signing**: Authenticode for the installer and binaries, so first launches stop tripping SmartScreen. macOS builds already ship signed and notarized.
- **macOS hardening**: the open waves of [Spec 77](docs/specs/77-macos-capture-security-and-release-hardening.md), covering capture lifecycle and resource ownership, GPU-only recovery, capture telemetry, release provenance, and module decomposition.
- **New hardware families**: see below.

### More Hardware

The biggest gap is still hardware coverage. Prioritized work:

- **NZXT**: spec written, covers seven distinct protocol families (Smart Device 3, HUE 3, Kraken fans/LCD, lighting controller variants). Needs USB captures for final gaps.
- **Cooler Master**: large researched catalog (20 devices), needs a clean-room driver.
- **Logitech**: 9 known devices, USB analysis work underway.
- **Aqua Computer**: HID status reports and fixed-channel fan RGB, spec complete.
- **Wooting**: analog keyboards with per-key RGB, spec in progress.
- **Roccat**: 14 known devices, protocol under review.
- **Remaining Razer SKUs**: 38 researched devices, most share existing protocol variants.

If you own hardware in the `researched` column of the [compatibility matrix](docs/content/hardware/compatibility.md), your USB captures and testing are the fastest path to a working driver.

### SDK + Effect Ecosystem

- `hypercolor install <effect>`: install community effects from the CLI
- Wasmtime-based plugin system for community-authored backends, enabling drivers without a daemon rebuild

### Platform

- **Windows**: deeper session/power integration, and compiling the Servo lane in the per-PR Windows CI job (it currently builds only in the tag-gated release job)

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

- **ROLI Blocks**: the `DeviceBackend` bridges to `blocksd` over a Unix socket. Capability-aware discovery exposes Lightpad 15×15 grids and LUMI 24-key strips, with separate binary and key-frame transports. ROLI devices are not counted in the HAL compatibility database. Touch and button input reaches the daemon as of 0.6.3, and Spec 82 phase 2, planned for 0.7.0, carries it to effects. Full scene-compositor and musical-input acceptance remain separate hardware checks.
- **X11 screen capture**: Linux capture is Wayland-portal-only today; an XShm path would cover legacy sessions.
- **SMBus / I2C on more silicon**: motherboard and DRAM RGB beyond ASUS Aura (MSI Mystic Light and friends). The Windows PawnIO path and the Linux i2c-dev path both exist; each new controller family needs a hardened probe model.
- **Wired / wireless headset RGB**: the Razer Kraken Ultimate is supported as of 0.6.3. Most other headset protocols are closed, and community reverse-engineering is the prerequisite.

---

Corrections, additions, or contributions welcome on the [issue tracker](https://github.com/hyperb1iss/hypercolor/issues) and [Discussions](https://github.com/hyperb1iss/hypercolor/discussions).
