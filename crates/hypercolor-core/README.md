# hypercolor-core

*The engine library — render loop, device backends, effects, and the event bus.*

This is the largest and most central crate in the workspace. It owns the
five-stage render pipeline, device backend abstraction, effect system,
`HypercolorBus` event bus, spatial sampler, input pipeline (audio, screen,
keyboard, sensors), scene and session management, and the optional Servo
HTML/Canvas effect renderer. The daemon, the CLI, the TUI, the driver bundle,
and the desktop app shell all build on top of this crate.

`hypercolor-core` re-exports `hypercolor-persistence` under the alias
`persistence`. Shared vocabulary is imported directly from `hypercolor-types`
and `hypercolor-color`.

## Workspace position

**Depends on:** the shared vocabulary crates (`hypercolor-color`,
`hypercolor-types`, `hypercolor-gpu-frame`), the driver boundary
(`hypercolor-driver-api`, `hypercolor-hal`), infrastructure
(`hypercolor-platform-fs`, `hypercolor-persistence`,
`hypercolor-worker-retention`), and one platform crate per capability seam:
host input (`linux-input`, `macos-input`, `windows-input`), screen capture
(`pipewire-interop`, `macos-capture`, `windows-capture`), GPU import
(`linux-gpu-interop`, `macos-gpu-interop`, `windows-gpu-interop`, all optional
behind `servo-gpu-import`), telemetry (`windows-telemetry`), and media
(`macos-media`, macOS target only). Every platform crate compiles on every
target via stubs.

**Depended on by:** `hypercolor-daemon`, `hypercolor-cli`, `hypercolor-tui`,
`hypercolor-driver-builtin`, `hypercolor-app`, and the three session crates
(`linux-session`, `windows-session`, `macos-session`), which decode native
events into the `SessionMonitor` seam defined here. The network driver crates
(`hue`, `nanoleaf`, `wled`, `govee`) deliberately do **not** depend on core;
they sit on `hypercolor-driver-api`.

## Key types and traits

**Render pipeline**

- `engine::RenderLoop` — drives the five-stage pipeline at adaptive FPS.
- `engine::FpsController`, `FpsTier` — auto-shifts between 10/20/30/45/60 fps
  tiers; downshifts fast on budget misses, upshifts slowly on sustained headroom.

**Device layer**

- `hypercolor_driver_api::DeviceBackend` is the hardware communication trait
  for discovery, connection, frame delivery, and disconnection.
- `device::manager::BackendManager` — device registry and frame dispatch.
- `device` — per-device lifecycle state machine types, re-exported from a
  private `state_machine` module.

**Effect system**

Everything below is re-exported at `effect::`; the submodules holding them are
private, so `effect::traits::…` and friends are not reachable paths.

- `effect::EffectRenderer` — polymorphic renderer interface, implemented by
  `ServoRenderer` and the CPU builtins in `effect::builtin`. **Send but not
  Sync** — must be wrapped in `Mutex`, not `RwLock`.
- `effect::FrameInput` — per-frame data struct passed to every render tick.
- `effect::EffectRenderOutput` — bridges CPU `Canvas` and GPU
  `ImportedEffectFrame` outputs.
- `effect::EffectRegistry` — catalog of all known effects.
- `effect::EffectPool` — manages active renderer instances per render zone.
- `effect::ServoRenderer` — HTML/Canvas renderer via Servo (feature-gated, see
  below).

**Event bus**

- `bus::HypercolorBus` — lock-free bus mixing broadcast (256-capacity) and
  watch channels. Carries `HypercolorEvent`, `FrameData`, `SpectrumData`,
  `CanvasFrame`. Use broadcast for discrete events; use watch for high-frequency
  data streams.
- `bus::FilteredEventReceiver`, `EventFilter` — typed event subscription.

**Spatial and input**

- `spatial::SpatialEngine` — maps canvas pixels to LED positions via a
  precomputed lookup table. Call `update_layout()` after topology changes.
- `input::ManagedSource` — polymorphic input: audio (CPAL, PipeWire,
  PulseAudio), screen capture (xdg-portal/PipeWire on Linux, ScreenCaptureKit on
  macOS, Desktop Duplication on Windows), host keyboard and pointer (evdev,
  CGEventTap, Raw Input), and sensors.

**Scene, session, config**

- `scene::SceneManager` — scene activation, priority, and transition management.
- `session::SessionMonitor` — session and power policy gating, fed by the
  platform session crates (logind and screensaver on Linux,
  `WM_POWERBROADCAST` plus WTS on Windows, NSWorkspace plus IOKit on macOS).
- `attachment::ComponentRegistry` — device-to-zone wiring profile management.
- `config::ConfigManager` — TOML config loader with file-watcher hot-reload.
- `blend_math` — public RGBA blending helpers used by the compositor.

## Feature flags

| Feature | What it gates |
|---|---|
| `servo` | Servo HTML/Canvas renderer. Pulls in the top-level `servo` API, the unreexported `base` memory-report callback, `dpi`, `gleam`, `hypercolor-gpu-frame/servo-context`, and `hypercolor-windows-gpu-interop/servo-window` for the Windows hidden-window CPU path. On Windows, uses the `no-wgl` Servo variant. |
| `servo-gpu-import` | Extends `servo` with zero-copy GPU texture import on all three platforms: `hypercolor-linux-gpu-interop` (GL/Vulkan external memory), `hypercolor-macos-gpu-interop` (IOSurface/Metal), `hypercolor-windows-gpu-interop` (D3D11/Vulkan). Also pulls in `wgpu`. |
| `media-lottie` | Lottie playback in the media-player renderer, via `rlottie`. |
| `media-video` | GStreamer video playback in the media-player renderer. |
| `default` | Empty — all features are opt-in. |

The remaining features (`macos-capture-fixtures`, `macos-native-fixtures`,
`windows-capture-fixtures`, `spatial-workspace-test-hooks`,
`persistence-test-hooks`, `allocation-contract-tests`) exist only for the test
suites.

NVML GPU telemetry via `nvml-wrapper` is always-on (no feature flag); it
gracefully degrades to no readings when an NVIDIA driver is not present. On
Windows, `wmi` is a required transitive dep for ACPI thermal zone queries.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
