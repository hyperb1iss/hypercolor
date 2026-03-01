# 01 -- Core Engine Technical Specification

> The beating heart. Every type, trait, and thread boundary that makes Hypercolor render at 60fps.

**Status:** Implementation-ready
**Crate:** `hypercolor-core`
**Module path:** `hypercolor_core::{canvas, render, effect, frame, input, timer}`

---

## Table of Contents

1. [Canvas](#1-canvas)
2. [RenderLoop](#2-renderloop)
3. [EffectEngine Trait](#3-effectengine-trait)
4. [FrameData](#4-framedata)
5. [InputData](#5-inputdata)
6. [FrameTimer](#6-frametimer)
7. [Thread Model](#7-thread-model)
8. [Cross-Platform Considerations](#8-cross-platform-considerations)

---

## 1. Canvas

The canvas is the universal pixel surface. Both render paths (wgpu and Servo) produce one. The spatial sampler consumes one. Everything in between is just math on a flat RGBA buffer.

### 1.1 Core Type

```rust
use serde::{Deserialize, Serialize};

/// The canonical render surface for all effects.
///
/// A 2D RGBA pixel buffer at a fixed resolution (default 320x200).
/// Both the wgpu and Servo render paths write to this format.
/// The spatial sampler reads from it to produce LED colors.
///
/// Memory layout: row-major, top-left origin, 4 bytes per pixel (R, G, B, A).
/// Total size at 320x200: 256,000 bytes (250 KB).
#[derive(Clone)]
pub struct Canvas {
    /// Horizontal pixel count.
    width: u32,

    /// Vertical pixel count.
    height: u32,

    /// Row-major RGBA pixel data.
    /// Length is always `width * height * 4`.
    /// Invariant: `pixels.len() == (width * height * 4) as usize`.
    pixels: Vec<u8>,
}

/// The default canvas resolution, matching SignalRGB's standard.
pub const DEFAULT_CANVAS_WIDTH: u32 = 320;
pub const DEFAULT_CANVAS_HEIGHT: u32 = 200;

/// Bytes per pixel in the RGBA format.
pub const BYTES_PER_PIXEL: usize = 4;
```

### 1.2 Pixel Access

```rust
/// A single pixel value in linear sRGB space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Intermediate floating-point pixel for interpolation math.
/// Values are 0.0..=1.0 per channel. Clamped on conversion back to `Rgba`.
#[derive(Debug, Clone, Copy)]
pub struct RgbaF32 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Canvas {
    /// Create a new canvas filled with opaque black.
    pub fn new(width: u32, height: u32) -> Self { /* ... */ }

    /// Create from a raw RGBA byte slice. Panics if length != width * height * 4.
    pub fn from_rgba(data: &[u8], width: u32, height: u32) -> Self { /* ... */ }

    /// Wrap an existing Vec without copying. Takes ownership.
    /// Panics if `data.len() != (width * height * 4) as usize`.
    pub fn from_vec(data: Vec<u8>, width: u32, height: u32) -> Self { /* ... */ }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }

    /// Raw pixel slice for zero-copy handoff to the spatial sampler.
    pub fn as_rgba_bytes(&self) -> &[u8] { &self.pixels }

    /// Mutable pixel slice for renderers writing directly into the buffer.
    pub fn as_rgba_bytes_mut(&mut self) -> &mut [u8] { &mut self.pixels }

    /// Read a single pixel. Returns opaque black for out-of-bounds coords.
    pub fn get_pixel(&self, x: u32, y: u32) -> Rgba { /* ... */ }

    /// Write a single pixel. No-op for out-of-bounds coords.
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Rgba) { /* ... */ }

    /// Fill the entire canvas with a single color.
    pub fn fill(&mut self, color: Rgba) { /* ... */ }

    /// Reset to opaque black. Reuses the existing allocation.
    pub fn clear(&mut self) { /* ... */ }
}
```

### 1.3 Sampling Methods

The spatial sampler calls these to read canvas colors at sub-pixel LED positions. Three interpolation strategies serve different quality/performance tradeoffs.

```rust
/// Interpolation strategy for canvas sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SamplingMethod {
    /// Snap to the nearest pixel. Fastest, but aliased at low LED density.
    Nearest,

    /// Weighted average of the 4 surrounding pixels.
    /// Default. Good balance of quality and speed.
    Bilinear,

    /// Average all pixels within a rectangular area centered on the sample point.
    /// Best quality for zones that span many canvas pixels (e.g., a single LED
    /// covering a 20x20 pixel region). Slowest.
    Area {
        /// Half-width of the sample area in canvas pixels.
        /// A value of 5.0 samples an 11x11 pixel box.
        radius: f32,
    },
}

impl Canvas {
    /// Sample the canvas at normalized coordinates (0.0..=1.0).
    ///
    /// `nx` and `ny` are in [0.0, 1.0] where (0,0) is top-left
    /// and (1,1) is bottom-right. Values outside this range are clamped.
    ///
    /// Returns an `Rgba` pixel using the specified interpolation method.
    pub fn sample(&self, nx: f32, ny: f32, method: SamplingMethod) -> Rgba { /* ... */ }

    /// Sample with nearest-neighbor interpolation.
    ///
    /// Snaps `(nx, ny)` to the closest integer pixel coordinate.
    /// Cost: 1 pixel read.
    pub fn sample_nearest(&self, nx: f32, ny: f32) -> Rgba {
        let x = (nx * (self.width - 1) as f32).round() as u32;
        let y = (ny * (self.height - 1) as f32).round() as u32;
        self.get_pixel(x.min(self.width - 1), y.min(self.height - 1))
    }

    /// Sample with bilinear interpolation.
    ///
    /// Reads the 4 pixels surrounding the fractional coordinate and blends
    /// by distance. Produces smooth gradients between pixels.
    /// Cost: 4 pixel reads + 12 multiplies.
    pub fn sample_bilinear(&self, nx: f32, ny: f32) -> Rgba {
        let fx = nx * (self.width - 1) as f32;
        let fy = ny * (self.height - 1) as f32;

        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);

        let frac_x = fx.fract();
        let frac_y = fy.fract();

        let tl = self.get_pixel(x0, y0).to_f32();
        let tr = self.get_pixel(x1, y0).to_f32();
        let bl = self.get_pixel(x0, y1).to_f32();
        let br = self.get_pixel(x1, y1).to_f32();

        // Horizontal lerp, then vertical lerp
        let top = RgbaF32::lerp(&tl, &tr, frac_x);
        let bot = RgbaF32::lerp(&bl, &br, frac_x);
        RgbaF32::lerp(&top, &bot, frac_y).to_rgba()
    }

    /// Sample with area averaging.
    ///
    /// Averages all pixels within a `(2*radius+1)` square centered on the
    /// sample point. Best for low-resolution LED zones that map to large
    /// canvas regions -- prevents moire patterns and aliasing.
    /// Cost: `(2*radius+1)^2` pixel reads.
    pub fn sample_area(&self, nx: f32, ny: f32, radius: f32) -> Rgba {
        let cx = nx * (self.width - 1) as f32;
        let cy = ny * (self.height - 1) as f32;

        let r = radius.ceil() as i32;
        let mut sum_r = 0u32;
        let mut sum_g = 0u32;
        let mut sum_b = 0u32;
        let mut sum_a = 0u32;
        let mut count = 0u32;

        for dy in -r..=r {
            for dx in -r..=r {
                let px = (cx as i32 + dx).clamp(0, self.width as i32 - 1) as u32;
                let py = (cy as i32 + dy).clamp(0, self.height as i32 - 1) as u32;
                let p = self.get_pixel(px, py);
                sum_r += p.r as u32;
                sum_g += p.g as u32;
                sum_b += p.b as u32;
                sum_a += p.a as u32;
                count += 1;
            }
        }

        Rgba {
            r: (sum_r / count) as u8,
            g: (sum_g / count) as u8,
            b: (sum_b / count) as u8,
            a: (sum_a / count) as u8,
        }
    }
}
```

### 1.4 Conversions

```rust
impl Rgba {
    pub const BLACK: Rgba = Rgba { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Rgba = Rgba { r: 255, g: 255, b: 255, a: 255 };
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };

    /// Convert to floating-point representation for interpolation.
    pub fn to_f32(self) -> RgbaF32 {
        RgbaF32 {
            r: self.r as f32 / 255.0,
            g: self.g as f32 / 255.0,
            b: self.b as f32 / 255.0,
            a: self.a as f32 / 255.0,
        }
    }

    /// Extract RGB only, discarding alpha.
    pub fn to_rgb(self) -> Rgb {
        Rgb { r: self.r, g: self.g, b: self.b }
    }
}

impl RgbaF32 {
    /// Linear interpolation between two colors.
    pub fn lerp(a: &RgbaF32, b: &RgbaF32, t: f32) -> RgbaF32 {
        RgbaF32 {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        }
    }

    /// Convert back to byte representation, clamping to [0, 255].
    pub fn to_rgba(self) -> Rgba {
        Rgba {
            r: (self.r * 255.0).clamp(0.0, 255.0) as u8,
            g: (self.g * 255.0).clamp(0.0, 255.0) as u8,
            b: (self.b * 255.0).clamp(0.0, 255.0) as u8,
            a: (self.a * 255.0).clamp(0.0, 255.0) as u8,
        }
    }
}

/// Device-facing RGB color (no alpha). This is what backends receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
```

---

## 2. RenderLoop

The render loop is the frame pipeline. It sequences five stages within a 16.6ms budget, adapts to system load, and never blocks on device output.

### 2.1 Pipeline Stages

```
Sample Inputs --> Render Effect --> Sample Canvas --> Push Devices --> Publish Bus
   (1.0ms)         (8.0ms)          (0.5ms)          (2.0ms)         (0.1ms)
                                                                    [5.0ms slack]
```

### 2.2 Core Type

```rust
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch, mpsc};

/// The main render loop orchestrator.
///
/// Owns the effect engine, spatial sampler, device backends, and input sources.
/// Runs on a dedicated OS thread (pinned to a core) with its own tokio
/// `current_thread` runtime for async device I/O dispatch.
///
/// The loop never blocks on device output -- backends receive color data
/// via bounded mpsc channels and transmit asynchronously.
pub struct RenderLoop {
    /// The active effect renderer (wgpu or Servo).
    effect_engine: Box<dyn EffectEngine>,

    /// Spatial layout describing all device zones and LED positions.
    layout: SpatialLayout,

    /// Sampling method used for canvas-to-LED mapping.
    sampling_method: SamplingMethod,

    /// Per-backend output channels. The render loop sends `DeviceColors`
    /// to each backend's task via bounded mpsc.
    backend_sinks: Vec<BackendSink>,

    /// Aggregated input sources polled each frame.
    input_sources: InputAggregator,

    /// Event bus for fan-out to all frontends.
    bus: HypercolorBus,

    /// Frame timing and budget tracking.
    timer: FrameTimer,

    /// Double-buffered canvas: one for current render, one for readback.
    canvas: Canvas,

    /// Current performance tier.
    fps_tier: FpsTier,

    /// Monotonically increasing frame counter.
    frame_number: u64,

    /// The current render loop state (running, paused, suspended).
    state: RenderLoopState,
}

/// A channel endpoint for dispatching colors to a single device backend.
pub struct BackendSink {
    /// Human-readable backend name (e.g., "wled-living-room").
    pub name: String,

    /// Bounded mpsc sender. If the backend is slow, the oldest frame is dropped
    /// (latest-wins semantics achieved by the backend draining the channel).
    pub tx: mpsc::Sender<Vec<DeviceColors>>,
}

/// Render loop lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderLoopState {
    /// Actively rendering frames at the current FPS tier.
    Running,

    /// Paused during effect switch or configuration change.
    /// Last frame held on all devices.
    Paused,

    /// System suspended (sleep/hibernate). No output, no rendering.
    Suspended,
}
```

### 2.3 Adaptive FPS Tiers

```rust
/// Performance tiers that control target frame rate.
///
/// The system automatically shifts between tiers based on system load,
/// GameMode signals, battery state, and consecutive frame budget misses.
///
/// Downshift is fast (2 consecutive misses). Upshift is slow (5-10 seconds
/// of sustained headroom) to prevent oscillation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum FpsTier {
    /// 60 fps, 16.6ms budget. Desktop idle, light applications.
    Full = 0,

    /// 30 fps, 33.3ms budget. Game detected, moderate GPU/CPU load.
    Gaming = 1,

    /// 15 fps, 66.6ms budget. Heavy system load, laptop on battery.
    Economy = 2,

    /// 5 fps, 200ms budget. Screen off, idle breathing effect.
    Standby = 3,

    /// 0 fps. System sleep, no connected devices, daemon backgrounded.
    Suspended = 4,
}

impl FpsTier {
    /// Target frame interval for this tier.
    pub fn frame_interval(&self) -> Duration {
        match self {
            FpsTier::Full => Duration::from_micros(16_666),
            FpsTier::Gaming => Duration::from_micros(33_333),
            FpsTier::Economy => Duration::from_micros(66_666),
            FpsTier::Standby => Duration::from_millis(200),
            FpsTier::Suspended => Duration::MAX,
        }
    }

    /// Target FPS as a human-readable integer.
    pub fn fps(&self) -> u32 {
        match self {
            FpsTier::Full => 60,
            FpsTier::Gaming => 30,
            FpsTier::Economy => 15,
            FpsTier::Standby => 5,
            FpsTier::Suspended => 0,
        }
    }
}

/// Tier transition thresholds and hysteresis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierTransitionConfig {
    /// Consecutive frame misses to trigger immediate downshift.
    pub downshift_miss_threshold: u32,            // default: 2

    /// Seconds of sustained headroom before upshifting.
    pub upshift_sustain_seconds: f64,             // default: 5.0

    /// GPU usage percent above which Gaming tier is recommended.
    pub gpu_gaming_threshold: f32,                // default: 70.0

    /// CPU usage percent above which Gaming tier is recommended.
    pub cpu_gaming_threshold: f32,                // default: 80.0

    /// Seconds after GameMode deactivates before upshifting from Gaming.
    pub gamemode_cooldown_seconds: f64,           // default: 10.0
}
```

### 2.4 Frame Pipeline Contract

```rust
impl RenderLoop {
    /// Execute one frame of the render pipeline.
    ///
    /// Called in a tight loop by the render thread. Returns the duration
    /// to sleep before the next frame (may be `Duration::ZERO` if behind).
    ///
    /// # Pipeline stages
    ///
    /// 1. **Input sampling** -- Read latest audio, screen, keyboard data
    ///    from lock-free triple buffers. Budget: 1.0ms.
    ///
    /// 2. **Effect rendering** -- Dispatch to the active `EffectEngine`.
    ///    Produces an RGBA canvas. Budget: 8.0ms.
    ///
    /// 3. **Spatial sampling** -- Walk all LED positions in the layout,
    ///    sample the canvas at each position. Budget: 0.5ms.
    ///
    /// 4. **Device output** -- Dispatch `DeviceColors` to each backend's
    ///    mpsc channel. Non-blocking fire-and-forget. Budget: 2.0ms.
    ///
    /// 5. **Bus publish** -- `watch::Sender::send_replace` with the new
    ///    `FrameData`. Single atomic swap. Budget: 0.1ms.
    ///
    /// Remaining time is slack (~5.0ms) absorbing variance from GC pauses,
    /// USB scheduling, and OS thread jitter.
    pub async fn tick(&mut self) -> Duration {
        self.timer.begin_frame();
        self.frame_number += 1;

        // Stage 1: Input sampling
        self.timer.begin_stage(PipelineStage::InputSampling);
        let input_data = self.input_sources.sample();
        self.timer.end_stage(PipelineStage::InputSampling);

        // Stage 2: Effect rendering
        self.timer.begin_stage(PipelineStage::EffectRendering);
        self.effect_engine.render(&input_data, &mut self.canvas).await;
        self.timer.end_stage(PipelineStage::EffectRendering);

        // Stage 3: Spatial sampling
        self.timer.begin_stage(PipelineStage::SpatialSampling);
        let device_colors = self.sample_all_zones();
        self.timer.end_stage(PipelineStage::SpatialSampling);

        // Stage 4: Device output dispatch (non-blocking)
        self.timer.begin_stage(PipelineStage::DeviceOutput);
        self.dispatch_to_backends(&device_colors).await;
        self.timer.end_stage(PipelineStage::DeviceOutput);

        // Stage 5: Bus publish
        self.timer.begin_stage(PipelineStage::BusPublish);
        let frame_data = FrameData::from_device_colors(
            self.frame_number,
            &device_colors,
        );
        self.bus.frame.send_replace(frame_data);
        self.timer.end_stage(PipelineStage::BusPublish);

        self.timer.frame_complete()
    }
}
```

### 2.5 Skip Strategy

```rust
/// Decision on which pipeline stages to skip when over budget.
///
/// Priority: device output and bus publish are *never* skipped.
/// The render stage is the most expensive and the first to be skipped,
/// reusing the previous frame's canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipDecision {
    /// Execute all stages normally.
    None,

    /// Reuse previous input data (audio, screen). Save ~1ms.
    ReuseInputs,

    /// Reuse previous canvas entirely. Skip effect render + inputs.
    /// Only spatial sample (if layout changed) and device output run.
    ReuseCanvas,
}
```

---

## 3. EffectEngine Trait

The trait that both render paths implement. One interface, two worlds: wgpu shaders running in microseconds, Servo pages running in milliseconds. The `RenderLoop` is polymorphic over this trait.

### 3.1 Trait Definition

```rust
use std::fmt;

/// The interface every render backend must implement.
///
/// An `EffectEngine` receives aggregated input data and writes RGBA pixels
/// into a `Canvas`. The render loop calls `render()` once per frame and
/// expects the canvas to be fully updated when the call returns.
///
/// # Implementors
///
/// - `WgpuRenderer` -- Native WGSL/GLSL shaders via wgpu. Sub-millisecond
///   frame times at 320x200. GPU pixel readback via async staging buffer.
///
/// - `ServoRenderer` -- Embedded Servo browser engine running HTML/Canvas/WebGL
///   effects. 3-10ms frame times. Provides full Lightscript API compatibility
///   for existing SignalRGB effects.
///
/// # Thread Safety
///
/// Implementations are `Send` but *not* required to be `Sync`. The render loop
/// owns the engine exclusively -- no concurrent access.
#[async_trait::async_trait]
pub trait EffectEngine: Send {
    /// Human-readable name of this renderer for logging and metrics.
    fn name(&self) -> &str;

    /// Which renderer type this is (for metrics labels and config).
    fn renderer_type(&self) -> RendererType;

    /// Load an effect by its resolved path or identifier.
    ///
    /// For wgpu: compiles the WGSL shader and creates the render pipeline.
    /// For Servo: navigates to the HTML file and waits for initial render.
    ///
    /// Returns the effect's metadata (controls, categories, audio reactivity).
    async fn load_effect(&mut self, effect: &EffectSource) -> Result<EffectMetadata, EffectError>;

    /// Render one frame into the provided canvas.
    ///
    /// The engine reads from `input` (audio, time, controls) and writes
    /// RGBA pixels into `canvas`. The canvas dimensions are guaranteed
    /// stable across frames -- they only change on explicit resize.
    ///
    /// # Performance contract
    ///
    /// - wgpu path: target <1ms, hard limit 5ms
    /// - Servo path: target <5ms, hard limit 12ms
    async fn render(&mut self, input: &InputData, canvas: &mut Canvas);

    /// Update a user-controlled parameter.
    ///
    /// For Servo: injects `window['name'] = value; window.update?.();`
    /// For wgpu: updates the corresponding uniform buffer field.
    fn set_control(&mut self, name: &str, value: &ControlValue);

    /// Release all resources held by the current effect.
    ///
    /// Called before loading a new effect and during shutdown.
    /// For wgpu: destroys the render pipeline (device/queue persist).
    /// For Servo: navigates to `about:blank` and triggers aggressive GC.
    fn unload(&mut self);

    /// Whether this engine is currently healthy and producing frames.
    fn is_healthy(&self) -> bool;
}

/// Identifies which render backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RendererType {
    /// Native GPU shaders via wgpu (Vulkan/OpenGL/Metal).
    Wgpu,

    /// Embedded Servo browser engine (HTML/Canvas/WebGL).
    Servo,
}

impl fmt::Display for RendererType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RendererType::Wgpu => write!(f, "wgpu"),
            RendererType::Servo => write!(f, "servo"),
        }
    }
}
```

### 3.2 Effect Source

```rust
/// Resolved location and type of an effect to load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EffectSource {
    /// A WGSL shader file with an accompanying TOML metadata file.
    Wgsl {
        /// Absolute path to the `.wgsl` shader file.
        shader_path: std::path::PathBuf,
        /// Absolute path to the `.toml` metadata file.
        metadata_path: std::path::PathBuf,
    },

    /// An HTML file to be loaded in the Servo renderer.
    Html {
        /// Absolute path to the `.html` effect file.
        html_path: std::path::PathBuf,
    },

    /// A built-in effect identified by name (compiled into the binary).
    Builtin {
        /// Effect name, e.g. "solid-color", "rainbow".
        name: String,
    },
}
```

### 3.3 Effect Errors

```rust
/// Errors that can occur during effect lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum EffectError {
    /// WGSL shader failed to compile.
    #[error("shader compilation failed: {message}")]
    ShaderCompilation { message: String },

    /// Effect file not found or unreadable.
    #[error("effect not found: {path}")]
    NotFound { path: String },

    /// TOML metadata is malformed or missing required fields.
    #[error("invalid effect metadata: {reason}")]
    InvalidMetadata { reason: String },

    /// Servo failed to load the HTML page.
    #[error("servo navigation failed: {message}")]
    ServoNavigation { message: String },

    /// The effect exceeded its frame budget for too many consecutive frames.
    #[error("effect timed out after {consecutive_overruns} consecutive overruns")]
    Timeout { consecutive_overruns: u32 },

    /// GPU device was lost (driver crash, sleep/resume).
    #[error("GPU device lost: {message}")]
    DeviceLost { message: String },

    /// Catch-all for unexpected failures.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
```

---

## 4. FrameData

The LED color output published on the event bus. This is the data structure that every frontend (web UI, TUI, CLI) receives via `watch::Receiver<FrameData>`.

### 4.1 Core Types

```rust
/// A snapshot of all LED colors for one rendered frame.
///
/// Published to the event bus via `watch::Sender::send_replace()` every frame.
/// Subscribers receive only the latest value -- stale frames are automatically
/// skipped, which is exactly the semantics we want for live LED preview.
///
/// Serializable for WebSocket streaming to the web UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameData {
    /// Monotonically increasing frame counter.
    pub frame_number: u64,

    /// Timestamp when this frame was rendered (milliseconds since daemon start).
    pub timestamp_ms: u64,

    /// Per-zone color data, ordered by zone registration order.
    pub zones: Vec<ZoneColors>,

    /// Active FPS tier at the time of this frame.
    pub fps_tier: FpsTier,

    /// Active renderer type that produced this frame.
    pub renderer: RendererType,
}

/// LED colors for a single device zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneColors {
    /// Unique device identifier (matches `DeviceInfo::id`).
    pub device_id: String,

    /// Zone name within the device (e.g., "Channel 1", "ATX Strimer").
    pub zone_name: String,

    /// RGB color for each LED in this zone, in device-native order.
    /// Length matches the zone's LED count from the spatial layout.
    pub colors: Vec<Rgb>,
}

/// Color data grouped by device for backend dispatch.
///
/// The render loop produces one of these per device per frame.
/// Backends receive their own `DeviceColors` and transmit to hardware.
#[derive(Debug, Clone)]
pub struct DeviceColors {
    /// Device identifier for routing to the correct backend.
    pub device_id: String,

    /// Zone name for devices with multiple addressable zones.
    pub zone_name: String,

    /// The LED colors in device-native order.
    pub colors: Vec<Rgb>,
}
```

### 4.2 WebSocket Serialization

```rust
impl FrameData {
    /// Encode as a compact binary frame for WebSocket streaming.
    ///
    /// Format: 8-byte header (frame_number: u64) followed by packed RGB triplets
    /// for every LED across all zones, in zone order. No zone boundaries in the
    /// binary format -- the client knows the layout.
    ///
    /// At 2000 LEDs: 8 + (2000 * 3) = 6008 bytes per frame.
    /// At 30fps WebSocket rate: ~180 KB/s. Negligible bandwidth.
    pub fn to_binary(&self) -> Vec<u8> { /* ... */ }

    /// Total LED count across all zones.
    pub fn total_leds(&self) -> usize {
        self.zones.iter().map(|z| z.colors.len()).sum()
    }
}
```

---

## 5. InputData

The aggregated input snapshot consumed by the effect engine each frame. All input sources (audio, screen, keyboard, controls, time) are unified into one struct.

### 5.1 Core Type

```rust
/// Aggregated input data for one frame.
///
/// Assembled by the `InputAggregator` from all active input sources.
/// Passed to `EffectEngine::render()` each frame. The engine reads
/// whichever fields it needs and ignores the rest.
///
/// Not serialized -- this is an internal hot-path struct.
#[derive(Debug, Clone)]
pub struct InputData {
    /// Frame timing information.
    pub time: TimeData,

    /// Audio analysis data. `None` if no audio source is active.
    pub audio: Option<AudioData>,

    /// Screen capture data. `None` if screen capture is inactive.
    pub screen: Option<ScreenData>,

    /// Keyboard state. `None` if keyboard input source is inactive.
    pub keyboard: Option<KeyboardData>,

    /// Current values of all user-controlled effect parameters.
    pub controls: ControlValues,
}

/// Time-related data injected into every frame.
#[derive(Debug, Clone, Copy)]
pub struct TimeData {
    /// Seconds since the current effect was loaded. Monotonically increasing.
    /// This is the `iTime` uniform for shaders.
    pub elapsed_seconds: f64,

    /// Time since the previous frame in seconds. Typically ~0.0167 at 60fps.
    /// Effects should use this for frame-rate-independent animation.
    pub delta_seconds: f32,

    /// Current frame number since effect load.
    pub frame: u64,

    /// Canvas resolution (width, height). Matches `iResolution` uniform.
    pub resolution: (f32, f32),
}
```

### 5.2 Audio Data

```rust
/// Complete audio analysis data, computed once per frame (60 Hz).
///
/// This struct is the Rust-native representation of the Lightscript
/// `window.engine.audio` contract. Every field maps 1:1 to a Lightscript
/// property. The Servo renderer serializes this to JavaScript; the wgpu
/// renderer packs scalar fields into a uniform buffer and array fields
/// into 1D textures.
///
/// All normalized values are in [0.0, 1.0] unless otherwise noted.
#[derive(Debug, Clone)]
pub struct AudioData {
    // -- Standard (SignalRGB compatible) --

    /// Overall RMS audio level, 0.0 (silence) to 1.0 (clipping).
    pub level: f32,

    /// Bass band energy (20-250 Hz), 0.0 to 1.0.
    pub bass: f32,

    /// Mid band energy (250-4000 Hz), 0.0 to 1.0.
    pub mid: f32,

    /// Treble band energy (4000-20000 Hz), 0.0 to 1.0.
    pub treble: f32,

    /// 200-bin log-scaled frequency magnitudes, 0.0 to 1.0 per bin.
    /// Bin spacing is logarithmic from 20 Hz to 20 kHz.
    pub freq: [f32; 200],

    /// True on the frame where a beat onset is detected.
    pub beat: bool,

    /// Decaying pulse envelope: jumps to 1.0 on beat, decays exponentially.
    /// Decay rate: ~5.0/sec (~200ms to zero).
    pub beat_pulse: f32,

    // -- Beat detection (extended) --

    /// Continuous phase within the current beat period.
    /// 0.0 = on beat, 1.0 = just before next predicted beat.
    pub beat_phase: f32,

    /// Confidence in the current BPM estimate, 0.0 to 1.0.
    pub beat_confidence: f32,

    /// Ramps from 0.0 to 1.0 in the ~20ms before the predicted next beat.
    /// Compensates for render + device output latency.
    pub beat_anticipation: f32,

    /// True on any transient onset (not just beat-aligned ones).
    pub onset: bool,

    /// Decaying onset pulse, same envelope as `beat_pulse`.
    pub onset_pulse: f32,

    /// Estimated tempo in beats per minute. 0.0 if unknown.
    pub tempo: f32,

    // -- Mel scale (perceptually uniform) --

    /// 24 mel-scaled frequency bands (raw energy values).
    pub mel_bands: [f32; 24],

    /// 24 mel bands auto-normalized to 0.0-1.0 (divided by running max).
    pub mel_bands_normalized: [f32; 24],

    // -- Chromagram (musical pitch analysis) --

    /// Energy per pitch class (C, C#, D, ..., B). 12 bins, 0.0 to 1.0.
    pub chromagram: [f32; 12],

    /// Index (0-11) of the pitch class with highest energy.
    pub dominant_pitch: u8,

    /// Confidence of the dominant pitch, 0.0 to 1.0.
    pub dominant_pitch_confidence: f32,

    // -- Spectral features --

    /// Rate of spectral change between consecutive frames.
    pub spectral_flux: f32,

    /// Per-band spectral flux: [bass, mid, treble].
    pub spectral_flux_bands: [f32; 3],

    /// Spectral centroid (perception of "brightness"), 0.0 to 1.0.
    pub brightness: f32,

    /// Spectral bandwidth around the centroid, 0.0 to 1.0.
    pub spread: f32,

    /// Frequency below which 85% of spectral energy is contained, 0.0 to 1.0.
    pub rolloff: f32,

    // -- Harmonic analysis --

    /// Hue derived from the dominant pitch class via the circle-of-fifths
    /// color mapping. 0.0 to 1.0 (maps to 0-360 degrees on the color wheel).
    pub harmonic_hue: f32,

    /// Chord mood: -1.0 (strongly minor/sad) to +1.0 (strongly major/happy).
    /// 0.0 = ambiguous or no clear chord.
    pub chord_mood: f32,

    // -- Derived convenience values --

    /// Fraction of active frequency bins above the noise floor, 0.0 to 1.0.
    pub density: f32,

    /// Stereo width, 0.0 (mono) to 1.0 (full stereo).
    /// Only meaningful when the capture source is stereo.
    pub width: f32,
}

impl Default for AudioData {
    /// Returns silence -- all zeros, arrays zeroed, no beat.
    fn default() -> Self { /* ... */ }
}
```

### 5.3 GPU Audio Uniforms

```rust
/// Packed audio data for the wgpu uniform buffer.
///
/// Scalar audio fields laid out for GPU consumption. Array data
/// (freq, mel, chromagram) is uploaded as 1D textures at separate bindings.
///
/// `repr(C)` and `Pod` ensure this struct can be memcpy'd directly
/// into a wgpu `Buffer`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AudioUniforms {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat_pulse: f32,
    pub beat_phase: f32,
    pub tempo: f32,
    pub harmonic_hue: f32,
    pub spectral_flux: f32,
    pub brightness: f32,
    pub chord_mood: f32,
    pub beat_anticipation: f32,
    /// Padding to 64-byte alignment for GPU uniform buffer requirements.
    pub _padding: [f32; 4],
}
```

### 5.4 Screen Data

```rust
/// Downsampled screen capture for screen-reactive effects.
///
/// The screen capture input source runs on a dedicated thread. It captures
/// the full display via PipeWire (Wayland) or xcap (X11), downsamples to
/// a grid, and publishes the result via triple buffer.
///
/// The grid resolution matches SignalRGB's `engine.zone` interface for
/// compatibility with existing Screen Ambience effects.
#[derive(Debug, Clone)]
pub struct ScreenData {
    /// HSL color values in a 28x20 grid covering the screen.
    /// Indexed as `grid[y * grid_width + x]`.
    pub grid: Vec<HslColor>,

    /// Grid dimensions.
    pub grid_width: u32,   // default: 28
    pub grid_height: u32,  // default: 20

    /// Full-resolution canvas-sized downscale for direct canvas blitting.
    /// Only populated when an effect requests it (lazy computation).
    pub canvas_downscale: Option<Canvas>,
}

/// HSL color for screen capture grid cells.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HslColor {
    /// Hue in degrees, 0.0 to 360.0.
    pub h: f32,
    /// Saturation, 0.0 to 1.0.
    pub s: f32,
    /// Lightness, 0.0 to 1.0.
    pub l: f32,
}
```

### 5.5 Keyboard Data

```rust
/// Keyboard state snapshot for interactive effects.
///
/// Captures which keys are currently pressed and recent key events.
/// The effect reads this to render per-key lighting (ripples, trails, heatmaps).
#[derive(Debug, Clone)]
pub struct KeyboardData {
    /// Set of currently held-down key codes (Linux evdev keycodes).
    pub pressed_keys: Vec<u16>,

    /// Key events since the last frame, in chronological order.
    pub events: Vec<KeyEvent>,
}

/// A single key press or release event.
#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    /// Linux evdev keycode.
    pub keycode: u16,

    /// Whether the key was pressed (true) or released (false).
    pub pressed: bool,

    /// Fractional seconds since the event occurred (relative to frame start).
    pub age_seconds: f32,
}
```

### 5.6 Control Values

```rust
/// Current values of all user-controlled effect parameters.
///
/// Keyed by the control's string ID (matching the `<meta property="...">` tag
/// in HTML effects or the `[[controls]] id = "..."` in TOML metadata).
#[derive(Debug, Clone, Default)]
pub struct ControlValues {
    /// Map from control ID to current value.
    values: std::collections::HashMap<String, ControlValue>,
}

/// A single control parameter value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Number(f32),
    Boolean(bool),
    String(String),
}

impl ControlValues {
    pub fn get(&self, key: &str) -> Option<&ControlValue> {
        self.values.get(key)
    }

    pub fn get_number(&self, key: &str) -> Option<f32> {
        match self.values.get(key) {
            Some(ControlValue::Number(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.values.get(key) {
            Some(ControlValue::Boolean(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_string(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(ControlValue::String(v)) => Some(v),
            _ => None,
        }
    }

    pub fn set(&mut self, key: String, value: ControlValue) {
        self.values.insert(key, value);
    }
}
```

---

## 6. FrameTimer

Timing infrastructure for frame budget tracking, per-stage measurement, consecutive miss detection, and adaptive tier transitions.

### 6.1 Core Type

```rust
use std::time::{Duration, Instant};

/// Frame timing and budget enforcement.
///
/// Tracks per-stage durations, maintains an EWMA of total frame time,
/// and detects consecutive budget misses to trigger FPS tier transitions.
///
/// The EWMA (exponentially weighted moving average) smooths out individual
/// spikes -- a single USB stall should not trigger FPS reduction.
pub struct FrameTimer {
    /// Target frame interval for the current FPS tier.
    target_interval: Duration,

    /// When the current frame started.
    frame_start: Instant,

    /// Per-stage timing slots.
    stage_timings: [StageTiming; PIPELINE_STAGE_COUNT],

    /// Number of consecutive frames that exceeded the target interval.
    /// Reset to zero when a frame completes within budget.
    consecutive_misses: u32,

    /// EWMA of total frame time in seconds.
    /// Alpha = 0.05 (95% weight on history, 5% on current frame).
    ewma_frame_time: f64,

    /// Total frames rendered since last tier change.
    frames_since_tier_change: u64,

    /// Timestamp when the last tier upshift check passed.
    /// Used for the sustained-headroom requirement.
    upshift_eligible_since: Option<Instant>,
}

/// The five pipeline stages that are independently timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PipelineStage {
    InputSampling = 0,
    EffectRendering = 1,
    SpatialSampling = 2,
    DeviceOutput = 3,
    BusPublish = 4,
}

const PIPELINE_STAGE_COUNT: usize = 5;

/// Timing measurement for a single pipeline stage within one frame.
#[derive(Debug, Clone, Copy, Default)]
struct StageTiming {
    start: Option<Instant>,
    duration: Duration,
}
```

### 6.2 Interface

```rust
impl FrameTimer {
    /// Create a new timer targeting the given FPS tier.
    pub fn new(tier: FpsTier) -> Self { /* ... */ }

    /// Mark the beginning of a new frame. Captures the start timestamp.
    pub fn begin_frame(&mut self) {
        self.frame_start = Instant::now();
    }

    /// Mark the beginning of a pipeline stage.
    pub fn begin_stage(&mut self, stage: PipelineStage) {
        self.stage_timings[stage as usize].start = Some(Instant::now());
    }

    /// Mark the end of a pipeline stage. Records its duration.
    pub fn end_stage(&mut self, stage: PipelineStage) {
        if let Some(start) = self.stage_timings[stage as usize].start.take() {
            self.stage_timings[stage as usize].duration = start.elapsed();
        }
    }

    /// Finalize the current frame. Returns the duration to sleep before
    /// the next frame. Returns `Duration::ZERO` if the frame overran.
    ///
    /// Updates the EWMA and consecutive miss counter.
    pub fn frame_complete(&mut self) -> Duration {
        let elapsed = self.frame_start.elapsed();

        // Update EWMA (alpha = 0.05)
        self.ewma_frame_time =
            0.95 * self.ewma_frame_time + 0.05 * elapsed.as_secs_f64();

        if elapsed > self.target_interval {
            self.consecutive_misses += 1;
            self.upshift_eligible_since = None;
            Duration::ZERO
        } else {
            self.consecutive_misses = 0;
            self.target_interval - elapsed
        }
    }

    /// Number of consecutive frames that exceeded the budget.
    pub fn consecutive_misses(&self) -> u32 {
        self.consecutive_misses
    }

    /// Smoothed average frame time.
    pub fn ewma_frame_time(&self) -> Duration {
        Duration::from_secs_f64(self.ewma_frame_time)
    }

    /// Duration of a specific stage in the most recent frame.
    pub fn stage_duration(&self, stage: PipelineStage) -> Duration {
        self.stage_timings[stage as usize].duration
    }

    /// Total elapsed time for the most recent frame.
    pub fn frame_elapsed(&self) -> Duration {
        self.frame_start.elapsed()
    }

    /// Change the target FPS tier. Resets consecutive miss counter.
    pub fn set_tier(&mut self, tier: FpsTier) {
        self.target_interval = tier.frame_interval();
        self.consecutive_misses = 0;
        self.frames_since_tier_change = 0;
        self.upshift_eligible_since = None;
    }

    /// Check if downshift should occur based on consecutive misses.
    pub fn should_downshift(&self, config: &TierTransitionConfig) -> bool {
        self.consecutive_misses >= config.downshift_miss_threshold
    }

    /// Check if upshift should occur based on sustained headroom.
    pub fn should_upshift(&mut self, config: &TierTransitionConfig) -> bool {
        // Must have sufficient headroom: EWMA under 70% of budget
        let headroom_ratio = self.ewma_frame_time / self.target_interval.as_secs_f64();
        if headroom_ratio > 0.7 {
            self.upshift_eligible_since = None;
            return false;
        }

        // Must sustain headroom for the configured duration
        let now = Instant::now();
        match self.upshift_eligible_since {
            None => {
                self.upshift_eligible_since = Some(now);
                false
            }
            Some(since) => {
                now.duration_since(since).as_secs_f64()
                    >= config.upshift_sustain_seconds
            }
        }
    }
}
```

### 6.3 Frame Metrics

```rust
/// Complete timing snapshot for one frame. Published to the metrics subsystem.
///
/// This struct feeds the performance dashboard, Prometheus exporter, and
/// tracing spans. Generated once per frame by the render loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameMetrics {
    /// Monotonically increasing frame counter.
    pub frame_number: u64,

    /// Total wall-clock time for this frame (all stages + overhead).
    pub total_time_us: u64,

    /// Per-stage durations in microseconds.
    pub input_sample_us: u64,
    pub render_us: u64,
    pub spatial_sample_us: u64,
    pub device_output_us: u64,
    pub bus_publish_us: u64,

    /// Which renderer produced this frame.
    pub renderer: RendererType,

    /// GPU-side execution time in microseconds (wgpu timestamp queries).
    /// `None` for Servo frames or when GPU timing is unavailable.
    pub gpu_time_us: Option<u64>,

    /// Active FPS tier.
    pub fps_tier: FpsTier,

    /// Whether this frame exceeded the target budget.
    pub budget_exceeded: bool,

    /// EWMA smoothed frame time in microseconds.
    pub ewma_frame_time_us: u64,

    /// Current consecutive miss count.
    pub consecutive_misses: u32,
}
```

---

## 7. Thread Model

Hypercolor uses a hybrid threading model: dedicated OS threads for latency-sensitive work, a tokio async runtime for I/O-bound operations. The two worlds communicate exclusively via lock-free channels.

### 7.1 Thread Map

```
Process: hypercolord

Thread 0: Render Loop ──────────────────────────────── OS thread, pinned
  - Frame timing (FrameTimer)
  - Effect dispatch (wgpu submit or Servo pump)
  - Spatial sampling (canvas -> LED colors)
  - Device output dispatch (non-blocking mpsc sends)
  - Reads from: audio triple buffer, screen triple buffer
  - Writes to: device backend mpsc channels, watch<FrameData>

Thread 1: Audio Capture ────────────────────────────── OS thread, SCHED_FIFO
  - cpal callback thread (managed by the OS/PipeWire)
  - Ring buffer write (lock-free)
  - FFT + feature extraction at callback rate
  - Writes to: triple_buffer::Input<AudioData>

Thread 2: Screen Capture ──────────────────────────── OS thread, pinned
  - PipeWire DMA-BUF receiver (Wayland) or xcap (X11)
  - Downscale full resolution -> 320x200
  - Writes to: triple_buffer::Input<ScreenData>

Thread 3: Servo Main ──────────────────────────────── OS thread (if active)
  - SpiderMonkey JS execution (single-threaded)
  - DOM layout + style computation
  - Canvas 2D / WebGL rendering
  - Compositing -> pixel readback
  - Owned by the render loop; called synchronously within render()

Threads 4..N: Servo Workers ───────────────────────── (if Servo is active)
  - Servo's internal thread pool (style, layout)
  - Typically 2-4 threads, mostly idle at 320x200

Tokio Runtime (multi-thread, 2-4 workers):
  - Axum web server + WebSocket streaming
  - Device backend I/O tasks (TCP/UDP/DTLS)
  - D-Bus service (zbus)
  - Unix socket IPC (TUI/CLI connections)
  - mDNS discovery
  - Config file watching (notify)
  - Metrics aggregation + Prometheus export

Thread: Watchdog ──────────────────────────────────── OS thread, low priority
  - Monitors render loop heartbeat
  - Triggers restart if render loop hangs (>5 seconds)

Thread: wgpu Device Poll ──────────────────────────── (managed by wgpu)
  - GPU fence polling and callback dispatch
```

### 7.2 Inter-Thread Communication

```rust
/// All hot-path inter-thread channels.
///
/// No `Mutex` touches the render path. All communication is lock-free
/// or uses async channels that the render loop never blocks on.
pub struct ThreadChannels {
    // -- Lock-free triple buffers (producer -> render loop) --

    /// Audio thread writes `AudioData` here every FFT frame (~60 Hz).
    /// Render loop reads the latest value each frame. Zero-copy, wait-free.
    pub audio_output: triple_buffer::Output<AudioData>,

    /// Screen capture thread writes downsampled frames here.
    /// Render loop reads latest. Triple-buffered to prevent blocking.
    pub screen_output: triple_buffer::Output<ScreenData>,

    // -- Async channels (render loop -> consumers) --

    /// Latest LED frame data. Subscribers (web UI, TUI) skip stale values.
    pub frame_watch: watch::Sender<FrameData>,

    /// Latest audio spectrum for visualization in frontends.
    pub spectrum_watch: watch::Sender<SpectrumData>,

    /// Fan-out event channel for system-wide events.
    pub event_broadcast: broadcast::Sender<HypercolorEvent>,

    // -- Per-backend output channels --

    /// Bounded mpsc senders, one per active device backend.
    /// Capacity: 2 frames. If a backend is slow, the oldest frame is dropped
    /// by the backend task (it drains the channel, keeping only the latest).
    pub backend_sinks: Vec<BackendSink>,

    // -- Control channel (frontends -> render loop) --

    /// Inbound commands from API/CLI/TUI (effect switch, control change, etc.).
    /// Unbounded -- commands are rare and small.
    pub command_rx: mpsc::UnboundedReceiver<EngineCommand>,
}

/// Compact spectrum data for the web UI visualizer.
///
/// Smaller than `AudioData` -- only the fields needed for the spectrum widget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectrumData {
    /// 200-bin frequency magnitudes, 0.0 to 1.0.
    pub freq: Vec<f32>,

    /// Band energies for the 3-bar summary display.
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,

    /// Overall level.
    pub level: f32,

    /// Estimated BPM (0 if unknown).
    pub tempo: f32,
}
```

### 7.3 Triple Buffer Topology

```
Audio Thread                  Render Thread
     |                             |
     v                             v
 [Write Buf] <--swap--> [Middle Buf] <--swap--> [Read Buf]
     ^                                               |
     |                                               v
  AudioData                                    InputData.audio
  produced at                                  consumed at
  ~60 Hz                                       ~60 Hz
```

The triple buffer guarantees:
- The producer (audio thread) is **never blocked** by the consumer.
- The consumer (render thread) always gets the **most recent** data.
- There is no lock, no mutex, no CAS retry loop on the hot path.
- One frame of latency at most (the frame currently being written).

### 7.4 Engine Commands

```rust
/// Commands sent from frontends to the render loop.
///
/// These are control-plane messages, not data-plane. They arrive
/// infrequently (user clicks, API calls) and are processed between frames.
#[derive(Debug, Clone)]
pub enum EngineCommand {
    /// Switch to a different effect.
    LoadEffect(EffectSource),

    /// Update a control parameter value.
    SetControl { name: String, value: ControlValue },

    /// Change the target FPS tier (manual override).
    SetFpsTier(FpsTier),

    /// Pause rendering (hold last frame on devices).
    Pause,

    /// Resume rendering after a pause.
    Resume,

    /// Graceful shutdown. Render loop exits after this.
    Shutdown,

    /// Force-reload the current effect (hot-reload trigger).
    ReloadEffect,

    /// Update the spatial layout (zone positions changed in the editor).
    UpdateLayout(SpatialLayout),

    /// Change the canvas sampling method.
    SetSamplingMethod(SamplingMethod),
}
```

---

## 8. Cross-Platform Considerations

Hypercolor is Linux-first, but the architecture isolates platform-specific code behind trait boundaries. This section catalogs what is universal and what needs platform-specific implementations.

### 8.1 Platform Matrix

| Component | Linux | macOS | Windows | Notes |
|---|---|---|---|---|
| **Canvas** | Universal | Universal | Universal | Pure Rust, no platform deps |
| **RenderLoop** | Universal | Universal | Universal | Pure Rust + tokio |
| **FrameTimer** | Universal | Universal | Universal | `std::time::Instant` |
| **wgpu renderer** | Vulkan/OpenGL | Metal | DX12/Vulkan | wgpu abstracts GPU APIs |
| **Servo renderer** | Software/EGL | Software | Software | Servo supports all three; Software is portable fallback |
| **Audio capture** | PipeWire/PulseAudio | CoreAudio | WASAPI | cpal abstracts all three |
| **Monitor source** | `libpulse` API | CoreAudio loopback | WASAPI loopback | Platform-specific discovery |
| **Screen capture** | PipeWire DMA-BUF / xcap | CGWindowListCreateImage | DXGI Desktop Dup | Fully platform-specific |
| **Keyboard input** | evdev | IOKit/CGEvent | Raw Input / Win32 | Fully platform-specific |
| **USB HID** | hidapi (libusb) | hidapi (IOKit) | hidapi (win32) | hidapi abstracts platforms |
| **mDNS** | Avahi / mdns-sd | Bonjour | Bonjour | mdns-sd crate is cross-platform |
| **D-Bus / IPC** | zbus (D-Bus) | XPC (future) | Named pipes (future) | D-Bus is Linux-only |
| **Thread pinning** | `core_affinity` | `core_affinity` | `core_affinity` | cross-platform crate |
| **RT audio priority** | rtkit / SCHED_FIFO | pthread priority | `SetThreadPriority` | Platform-specific |
| **systemd watchdog** | sd_notify | launchd (future) | Windows Service (future) | Platform-specific |
| **GPU load detection** | sysfs / NVML | IOKit | NVML / DXGI | Platform-specific |
| **GameMode integration** | D-Bus signal | N/A | N/A | Linux-only |

### 8.2 Platform Abstraction Strategy

The core engine crate (`hypercolor-core`) contains zero platform-specific code. All platform boundaries are expressed as traits:

```rust
/// Platform-specific audio monitor source discovery.
///
/// Implemented per-platform to find the system audio output's
/// loopback/monitor source for capturing what's playing.
pub trait AudioMonitorDiscovery: Send + Sync {
    /// Find the monitor source for the default audio output.
    /// Returns the source name/identifier that cpal can open.
    fn find_default_monitor(&self) -> Result<String, AudioError>;

    /// List all available monitor sources.
    fn list_monitors(&self) -> Result<Vec<AudioSourceInfo>, AudioError>;
}

/// Platform-specific screen capture.
pub trait ScreenCaptureBackend: Send {
    /// Capture the current screen contents.
    /// Returns a full-resolution RGBA buffer.
    fn capture_frame(&mut self) -> Result<RawFrame, CaptureError>;

    /// Supported capture methods on this platform.
    fn capabilities(&self) -> CaptureCapabilities;
}

/// Platform-specific keyboard input.
pub trait KeyboardBackend: Send {
    /// Poll current keyboard state and recent events.
    fn poll(&mut self) -> Result<KeyboardData, InputError>;
}

/// Audio source metadata for UI display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSourceInfo {
    /// Internal identifier (e.g., PulseAudio source name).
    pub id: String,

    /// Human-readable name (e.g., "Starship/Matisse HD Audio Monitor").
    pub display_name: String,

    /// Whether this is a monitor (loopback) source.
    pub is_monitor: bool,
}
```

### 8.3 Feature Flags for Platform Code

```toml
# hypercolor-core/Cargo.toml
[features]
default = ["audio-pulse"]

# Audio monitor discovery implementations
audio-pulse = ["dep:libpulse-binding"]   # Linux: PulseAudio/PipeWire
audio-coreaudio = []                      # macOS: CoreAudio (future)
audio-wasapi = []                         # Windows: WASAPI (future)

# Screen capture implementations
screen-pipewire = ["dep:pipewire"]        # Linux: Wayland
screen-xcap = ["dep:xcap"]               # Linux: X11 fallback
screen-mac = []                           # macOS (future)
screen-win = []                           # Windows (future)

# Keyboard input implementations
keyboard-evdev = ["dep:evdev"]            # Linux
keyboard-mac = []                         # macOS (future)
keyboard-win = []                         # Windows (future)

# System integration
dbus = ["dep:zbus"]                       # Linux: D-Bus IPC
gamemode = ["dbus"]                       # Linux: Feral GameMode
```

### 8.4 Universal Invariants

These hold true on every platform:

1. **Canvas is always 320x200 RGBA** unless explicitly resized by the user. The resolution is a configuration value, not a compile-time constant, but the default is canonical.

2. **The render loop is always a dedicated OS thread.** Never runs on the tokio runtime. Frame timing requires predictable scheduling that async task stealing cannot guarantee.

3. **Audio data is always delivered via triple buffer.** The capture mechanism varies by platform, but the lock-free handoff to the render thread is universal.

4. **Device output is always non-blocking from the render loop's perspective.** Backends own their I/O threads or tokio tasks. The render loop dispatches via mpsc and moves on.

5. **The event bus (`watch` + `broadcast`) is platform-independent.** All frontends subscribe to the same Rust channel types regardless of transport (WebSocket, Unix socket, named pipe).

6. **Effects are portable.** HTML effects run identically on every platform (Servo is cross-platform). WGSL shaders are GPU-API-agnostic (wgpu translates to Vulkan/Metal/DX12). The `InputData` struct is the same everywhere -- only the sources that populate it vary.

---

## Appendix A: Type Dependency Graph

```
EffectEngine (trait)
    |
    +-- reads InputData
    |       +-- TimeData
    |       +-- AudioData  <---- triple_buffer::Output <---- Audio Thread
    |       +-- ScreenData <---- triple_buffer::Output <---- Screen Thread
    |       +-- KeyboardData
    |       +-- ControlValues
    |
    +-- writes Canvas
            |
            +-- sampled by SpatialSampler (SamplingMethod)
                    |
                    +-- produces Vec<DeviceColors>
                            |
                            +-- dispatched to BackendSink (mpsc)
                            |
                            +-- published as FrameData (watch)

FrameTimer
    |
    +-- tracks PipelineStage durations
    +-- manages FpsTier transitions
    +-- emits FrameMetrics
```

## Appendix B: Size Budget at Default Resolution

| Buffer | Formula | Size |
|---|---|---|
| Canvas (single) | 320 * 200 * 4 | 256,000 bytes (250 KB) |
| Canvas (double-buffered) | 2 * 256,000 | 512,000 bytes (500 KB) |
| LED colors (2000 LEDs) | 2000 * 3 | 6,000 bytes (5.9 KB) |
| AudioData (single) | ~1,800 bytes | 1.8 KB |
| AudioData (triple-buffered) | 3 * 1,800 | 5.4 KB |
| FrameData (2000 LEDs) | ~6,100 bytes | 6.0 KB |
| AudioUniforms (GPU) | 64 bytes | 64 bytes |
| Freq texture (1D, 200 bins) | 200 * 4 | 800 bytes |
| **Total hot-path buffers** | | **~530 KB** |

The entire hot-path data set fits in L2 cache on any modern CPU.

## Appendix C: Binding Layout for wgpu Shaders

```
Group 0: Effect data
  Binding 0: Uniform buffer   -- time, resolution, user controls
  Binding 1: Uniform buffer   -- AudioUniforms (scalar audio data)
  Binding 2: Texture 1D       -- freq[200] (FFT spectrum)
  Binding 3: Texture 1D       -- mel_bands[24]
  Binding 4: Texture 1D       -- chromagram[12]
  Binding 5: Sampler          -- nearest-neighbor for 1D textures

Group 1: Render target
  Binding 0: Texture 2D       -- output color attachment (320x200 RGBA)
```
