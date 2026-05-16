# hypercolor-core

*The engine library — render loop, device backends, effects, and the event bus.*

This is the largest and most central crate in the workspace. It owns the
five-stage render pipeline, device backend abstraction, effect system,
`HypercolorBus` event bus, spatial sampler, input pipeline (audio, screen,
keyboard, sensors), scene and session management, and the optional Servo
HTML/Canvas effect renderer. The daemon, network drivers, and plugin crates
all build on top of this crate.

`hypercolor-core` re-exports `hypercolor-types` under the alias `types`,
so downstream crates can access the shared vocabulary through a single
import path.

## Workspace position

**Depends on:** `hypercolor-types`, `hypercolor-driver-api`, `hypercolor-hal`;
optionally `hypercolor-linux-gpu-interop` (feature `servo-gpu-import`).

**Depended on by:** `hypercolor-daemon`, `hypercolor-cli`, `hypercolor-tray`,
`hypercolor-tui`, all network driver crates (`hue`, `nanoleaf`, `wled`,
`govee`), `hypercolor-driver-builtin`, `hypercolor-app`, `hypercolor-network`.

## Key types and traits

**Render pipeline**

- `engine::RenderLoop` — drives the five-stage pipeline at adaptive FPS.
- `engine::FpsController`, `FpsTier` — auto-shifts between 10/20/30/45/60 fps
  tiers; downshifts fast on budget misses, upshifts slowly on sustained headroom.

**Device layer**

- `device::traits::DeviceBackend` — hardware communication trait: `discover`,
  `connect`, `write_colors`, `disconnect`.
- `device::traits::DevicePlugin` — lifecycle hooks for backend registration.
- `device::manager::BackendManager` — device registry and frame dispatch.
- `device::state_machine` — per-device lifecycle state machine.

**Effect system**

- `effect::traits::EffectRenderer` — polymorphic renderer interface (wgpu and
  Servo both implement this). **Send but not Sync** — must be wrapped in
  `Mutex`, not `RwLock`.
- `effect::traits::FrameInput` — per-frame data struct passed to every render tick.
- `effect::traits::EffectRenderOutput` — bridges CPU `Canvas` and GPU
  `ImportedEffectFrame` outputs.
- `effect::registry::EffectRegistry` — catalog of all known effects.
- `effect::pool::EffectPool` — manages active renderer instances per render group.
- `effect::servo::renderer::ServoRenderer` — HTML/Canvas renderer via Servo
  (feature-gated, see below).

**Event bus**

- `bus::HypercolorBus` — lock-free bus mixing broadcast (256-capacity) and
  watch channels. Carries `HypercolorEvent`, `FrameData`, `SpectrumData`,
  `CanvasFrame`. Use broadcast for discrete events; use watch for high-frequency
  data streams.
- `bus::FilteredEventReceiver`, `EventFilter` — typed event subscription.

**Spatial and input**

- `spatial::SpatialEngine` — maps canvas pixels to LED positions via a
  precomputed lookup table. Call `update_layout()` after topology changes.
- `input::traits::InputSource` — polymorphic input: audio (CPAL/PipeWire/
  PulseAudio), screen capture (Wayland portal), keyboard (evdev), sensors.

**Scene, session, config**

- `scene::SceneManager` — scene activation, priority, and transition management.
- `session::SessionMonitor` — logind/screensaver session policy gating.
- `attachment::AttachmentRegistry` — device-to-zone wiring profile management.
- `config::ConfigManager` — TOML config loader with file-watcher hot-reload.
- `blend_math` — public RGBA blending helpers used by the compositor.

## Feature flags

| Feature | What it gates |
|---|---|
| `servo` | Servo HTML/Canvas renderer. Pulls in `servo`, `servo-base`, `profile_traits`, `dpi`, `gleam`, `tao`, `raw-window-handle`. On Windows, uses the `no-wgl` Servo variant. |
| `servo-gpu-import` | Extends `servo` with zero-copy GL-to-wgpu texture import via `hypercolor-linux-gpu-interop`. Linux only in practice. |
| `nvidia` | NVML GPU telemetry via `nvml-wrapper`. |
| `default` | Empty — all features are opt-in. |

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux. Licensed under Apache-2.0.
