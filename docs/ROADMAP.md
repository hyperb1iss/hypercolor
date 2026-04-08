# Hypercolor Implementation Roadmap

> Swarm-optimized build plan. Every task is independently testable with zero human intervention.

---

## Execution Model

**Strategy:** Wave-based parallel dispatch. Each wave unlocks the next. Within a wave, all tasks run simultaneously via agent swarms.

**Verification gate** (every task, no exceptions):
```bash
cargo check --workspace          # Types resolve
cargo test --workspace           # All tests pass
cargo clippy --workspace -- -D warnings  # No lint violations
```

**Agent contract:** Each agent receives:
1. This roadmap (task ID + description)
2. The relevant spec(s) from `docs/specs/`
3. The workspace context (what exists so far)

Each agent produces:
1. Source files in the correct crate/module
2. Unit tests in the same file or `tests/` directory
3. A clean `cargo test` run

**Crate dependency DAG:**
```
hypercolor-types  (0 deps — pure types)
       ↓
hypercolor-core   (depends on types)
       ↓
hypercolor-daemon (depends on core)
hypercolor-cli    (depends on core)
hypercolor-tui    (depends on core)
```

---

## Wave 0: Scaffold — The Skeleton

**Parallelism: 1 agent (sequential — sets up workspace for everything else)**
**Duration estimate: ~5 minutes**
**Spec sources:** ARCHITECTURE.md §Cargo Workspace Layout

### Task 0.1: Cargo Workspace + CI Foundation

**Files:**
- `Cargo.toml` (workspace root)
- `crates/hypercolor-types/Cargo.toml` + `src/lib.rs`
- `crates/hypercolor-core/Cargo.toml` + `src/lib.rs`
- `crates/hypercolor-daemon/Cargo.toml` + `src/main.rs`
- `crates/hypercolor-cli/Cargo.toml` + `src/main.rs`
- `crates/hypercolor-tui/Cargo.toml` + `src/main.rs`
- `rust-toolchain.toml`
- `.gitignore`
- `.cargo/config.toml`
- `clippy.toml`
- `CLAUDE.md`

**Implementation:**
- Workspace root with all 5 crate members
- `hypercolor-types`: zero dependencies, pure data types, `#![forbid(unsafe_code)]`
- `hypercolor-core`: depends on `hypercolor-types`, tokio, serde
- `hypercolor-daemon`: depends on `hypercolor-core`, axum, tokio
- `hypercolor-cli`: depends on `hypercolor-core`, clap
- `hypercolor-tui`: depends on `hypercolor-core`, ratatui
- Rust edition 2024, MSRV 1.85
- `.gitignore` for Rust + Node (web/ dir later)
- `.cargo/config.toml` with default target, incremental build settings
- `clippy.toml` with project-specific lint config
- `CLAUDE.md` with project conventions, build commands, test commands, module ownership rules, and agent coordination guidelines

**Verify:**
```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

---

## Wave 1: Types — The Vocabulary

**Parallelism: 8 agents (all independent, zero shared files)**
**Spec sources:** All 15 specs contribute type definitions

This wave builds `hypercolor-types` — the shared vocabulary crate. Every type used across crate boundaries lives here. **No logic, no I/O, no async** — pure data structures with serde derives.

Each agent owns one module file. No two agents touch the same file.

### Task 1.1: Canvas + Color Types

**Module:** `crates/hypercolor-types/src/canvas.rs`
**Spec:** `docs/specs/01-core-engine.md` §1-2
**Depends on:** Wave 0

```rust
// Canvas buffer, pixel operations, color types
// Canvas { width, height, pixels: Vec<[u8; 4]> }
// Color { r: f32, g: f32, b: f32, a: f32 } — linear sRGB
// BlendMode enum (Normal, Screen, Multiply, Add, Overlay, SoftLight, ColorDodge)
// Oklab/Oklch conversion functions
// ColorFormat enum (Rgb, Rgbw, RgbW16)
```

**Tests:** Construction, pixel get/set, blend mode math, color space roundtrips, serde

### Task 1.2: Device Types

**Module:** `crates/hypercolor-types/src/device.rs`
**Spec:** `docs/specs/02-device-backend.md` §4-8
**Depends on:** Wave 0

```rust
// DeviceId, DeviceInfo, DeviceHandle, DeviceState enum
// DeviceCapabilities, ZoneInfo, LedTopology enum
// ConnectionType enum (Usb, Network, Bluetooth, Bridge)
// DeviceFamily enum (Wled, Hue, PrismRgb, Nollie, Custom)
// DeviceIdentifier enum (UsbHid, Network, HueBridge)
// DeviceError enum (thiserror)
```

**Tests:** State machine transitions, identifier fingerprinting, serde roundtrips

### Task 1.3: Effect Types

**Module:** `crates/hypercolor-types/src/effect.rs`
**Spec:** `docs/specs/07-effect-system.md` §1-4
**Depends on:** Wave 0

```rust
// EffectId, EffectMetadata, EffectCategory enum (8 variants)
// ControlDefinition, ControlValue enum (Float, Int, Bool, Color, Gradient, Enum, String)
// ControlGroup, GradientStop
// EffectSource enum (Native, Html, Shader)
// EffectState enum (Loading, Initializing, Running, Paused, Destroying)
// LayerConfig, BlendMode (re-export from canvas)
```

**Tests:** ControlValue conversions, metadata validation, serde roundtrips

### Task 1.4: Audio Types

**Module:** `crates/hypercolor-types/src/audio.rs`
**Spec:** `docs/specs/08-audio-pipeline.md` §1
**Depends on:** Wave 0

```rust
// AudioData { spectrum: [f32; 200], mel_bands: [f32; 24], chromagram: [f32; 12],
//   beat_detected: bool, beat_confidence: f32, bpm: f32,
//   rms_level: f32, peak_level: f32, spectral_centroid: f32, spectral_flux: f32 }
// AudioConfig { device: String, fft_size: usize, smoothing: f32, gain: f32 }
// AudioSourceType enum (SystemMonitor, Named, Microphone, None)
```

**Tests:** Default silence values, field ranges, serde roundtrips

### Task 1.5: Spatial Types

**Module:** `crates/hypercolor-types/src/spatial.rs`
**Spec:** `docs/specs/06-spatial-engine.md` §1-4
**Depends on:** Wave 0

```rust
// NormalizedPosition { x: f32, y: f32 }
// DeviceZone { device_id, zone_name, position, size, rotation, topology, led_positions }
// LedTopology enum (Strip, Matrix, Ring, ConcentricRings, PerimeterLoop, Point, Custom)
// SpatialLayout { id, name, canvas_width, canvas_height, zones: Vec<DeviceZone> }
// SamplingMode enum (Nearest, Bilinear, AreaAverage, Gaussian)
// EdgeBehavior enum (Clamp, Wrap, FadeToBlack, Mirror)
// ZoneShape enum (Rectangle, Arc, Ring, Custom)
```

**Tests:** NormalizedPosition clamping, coordinate transforms, topology LED count, serde

### Task 1.6: Event Types

**Module:** `crates/hypercolor-types/src/event.rs`
**Spec:** `docs/specs/09-event-bus.md` §2
**Depends on:** Wave 0

```rust
// HypercolorEvent enum — complete taxonomy from spec:
//   Device*, Effect*, Scene*, Audio*, System*, Automation*, Layout*, Integration*
// EventCategory enum, EventPriority enum
// FrameData { zone_colors: Vec<ZoneColors>, timestamp, fps }
// ZoneColors { device_id, zone_id, colors: Vec<[u8; 3]> }
// SpectrumData { spectrum: Vec<f32>, mel_bands, beat, bpm }
// FrameTiming { frame_number, render_us, sample_us, output_us, total_us }
```

**Tests:** Event categorization, priority ordering, serde roundtrips

### Task 1.7: Config Types

**Module:** `crates/hypercolor-types/src/config.rs`
**Spec:** `docs/specs/12-configuration.md` §8
**Depends on:** Wave 0

```rust
// HypercolorConfig — top-level with all sections
// DaemonConfig { port, fps, log_level, canvas_width, canvas_height }
// WebConfig { enabled, bind, cors_origins, auth_token }
// AudioConfig (re-export or extend from audio.rs)
// CaptureConfig { enabled, source, fps, monitor }
// TuiConfig { theme, preview_fps }
// DiscoveryConfig { scan_interval, backends: Vec<String> }
// FeatureFlags { wled, hue, hid, audio, screen_capture, servo }
// ProfileConfig, SceneConfig, LayoutConfig (for file formats)
```

**Tests:** Default values, TOML roundtrips, feature flag combinations

### Task 1.8: Scene & Automation Types

**Module:** `crates/hypercolor-types/src/scene.rs`
**Spec:** `docs/specs/13-scenes-automation.md` §1-4
**Depends on:** Wave 0

```rust
// SceneId, Scene { id, name, scope, zones, transition, metadata }
// SceneScope enum (Full, PcOnly, RoomOnly, Devices, Zones)
// TransitionSpec { duration_ms, easing, color_interp }
// EasingFunction enum (Linear, EaseIn, EaseOut, EaseInOut, CubicBezier)
// TriggerSource enum (TimeOfDay, Sunset, AppLaunched, AudioLevel, GameDetected, etc.)
// AutomationRule { name, trigger, conditions, action, cooldown, enabled }
// ConditionExpr enum, ActionExpr enum, TriggerExpr enum
// ScenePriority (u8 newtype)
```

**Tests:** Priority ordering, expression evaluation, easing math, serde

**Wave 1 lib.rs:** After all 8 agents complete, a final quick pass wires up `crates/hypercolor-types/src/lib.rs` with `pub mod` declarations for all modules.

---

## Wave 2: Core Traits + Engine Skeleton

**Parallelism: 7 agents**
**Depends on:** Wave 1 (all types exist)

This wave builds the trait definitions and core engine logic in `hypercolor-core`. Each agent owns a submodule. Traits define interfaces; implementations come in later waves.

### Task 2.1: Device Backend Traits

**Module:** `crates/hypercolor-core/src/device/traits.rs`
**Spec:** `docs/specs/02-device-backend.md` §2-3
**Depends on:** Task 1.2

```rust
// #[async_trait] DeviceBackend trait
//   fn discover(&mut self) -> Vec<DeviceInfo>
//   fn connect(&mut self, id: &DeviceId) -> Result<DeviceHandle>
//   fn push_frame(&mut self, handle: &DeviceHandle, colors: &[Color]) -> Result<()>
//   fn disconnect(&mut self, handle: &DeviceHandle) -> Result<()>
//   fn info(&self) -> BackendInfo
//
// DevicePlugin trait — lifecycle: build(), ready(), cleanup()
// DiscoveryOrchestrator — parallel scanner coordination
// DeviceRegistry — thread-safe device tracking
```

**Tests:** Mock backend implementing traits, registry add/remove/lookup, discovery dedup

### Task 2.2: Effect Engine Traits

**Module:** `crates/hypercolor-core/src/effect/traits.rs`
**Spec:** `docs/specs/07-effect-system.md` §6-8, `docs/specs/01-core-engine.md` §4
**Depends on:** Task 1.1, 1.3, 1.4

```rust
// EffectRenderer trait
//   fn init(&mut self, metadata: &EffectMetadata) -> Result<()>
//   fn tick(&mut self, frame: &FrameInput) -> Result<Canvas>
//   fn control_changed(&mut self, name: &str, value: &ControlValue)
//   fn destroy(&mut self)
//
// FrameInput { time_secs, delta_secs, frame_number, audio: AudioData, controls }
// EffectEngine — orchestrator that selects wgpu/servo path
// EffectRegistry — scan dirs, load metadata, hot-reload watch
```

**Tests:** Mock renderer, registry scanning test fixtures, lifecycle state machine

### Task 2.3: Event Bus

**Module:** `crates/hypercolor-core/src/bus/mod.rs`, `events.rs`, `state.rs`
**Spec:** `docs/specs/09-event-bus.md` §3-7
**Depends on:** Task 1.6

```rust
// HypercolorBus { events: broadcast::Sender, frame: watch::Sender, spectrum: watch::Sender }
// EventFilter { categories, exclude_types, device_ids, min_priority }
// FilteredEventReceiver — wraps broadcast::Receiver with filter logic
// Bus::new(), publish(), subscribe_all(), subscribe_filtered()
```

**Tests:** Publish/subscribe roundtrip, filtered subscription, watch latest-value semantics, lagged receiver handling, concurrent publisher stress test

### Task 2.4: Spatial Sampler

**Module:** `crates/hypercolor-core/src/spatial/sampler.rs`, `topology.rs`, `layout.rs`
**Spec:** `docs/specs/06-spatial-engine.md` §5-8
**Depends on:** Task 1.1, 1.5

```rust
// SpatialEngine { layout: SpatialLayout, lut: SamplingLut }
// SamplingLut — precomputed per-LED pixel weights
// SummedAreaTable — O(1) rectangle average queries
// sample_nearest(), sample_bilinear(), sample_area_average(), sample_gaussian()
// SpatialEngine::sample(&self, canvas: &Canvas) -> Vec<ZoneColors>
// LedTopology position generators (strip, matrix, ring, etc.)
```

**Tests:** Known-pixel canvases with exact expected outputs for each sampling mode, SAT correctness, topology position generation, LUT rebuild

### Task 2.5: Config Manager

**Module:** `crates/hypercolor-core/src/config/mod.rs`, `manager.rs`, `profile.rs`, `layout.rs`
**Spec:** `docs/specs/12-configuration.md` §7-10
**Depends on:** Task 1.7

```rust
// ConfigManager { config: ArcSwap<HypercolorConfig>, watcher: notify::RecommendedWatcher }
// ConfigManager::load(path), reload(), watch(), get() -> arc_swap::Guard
// XDG path resolution (config_dir, data_dir, cache_dir)
// Profile loading/saving, layout loading/saving
// Migration system: version field + migration chain
// Environment variable overrides: HYPERCOLOR_*
```

**Tests:** Load from TOML string, hot-reload simulation, migration chain, env override precedence, XDG path logic

### Task 2.6: Render Loop

**Module:** `crates/hypercolor-core/src/engine/mod.rs`, `render_loop.rs`
**Spec:** `docs/specs/01-core-engine.md` §3, 5-7
**Depends on:** Task 2.1, 2.2, 2.3, 2.4

```rust
// RenderLoop { effect_engine, spatial_engine, backends, input_sources, bus, fps }
// Frame pipeline: input sample → effect tick → canvas → spatial sample → device output → bus publish
// Adaptive FPS: FpsController with 5 tiers (10/20/30/45/60)
// Frame timing measurement + publish
// Layer composition with blend modes
```

**Tests:** Single-frame execution with mock effect + mock backend, FPS controller tier transitions, blend mode composition math, frame timing accuracy

### Task 2.7: Input Source Traits

**Module:** `crates/hypercolor-core/src/input/traits.rs`
**Spec:** `docs/specs/08-audio-pipeline.md` §7, `docs/specs/14-screen-capture.md` §1
**Depends on:** Task 1.4

```rust
// InputSource trait
//   fn name(&self) -> &str
//   fn sample(&mut self) -> Result<InputData>
//   fn start(&mut self) -> Result<()>
//   fn stop(&mut self)
//
// InputData enum { Audio(AudioData), Screen(ScreenData), Keyboard(KeyboardState) }
// InputManager — holds Vec<Box<dyn InputSource>>, samples all per frame
```

**Tests:** Mock input source, InputManager multi-source aggregation

---

## Wave 3: Device Backends + Audio Pipeline

**Parallelism: 6 agents**
**Depends on:** Wave 2 (traits exist)

Each backend implements `DeviceBackend`. Each agent owns one backend module. Audio pipeline is independent of backends.

### Task 3.1: WLED Backend

**Module:** `crates/hypercolor-core/src/device/wled/mod.rs`, `ddp.rs`, `discovery.rs`
**Spec:** `docs/specs/03-wled-backend.md` (entire spec)
**Depends on:** Task 2.1

```rust
// WledBackend implements DeviceBackend
// DDP packet builder: header construction, fragmentation, sequence tracking
// E1.31 packet builder: full sACN format
// mDNS discovery via mdns-sd
// WledDevice runtime state, health tracking
// UDP socket management, multi-device dispatch
```

**Tests:** DDP packet construction (verify byte layout against spec), E1.31 packet format, mDNS response parsing (from fixture data), frame fragmentation for >480 pixel devices, sequence number wrapping

### Task 3.2: USB HID Backend (PrismRGB)

**Module:** `crates/hypercolor-core/src/device/hid/mod.rs`, `prism.rs`, `nollie.rs`
**Spec:** `docs/specs/04-usb-hid-backend.md` (entire spec)
**Depends on:** Task 2.1

```rust
// HidBackend implements DeviceBackend
// PrismRgb8Controller — init/render/shutdown packet sequences
// PrismRgbSController — 6-channel variant
// PrismRgbMiniController — single channel
// Nollie8Controller — different protocol family
// Packet builders with checksum computation
// VID/PID detection table
```

**Tests:** Packet construction for all 4 controllers (verify against DRIVERS.md hex examples), channel LED count validation, checksum computation, init/render/shutdown sequences. Uses mock USB transport — no actual hardware needed.

### Task 3.4: Audio Pipeline

**Module:** `crates/hypercolor-core/src/input/audio/mod.rs`, `fft.rs`, `beat.rs`, `features.rs`
**Spec:** `docs/specs/08-audio-pipeline.md` (entire spec)
**Depends on:** Task 2.7

```rust
// AudioInput implements InputSource
// FFT pipeline: ring buffer → Hann window → realfft → 200-bin log resampling
// Beat detection: spectral flux onset, adaptive threshold, tempo estimation
// Mel filterbank: 24 triangular filters
// Chromagram: 12 pitch classes
// Spectral features: centroid, flux, rolloff, flatness
// Asymmetric EMA smoothing (fast attack, slow decay)
// Lock-free triple buffer for audio → render thread
```

**Tests:** FFT on known sine waves (verify peak at correct bin), beat detection on synthetic pulses, mel band energy distribution, smoothing convergence, silence handling. All tests use synthetic audio data — no microphone needed.

### Task 3.5: Screen Capture Input

**Module:** `crates/hypercolor-core/src/input/screen/mod.rs`, `sector.rs`, `smooth.rs`
**Spec:** `docs/specs/14-screen-capture.md` (entire spec)
**Depends on:** Task 2.7

```rust
// ScreenCaptureInput implements InputSource
// SectorGrid: N×M frame subdivision, per-sector color averaging
// TemporalSmoothing: adaptive EMA with scene-cut detection
// CaptureConfig, MonitorSelect, ContentMode
// Platform backends behind #[cfg]: xcap (cross-platform fallback)
// ScreenData output type with zone_colors
```

**Tests:** Sector grid color averaging on synthetic frames, temporal smoothing convergence, scene-cut detection (sudden frame change), letterbox detection, adaptive quality tier transitions. All tests use synthetic frame buffers.

### Task 3.6: Scene & Automation Engine

**Module:** `crates/hypercolor-core/src/scene/mod.rs`, `transition.rs`, `priority.rs`, `automation.rs`
**Spec:** `docs/specs/13-scenes-automation.md` (entire spec)
**Depends on:** Task 2.2, 2.3, 2.5

```rust
// SceneManager — CRUD + activation + priority stack
// TransitionEngine — cross-fade rendering, Oklab interpolation, easing
// PriorityStack — BTreeMap<u8, Vec<StackEntry>>, auto-restore cascade
// AutomationRuleEngine — trigger evaluation, condition checking, action dispatch
// CircadianEngine — color temperature curve over 24h, solar calc
```

**Tests:** Scene activation/deactivation, transition blending (Oklab correctness), priority stack push/pop/cascade, easing function curves, automation rule evaluation with mock triggers, circadian color temperature at known times

---

## Wave 4: API + CLI + Integration

**Parallelism: 5 agents**
**Depends on:** Wave 3 (backends + engine complete)

### Task 4.1: REST API (Axum)

**Module:** `crates/hypercolor-daemon/src/api/mod.rs`, `routes/`, `middleware.rs`
**Spec:** `docs/specs/10-rest-websocket-api.md` §1-13
**Depends on:** Wave 3

```rust
// Axum router with all endpoint groups
// Device endpoints: /api/v1/devices/*
// Effect endpoints: /api/v1/effects/*
// Scene endpoints: /api/v1/scenes/*
// Profile endpoints: /api/v1/profiles/*
// Layout endpoints: /api/v1/layouts/*
// System endpoints: /api/v1/status, /api/v1/health
// Standard envelope: { data, meta } / { error, meta }
// Rate limiting middleware, CORS, optional auth
```

**Tests:** Integration tests using `axum::test::TestClient` — every endpoint tested with mock state. JSON schema validation. Error format compliance. Rate limiting. No actual daemon needed.

### Task 4.2: WebSocket Server

**Module:** `crates/hypercolor-daemon/src/api/ws.rs`
**Spec:** `docs/specs/10-rest-websocket-api.md` §14
**Depends on:** Task 4.1

```rust
// WebSocket handler with hypercolor-v1 subprotocol
// Channel subscriptions (frames, events, audio, metrics)
// Binary frame protocol: 0x01 LED, 0x02 spectrum, 0x03 canvas
// JSON event relay from event bus
// Bidirectional command/response (REST-over-WS)
// Backpressure: bounded buffers, frame dropping
```

**Tests:** WebSocket connection lifecycle, subscription filtering, binary frame encoding/decoding, backpressure behavior under simulated slow consumer

### Task 4.3: CLI (clap)

**Module:** `crates/hypercolor-cli/src/main.rs`, `commands/`
**Spec:** `docs/specs/15-cli-commands.md` (entire spec)
**Depends on:** Wave 2

```rust
// Top-level Cli struct with clap derive
// Subcommand groups: status, devices, effects, scenes, profiles, layout, config, diagnose
// Global flags: --host, --port, --json, --quiet, --no-color, --verbose
// Output formatting: table, json, plain
// Shell completions via clap_complete
// IPC client for daemon communication
```

**Tests:** CLI argument parsing (assert all subcommands parse), help text generation, output formatting, shell completion generation. No daemon connection needed for parsing tests.

### Task 4.4: MCP Server

**Module:** `crates/hypercolor-daemon/src/mcp/mod.rs`, `tools.rs`, `resources.rs`
**Spec:** `docs/specs/11-mcp-server.md` (entire spec)
**Depends on:** Task 4.1

```rust
// MCP server via rmcp crate
// 14 tools: set_effect, list_effects, set_color, get_devices, etc.
// 5 resources: hypercolor://state, devices, effects, profiles, audio
// 3 prompt templates: mood_lighting, troubleshoot, setup_automation
// Natural language fuzzy matching for effect/color names
// stdio + SSE transports
```

**Tests:** Tool input/output schema validation, fuzzy matching accuracy (known inputs → expected matches), resource URI resolution, prompt template generation

### Task 4.5: Daemon Bootstrap

**Module:** `crates/hypercolor-daemon/src/main.rs`, `startup.rs`
**Spec:** `docs/specs/01-core-engine.md`, `docs/specs/12-configuration.md`
**Depends on:** Task 4.1, 4.2

```rust
// #[tokio::main] entry point
// Config loading + validation
// Backend discovery + initialization (feature-gated)
// RenderLoop setup + spawn
// API server startup (Axum)
// Signal handling (SIGTERM, SIGINT)
// Graceful shutdown sequence
```

**Tests:** Startup with mock backends, shutdown signal handling, config validation errors produce clean exits

---

## Wave 5: TUI + Polish

**Parallelism: 4 agents**
**Depends on:** Wave 4 (daemon functional)

### Task 5.1: TUI Framework

**Module:** `crates/hypercolor-tui/src/`
**Spec:** `docs/specs/15-cli-commands.md` §12
**Depends on:** Wave 4

```rust
// Ratatui application structure
// IPC client connection to daemon
// Main layout: sidebar + content + status bar
// Views: Dashboard, Devices, Effects, Audio, Layouts, Scenes, Settings
// Keyboard navigation: vim-style (hjkl), tab switching, command mode (:)
```

**Tests:** Widget rendering to test backend (ratatui::backend::TestBackend), keyboard event handling, view switching

### Task 5.2: Effect Registry + Package Format

**Module:** `crates/hypercolor-core/src/effect/registry.rs`, `package.rs`
**Spec:** `docs/specs/07-effect-system.md` §4-5, 9
**Depends on:** Wave 2

```rust
// EffectRegistry: scan directories, parse metadata, index by category/tag
// HTML meta tag parser for Servo effects
// .hyper package format: tar.gz with manifest.toml + assets
// Hot-reload: notify file watcher, debounced reload
// Effect search: fuzzy match on name, tags, description
```

**Tests:** Registry scan on test fixture directory, meta tag parsing from sample HTML, package creation/extraction roundtrip, search ranking

### Task 5.3: LightScript Compatibility Layer

**Module:** `crates/hypercolor-core/src/effect/compat/mod.rs`, `lightscript.rs`
**Spec:** `docs/specs/07-effect-system.md` §10
**Depends on:** Task 5.2

```rust
// LightScript API shim: legacy effect compatibility polyfill
// window.engine.audio injection matching the LightScript format
// Control decorator parsing (@Slider, @Toggle, @ColorPicker, etc.)
// HTML meta tag extraction for effect metadata
// Canvas 2D and WebGL context compatibility notes
```

**Tests:** JavaScript polyfill output validation, control decorator parsing from sample effects, audio API shape matching, known HTML effects loading metadata correctly

### Task 5.4: Desktop Integration

**Module:** `crates/hypercolor-daemon/src/desktop/mod.rs`
**Spec:** `docs/specs/13-scenes-automation.md` §7-8 (context detection)
**Depends on:** Wave 4

```rust
// D-Bus interface via zbus (Linux only, feature-gated)
// Active window detection for context triggers
// GameMode integration (Feral)
// System tray / applet hints
// Autostart via XDG autostart or systemd user unit
```

**Tests:** D-Bus interface definition validation, context detection with mock D-Bus responses

---

## Wave 6: Integration Testing + Packaging

**Parallelism: 3 agents**
**Depends on:** Wave 5

### Task 6.1: End-to-End Integration Tests

**Module:** `tests/integration/`
**Depends on:** All previous waves

```rust
// Full pipeline test: synthetic audio → effect → canvas → spatial → mock device
// Multi-backend test: WLED + HID simultaneously with frame synchronization
// Scene transition test: activate scene, verify transition frames, verify final state
// API test: start daemon with mock backends, exercise REST endpoints, verify WebSocket streams
// Config reload test: modify TOML, verify live reload propagates
```

### Task 6.2: Benchmarks

**Module:** `benches/`
**Depends on:** All previous waves

```rust
// Criterion benchmarks:
// - Canvas rendering: 1000 frames, measure mean/p95/p99
// - Spatial sampling: 100 zones × 20 LEDs, all 4 sampling modes
// - Audio FFT: 1024-point FFT pipeline, end-to-end latency
// - DDP packet construction: 1000 packets
// - Event bus throughput: 10k events/sec fan-out to 5 subscribers
// - JSON serialization: all API response types
```

### Task 6.3: Packaging + Distribution

**Files:** `Cargo.toml` metadata, `LICENSE-MIT`, `LICENSE-APACHE`, systemd unit, AUR PKGBUILD sketch
**Depends on:** All previous waves

```
// License files (MIT + Apache-2.0 dual license)
// Cargo.toml metadata: description, repository, homepage, keywords, categories
// systemd user unit: hypercolor.service
// Desktop entry: hypercolor.desktop
// Binary naming: hypercolor (daemon), hyper (cli)
// Feature flag documentation in README
```

---

## Task Dependency Graph

```
Wave 0 ─── Task 0.1 (scaffold)
               │
Wave 1 ───┬── Task 1.1 (canvas/color)
           ├── Task 1.2 (device types)
           ├── Task 1.3 (effect types)
           ├── Task 1.4 (audio types)
           ├── Task 1.5 (spatial types)
           ├── Task 1.6 (event types)
           ├── Task 1.7 (config types)
           └── Task 1.8 (scene types)
               │
Wave 2 ───┬── Task 2.1 (device traits)     ← 1.2
           ├── Task 2.2 (effect traits)     ← 1.1, 1.3, 1.4
           ├── Task 2.3 (event bus)         ← 1.6
           ├── Task 2.4 (spatial sampler)   ← 1.1, 1.5
           ├── Task 2.5 (config manager)    ← 1.7
           ├── Task 2.6 (render loop)       ← 2.1, 2.2, 2.3, 2.4
           └── Task 2.7 (input traits)      ← 1.4
               │
Wave 3 ───┬── Task 3.1 (WLED backend)      ← 2.1
           ├── Task 3.2 (USB HID backend)   ← 2.1
           ├── Task 3.4 (audio pipeline)    ← 2.7
           ├── Task 3.5 (screen capture)    ← 2.7
           └── Task 3.6 (scene engine)      ← 2.2, 2.3, 2.5
               │
Wave 4 ───┬── Task 4.1 (REST API)          ← Wave 3
           ├── Task 4.2 (WebSocket)         ← 4.1
           ├── Task 4.3 (CLI)              ← Wave 2
           ├── Task 4.4 (MCP server)        ← 4.1
           └── Task 4.5 (daemon bootstrap)  ← 4.1, 4.2
               │
Wave 5 ───┬── Task 5.1 (TUI)              ← Wave 4
           ├── Task 5.2 (effect registry)   ← Wave 2
           ├── Task 5.3 (LightScript compat) ← 5.2
           └── Task 5.4 (desktop integration) ← Wave 4
               │
Wave 6 ───┬── Task 6.1 (integration tests) ← All
           ├── Task 6.2 (benchmarks)        ← All
           └── Task 6.3 (packaging)         ← All
```

## Parallelism Summary

| Wave | Tasks | Parallel | Sequential Gates | Agent Count |
|------|-------|----------|-----------------|-------------|
| 0    | 1     | 0        | 1               | 1           |
| 1    | 8     | 8        | 0               | 8           |
| 2    | 7     | 6+1      | Task 2.6 waits  | 7           |
| 3    | 6     | 6        | 0               | 6           |
| 4    | 5     | 3+2      | 4.2→4.1, 4.5→4.1| 5           |
| 5    | 4     | 3+1      | 5.3→5.2         | 4           |
| 6    | 3     | 3        | 0               | 3           |
| **Total** | **34** | **~85% parallel** | | **34 agents** |

## Agent CLAUDE.md Contract

Every agent MUST follow these rules (enforced via workspace `CLAUDE.md`):

1. **Own your files.** Never modify files outside your assigned module.
2. **Test everything.** Every public function has at least one test. `cargo test` must pass.
3. **No warnings.** `cargo clippy -- -D warnings` must pass.
4. **No unsafe.** `#![forbid(unsafe_code)]` in `hypercolor-types`. Justify any `unsafe` elsewhere.
5. **Depend on types, not implementations.** Import from `hypercolor-types`, not from sibling modules.
6. **Mock external I/O.** Tests never touch network, USB, filesystem (except temp dirs), or audio hardware.
7. **Document public API.** All `pub` items get `///` doc comments.
8. **Use the specs.** Your assigned spec is the source of truth for types, field names, and behavior.

## Version Pinning

Core dependencies (pin in workspace `Cargo.toml`):

```toml
[workspace.dependencies]
tokio = { version = "1.44", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1.11", features = ["v7", "serde"] }
arc-swap = "1.7"

# Wave 2+
async-trait = "0.1"
notify = "7.0"
toml = "0.8"
dirs = "6.0"

# Wave 3
realfft = "3.4"
cpal = "0.15"
nusb = "0.1"
mdns-sd = "0.11"
tonic = "0.12"
prost = "0.13"
xcap = "0.0"

# Wave 4
axum = { version = "0.8", features = ["ws"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
clap = { version = "4.5", features = ["derive"] }
rmcp = "0.1"

# Wave 5
ratatui = "0.29"
crossterm = "0.28"
```
