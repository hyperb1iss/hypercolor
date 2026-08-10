# Changelog

All notable changes to Hypercolor will be documented here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-08-10

This release delivers **interactive input capture** across Linux and Windows, a fully rearchitected **screen capture pipeline** with arbitrary-resolution support, **portable device identity** for hardware rebinding, and a **mobile-responsive web UI** built around the promoted Studio workspace.

### Added

- ✨ Add **interactive input pipeline** (Spec 71): consent-gated keyboard, mouse, and media capture with privacy-first WebSocket isolation (0a6cbf1, 210563b, b159f32, 245e987)
- ✨ Add `hypercolor-windows-input` crate: Raw Input interop for Windows host keyboard and media key capture (ab88979)
- ✨ Add `hypercolor-windows-capture` crate: Desktop Duplication screen capture with D3D11 GPU reduction (43c4d38)
- ✨ Add `hypercolor-platform-fs` crate: atomic state file replacement with Windows filesystem identity support
- ✨ Add **portable device identity**: USB serial, MAC, and bridge ID claims carried on every discovery carrier, persisted as `device-aliases.json` key pins (3dd5480, 5cc404e, 837b1ac, c89f41c)
- ✨ Add device rebinding REST API (`GET /devices/bindings`, `POST /devices/rebind`) to migrate orphaned layout bindings after hardware replacement (a446fb5)
- ✨ Add **Keystrike** interactive showcase effect demonstrating keyboard-reactive lighting (613a9a1)
- ✨ Add cover artwork for 40 bundled effects at 960px for high-DPI cards, embedded inline at SDK build time (b4bbd65, 16e6671, e0bae71)
- ✨ Add **mobile-responsive web UI** with bottom-nav shell, compact dashboards, stacked settings, and brand header (a4939c0, 048eed1, eb94ef1)
- ✨ Add mobile-only nav items for widget-owned destinations (334bde9)
- ✨ Add wide and chunked preview wire frames with byte-bounded latest queue transport (6497af6, 118af09)
- ✨ Add addressed interactive preview frames for connection-scoped browser routing (06abc03, aa6c270)
- ✨ Add isolated interactive render lanes for per-connection effect previews (bf3ba27)
- ✨ Add input source health primitives: denied resource counts, source freshness, and degradation reporting (c2d2fa9, 35abdff, 7e08c2a, 5fc2f7d)
- ✨ Add durable driver inventory store for learned device targets (9f88ab0)
- ✨ Add screen capture capacity status on Linux (732c6de)
- ✨ Add **arbitrary-resolution CPU reducer** for screen capture with zero-copy sampling geometry (e3d603a, 73e5642)
- ✨ Add GPU-native screen publication path: exact surfaces retained on GPU through `wgpu` bridge (63182af, f5b7710)
- ✨ Add exact publication demand control plane for screen capture (78b5ff3, 976a220)
- ✨ Add transactional render resource preparation and atomic layout activation (1d4e286, 93bd67f)
- ✨ Add scalable GPU area sampling with summed-area table (SAT) hierarchical scans (c3b555d, 3391eed)
- ✨ Add GPU area SAT shaders (`area_sat.wgsl`, `area_hierarchy.wgsl`) for resolution-independent spatial sampling
- ✨ Add `input_access` banner: UI notification when interactive effects need input permissions (bb8d89c)
- ✨ Add input routing and source health display in the web UI (02ceec5)
- ✨ Add monitor enumeration served over the capture API (b3e900d)
- ✨ Add background worker gate for discovery subsystem (0c8d2e0)
- ✨ Add udev rules for input device access (`70-hypercolor-input.rules`, `70-hypercolor-input-all.rules`)

### Changed

- 🔄 Promote **Studio** to the sole navigation entry, drop legacy pages and feature flag (d753a92)
- 🔄 Consolidate dashboard telemetry into a single Performance strip (ff4fb73)
- 🔄 Redesign settings page to read as a product, not a config editor (f3b6e90)
- 🔄 Normalize device-settings key space to schema v2 with quarantine and rebind inheritance (3198c35)
- 🔄 Upgrade Servo embedder stack to 0.4.0 (ba596b5)
- 🔄 Default Windows renderer GPU backend to DX12 (d459659)
- 🔄 Make surface pool allocation fully fallible across the type system (0010619, 2bb1e85)
- 🔄 Make preview encoding fallible and wide-frame aware (58c9eaa)
- 🔄 Replace state files atomically on Windows with ordered snapshot commits (1033f4a, 4f6735a)
- 🔄 Portal-mount modals, declutter sidebar, add setup-hook seam (5abf30a)
- 🔄 Device detail becomes a dismissible overlay with slim status strip (ab75829)
- 🔄 Share one library and profile store per process (4748ae6)
- 🔄 Hint observers on every persisted settings mutation (89914f4)
- 🔄 Summarize slow-consumer WebSocket drops instead of logging each one (87e37ca)

### Fixed

- 🐛 Fix letterbox detection eating the whole capture frame (93bf8a0)
- 🐛 Fix screen downscale breaking source aspect ratio (05c1251)
- 🐛 Fix Windows input double-delivery: stop delivering every batch to core twice (cef4cd0)
- 🐛 Fix SMBus ENE delays: send in-batch instead of fragmenting (6c8fe8b)
- 🐛 Fix PawnIO forcing SMBus polls onto the kernel sleep timer (b6b433e)
- 🐛 Fix Hue bridge connect deadline: budget for the whole handshake (956eddf)
- 🐛 Fix audio fallback: use cpal when PulseAudio cannot answer (0c52e15)
- 🐛 Fix Push2 MIDI pacing and bound palette sysex to prevent firmware wedge (364b663)
- 🐛 Fix Windows installer: register SMBus broker during install (d89177d)
- 🐛 Fix Windows CPU temperature reader re-probe (bac7d60)
- 🐛 Fix macOS installer: stop presenting Apache license as a EULA (31e14a5)
- 🐛 Fix macOS CI: ad-hoc sign the app bundle instead of `--no-sign` (a186e36)
- 🐛 Fix input worker lifecycles: make transactional, retain on reaper failure (c4230d8, 46753390)
- 🐛 Fix timed event metadata preservation through LightScript (e8381c8)
- 🐛 Fix Windows Raw Input lifecycle races and late initialization fencing (fc8aa74, 3b7e563)
- 🐛 Fix persistence: coordinate concurrent replacements, retry dirty snapshots, honor Windows filesystem identity (c526bc2, 2b65222, dd6aa1a)
- 🐛 Fix layout mutations: make cancellation-safe, serialize related workflows (4072c4e, e0bcc98)
- 🐛 Fix renderer: retain last-good scene after GPU projection failure (37bde32)
- 🐛 Fix renderer: preserve preview request across resize, fallback, and retained-frame paths (5de7a84, acc2534, 68afaa6)
- 🐛 Fix GPU readback admission: make transactional, preserve state across allocation failure (5b7ed2b, fcd079b)
- 🐛 Fix Wayland capture: fence decode and worker lifetime, linearize adoption and recovery (0022bf3, a2bed12)
- 🐛 Fix screen smoothing cadence invariance and canonical crop origins (879f237, 916e89f)
- 🐛 Fix canvas: preserve arbitrary geometry, remove resolution ceilings (753ba21, 9ac3e8f, 1f8084a)
- 🐛 Fix spatial sampling: clamp bilinear coordinates, commit layouts atomically (7d14ba8, df465f2)
- 🐛 Fix Nanoleaf external control: stream to an injectable UDP port (660f065)
- 🐛 Fix config: preserve unknown top-level sections across saves (b0114f0)
- 🐛 Fix daemon startup: roll back partial subsystem startup, restore layouts before renderer spawn (1ea3d54, 0198f50)
- 🐛 Fix Wayland screen capture: negotiate compositor-real formats, stamp restore-token updates with session identity (ba15626, 6fe3254)
- 🐛 Fix discovery: connect devices placed only through a scene zone, resync on activation (2607914, 20a6386)
- 🐛 Fix discovery: forget learned targets on device deletion, make inventory updates race-safe (18a2576, ac6b526)
- 🐛 Fix UI: route bundled presets to zone instead of dead endpoint (5d2de3f)
- 🐛 Fix UI: resolve card artwork by effect ID instead of guessed slug (932a17f)
- 🐛 Fix packaging: ship input udev rules in release payloads (29704a3)
- 🐛 Fix render metrics: stop billing pre-sampling work to `sample_us` (7260cb3)
- 🐛 Fix daemon: retire scene transactions while render loop is paused (900aca5)
- 🐛 Fix daemon: prune default faces and preferences on device deletion (1674dc1)
- 🐛 Fix HAL: use native Windows HID backend (a29252b)

### Security

- 🔒 Input capture is consent-gated and disabled by default; requires explicit `[input] enabled = true` in config (0a6cbf1)
- 🔒 Move `InputEventReceived` from public event bus to control-tier `input_events` channel, preventing casual subscriber access (0a6cbf1)
- 🔒 Fence Windows Raw Input initialization and device identity lifetimes to prevent use-after-free (fc8aa74, e611d09)
- 🔒 Make Windows capture adoption transactional to prevent GPU resource exhaustion (be8064d)
- 🔒 Validate portable identity claims at capture point instead of re-deriving fingerprints (3dd5480)
- 🔒 Bound native media player scans and exclude failed cache entries to prevent unbounded memory growth (eabc938, aaa8ab9)
- 🔒 Harden frame ownership and geometry to prevent invalid memory access across capture mode transitions (2007ac6)

### Removed

- 🔥 Remove legacy UI pages (Displays, Layout, Assets): all functionality consolidated into Studio (d753a92)
- 🔥 Remove simulator UI E2E journey (d7fc98a)
- 🔥 Remove arbitrary canvas resolution ceilings: arbitrary dimensions now supported (9ac3e8f, 56b208b)
- 🔥 Remove legacy canvas resolution warning from SDK (3eebfcd)

### Metrics

- Total Commits: 523
- Files Changed: 816
- Insertions: +188,480
- Deletions: -15,055
<!-- -------------------------------------------------------------- -->

## [Unreleased]

### Changed

- `getInputData()` now reports input declaration, routing, health, freshness,
  and degradation independently. Recent keyboard or mouse activity no longer
  implies source availability.

### Deprecated

- `InputData.available` now means `routed && healthy` and remains as a
  compatibility alias through SDK 0.3.x. Read the explicit lifecycle fields
  instead; the alias will be removed in SDK 0.4.0.

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
