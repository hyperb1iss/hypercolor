# Changelog

All notable changes to Hypercolor will be documented here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-07-15

First public release of Hypercolor, a cross-platform RGB LED orchestration daemon with a GPU-accelerated render pipeline, multi-vendor hardware support, and a full effect authoring SDK.

### Added

- ✨ Scaffold the entire **Cargo workspace** with 25+ crates: `hypercolor-core`, `hypercolor-daemon`, `hypercolor-types`, `hypercolor-hal`, `hypercolor-cli`, `hypercolor-tui`, `hypercolor-ui`, `hypercolor-app`, `hypercolor-tray`, `hypercolor-driver-api`, and platform GPU interop crates (dde4391, 89dbf22)
- ✨ Implement the **Sparkleflinger render pipeline** with CPU and GPU (wgpu) compositor backends, scene transition crossfades, render group isolation, deferred GPU zone sampling, and admission-controlled frame pacing (06eba59, abbc6b9, c5f09097)
- ✨ Add **multi-zone scene system** with per-zone effect assignment, layer stacks, blend modes, media layers (image, animated WebP, Lottie, video, stream URLs), and snapshot mutation guards (0c8d7ae, 59587d4, afc80d7)
- ✨ Add **Servo (embedded browser) HTML effect renderer** with LightScript runtime, GPU import pipelines for Linux (Vulkan/GL), macOS (IOSurface), and Windows (ANGLE/D3D11), multi-session support, and circuit breaker fault isolation (001cea9, 4883d3e, c7f0603, 548fd71)
- ✨ Ship **33+ SDK effects** across canvas, WebGL, and GLSL renderers with the `@hypercolor/create-hypercolor` scaffolding CLI, declarative `effect()` and `canvas()` APIs, preset templates, control groups, and curated cosine palettes (011c94c, 6476e42, 670c548)
- ✨ Add **7 display faces** (Neon Clock, Pulse Temp, Sensor Grid, SilkCircuit HUD, Now Playing, Spectrum, System Pulse) with the Face SDK, descriptor-aware layouts, hermetic vendored fonts, and atmosphere effects (c1600c2, e92355, 583ee5c)
- ✨ Implement **hardware drivers** for Razer (USB HID, scroll wheel, Seiren V3, Blade laptops), Corsair (Lighting Node, iCUE LINK, LCD, Bragi peripherals), ASUS Aura (USB + SMBus/DRAM), Lian Li Uni Hub (ENE/TL/legacy), Dygma (Focus serial), PrismRGB, QMK (OpenRGB protocol), Ableton Push 2 (MIDI + display), ROLI Blocks, and Nollie (Gen1/Gen2/NOS2/Stream65/Legacy) (57c294d, a17c350, 327783e, 0564b64, 935e73d)
- ✨ Add **network device drivers** as isolated crates: WLED (DDP/E131, RGBW, fuzzy dedup), Philips Hue (DTLS entertainment streaming, bridge pairing), Nanoleaf (UDP streaming, topology refresh), Govee (LAN multicast, cloud v1 API key pairing, rate limiting), and OpenRGB fallback bridge (fdedbfc, d13bcb5, faa7ed7, c1917512, 4c36331)
- ✨ Add the **unified driver module API** (`hypercolor-driver-api`) with extensible config registry, dynamic control surfaces, typed actions with confirmation prompts, device pairing flows, presentation metadata, and protocol catalog capabilities (45edd5c, 3eea2da, caa9f74)
- ✨ Build the **Leptos 0.8 CSR web UI** with Luminary (SilkCircuit) design system, Studio composition page with multi-zone tree, layout editor with undo/redo, display face management, device pairing modal, effect controls, preset library, viewport designer, media gallery, WebGL/WebSocket preview, and WebSocket auto-reconnect with exponential backoff (5d2f5b5, c6f2c2b, 996bcec, 901bc70)
- ✨ Add the **Ratatui TUI** with 60fps rendering, Kitty/Sixel/halfblocks live preview, motion effects (border pulse, ambient bleed, breathing, crossfade), HSL color picker, spectrum-reactive borders, resizable split panels, mouse interaction, and multi-zone/scene support (a21226, c023ee0, 2401f09)
- ✨ Add the **hypercolor CLI** with SilkCircuit-themed help, connection profiles, `hyper status` visual output, dynamic driver/device control commands, service management, completions, and TUI as a subcommand (8959a20, 75c61d8, 709deb5, 2b04338)
- ✨ Add the **Tauri desktop app** (`hypercolor-app`) with supervised daemon lifecycle, system tray with brightness presets and scene status, rolling file logging, first-run welcome overlay, pause on window hide, and native installers for Linux/macOS/Windows (7e39e5e, c28cfaa, 69e2628)
- ✨ Add **Windows platform support** with PawnIO SMBus transport and broker service, per-machine NSIS installer with hardware setup, Windows service mode, ANGLE GPU import, ACPI/NVML sensors, sleep/resume rediscovery, and elevated helper for SMBus repair (cbf226a, 65c685, 69e2628, 5310d86)
- ✨ Add the **Python client** (`hypercolor` on PyPI) with async/sync clients generated from OpenAPI, WebSocket protocol helpers, scene/zone surface with If-Match concurrency, and Home Assistant integration helpers (d2b7b06, 47a8b8d, 3007f1f)
- ✨ Add **audio reactive pipeline** with PulseAudio native capture, FFT spectrum analysis, beat detection, transient gating, motion-driven smoothing, and live input switching (0936bca, c2b2168, bf9ed0f)
- ✨ Add **Wayland screen capture** (PipeWire portal), live crop editor, ambilight edge-projection effect, and color tuning pipeline (d625b55, 0d1fdde, 85c9e31)
- ✨ Add **asset library** for user media with drag-and-drop upload, Lottie/WebP/video/stream URL support, and scene media admission caps (a8228d1, d1faf87, 4281b6f)
- ✨ Add **mDNS network discovery**, multi-server support, and per-device brightness control with direct-control locks (b3bcb43, 5ad58de)
- ✨ Add **session and power awareness** via systemd-logind, screensaver monitoring, configurable off-output behavior, and Windows sleep/resume (696b15e, e45f3d3, 4eed5e4)
- ✨ Add **MCP server** (Model Context Protocol) with tool handlers for effects, devices, scenes, displays, and structured output over HTTP (bde7c5e, b937e38, 6598a9b)
- ✨ Add **REST API** with OpenAPI spec generation, auth tiers, rate limiting, CORS, access log middleware, WebSocket binary frame channels, and JPEG preview endpoints (26fc6a5, 1458aaf, 59914f9)
- ✨ Add **CI/CD pipeline** with Rust/Servo/WASM/e2e lanes, Playwright harness, Criterion benchmarks, GitHub Actions release workflow with `.deb`/AUR/Homebrew/NSIS artifacts, and trusted npm publishing (adf39a9, a3b289d, 5cbc1b2)
- ✨ Add **documentation site** (Zola) with Luminary theme, 70+ spec documents, effect authoring guides, hardware compatibility database, and public roadmap (49ed277, b638191, 854ec5d)

### Changed

- 🔄 Switch color pipeline to **linear-light interpolation** with sRGB encode/decode, precomputed LUTs, and Oklch gradient blending (5ea5167, c688c37)
- 🔄 Raise default canvas to **640x480** with live FPS retune and adaptive SDK API (a0ecd22)
- 🔄 Replace the legacy `EffectEngine` with **scene-backed render groups** as the single rendering path (4cde65a, 9b8d221)
- 🔄 Migrate all HAL protocol encoders to **zerocopy typed structs** (Razer, Corsair, ASUS, PrismRGB, Blocks) for zero-copy frame encoding (9f61802, 98b556c, 2525f10)
- 🔄 Rename CLI binary to `hypercolor` and daemon to `hypercolor-daemon` (a7e25a5, 2b04338)
- 🔄 Rename SDK npm packages to `hypercolor` and `create-hypercolor` (ea73f10)

### Fixed

- 🐛 Preserve color saturation on RGBW WLED devices by sending RGB-only DDP frames (178a22c, 51e9c57)
- 🐛 Deduplicate devices by scanner fingerprint across rescans (1fdb0bf)
- 🐛 Cap reconnect retries and harden lifecycle wiring to prevent runaway loops (c08afef)
- 🐛 Fix memory leaks in Servo worker lifecycle, bound WS queues, and manage webview cleanup (585689, 701388f)
- 🐛 Stabilize frame pacing with admission-controlled cadence and paced outputs (ccd4321, 30780f0)
- 🐛 Fix reactive flickering in the web UI with Memo gates and signal identity fixes (13144c2)
- 🐛 Prevent canvas flicker from reactive DOM rebuilds (13ab279)
- 🐛 Preserve animation clocks across long uptime with monotonic daemon clocks (d3248d9, 84ffeb9)
- 🐛 Fix USB reconnect stalls and isolate USB device output actors (09b5466, 74392ab)
- 🐛 Harden WLED connection stability, protocol reliability, and endpoint metadata surfacing (1060ffb, 326109c)

### Security

- 🔒 Require auth for network daemon binds and make CORS config auth-bound (d02b4e5, 11f6aab)
- 🔒 Restrict credential file modes on disk (b05ae75)
- 🔒 Enforce media admission caps for MCP scene activation and validate broadcast targets (748e386, 71c051a)
- 🔒 Harden stream URL SSRF validation including IPv6/unspecified bypass (052a238, 804935f)
- 🔒 Validate WebSocket origin before upgrade and cap WS command body sizes (0a7b47b, 732cfe0)
- 🔒 Block cross-site loopback write requests and enforce control auth for preview writes (11423f1, 36372ef)
- 🔒 Bound animated media decode to prevent OOM (592867d, 18e983f)
- 🔒 Reject per-user PawnIO module directories for Windows services and secure service install paths (18df538, 4a8b24b, 9e4341a)
- 🔒 Pin CI actions to immutable commit SHAs (350d47a, b676e31)
- 🔒 Add unified network access modes with loopback-only defaults (ada10c9, e2812223)

### Removed

- 🔥 Remove OpenRGB direct backend in favor of the clean-room OpenRGB SDK bridge driver (0ecdee6, 4c36331)
- 🔥 Remove legacy `EffectEngine`, compatibility aliases, and stale migration paths (4cde65a, 91b0de1, 2c79e01)
- 🔥 Remove the standalone `hypercolor-desktop` crate, superseded by `hypercolor-app` (5af371a)
- 🔥 Remove the marketing website from the repository (83ac651)
- 🔥 Remove display overlay compositor, subsumed into display face composition blending (9c33e0d, 16e8222)

### Metrics

- Total Commits: 1,308
- Files Changed: 2,591
- Insertions: +720,254
- Deletions: -2,397
<!-- -------------------------------------------------------------- -->

## [Unreleased]

### Added

- Launch hardening branch for v0.1.0 release readiness.

## [0.1.0] - Unreleased

### Added

- Linux-first RGB lighting daemon with REST, WebSocket, and MCP control surfaces.
- Servo HTML effect renderer for Canvas, WebGL, and GLSL effects.
- Native wgpu render path and SparkleFlinger frame compositor.
- Web UI, terminal UI, CLI, tray applet, and unified Tauri desktop app.
- TypeScript effect SDK with built-in HTML effect packs.
- Hardware support for 179 devices across 12 driver families.
- Network drivers for Hue, Nanoleaf, and WLED.
- Release tarballs with shell completions, systemd/launchd assets, udev rules,
  bundled UI assets, bundled effects, and checksum verification.

### Security

- Fail-closed daemon startup for unauthenticated non-loopback control binds.
- Credential store seed and encrypted payload permission hardening.
- Documented unsafe-code boundary for audited platform interop crates.

### Notes

- Linux is the supported launch runtime and install path.
- macOS and Windows artifacts are experimental until their installer and runtime
  gates match Linux.
- SDK packages and the Python client are source-only until their package
  registries are published.
