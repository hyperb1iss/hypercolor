# Hypercolor Prior Art: Plugin Architecture, Effect Authoring, and WASM Plans

**Date:** April 2026  
**Scope:** Synthesis of existing design and specification documents  
**Purpose:** Complete prior knowledge before designing a new WASM effect loader

---

## Table of Contents

1. [Three-Phase Plugin Architecture](#1-three-phase-plugin-architecture)
2. [Phase 2 Effect-Renderer Concern](#2-phase-2-effect-renderer-concern)
3. [Current EffectRenderer Trait](#3-current-effectrenderer-trait)
4. [FrameInput Shape](#4-frameinput-shape)
5. [EffectMetadata and ControlValue](#5-effectmetadata-and-controlvalue)
6. [Current Native Effect Patterns](#6-current-native-effect-patterns)
7. [TypeScript SDK and HTML Effect Contract](#7-typescript-sdk-and-html-effect-contract)
8. [Open Questions from Prior Docs](#8-open-questions-from-prior-docs)
9. [Integration Points for WASM Effects](#9-integration-points-for-wasm-effects)
10. [What's Already Decided](#10-whats-already-decided)
11. [What's Unresolved and Open](#11-whats-unresolved-and-open)
12. [Key Load-Bearing Quotes](#12-key-load-bearing-quotes)

---

## 1. Three-Phase Plugin Architecture

### Overview

The plugin architecture is defined as three progressive phases, evolving from most to least constrained. This is **not** a migration path; each phase adds capability while prior mechanisms remain available.

**Source:** `docs/design/09-plugin-ecosystem.md:283–345`

### Phase 1: Compile-Time Trait Objects (Shipped)

**When:** Day one (current state).

**Mechanism:**

- Bevy-style `Plugin` trait
- All backends compiled into the binary
- Gated by Cargo feature flags
- Zero runtime overhead, full type safety
- Access to all Rust APIs

**Example:** Device backends (WLED, OpenRGB, HID) are Phase 1 trait implementations.

### Phase 2: Wasm Extensions (Future, Not Yet Implemented)

**When:** When first external contributor wants to write a plugin without forking the repo.

**Mechanism:**

- Wasmtime runtime with WIT (WebAssembly Interface Types) contracts
- Plugins compile to `wasm32-wasip2` and run sandboxed inside the daemon process
- Host grants capabilities (network, timers) through WIT interfaces
- Per-plugin manifest (`plugin.toml`) declares permissions and metadata

**Plugin Layout:**

```
~/.config/hypercolor/plugins/
├── govee-wifi/
│   ├── plugin.toml              # Manifest
│   ├── govee_wifi.wasm          # Compiled plugin
│   └── ui/
│       └── settings.js          # Optional web component
└── spotify-input/
    ├── plugin.toml
    └── spotify_input.wasm
```

**Manifest Format (`plugin.toml`):**

```toml
[plugin]
id = "govee-wifi"
name = "Govee WiFi LED Backend"
version = "0.2.1"
description = "Control Govee WiFi LED strips via LAN API"
author = "kai"
license = "MIT"
min_hypercolor = "0.3.0"

[capabilities]
type = "device-backend"
network = true
filesystem = false
usb = false

[capabilities.network]
allowed_hosts = ["*.local", "10.*", "192.168.*"]
allowed_ports = [4003]

[ui]
settings_panel = "ui/settings.js"
```

**WIT Interface:** See Section 4 of `docs/design/09-plugin-ecosystem.md` for complete WIT definitions.

### Phase 3: gRPC Process Bridge (Future, For GPL Isolation)

**When:** Needed from Day 1 for OpenRGB (GPL isolation). Escape hatch for non-Wasm languages or native USB requirements.

**Mechanism:**

- Plugins run as separate processes over Unix domain sockets using gRPC
- Same WIT-derived interfaces, serialized over protobuf
- Separate binary lifecycle management with health checks and restart logic

**Example:** `hypercolor-openrgb-bridge` (GPL-2.0) spawned as separate process; core daemon stays MIT/Apache-2.0.

---

## 2. Phase 2 Effect-Renderer Concern

### The Hot Path Risk

**Exact Quote from `docs/design/09-plugin-ecosystem.md:94`:**

> "A Wasm-based effect renderer would add latency on the hot path -- acceptable for some formats, not others."

This is the **critical concern** that drives the architecture decision to keep effect rendering in Phase 1 (compile-time) only.

### Why Effects Are Not Phase 2 Wasm

**Extension Point:** Device backends, input sources, integrations, color transforms can all be Phase 2 Wasm. **Effect rendering cannot** (in the current design).

**Reasoning:**

- Effects render at 60fps (hot path, <16.67ms per frame)
- Wasm adds per-frame IPC overhead and memory marshaling
- Some formats (static images, screen capture) are timing-critical
- Other formats (slow ambient effects) could tolerate Wasm latency

**Quote from `docs/design/02-effect-system.md` (estimated line 94):**

> "Best mechanism: Compile-time (these are performance-critical and deeply integrated)."

### The Nuance

The phrase "acceptable for some formats, not others" implies:

- **Not acceptable:** Native shaders (wgpu), screen capture, performance-critical effects
- **Possibly acceptable:** Web-based effects (HTML/Canvas via Servo), slow ambient effects, file-based effects

This distinction is **never fully resolved** in the docs. It remains an open design question (see Section 8).

---

## 3. Current EffectRenderer Trait

### Trait Definition

**Source:** `crates/hypercolor-core/src/effect/traits.rs:77–151`

```rust
pub trait EffectRenderer: Send {
    /// Initialize the renderer for the given effect.
    /// Called once when the effect transitions from `Loading` to `Initializing`.
    fn init(&mut self, metadata: &EffectMetadata) -> anyhow::Result<()>;

    /// Optional: Initialize with final canvas size.
    /// Backends that need presentation size before first frame override this.
    fn init_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> anyhow::Result<()> { ... }

    /// Produce a single frame into caller-owned target storage.
    /// Called once per render loop iteration while the effect is `Running`.
    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas) -> anyhow::Result<()>;

    /// Legacy convenience wrapper that allocates a fresh Canvas.
    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> { ... }

    /// Update a control parameter value.
    /// Called when a user adjusts a control, preset is loaded, or API pushes a value.
    fn set_control(&mut self, name: &str, value: &ControlValue);

    /// Optional auxiliary preview canvas (e.g., high-res source before sampling).
    fn preview_canvas(&self) -> Option<Canvas> { None }

    /// Tear down the renderer and release all resources.
    /// Called when the effect transitions to `Destroying`.
    fn destroy(&mut self);
}
```

### Lifecycle

**State Machine:**

```
Loading
  ↓
Initializing → init() or init_with_canvas_size()
  ↓
Running → render_into() called per frame, set_control() on user input
  ↓
Paused (optional transition state during crossfades)
  ↓
Destroying → destroy()
```

### Key Invariants

1. **Single-threaded:** `EffectRenderer: Send` but not `Sync` — one renderer per effect, no concurrent frame production
2. **Mutable per frame:** `render_into(&mut self, ...)` mutates internal state (e.g., accumulator, shader uniforms)
3. **Ownership:** Caller owns target `Canvas`; renderer fills it in-place
4. **Error handling:** Errors are fatal per-frame; engine may retry or transition to error state

---

## 4. FrameInput Shape

### Complete Definition

**Source:** `crates/hypercolor-core/src/effect/traits.rs:22–50`

```rust
pub struct FrameInput<'a> {
    /// Elapsed time in seconds since the effect was activated.
    pub time_secs: f32,

    /// Time delta since the previous frame, in seconds.
    pub delta_secs: f32,

    /// Monotonically increasing frame counter (starts at 0).
    pub frame_number: u64,

    /// Current audio analysis snapshot. Use `AudioData::silence()` when unavailable.
    pub audio: &'a AudioData,

    /// Host keyboard and mouse state for interactive HTML effects.
    pub interaction: &'a InteractionData,

    /// Latest screen-capture snapshot for screen-reactive effects.
    pub screen: Option<&'a ScreenData>,

    /// Latest system telemetry snapshot shared across all renderers.
    pub sensors: &'a SystemSnapshot,

    /// Target canvas width in pixels.
    pub canvas_width: u32,

    /// Target canvas height in pixels.
    pub canvas_height: u32,
}
```

### Minimum Viable Payload

A WASM effect loader **must serialize or share** these per frame:

- `time_secs: f32` — essential for animation
- `delta_secs: f32` — for framerate-independent motion
- `frame_number: u64` — for seeding randomness, periodic actions
- `audio: &AudioData` — optional, but required for audio-reactive effects (see types below)
- `canvas_width, canvas_height: u32` — canvas dimensions

**Optional, high-cost:**

- `interaction: &InteractionData` — keyboard/mouse state (large struct, add per-frame overhead)
- `screen: Option<&ScreenData>` — screen capture pixels (texture data, expensive to share)
- `sensors: &SystemSnapshot` — system telemetry

### AudioData Type

Defined in `hypercolor-types::audio`, passed by reference. Contains:

- Bass, mid, treble levels (0.0–1.0)
- Overall RMS level
- Beat detection pulse
- Full frequency spectrum (FFT)
- Harmonics and pitch information

**Important:** Audio is **always available** but may be silence. No runtime flag needed.

### Serialization Strategy

For WASM boundaries:

- **By value:** `time_secs, delta_secs, frame_number, canvas_width, canvas_height` (copy cost negligible)
- **By reference (shared buffer):** Audio spectrum, screen pixels (too large to copy per frame)
- **Optional:** Interaction, sensors (add them only if effect declares dependency)

---

## 5. EffectMetadata and ControlValue

### EffectMetadata Structure

**Source:** `crates/hypercolor-types/src/effect.rs:606–642`

Universal descriptor attached to every effect:

```rust
pub struct EffectMetadata {
    pub id: EffectId,                          // Unique UUID v7
    pub name: String,                          // Display name
    pub author: String,                        // Author/publisher
    pub version: String,                       // Semantic version
    pub description: String,                   // Max 200 chars
    pub category: EffectCategory,              // Enum: Ambient, Audio, Generative, Particle, Scenic, Interactive, Fun, Source, Utility, Display
    pub tags: Vec<String>,                     // Lowercase, hyphenated
    pub controls: Vec<ControlDefinition>,      // User-facing parameters
    pub presets: Vec<PresetTemplate>,          // Built-in snapshots
    pub audio_reactive: bool,                  // Expects audio payload
    pub screen_reactive: bool,                 // Expects screen capture payload
    pub source: EffectSource,                  // Native { path } | Html { path } | Shader { path }
    pub license: Option<String>,               // SPDX identifier
}
```

### ControlDefinition

A single user-facing parameter:

```rust
pub struct ControlDefinition {
    pub id: String,                            // Stable identifier
    pub name: String,                          // Display label
    pub kind: ControlKind,                     // Number, Boolean, Color, Combobox, Sensor, Hue, Area, Text, Rect, Other
    pub control_type: ControlType,             // Slider, Toggle, ColorPicker, GradientEditor, Dropdown, TextInput, Rect
    pub default_value: ControlValue,           // Initial value
    pub min: Option<f32>,                      // Numeric bounds
    pub max: Option<f32>,
    pub step: Option<f32>,                     // Quantization
    pub labels: Vec<String>,                   // Dropdown options
    pub group: Option<String>,                 // UI grouping
    pub tooltip: Option<String>,               // Help text
    pub aspect_lock: Option<f32>,              // For rect controls
    pub preview_source: Option<PreviewSource>, // ScreenCapture, WebViewport, EffectCanvas
    pub binding: Option<ControlBinding>,       // Optional sensor mapping
}
```

### ControlValue Enum

Runtime value of a control:

```rust
pub enum ControlValue {
    Float(f32),                      // Slider, Hue, Area
    Integer(i32),                    // Integer parameters
    Boolean(bool),                   // Toggle
    Color([f32; 4]),                 // RGBA, linear color space
    Gradient(Vec<GradientStop>),     // Multi-stop gradient
    Enum(String),                    // Dropdown selection
    Text(String),                    // Text input
    Rect(ViewportRect),              // Normalized rectangular viewport
}
```

### Serialization to JavaScript

WASM effects need to expose controls to the daemon. The `ControlValue::to_js_literal()` method converts values to JavaScript:

```rust
fn to_js_literal(&self) -> String {
    match self {
        Float(v) => v.to_string(),                              // "1.5"
        Boolean(v) => if *v { "true" } else { "false" }.into(),
        Color([r, g, b, _a]) => format!("\"#{r:02x}{g:02x}{b:02x}\""), // "#ff6ac1"
        Enum(v) | Text(v) => format!("\"{}\"", v),
        // ... etc
    }
}
```

---

## 6. Current Native Effect Patterns

### Built-in Effects

Located in `crates/hypercolor-core/src/effect/builtin/`. Each implements `EffectRenderer`.

| Effect           | File              | Pattern                    | Complexity |
| ---------------- | ----------------- | -------------------------- | ---------- |
| **Solid Color**  | `solid_color.rs`  | Stateless canvas fill      | 1/5        |
| **Breathing**    | `breathing.rs`    | Pulsing brightness         | 2/5        |
| **Rainbow**      | `rainbow.rs`      | Hue sweep loop             | 2/5        |
| **Gradient**     | `gradient.rs`     | Spatial color gradient     | 2/5        |
| **Color Wave**   | `color_wave.rs`   | Sinusoidal color animation | 3/5        |
| **Color Zones**  | `color_zones.rs`  | Per-zone control           | 3/5        |
| **Audio Pulse**  | `audio_pulse.rs`  | Audio-reactive burst       | 4/5        |
| **Screen Cast**  | `screen_cast.rs`  | Screen capture sampler     | 4/5        |
| **Calibration**  | `calibration.rs`  | Diagnostic patterns        | 2/5        |
| **Web Viewport** | `web_viewport.rs` | Servo HTML renderer        | 5/5        |

### Solid Color Renderer (Reference)

**Source:** `crates/hypercolor-core/src/effect/builtin/solid_color.rs`

```rust
pub struct SolidColorRenderer {
    color: [f32; 4],
    secondary_color: [f32; 4],
    brightness: f32,
    pattern: SolidPattern,      // Enum: Solid, VerticalSplit, HorizontalSplit, Checker, Quadrants
    position: f32,
    softness: f32,
    scale: f32,
}

impl EffectRenderer for SolidColorRenderer {
    fn init(&mut self, metadata: &EffectMetadata) -> Result<()> {
        // Set defaults from metadata.controls
        for control in &metadata.controls {
            match control.id.as_str() {
                "color" => self.color = /* extract from default_value */,
                "brightness" => self.brightness = /* ... */,
                _ => {}
            }
        }
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas) -> Result<()> {
        prepare_target_canvas(target, input.canvas_width, input.canvas_height);

        for y in 0..input.canvas_height {
            for x in 0..input.canvas_width {
                let nx = x as f32 / input.canvas_width as f32;
                let ny = y as f32 / input.canvas_height as f32;
                let mix = self.pattern_mix(nx, ny, w as f32, h as f32);
                let color = mix_colors(self.color, self.secondary_color, mix);
                target.set_pixel(x, y, color);
            }
        }
        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color" => self.color = value.as_color().unwrap_or_default(),
            "brightness" => self.brightness = value.as_f32().unwrap_or(1.0),
            _ => {}
        }
    }

    fn destroy(&mut self) {
        // No GPU resources to free; trivial cleanup
    }
}
```

### Canvas Type

Target canvas used by all renderers:

```rust
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u32>,  // RGBA8888 in linear color space
}

impl Canvas {
    fn set_pixel(&mut self, x: u32, y: u32, color: Rgba);
    fn fill(&mut self, color: Rgba);
    fn width(&self) -> u32;
    fn height(&self) -> u32;
}
```

---

## 7. TypeScript SDK and HTML Effect Contract

### SDK Overview

**Packages:** `@hypercolor/sdk` (library + CLI)  
**Focus:** Effect authoring for external developers  
**Status:** Implemented (Spec 21)

### Declarative Effect API

**Simplest shader effect (9 lines):**

```typescript
import { effect } from "@hypercolor/sdk";
import shader from "./fragment.glsl";

export default effect("Meteor Storm", shader, {
  speed: [1, 10, 5], // [min, max, default] → slider
  density: [10, 100, 50],
  trailLength: [10, 100, 60],
  glow: [10, 100, 65],
  palette: ["SilkCircuit", "Fire", "Ice", "Aurora", "Cyberpunk"], // string[] → combobox
});
```

**Canvas effect (stateless draw function):**

```typescript
import { canvas } from "@hypercolor/sdk";

export default canvas(
  "Particles",
  {
    speed: [1, 10, 5],
    count: [10, 500, 100],
    palette: ["SilkCircuit", "Fire", "Aurora"],
  },
  (ctx, time, { speed, count, palette }) => {
    ctx.clearRect(0, 0, 320, 200);
    for (let i = 0; i < count; i++) {
      const x = Math.sin(time * speed + i * 0.7) * 140 + 160;
      const y = Math.cos(time * speed * 0.8 + i * 1.1) * 80 + 100;
      ctx.fillStyle = palette(i / count); // Palette is a function in canvas context
      ctx.arc(x, y, 2, 0, Math.PI * 2);
      ctx.fill();
    }
  },
);
```

### Control System

**Shape-based type inference:**

- `[min, max, default]` → number slider
- `['Option1', 'Option2']` → dropdown (combobox)
- `true/false` → toggle
- `'#ff6ac1'` → color picker

**Magic names:**

- `speed` → auto-applies `normalizeSpeed()` exponential curve
- `palette` → in shaders: index; in canvas: color function

### HTML Output Contract

All effects compile to identical HTML structure:

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="hypercolor-version" content="1" />
    <title>Effect Name</title>
    <!-- Controls encoded as meta tags -->
    <meta
      property="speed"
      label="Speed"
      type="number"
      min="1"
      max="10"
      default="5"
    />
    <meta
      property="palette"
      label="Palette"
      type="combobox"
      default="SilkCircuit"
      values="SilkCircuit,Fire,Ice,Aurora,Cyberpunk"
    />
    <!-- More meta tags... -->
  </head>
  <body>
    <canvas id="exCanvas" width="320" height="200"></canvas>
    <script>
      /* Bundled effect code */
    </script>
  </body>
</html>
```

### Runtime Environment

Daemon injects `window.engine` before effect script runs:

```typescript
window.engine = {
    width: number,                  // Target LED width
    height: number,                 // Target LED height
    getControlValue(id: string): string | number | boolean,
    audio?: {                       // Present if audio-reactive
        freq: Float32Array,
        level: number,
        bass: number,
        mid: number,
        treble: number,
        // ... full AudioData API
    }
}
```

### Servo Renderer Integration

**Source:** `crates/hypercolor-core/src/effect/servo/renderer.rs`

The `ServoRenderer` struct:

1. Loads HTML from `EffectMetadata.source.Html { path }`
2. Creates a `ServoSessionHandle` (shared Servo worker)
3. Injects `window.engine` globals and control values
4. Per frame: enqueues JavaScript to push new frame data, polls for rendered canvas

**Lifecycle:**

```
init(metadata)
  → resolve HTML path
  → create ServoSessionHandle
  → load HTML into session
  → inject engine globals

render_into(input, target)
  → enqueue frame scripts (time, audio, controls, resize)
  → poll for completed render
  → copy canvas pixels to target

set_control(name, value)
  → store in HashMap
  → enqueued on next render

destroy()
  → close Servo session
  → release GPU resources
```

---

## 8. Open Questions from Prior Docs

### Unresolved in Spec 21

**Section 17: Open Questions**

1. **Tier 0 (pragma) generation path:** Should `.glsl` files compile directly to `.html`, or via intermediate TypeScript?
   - **Status:** Unresolved
   - **Impact:** Affects SDK CLI design

2. **Canvas effect dimensions:** Hardcoded 320x200 or user-configurable?
   - **Status:** Unresolved
   - **Impact:** Breaking change if changed post-1.0

3. **Palette function caching:** Oklab interpolation every call vs. pre-computed LUT?
   - **Status:** Unresolved (performance optimization)

4. **Audio auto-detection:** Should build script auto-flag effects as audio-reactive if they import `audio()`?
   - **Status:** Unresolved
   - **Impact:** Hidden metadata generation

5. **Shader validation at build time:** Compile shader and report uniform mismatches?
   - **Status:** Unresolved
   - **Impact:** Better DX if implemented

6. **Canvas noise function:** Should SDK provide Simplex noise for canvas effects?
   - **Status:** Out of scope (marked as potential future addition)

### Unresolved in Design Doc 09

**Section 3: Plugin Architecture**

- **When Phase 2 actually ships:** No timeline committed
- **WASM effect renderer acceptance criteria:** "Acceptable for some formats, not others" — never specified which formats qualify
- **Per-frame execution budget enforcement:** Fuel values are **example only**, not finalized
- **Hot-reload semantics for Wasm plugins:** How do long-running effects survive plugin restarts?
- **Plugin versioning and backward compatibility:** No strategy for breaking changes

### Unresolved in Design Doc 02

**Effect system gaps:**

- Native wgpu shader format details (WGSL vs. GLSL, vertex shader contract)
- Render pipeline slots for custom geometry or multi-pass effects
- Per-device effect scaling (e.g., 100 LEDs vs. 1000 LEDs)

---

## 9. Integration Points for WASM Effects

### Where WASM Effect Loader Lives

**Proposed module structure:**

```
crates/hypercolor-core/src/effect/
├── traits.rs                  ← EffectRenderer trait (fixed)
├── factory.rs                 ← create_renderer_for_metadata() entry point
├── builtin/                   ← native effects
├── servo/                      ← HTML renderer
├── wasm/                       ← **NEW: WASM effect backend**
│   ├── mod.rs                 ← WasmEffectRenderer struct
│   ├── loader.rs              ← Load + compile .wasm file
│   ├── linker.rs              ← WIT bindings to host
│   ├── state.rs               ← Per-instance plugin state
│   └── memory.rs              ← Shared memory / buffer management
└── paths.rs                   ← resolve_wasm_source_path() analogous to resolve_html_source_path()
```

### Factory Integration

Extend `create_renderer_for_metadata()` in `factory.rs`:

```rust
fn create_renderer_for_metadata_internal(
    metadata: &EffectMetadata,
) -> Result<Box<dyn EffectRenderer>> {
    match &metadata.source {
        EffectSource::Native { .. } => { /* existing */ }
        EffectSource::Html { .. } => { /* existing */ }
        EffectSource::Shader { .. } => { /* existing */ }
        EffectSource::Wasm { path } => {  // ← NEW VARIANT NEEDED
            create_wasm_renderer(metadata, path)
        }
    }
}
```

**Note:** `EffectSource` enum in `hypercolor-types` needs new variant.

### WIT Interface Binding

Wasmtime linker must provide:

```rust
pub struct WasmEffectHost {
    engine: wasmtime::Engine,
    linker: wasmtime::Linker<PluginState>,
    // Bound to WIT interfaces:
    // - host::log(level, message)
    // - host::get_config(key) → Option<String>
    // - host::set_config(key, value)
    // - host::emit_event(event_type, payload)
    // - timer::now_ms() → u64
    // - timer::sleep_ms(ms)
    // - audio::get_audio_data() → audio_sample
}
```

### Per-Frame Serialization

**Data crossing WASM boundary per frame:**

```rust
// Host → WASM (per frame)
let frame_packet = WasmFrameInput {
    time_secs: f32,
    delta_secs: f32,
    frame_number: u64,
    canvas_width: u32,
    canvas_height: u32,
    audio_buffer_ptr: u32,     // Shared memory offset
    audio_buffer_len: u32,     // Bytes available
    audio_sample: AudioSample,  // Copy of just the metadata + spectrum bins
    // interaction, screen, sensors: optional, gated by capability
};

// WASM → Host (per frame)
let render_result = WasmRenderOutput {
    status: RenderStatus,  // Ok, Error, Timeout
    canvas_ptr: u32,       // Shared memory offset where WASM wrote pixel data
    canvas_len: u32,       // Bytes written
    // or error message
};
```

---

## 10. What's Already Decided

### Locked-In Decisions (Do Not Re-Litigate)

1. **WIT as the contract language** — `docs/design/09-plugin-ecosystem.md:431`
   - "One source of truth, two transport mechanisms" (Wasm + gRPC)
   - WIT specs are authoritative; see Section 4 of plugin doc

2. **Wasmtime as the runtime** — `docs/design/09-plugin-ecosystem.md:287`
   - Zed-inspired extension model
   - Already used by Hypercolor for plugin system
   - `wasm32-wasip2` target

3. **Capability model with `plugin.toml`** — `docs/design/09-plugin-ecosystem.md:310–325`
   - Network, filesystem, USB capabilities declared in manifest
   - Host grants permissions explicitly
   - Enforced by Wasmtime WASI linker

4. **Per-frame execution budget (fuel-based)** — `docs/design/09-plugin-ecosystem.md:332–334`
   - Device backend `push_frame`: 8ms budget
   - Color transform `process`: 2ms budget
   - Input source `sample`: 4ms budget
   - Overrun → skip frame / passthrough / reuse last value (depends on extension point)

5. **Canvas dimensions: 320x200 for effects** — `docs/specs/21-sdk-effect-authoring.md:169, 31-effect-developer-experience.md:164`
   - Effects authored at 320x200 design size
   - Runtime injects actual `window.engine.width/height`
   - SDK's `scaleContext()` helper for legacy compatibility

6. **Phase 1 (compile-time) owns all shipping first-party effects** — `docs/design/09-plugin-ecosystem.md:281`
   - No breaking changes to plugin model before Phase 2 ships
   - All current builtin effects are native Rust

### Architectural Invariants (Respect These)

1. **Single-threaded per-effect renderer** — `EffectRenderer: Send, !Sync`
   - No concurrent frame production per effect
   - Simplifies state management

2. **Caller owns target canvas** — `render_into(&mut self, input, target: &mut Canvas)`
   - Renderer fills in-place; no allocation on hot path
   - Enables zero-copy pipeline for spatial sampler

3. **Controls are immutable across frame** — `set_control()` called between frames, not during
   - Renderer caches control value; applies on next `render_into()`
   - No race conditions

4. **Audio is always available** — `audio: &AudioData` always present in `FrameInput`
   - May be silence; no runtime flag needed
   - Simplifies audio-reactive effect logic

5. **Servo renderer is always available** (when feature-gated) — `[target.x86_64-unknown-linux-gnu] features = ["servo"]`
   - HTML effects are first-class citizens
   - Not gated behind experimental flag post-Phase-1

---

## 11. What's Unresolved and Open

### Effect-Facing ABI (Major Design Question)

**The core question the prior docs did NOT answer:**

If WASM effects ship, what is the **exact Rust-to-WASM boundary**?

**Options:**

1. **WIT-based (like plugins):** Effects are full WIT modules; host provides effect-specific interfaces
   - Pro: Consistent with plugin architecture
   - Con: Overhead; effects are simpler than backends

2. **Direct function pointer (like Servo):** Effect = entry point `fn render_frame(FrameInput, &mut Canvas)`
   - Pro: Minimal overhead; matches current EffectRenderer contract
   - Con: Not language-agnostic; breaks Rust-only assumption

3. **Hybrid:** Core rendering in native Rust, WASM-only for pure compute (shaders, transforms)
   - Pro: Hot path stays in Rust; WASM for color math / filtering only
   - Con: Splits effect development experience

**Current decision:** UNRESOLVED. Design doc says "acceptable for some formats, not others" but never specifies which.

### Per-Pixel vs. Per-Canvas Rendering

**Unknown:**

- Does WASM effect render per-pixel (iterate over LEDs)?
- Or does it render to an off-screen canvas buffer then sampler maps to LEDs?

**Current assumption (from SDK):**

- Effects render to 320x200 canvas
- Spatial sampler maps canvas colors → LED colors based on device layout
- This is how Servo effects work

**For WASM:** Same model applies, but needs verification that framebuffer sharing is acceptable.

### Hot Reload Semantics

**Not documented:**

- If a WASM effect is updated on disk during playback, what happens?
- Does daemon reload the .wasm and re-init?
- Or does it finish the current playback and reload on next effect activation?

**Current answer for Phase 1:** Not applicable (no hot reload; recompile + restart).

### SDK Shape for WASM Effects

**Unresolved:**

- Do WASM effects use the same TypeScript SDK (compiled to WASM)?
- Or do they use different SDKs (Rust, Go, C, etc.)?

**Current answer:** `docs/design/09-plugin-ecosystem.md:1085–1090` mentions "Go, TinyGo, Python" for plugins, but doesn't clarify if effects follow the same language-agnostic model.

### Control Schema Format for WASM

**Unknown:**

- Do WASM effects declare controls in the same `ControlDefinition` format?
- Or a simpler, more serializable format (JSON schema)?

**Current assumption:**

- Controls are declared in metadata
- Metadata is serialized as JSON or embedded in WASM custom sections
- Unclear how WASM effect author declares, validates, serializes controls

---

## 12. Key Load-Bearing Quotes

### 1. The Hot Path Concern (Most Important)

**Source:** `docs/design/09-plugin-ecosystem.md:94`

> "A Wasm-based effect renderer would add latency on the hot path -- acceptable for some formats, not others."

**Why it matters:** This is the decision that keeps effects in Phase 1 (compile-time). Any WASM effect loader must directly address this concern.

### 2. The Phase 2 Moment Trigger

**Source:** `docs/design/09-plugin-ecosystem.md:283`

> "When the first external contributor wants to write a plugin without forking the repo."

**Why it matters:** This defines what Phase 2 solves. WASM effects are NOT the initial Phase 2 scope; Phase 2 starts with device backends and input sources.

### 3. WIT as Contract Language

**Source:** `docs/design/09-plugin-ecosystem.md:431`

> "WIT (WebAssembly Interface Types) defines the contract between the host and Wasm plugins. These same interfaces inform the gRPC protobuf definitions -- one source of truth, two transport mechanisms."

**Why it matters:** If WASM effects are added, they must follow this pattern. No ad-hoc serialization.

### 4. Plugin Safety Model

**Source:** `docs/design/09-plugin-ecosystem.md:36`

> "Safety by default. A community plugin cannot crash the daemon, access the filesystem without permission, or exfiltrate user data. Wasm sandboxing and the permission model are non-negotiable. Trust is earned, not assumed."

**Why it matters:** WASM effect loader must enforce the same safety constraints. Effects are not special cases.

### 5. Progressive Complexity Principle

**Source:** `docs/design/09-plugin-ecosystem.md:34`

> "A simple device backend in Rust behind a feature flag should take an afternoon. A sandboxed Wasm plugin in Go should take a weekend. A gRPC bridge in Python should take an evening. Match the complexity to the author's ambition."

**Why it matters:** WASM effect authoring DX should be proportional to complexity. Simpler than device backends, similar to input sources.

### 6. Effect System Not Designed for WASM (Yet)

**Source:** `docs/design/02-effect-system.md` (estimated line 94, confirmed in plugin doc)

> "Best mechanism: Compile-time (these are performance-critical and deeply integrated)."

**Why it matters:** The current effect system design actively avoids WASM on hot path. Adding WASM effects is a **design change**, not an implementation detail.

### 7. Servo Compatibility Goal

**Source:** `docs/design/15-community-ecosystem.md:502` (from 15-community-ecosystem section of plugin doc quote output)

> "Lightscript compatibility. The only open-source engine that can run existing community HTML effects with minimal or no modification."

**Why it matters:** WASM effects should not break existing Servo-based effects. Both paths must coexist.

### 8. Canvas as the Universal Format

**Source:** `docs/specs/31-effect-developer-experience.md:170`

> "All four personas produce the same artifact: a standalone `.html` file that conforms to the effect contract."

**Why it matters:** WASM effects may need a similar universal format. Decision: WASM effects as binary artifacts (.wasm) or wrapped HTML?

### 9. HTML as Universal Format (Alternative)

**Source:** `docs/specs/31-effect-developer-experience.md:43-50`

> "All four personas [HTML Hacker, AI Prompter, TypeScript Dev, Shader Artist] produce the same artifact: a standalone `.html` file that conforms to the effect contract (section 3)."

**Why it matters:** If WASM effects are NOT wrapped in HTML, the universal format assumption breaks. Design choice needed.

### 10. Dependency Isolation (GPL Strategy)

**Source:** `docs/design/15-community-ecosystem.md:27-48`

> "GPL isolation is architectural, not philosophical. The `openrgb2` crate is GPL-2.0. Rather than debating license compatibility, we run it as a separate process (`hypercolor-openrgb-bridge`) communicating over gRPC/Unix socket. Clean boundary. No contamination."

**Why it matters:** WASM effects run in-process (unlike gRPC plugins). Ensure license contamination cannot happen; MIT/Apache-2.0 must stay pure.

---

## Summary: What We Know and What We Don't

### Known ✓

- Three-phase architecture (Phase 1 shipped, Phase 2/3 not yet)
- Effect hot path constraint (60fps, <16.67ms per frame)
- EffectRenderer trait contract (5 methods, lifecycle)
- FrameInput per-frame payload shape
- ControlDefinition and ControlValue types
- Current HTML effect contract (320x200 canvas, meta tags, JS globals)
- Current native effect patterns (10+ builtin effects)
- TypeScript SDK (implemented, Spec 21)
- WIT interface model (defined in design doc)
- Wasmtime + wasm32-wasip2 target (decided)

### Unknown / Unresolved ❓

- Whether WASM effect rendering is acceptable on hot path (or only for slow ambient effects)
- Effect-facing WASM ABI (WIT-based? Direct function pointers? Hybrid?)
- Per-frame data serialization strategy (shared memory buffers? Value copies? Async?)
- SDK shape for WASM effect authors (TypeScript compiled to WASM? Rust? Multi-language?)
- Control schema serialization for WASM effects
- Hot reload semantics
- Whether WASM effects wrap in HTML or ship as raw .wasm binaries
- License safety model (how to prevent GPL contamination in in-process WASM)

---

**Synthesis complete. This document is ready for design session input.**
