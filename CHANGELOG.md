# Changelog

All notable changes to Hypercolor will be documented here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-07-14

First public release of Hypercolor, a Linux-first open-source RGB lighting engine. A single daemon drives every RGB device from one render pipeline at up to 60fps (default 30fps with adaptive skip). 25 Rust crates cover the full stack: type system, core engine, daemon, HAL, driver API, seven driver crates (WLED, Govee, Hue, Nanoleaf, OpenRGB bridge, plus built-in USB/SMBus), GPU interop for Linux/macOS/Windows, a Leptos web UI, Ratatui TUI, system tray, desktop app shell, CLI, and a TypeScript effect SDK.

### Added

- ✨ Add **SparkleFlinger** render-thread compositor with CPU and wgpu GPU backends, crossfade scene transitions, layer stacks, deferred GPU zone sampling, and adaptive frame admission control
- ✨ Add 45 built-in effects via the `@hypercolor/sdk` TypeScript package: GLSL shaders (Arc Storm, Borealis, Cyber Descent, Cymatics, etc.), Canvas 2D (Digital Rain, Fiberflies, Nyan Dash, etc.), and WebGL (Bubble Garden), each with curated presets and grouped controls
- ✨ Add 7 display faces for LCD devices (Neon Clock, Pulse Temp, Sensor Grid, SilkCircuit HUD, Now Playing, Spectrum, System Pulse) with a face SDK, motion module, gauge library, and Servo-rendered transparent overlays
- ✨ Add **Servo** HTML effect renderer with multi-session support, LightScript audio bridge, GPU import pipelines (Vulkan/dmabuf on Linux, IOSurface on macOS, ANGLE/D3D11 on Windows), and circuit-breaker fault isolation
- ✨ Add hardware abstraction layer (`hypercolor-hal`) with native protocol drivers for Razer (keyboards, mice, laptops, Seiren V3, scroll wheel), Corsair (Lighting Node, iCUE LINK, LCD, Bragi/NXP peripherals), ASUS Aura (USB HID and SMBus/DRAM), Lian Li Uni Hub (ENE/TL/legacy), Dygma (Focus protocol), PrismRGB, Ableton Push 2 (MIDI + display), QMK (OpenRGB HID), Nollie (Gen1/Gen2/Legacy/NOS2/Stream65), and ROLI Blocks
- ✨ Add unified **driver module API** (`hypercolor-driver-api`) with typed control surfaces, config registry, pairing flows, credential storage, and hot-pluggable backend discovery
- ✨ Add network device drivers as independent crates: `hypercolor-driver-wled` (DDP/sACN, known-IP cache, fuzzy dedup), `hypercolor-driver-hue` (DTLS entertainment streaming, bridge scanner), `hypercolor-driver-nanoleaf` (topology refresh, panel streaming), `hypercolor-driver-govee` (LAN multicast, cloud v1 API key pairing, rate limiting), and `hypercolor-driver-openrgb` (clean-room SDK client, fallback bridge with mode restore)
- ✨ Add **Leptos 0.8 CSR web UI** (`hypercolor-ui`) with SilkCircuit/Luminary design system, Studio composition page with multi-zone tree, layout editor with undo/redo, display face picker with live JPEG previews, device management with vendor brand logos (18 vendors), effect browser with live thumbnails, dashboard with frame-timeline waterfall, and settings page
- ✨ Add **Ratatui TUI** (`hypercolor-tui`) with live Kitty/Sixel/halfblock canvas previews, tachyonfx motion layer (spectrum-reactive borders, ambient canvas bleed, idle breathing, transition crossfades), multi-zone and multi-scene support, HSL color picker, and mouse-draggable split panels
- ✨ Add **desktop app** (`hypercolor-app`) via Tauri with daemon supervisor (backoff + circuit breaker), system tray with brightness presets, first-run welcome overlay, Windows NSIS installer with per-machine hardware setup, macOS bundle with hardened runtime, and rolling file logging
- ✨ Add CLI (`hypercolor`) with connection profiles, SilkCircuit-themed help output, `hyper status` visual display, scene/zone/layer/driver/control subcommands, shell completions, and TUI as a subcommand
- ✨ Add standalone system tray applet (`hypercolor-tray`) for Linux/macOS with daemon state polling, brightness submenu, and active scene display
- ✨ Add `hypercolor-leptos-ext` shared crate with typed WebSocket channels, reconnecting transport, binary preview/spectrum frame codecs, RPC envelopes, session replay, RAF scheduler, and browser event/canvas/storage helpers
- ✨ Add **TypeScript effect SDK** (`@hypercolor/sdk`) with declarative `effect()`, `canvas()`, and `face()` APIs, Bun build pipeline, `create-hypercolor` scaffolding CLI, 65 curated cosine palettes, GLSL utility library (noise, color, palette), and hot-reload file watcher
- ✨ Add **Python client** (`hypercolor` on PyPI) with OpenAPI-generated REST client, typed WebSocket protocol, scene/zone/layer helpers with `If-Match` concurrency, Home Assistant helpers, sync and async APIs
- ✨ Add multi-zone scene system with per-zone effect assignment, layer stack composition (effect, media, screen-region, web-viewport sources), blend modes (Normal, Screen, Add, Multiply, plus material modes), and zone layout editing API
- ✨ Add media asset library with image/animated WebP/Lottie/video/stream URL layer sources, drag-and-drop multi-file upload, and configurable stream URL allowlists
- ✨ Add Wayland screen capture input source with crop editor, ambilight edge-projection effect, and color tuning pipeline
- ✨ Add PulseAudio native audio capture with live input switching, spectrum analysis, beat detection, and per-effect audio opt-in
- ✨ Add MPRIS now-playing media input with bounded album art
- ✨ Add MCP (Model Context Protocol) server over HTTP with structured output, device/effect/scene/display/layer tools, and prompt templates for AI agent control
- ✨ Add REST API with OpenAPI spec generation, tiered auth (loopback/API key/network access modes), rate limiting, CORS enforcement, HTTP access logging, and WebSocket binary frame streaming (canvas, spectrum, device metrics, per-zone previews, display previews)
- ✨ Add sensor input system (CPU/GPU temps via PawnIO MSR/SMN on Windows, ACPI thermal zones, NVML) with live bindings to effect controls
- ✨ Add session and power awareness: systemd-logind integration, screensaver monitor, Windows sleep/resume rediscovery, configurable off-output behavior
- ✨ Add mDNS network discovery and multi-server support
- ✨ Add virtual display simulator with frame inspection, CRUD API, and browser preview
- ✨ Add Criterion benchmark suites for core pipeline, HAL protocol encoding, render group composition, and GPU compositor
- ✨ Add Playwright e2e test harness covering REST API, WebSocket protocol, CLI, MCP, Servo rendering, and UI smoke tests
- ✨ Add CI/CD pipeline with PR gates (clippy, tests, WASM, e2e), Servo cache warming, release workflow (GitHub releases, PyPI, npm, Homebrew, AUR, Debian packages), and dependabot
- ✨ Add Zola documentation site with Luminary theme, full user guide, effect authoring tutorials, API reference, hardware compatibility matrix, and architecture docs
- ✨ Add canonical brand asset pipeline with SVG-to-PNG generation, tray icon variants, and app icon ladders
- ✨ Add `justfile` with 50+ developer recipes covering build, dev, test, release, GPU, Servo, TUI, and setup workflows
- ✨ Add cross-platform `setup.sh` / `setup.ps1` dev environment bootstrap
- ✨ Add hardware compatibility database (`data/compat/`) with structured vendor device catalogs
- ✨ Add udev rules for USB device permissions with `just install-udev` recipe

### Changed

- 🔄 Default render canvas to 640x480 with live FPS retune and adaptive SDK API
- 🔄 Default target frame rate to 30fps with adaptive skip policy; throttle idle render loop
- 🔄 Switch color pipeline to linear-light interpolation with sRGB encode/decode and perceptual LED compensation
- 🔄 Rename CLI binary to `hypercolor` and daemon to `hypercolor-daemon`
- 🔄 Replace EffectEngine with scene-backed render groups as the primary effect routing model
- 🔄 Replace overlay compositor with display face composition blending
- 🔄 Migrate all HAL protocol structs to zerocopy typed layouts (Razer, Corsair, ASUS, PrismRGB, Seiren V3, ROLI Blocks)
- 🔄 Move network driver implementations into independent crates (`hypercolor-driver-wled`, `hypercolor-driver-hue`, `hypercolor-driver-nanoleaf`, `hypercolor-driver-govee`)
- 🔄 Route all device discovery through the unified driver module registry
- 🔄 Upgrade web UI preview to WebGL with negotiated RGBA/JPEG transport
- 🔄 Pin Servo embedder to crates.io `0.1.0` LTS with baked-in resources and JS JIT
- 🔄 Bump MSRV to 1.94, pin CI to Rust 1.95

### Fixed

- 🐛 Fix WebSocket preview relay retry spin after drops
- 🐛 Fix WLED color saturation loss on RGBW devices by sending RGB-only DDP frames
- 🐛 Fix Servo HTML worker lifetime across effect switches and clean shutdown
- 🐛 Fix canvas flicker from reactive DOM rebuilds in the web UI
- 🐛 Fix `glslopt` build on glibc 2.39+ (C23 thread compat patch)
- 🐛 Fix Razer Blade Pro 2016 custom-frame path and Tartarus Chroma support
- 🐛 Fix Corsair Bragi zero-prefixed reply parsing
- 🐛 Fix ASUS DRAM identity merge after remapping
- 🐛 Fix SMBus DRAM probing to match OpenRGB bus detection addresses
- 🐛 Fix Push 2 raw MIDI dynamic color routing and palette slot stability
- 🐛 Fix layout jitter from config-driven canvas resizes
- 🐛 Fix GPU readback buffer leaks on map poll timeout
- 🐛 Fix daemon preview clamp panic on zero-sized canvas
- 🐛 Fix IPv6/unspecified SSRF bypass in web viewport URL validation
- 🐛 Fix cross-site loopback CSRF without API keys
- 🐛 Fix animated media decode OOM by bounding resource usage
- 🐛 Fix Lottie decode resource bounding

### Security

- 🔒 Require auth for network daemon binds; restrict credential file permissions
- 🔒 Enforce CORS config bound to auth settings
- 🔒 Validate WebSocket origin before upgrade; cap WS command body size
- 🔒 Block cross-site loopback write requests; enforce auth for proxied non-loopback clients
- 🔒 Restrict web viewport URLs to public HTTP/HTTPS (close IPv6 bypass)
- 🔒 Guard GPU media uploads against invalid texture sizes
- 🔒 Enforce media admission caps for MCP scene activation and active layer mutations
- 🔒 Validate broadcast-media target groups
- 🔒 Bound pending cloud login sessions and require enabled cloud config
- 🔒 Require explicit intent header for cloud connect endpoint
- 🔒 Anchor bundled PawnIO payload verification; reject per-user module directories for Windows services
- 🔒 Secure Windows service daemon install path
- 🔒 Validate cloud login URL before auto-open; bind daemon API keys to discovered endpoint
- 🔒 Pin CI actions (`rust-toolchain`, `install-action`) to immutable commit SHAs
- 🔒 Harden stream URL SSRF validation
- 🔒 Cap cloud heartbeat interval to avoid instant overflow
- 🔒 Lock entitlement cache file permissions
- 🔒 Add unified network access modes (local-only, LAN, custom ACL)

### Removed

- 🔥 Remove OpenRGB backend (replaced by clean-room `hypercolor-openrgb-sdk` and fallback bridge driver)
- 🔥 Remove legacy EffectEngine, global-canvas vocabulary, and compatibility migration paths
- 🔥 Remove legacy pairing routes and device backend aliases
- 🔥 Remove marketing website from repository
- 🔥 Remove `hypercolor-desktop` crate (superseded by `hypercolor-app`)

### Metrics

- Total Commits: 1,751
- Files Changed: 2,591
- Insertions: +720,204
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
