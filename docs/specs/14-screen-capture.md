# Spec 14 — Screen Capture System

> Technical specification for capturing screen content and transforming it into LED color data.

**Status:** Draft
**Design doc:** [08-screen-capture.md](../design/08-screen-capture.md)
**Performance doc:** [13-performance.md](../design/13-performance.md)

---

## Table of Contents

1. [ScreenCaptureInput — InputSource Implementation](#1-screencaptureinput--inputsource-implementation)
2. [Capture Backends](#2-capture-backends)
3. [CaptureConfig](#3-captureconfig)
4. [SectorGrid — Frame Subdivision](#4-sectorgrid--frame-subdivision)
5. [RegionMapping — LED Zone Assignment](#5-regionmapping--led-zone-assignment)
6. [Color Processing Pipeline](#6-color-processing-pipeline)
7. [ScreenData — Output Type](#7-screendata--output-type)
8. [Performance Budget](#8-performance-budget)
9. [Adaptive Quality](#9-adaptive-quality)
10. [Cross-Platform Strategy](#10-cross-platform-strategy)

---

## 1. ScreenCaptureInput — InputSource Implementation

`ScreenCaptureInput` is the top-level struct that implements the `InputSource` trait. It owns the full capture-to-color pipeline: backend selection, region mapping, color processing, and temporal smoothing. When active, it bypasses the effect engine entirely — screen colors flow directly to device backends.

```
Normal mode:   Effect Engine → Canvas → Spatial Sampler → Device Backends
Capture mode:  ScreenCaptureInput → RegionMapper → ColorProcessor → Device Backends
```

### Trait Contract

```rust
/// InputSource is Hypercolor's unified interface for anything that produces
/// color/data for the render pipeline. Audio, keyboard, and screen capture
/// all implement this trait.
pub trait InputSource: Send + Sync {
    /// Human-readable identifier for logging and config references.
    fn name(&self) -> &str;

    /// Produce one frame of input data. Called by the render loop at
    /// `sample_rate_hz()` frequency. Must not block longer than 2ms
    /// under normal operation.
    fn sample(&mut self) -> Result<InputData>;

    /// Desired sample rate. The render loop uses this to schedule calls.
    /// Screen capture typically returns 15.0–60.0.
    fn sample_rate_hz(&self) -> f64;
}
```

### ScreenCaptureInput

```rust
pub struct ScreenCaptureInput {
    /// Active capture backend (PipeWire, XShm, or xcap).
    backend: Box<dyn CaptureBackendTrait>,

    /// Translates screen geometry into LED sampling regions.
    region_mapper: RegionMapper,

    /// Saturation boost, black level, white balance, letterbox handling.
    color_processor: ColorProcessor,

    /// Adaptive EMA smoother with scene-cut detection.
    smoother: TemporalSmoother,

    /// Letterbox/pillarbox detector — adjusts sampling regions dynamically.
    letterbox: LetterboxDetector,

    /// Runtime configuration (fps, resolution, monitor selection, etc.).
    config: CaptureConfig,

    /// Adaptive quality controller — degrades resolution/fps under load.
    quality_ctrl: QualityController,

    /// Reusable buffer for the downsampled frame. Avoids per-frame allocation.
    staging: Vec<u8>,
}

impl InputSource for ScreenCaptureInput {
    fn name(&self) -> &str { "screen_capture" }

    fn sample(&mut self) -> Result<InputData> {
        // 1. Capture frame (backend-specific: DMA-BUF, XShm, or xcap)
        let frame = self.backend.capture_frame(&mut self.staging)?;

        // 2. Detect letterboxing (updates internal state over N frames)
        let bars = self.letterbox.analyze(&frame);

        // 3. Map regions → extract per-zone raw colors
        let mut zone_colors = self.region_mapper.extract(&frame, &bars);

        // 4. Process: aggregate, saturate, black-level, white-balance
        self.color_processor.process(&mut zone_colors);

        // 5. Temporal smoothing (adaptive EMA with scene-cut detection)
        self.smoother.apply(&mut zone_colors);

        // 6. Adaptive quality — measure this frame's cost, maybe adjust
        let frame_cost = frame.capture_duration;
        if let Some(adj) = self.quality_ctrl.evaluate(frame_cost) {
            self.backend.apply_quality_adjustment(adj)?;
        }

        Ok(InputData::ScreenCapture(ScreenData {
            zone_colors,
            capture_timestamp: frame.timestamp,
            frame_resolution: frame.resolution,
        }))
    }

    fn sample_rate_hz(&self) -> f64 {
        self.config.target_fps as f64
    }
}
```

### Lifecycle

| Event | Behavior |
|---|---|
| **Construction** | Auto-detect backend, request portal permissions (Wayland), allocate staging buffer |
| **First sample** | PipeWire: blocks until first frame arrives or 5s timeout. XShm/xcap: immediate |
| **Steady state** | Non-blocking reads from backend's frame buffer (double/triple buffered) |
| **Monitor disconnect** | Backend emits `CaptureError::MonitorLost`, input source signals the render loop to fall back |
| **Drop** | Release PipeWire stream, detach XShm segment, close portal session |

---

## 2. Capture Backends

All backends implement a common trait. Platform-specific backends are gated behind feature flags and `#[cfg]` attributes.

### 2.1 Backend Trait

```rust
/// Unified interface for all capture backends.
/// Implementations must be `Send` (owned by the capture thread) but need
/// not be `Sync` — only one thread calls `capture_frame`.
pub trait CaptureBackendTrait: Send {
    /// Capture the current frame into `staging`. Returns metadata about the
    /// captured frame. Must not allocate — use the provided staging buffer.
    fn capture_frame(&mut self, staging: &mut Vec<u8>) -> Result<CapturedFrame>;

    /// Apply a quality adjustment (resolution/fps change) from the adaptive
    /// quality controller. PipeWire renegotiates stream params; xcap changes
    /// its downsample factor.
    fn apply_quality_adjustment(&mut self, adj: QualityAdjustment) -> Result<()>;

    /// Backend identifier for logging/diagnostics.
    fn backend_name(&self) -> &str;
}

pub struct CapturedFrame {
    /// Pixel data in BGRA8 format, row-major, no padding.
    /// Points into the staging buffer (zero-copy reference).
    pub data: *const u8,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: PixelFormat,
    pub timestamp: Instant,
    pub capture_duration: Duration,
}

pub enum PixelFormat {
    Bgra8,
    Rgba8,
    /// HDR: PQ transfer, BT.2020 gamut, 10-bit per channel packed as u16.
    Rgba16Pq,
}
```

### 2.2 PipeWire Portal Backend (Wayland)

**Feature gate:** `#[cfg(target_os = "linux")]` + feature `screen-pipewire`

The primary backend on modern Linux desktops. Uses the XDG Desktop Portal D-Bus API to negotiate screen access, then receives frames as a PipeWire stream consumer.

```rust
#[cfg(target_os = "linux")]
pub struct PipeWireCapture {
    /// D-Bus connection to the XDG Desktop Portal.
    portal: ScreenCastPortal,

    /// PipeWire stream consumer. Receives frames on a dedicated thread,
    /// makes the latest frame available via a lock-free double buffer.
    stream: PipeWireStream,

    /// Persisted across sessions — avoids the "Hypercolor wants to record
    /// your screen" dialog on every launch.
    restore_token: Option<String>,

    /// Negotiated stream parameters.
    negotiated_format: VideoFormat,

    /// Double-buffer: backend thread writes to back, `capture_frame` reads front.
    frame_buffer: Arc<DoubleBuffer<FrameSlot>>,
}
```

**Session establishment flow:**

```
1. CreateSession()                → session_handle
2. SelectSources(session_handle)  → user picks monitor(s) via compositor dialog
3. Start(session_handle)          → returns PipeWire node_id
4. Connect PipeWire stream to node_id with format negotiation
5. Frames arrive as SPA buffers (DMA-BUF or MemPtr)
```

**DMA-BUF zero-copy path:**

When the compositor provides `SPA_DATA_DmaBuf` frames:

1. The DMA-BUF fd references GPU memory — no CPU copy on delivery.
2. For downsampling: import the DMA-BUF as a `wgpu::Texture` via `create_texture_from_dmabuf`, run a compute shader to produce the sector grid directly on the GPU, then read back only the ~2304-value result (64x36 sectors x 4 bytes = ~9 KB).
3. For fallback: `mmap()` the DMA-BUF for CPU access. Still faster than a full pixel-by-pixel copy.

**`SPA_DATA_MemPtr` fallback:**

When DMA-BUF is unavailable, PipeWire delivers frames as shared memory pointers. CPU access is direct but the entire frame must be read by the CPU (no GPU shortcut without an explicit upload).

**Restore tokens:**

```rust
/// Persist the portal restore token to avoid re-prompting.
/// Stored in: ~/.config/hypercolor/portal_tokens.json
pub struct PortalTokenStore {
    /// Map of monitor_id → restore_token.
    tokens: HashMap<String, String>,
}

impl PortalTokenStore {
    /// Load tokens from disk. Missing file → empty map (first run).
    pub fn load() -> Self { /* ... */ }

    /// Save after a successful session start.
    pub fn save(&self) -> Result<()> { /* ... */ }
}
```

**Multi-monitor:**

PipeWire supports multiple concurrent streams. For multi-monitor setups, Hypercolor opens one stream per captured monitor. The portal's `SelectSources` dialog lets the user select multiple outputs in a single interaction. Each stream produces frames independently — the region mapper combines them into a virtual canvas at extraction time.

### 2.3 X11 Backend (XShm)

**Feature gate:** `#[cfg(target_os = "linux")]` + feature `screen-x11`

Used when `$DISPLAY` is set and `$WAYLAND_DISPLAY` is not. No permissions required — X11 exposes all screen content to any client.

```rust
#[cfg(target_os = "linux")]
pub struct XShmCapture {
    /// X11 display connection.
    display: *mut x11::xlib::Display,

    /// Shared memory segment for zero-copy from X server.
    shm_info: XShmSegmentInfo,

    /// XImage backed by the shared memory segment.
    ximage: *mut x11::xlib::XImage,

    /// Dimensions of the captured root window (or specific monitor region).
    capture_rect: Rect,
}
```

**Capture flow:**

1. `shmget()` allocates a shared memory segment sized to the capture region.
2. `XShmCreateImage()` creates an XImage backed by that segment.
3. Per frame: `XShmGetImage(display, root_window, ximage, x, y, AllPlanes)` blits screen content into shared memory. One memcpy from X server internals — fast.
4. The staging buffer reads directly from the shared memory segment.

**Performance:** ~2-4ms per frame at 1920x1080. At our downsampled capture resolution (640x360), effectively free.

**Multi-monitor:** X11 captures the root window, which spans all monitors. The region mapper uses `XRRGetScreenResources` / `XRRGetCrtcInfo` to determine per-monitor geometry and maps virtual canvas coordinates accordingly.

### 2.4 xcap Crate Fallback

**Feature gate:** Always available (pure Rust, cross-platform)

The `xcap` crate provides a universal fallback. On Linux it uses XShm internally for X11 and PipeWire for Wayland. On Windows and macOS it uses native APIs (DXGI/WGC and SCKit respectively). This is the only backend available on non-Linux platforms.

```rust
pub struct XcapCapture {
    /// Cached monitor handle. Refreshed on hot-plug events.
    monitor: xcap::Monitor,

    /// Target capture size — xcap captures at native res,
    /// we downsample immediately to avoid holding large buffers.
    target_size: (u32, u32),
}

impl CaptureBackendTrait for XcapCapture {
    fn capture_frame(&mut self, staging: &mut Vec<u8>) -> Result<CapturedFrame> {
        // xcap returns image::RgbaImage at native resolution
        let screenshot = self.monitor.capture_image()
            .map_err(|e| CaptureError::BackendFailed(e.to_string()))?;

        // Downsample immediately — don't hold a 4K RGBA buffer around
        let small = image::imageops::resize(
            &screenshot,
            self.target_size.0,
            self.target_size.1,
            image::imageops::Triangle, // bilinear — fast, good enough
        );

        // Copy into staging buffer (RGBA8 row-major)
        staging.clear();
        staging.extend_from_slice(small.as_raw());

        Ok(CapturedFrame {
            data: staging.as_ptr(),
            width: self.target_size.0,
            height: self.target_size.1,
            stride: self.target_size.0 * 4,
            pixel_format: PixelFormat::Rgba8,
            timestamp: Instant::now(),
            capture_duration: /* measured */, // wall-clock time of capture_image + resize
        })
    }

    fn backend_name(&self) -> &str { "xcap" }
}
```

**Trade-offs:** No streaming mode — each call is a discrete screenshot. Higher latency than PipeWire streaming. But it works everywhere and has zero setup complexity.

### 2.5 Backend Auto-Detection

```rust
pub fn auto_detect_backend(config: &CaptureConfig) -> Result<Box<dyn CaptureBackendTrait>> {
    // 1. User-specified override in config
    if let Some(ref forced) = config.forced_backend {
        return match forced.as_str() {
            "pipewire" => Ok(Box::new(PipeWireCapture::new(config)?)),
            "xshm"     => Ok(Box::new(XShmCapture::new(config)?)),
            "xcap"     => Ok(Box::new(XcapCapture::new(config)?)),
            other      => Err(CaptureError::UnknownBackend(other.into())),
        };
    }

    // 2. Auto-detect from environment
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            match PipeWireCapture::new(config) {
                Ok(pw) => return Ok(Box::new(pw)),
                Err(e) => {
                    tracing::warn!("PipeWire unavailable ({e}), falling back to xcap");
                }
            }
        }

        if std::env::var("DISPLAY").is_ok() {
            match XShmCapture::new(config) {
                Ok(xshm) => return Ok(Box::new(xshm)),
                Err(e) => {
                    tracing::warn!("XShm unavailable ({e}), falling back to xcap");
                }
            }
        }
    }

    // 3. Universal fallback
    Ok(Box::new(XcapCapture::new(config)?))
}
```

### 2.6 Feature Flag Matrix

| Feature Flag | Platforms | Dependencies | What it enables |
|---|---|---|---|
| `screen-pipewire` | Linux only | `libpipewire-0.3`, `zbus`, `wayland-client` | `PipeWireCapture` with DMA-BUF + portal |
| `screen-x11` | Linux only | `x11`, `xcb` (XShm extension) | `XShmCapture` shared memory capture |
| *(default)* | All | `xcap`, `image` | `XcapCapture` universal fallback |

```toml
# Cargo.toml feature definitions
[features]
default = ["screen-capture"]
screen-capture = ["dep:xcap", "dep:image"]
screen-pipewire = ["screen-capture", "dep:pipewire", "dep:zbus"]
screen-x11 = ["screen-capture", "dep:x11"]
screen-full = ["screen-pipewire", "screen-x11"]
```

---

## 3. CaptureConfig

Runtime configuration for the capture pipeline. Loaded from TOML config, overridable per-profile, adjustable at runtime via the API.

```rust
pub struct CaptureConfig {
    // ── Backend selection ──────────────────────────────────────
    /// "auto", "pipewire", "xshm", "xcap". Default: "auto".
    pub forced_backend: Option<String>,

    // ── Monitor targeting ─────────────────────────────────────
    /// Which display(s) to capture.
    pub monitor: MonitorSelect,

    // ── Resolution & frame rate ────────────────────────────────
    /// Capture resolution requested from the backend.
    /// NOT the native monitor resolution — this is the pre-downsampled size.
    /// Default: (640, 360).
    pub capture_resolution: (u32, u32),

    /// Target frames per second. Default: 30.
    pub target_fps: u32,

    /// Prefer DMA-BUF zero-copy when available. Default: true.
    pub prefer_dmabuf: bool,

    /// Include the cursor in captured frames. Default: false.
    pub cursor_visible: bool,

    // ── Color processing ──────────────────────────────────────
    /// Color aggregation method per zone. Default: Dominant for edge zones,
    /// Average for sector grid zones.
    pub aggregation: AggregationMethod,

    /// HSL saturation multiplier. Range: 0.5–2.5. Default: 1.4.
    pub saturation_boost: f32,

    /// Base smoothing alpha for EMA. Range: 0.0–1.0. Default: 0.15.
    pub smoothing_alpha: f32,

    /// Content mode for color extraction. Default: Direct.
    pub content_mode: ContentMode,

    /// Black level handling. Default: DimAmbient.
    pub black_level: BlackLevelConfig,

    /// White balance correction (per-device overrides elsewhere).
    pub white_balance: WhiteBalance,

    // ── Letterbox ──────────────────────────────────────────────
    pub letterbox: LetterboxConfig,

    // ── Adaptive quality ───────────────────────────────────────
    pub adaptive: AdaptiveConfig,
}

pub enum MonitorSelect {
    /// The compositor's primary/focused output.
    Primary,
    /// All monitors stitched into a virtual canvas.
    All,
    /// A specific output by connector name (e.g., "DP-1", "HDMI-A-1").
    ByName(String),
    /// A specific output by index (0-based, ordered left-to-right).
    ByIndex(u32),
}

pub enum AggregationMethod {
    /// Arithmetic mean of all pixels in the region. Fast, stable, muddy.
    Average,
    /// Hue-histogram peak detection. Perceptually superior, slightly more CPU.
    Dominant { bucket_count: u32 },
}

pub enum ContentMode {
    /// Each zone gets its own region's color. Standard ambilight behavior.
    Direct,
    /// Entire frame reduced to a single dominant mood color.
    /// All zones share it, with luminance variation from edge proximity.
    Mood,
    /// Edge zones use Direct, interior/sector zones use Mood.
    Hybrid,
}
```

### Default Configuration (TOML)

```toml
[screen_capture]
backend = "auto"
fps = 30
resolution = [640, 360]
prefer_dmabuf = true
cursor = false
monitor = "primary"
aggregation = "dominant"
saturation_boost = 1.4
smoothing_alpha = 0.15
content_mode = "direct"

[screen_capture.black_level]
threshold = 0.03
strategy = "dim_ambient"
ambient_color = [255, 200, 150]
ambient_brightness = 0.05
fade_duration = 3.0

[screen_capture.white_balance]
r_gain = 1.0
g_gain = 1.0
b_gain = 0.85

[screen_capture.letterbox]
enabled = true
confidence_frames = 15
black_threshold = 0.02
min_bar_fraction = 0.05

[screen_capture.adaptive]
enabled = true
min_resolution = [160, 90]
max_resolution = [640, 360]
min_fps = 15
max_fps = 60
latency_threshold_ms = 4
recovery_threshold_ms = 2
```

---

## 4. SectorGrid — Frame Subdivision

The `SectorGrid` divides a captured frame into an N x M grid of rectangular sectors. Each sector represents a spatial region of the screen that can be mapped to one or more device zones. This is the intermediate representation between raw pixels and LED colors.

### Data Structures

```rust
/// A grid of sectors overlaid on the captured frame.
/// Each sector holds the aggregated color for its screen region.
pub struct SectorGrid {
    /// Number of columns (horizontal divisions). Default: 64.
    pub cols: u32,
    /// Number of rows (vertical divisions). Default: 36.
    pub rows: u32,
    /// Flat array of sector colors, row-major: index = row * cols + col.
    /// Length: cols * rows.
    pub colors: Vec<Rgb>,
}

/// Reference from a sector to its assigned device zone.
pub struct SectorAssignment {
    /// Grid coordinates of the sector.
    pub col: u32,
    pub row: u32,
    /// Which device/zone this sector drives.
    pub target: DeviceZoneRef,
    /// Blending weight when multiple sectors map to one zone.
    /// Default: 1.0. Used for weighted averaging at zone boundaries.
    pub weight: f32,
}

pub struct DeviceZoneRef {
    pub device_id: String,
    pub zone_name: String,
}
```

### Sector Grid Computation

Given a downsampled frame of `frame_width x frame_height` pixels, each sector covers a rectangular block of pixels. The computation is a simple box filter — average all pixels in the block.

```rust
impl SectorGrid {
    /// Compute sector colors from a downsampled BGRA8/RGBA8 frame buffer.
    ///
    /// # Algorithm
    ///
    /// Each sector spans a rectangular pixel region:
    ///   sector_w = frame_width / cols    (integer, remainder absorbed by last col)
    ///   sector_h = frame_height / rows   (integer, remainder absorbed by last row)
    ///
    /// For each sector (c, r):
    ///   x_start = c * sector_w
    ///   x_end   = if c == cols-1 { frame_width } else { (c+1) * sector_w }
    ///   y_start = r * sector_h
    ///   y_end   = if r == rows-1 { frame_height } else { (r+1) * sector_h }
    ///
    /// The sector color is the arithmetic mean of all pixels in [x_start..x_end, y_start..y_end].
    pub fn compute(
        frame: &[u8],
        frame_width: u32,
        frame_height: u32,
        stride: u32,
        format: PixelFormat,
        cols: u32,
        rows: u32,
    ) -> Self {
        let sector_w = frame_width / cols;
        let sector_h = frame_height / rows;
        let mut colors = Vec::with_capacity((cols * rows) as usize);

        for r in 0..rows {
            let y_start = r * sector_h;
            let y_end = if r == rows - 1 { frame_height } else { (r + 1) * sector_h };

            for c in 0..cols {
                let x_start = c * sector_w;
                let x_end = if c == cols - 1 { frame_width } else { (c + 1) * sector_w };

                let (mut sum_r, mut sum_g, mut sum_b) = (0u64, 0u64, 0u64);
                let mut count = 0u64;

                for y in y_start..y_end {
                    let row_offset = (y * stride) as usize;
                    for x in x_start..x_end {
                        let px = row_offset + (x * 4) as usize;
                        let (pr, pg, pb) = match format {
                            PixelFormat::Bgra8 => (frame[px + 2], frame[px + 1], frame[px]),
                            PixelFormat::Rgba8 => (frame[px], frame[px + 1], frame[px + 2]),
                            PixelFormat::Rgba16Pq => {
                                // HDR path: read u16 pairs, tonemap inline
                                // (see section 6 for HDR handling)
                                continue;
                            }
                        };
                        sum_r += pr as u64;
                        sum_g += pg as u64;
                        sum_b += pb as u64;
                        count += 1;
                    }
                }

                let n = count.max(1);
                colors.push(Rgb::new(
                    (sum_r / n) as u8,
                    (sum_g / n) as u8,
                    (sum_b / n) as u8,
                ));
            }
        }

        SectorGrid { cols, rows, colors }
    }

    /// Look up the color of sector (col, row).
    #[inline]
    pub fn get(&self, col: u32, row: u32) -> Rgb {
        self.colors[(row * self.cols + col) as usize]
    }

    /// Look up a sector by normalized coordinates (0.0–1.0).
    /// Useful for region mappers that work in fractional screen space.
    #[inline]
    pub fn sample_normalized(&self, nx: f32, ny: f32) -> Rgb {
        let col = ((nx * self.cols as f32) as u32).min(self.cols - 1);
        let row = ((ny * self.rows as f32) as u32).min(self.rows - 1);
        self.get(col, row)
    }
}
```

### Grid Size Rationale

| Grid | Sectors | Bytes (RGB) | Use Case |
|---|---|---|---|
| 16 x 9 | 144 | 432 B | Ultra-low-power, few LEDs |
| 32 x 18 | 576 | 1.7 KB | Good balance for typical setups (< 200 LEDs) |
| **64 x 36** | **2,304** | **6.9 KB** | **Default.** Fine-grained enough for 200+ LED strips |
| 128 x 72 | 9,216 | 27.6 KB | Overkill for most setups, useful for giant matrices |

The default 64x36 grid matches a 640x360 capture resolution perfectly: each sector is exactly 10x10 pixels. This integer alignment eliminates interpolation artifacts in the box filter.

### GPU Compute Path

When using the DMA-BUF path, the sector grid can be computed entirely on the GPU:

```wgsl
// Compute shader: downsample captured frame → sector grid
// Dispatch: (cols, rows, 1) workgroups, each workgroup averages one sector.

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;

@compute @workgroup_size(1, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let sector_w = textureDimensions(input_texture).x / GRID_COLS;
    let sector_h = textureDimensions(input_texture).y / GRID_ROWS;
    let x_start = gid.x * sector_w;
    let y_start = gid.y * sector_h;

    var sum = vec4<f32>(0.0);
    var count = 0.0;
    for (var y = y_start; y < y_start + sector_h; y++) {
        for (var x = x_start; x < x_start + sector_w; x++) {
            sum += textureLoad(input_texture, vec2<i32>(i32(x), i32(y)), 0);
            count += 1.0;
        }
    }

    let idx = gid.y * GRID_COLS + gid.x;
    output[idx] = sum / count;
}
```

The GPU reads back ~9 KB (2304 sectors x 4 floats) — negligible compared to reading back the full frame.

---

## 5. RegionMapping — LED Zone Assignment

The region mapper translates the sector grid into per-LED colors. Two primary mapping modes exist: **edge sampling** for monitor backlights and **sector grid** for ambient/area devices.

### 5.1 Edge Sampling (Monitor Backlight)

Each edge LED maps to a thin rectangular strip along the screen's border.

```rust
pub struct EdgeConfig {
    /// Inward sampling depth as a fraction of the screen dimension.
    /// 0.06 = sample the outer 6% of the screen.
    pub depth: f32,

    /// LED counts per edge.
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,

    /// Number of corner LEDs that blend between adjacent edges.
    /// These LEDs sample a weighted mix of both edge regions.
    pub corner_gap: u32,
}
```

**Region generation** for top edge LEDs (other edges follow the same pattern, with bottom reversed for continuous strip winding):

```rust
impl EdgeConfig {
    pub fn build_top_regions(&self, grid: &SectorGrid) -> Vec<EdgeRegion> {
        let depth_rows = (self.depth * grid.rows as f32).max(1.0) as u32;
        let segment_cols = grid.cols as f32 / self.top as f32;

        (0..self.top).map(|i| {
            let col_start = (i as f32 * segment_cols) as u32;
            let col_end = ((i + 1) as f32 * segment_cols).ceil() as u32;
            let col_end = col_end.min(grid.cols);

            // Average all sectors in this strip
            let mut sum_r = 0u32;
            let mut sum_g = 0u32;
            let mut sum_b = 0u32;
            let mut count = 0u32;

            for row in 0..depth_rows {
                for col in col_start..col_end {
                    let c = grid.get(col, row);
                    sum_r += c.r as u32;
                    sum_g += c.g as u32;
                    sum_b += c.b as u32;
                    count += 1;
                }
            }

            let n = count.max(1);
            EdgeRegion {
                led_index: i,
                edge: Edge::Top,
                color: Rgb::new(
                    (sum_r / n) as u8,
                    (sum_g / n) as u8,
                    (sum_b / n) as u8,
                ),
            }
        }).collect()
    }
}
```

**Corner blending** uses Hermite interpolation (smooth step) to create a gradual transition between adjacent edges at corner LEDs:

```rust
pub fn blend_corner(top_color: Rgb, side_color: Rgb, t: f32) -> Rgb {
    let t = t * t * (3.0 - 2.0 * t); // Hermite smooth step
    Rgb::new(
        lerp_u8(top_color.r, side_color.r, t),
        lerp_u8(top_color.g, side_color.g, t),
        lerp_u8(top_color.b, side_color.b, t),
    )
}
```

### 5.2 Sector Grid Mapping (Ambient Devices)

For non-edge devices — ceiling panels, desk underglow, WLED matrices, Hue bulbs — the screen divides into sectors, each assigned to a device zone.

```rust
pub struct SectorMapping {
    /// Which sectors feed this device zone.
    pub sectors: Vec<WeightedSector>,
    /// Target device and zone.
    pub target: DeviceZoneRef,
}

pub struct WeightedSector {
    pub col: u32,
    pub row: u32,
    pub weight: f32,
}

impl SectorMapping {
    /// Compute the zone color by weighted average of assigned sectors.
    pub fn resolve(&self, grid: &SectorGrid) -> Rgb {
        let (mut wr, mut wg, mut wb, mut tw) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for ws in &self.sectors {
            let c = grid.get(ws.col, ws.row);
            wr += c.r as f32 * ws.weight;
            wg += c.g as f32 * ws.weight;
            wb += c.b as f32 * ws.weight;
            tw += ws.weight;
        }
        let tw = tw.max(f32::EPSILON);
        Rgb::new(
            (wr / tw) as u8,
            (wg / tw) as u8,
            (wb / tw) as u8,
        )
    }
}
```

### 5.3 RegionMapper

The `RegionMapper` owns all mapping configurations and produces per-zone colors from a sector grid.

```rust
pub struct RegionMapper {
    /// Edge sampling configurations (one per monitor backlight strip).
    pub edge_configs: Vec<(MonitorRef, EdgeConfig)>,
    /// Sector-to-zone assignments (one per ambient device zone).
    pub sector_mappings: Vec<SectorMapping>,
    /// Exclusion zones (HUD areas, subtitles) in normalized coordinates.
    pub exclusion_zones: Vec<ExclusionRect>,
}

pub struct ExclusionRect {
    pub x: f32,      // 0.0–1.0 normalized
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub label: String,
}

impl RegionMapper {
    /// Extract per-zone colors from the sector grid, respecting letterbox
    /// offsets and exclusion zones.
    pub fn extract(
        &self,
        grid: &SectorGrid,
        bars: &LetterboxBars,
    ) -> Vec<ZoneColor> {
        let mut results = Vec::new();

        // Adjust grid sampling window for letterbox bars
        let effective_top = bars.top;
        let effective_bottom = grid.rows - bars.bottom;
        let effective_left = bars.left;
        let effective_right = grid.cols - bars.right;

        // Edge zones
        for (monitor, edge_cfg) in &self.edge_configs {
            let edge_colors = edge_cfg.build_all_regions(
                grid,
                effective_top,
                effective_bottom,
                effective_left,
                effective_right,
            );
            results.extend(edge_colors);
        }

        // Sector zones
        for mapping in &self.sector_mappings {
            let color = mapping.resolve(grid);
            results.push(ZoneColor {
                target: mapping.target.clone(),
                color,
            });
        }

        results
    }
}
```

### 5.4 Custom Region Shapes

Power users can define non-rectangular sampling regions for unusual device geometries:

```rust
pub enum RegionShape {
    /// Standard rectangle (most common, fastest).
    Rect { x: f32, y: f32, width: f32, height: f32 },
    /// Convex or concave polygon (Nanoleaf triangles, L-shaped zones).
    Polygon { vertices: Vec<(f32, f32)> },
    /// Circular region (fan rings, AIO cooler).
    Circle { center: (f32, f32), radius: f32 },
    /// Arc segment of an annular ring (addressable fan ring sectors).
    Arc { center: (f32, f32), inner_r: f32, outer_r: f32, start_angle: f32, sweep: f32 },
}
```

Non-rectangular shapes use point-in-polygon tests against the sector grid (testing ~2304 points, not native pixels), keeping performance trivial.

---

## 6. Color Processing Pipeline

Raw sector colors are unsuitable for direct LED output. The processing pipeline transforms them into visually pleasing, temporally stable LED colors.

```
Raw sector    ┌───────────┐   ┌────────────┐   ┌───────────┐   ┌──────────┐   ┌──────────┐
  colors   ──>│ Letterbox │──>│ Aggregate  │──>│ Saturate  │──>│  Black   │──>│ Temporal │──> LED
              │  Adjust   │   │ (avg/dom)  │   │  & Boost  │   │  Level   │   │  Smooth  │   Color
              └───────────┘   └────────────┘   └───────────┘   └──────────┘   └──────────┘
```

### 6.1 Downsampling Strategy

The capture pipeline reduces data volume in three stages:

```
Stage 1: Backend resolution negotiation
  Native (e.g., 2560x1440) → Requested (640x360)
  Reduction: ~16x
  Who: Compositor GPU scaler (PipeWire) or CPU resize (xcap)
  Cost: Near-zero for PipeWire, ~0.5ms for xcap

Stage 2: Sector grid computation
  640x360 (230,400 px) → 64x36 (2,304 sectors)
  Reduction: 100x
  Who: GPU compute shader (DMA-BUF) or CPU box filter
  Cost: ~0.2ms GPU, ~0.3ms CPU

Stage 3: Region mapping
  2,304 sectors → 60–200 LED zone colors
  Reduction: ~15x
  Who: CPU — trivial arithmetic
  Cost: <0.05ms

Total reduction: Native to LED colors = ~24,000x fewer values to process.
```

### 6.2 Dominant Color Extraction

When `AggregationMethod::Dominant` is configured, the hue-histogram approach extracts the most perceptually prominent color from a region instead of averaging everything into mud.

```rust
pub fn dominant_color(pixels: &[Rgb], bucket_count: u32) -> Rgb {
    let mut hue_buckets = vec![0u32; bucket_count as usize];
    let mut hue_accum: Vec<Vec<Hsl>> = vec![vec![]; bucket_count as usize];

    for px in pixels {
        let hsl = px.to_hsl();

        // Skip near-black (L < 0.08) and near-gray (S < 0.10):
        // these contribute noise, not signal.
        if hsl.lightness < 0.08 || hsl.saturation < 0.10 {
            continue;
        }

        let bucket = ((hsl.hue / 360.0) * bucket_count as f32) as usize;
        let bucket = bucket.min(bucket_count as usize - 1);
        hue_buckets[bucket] += 1;
        hue_accum[bucket].push(hsl);
    }

    // Find the most populated hue bucket
    let peak = hue_buckets.iter()
        .enumerate()
        .max_by_key(|(_, &count)| count)
        .map(|(i, _)| i)
        .unwrap_or(0);

    // If no colorful pixels found, return the average (it's a gray/dark frame)
    if hue_accum[peak].is_empty() {
        return average_color(pixels);
    }

    // Average all pixels within the peak bucket
    average_hsl(&hue_accum[peak]).to_rgb()
}
```

**When to use which:**

| Method | Pros | Cons | Best for |
|---|---|---|---|
| Average | Fast, stable, simple | Muddy when regions have mixed colors | Sector grid zones, large areas |
| Dominant | Perceptually accurate, bold | Slightly more CPU, can jump between hues | Edge zones, small strips |

Default behavior: dominant for edge-mapped zones, average for sector-mapped zones. Configurable per zone.

### 6.3 Saturation Boosting

Screen content is designed for direct-view displays with wide gamuts. LEDs viewed peripherally in ambient light need more punch to register visually.

```rust
pub struct SaturationBoost {
    /// Multiplier applied to HSL saturation. Range: 0.5–2.5. Default: 1.4.
    pub factor: f32,
}

impl SaturationBoost {
    pub fn apply(&self, color: Rgb) -> Rgb {
        let mut hsl = color.to_hsl();
        hsl.saturation = (hsl.saturation * self.factor).clamp(0.0, 1.0);
        hsl.to_rgb()
    }
}
```

Recommended defaults by use case:

| Profile | Factor | Rationale |
|---|---|---|
| Cinema | 1.3 | Subtle — preserve director's intent |
| TV | 1.4 | Moderate boost for typical viewing |
| Gaming | 1.5–1.6 | Punchy — match gaming aesthetic |
| Reactive | 1.8 | Maximum visual impact for music vis |

### 6.4 Black Level Handling

When a screen region is dark (loading screens, dark scenes, letterbox bars), raw output would produce dead-black LEDs — visually jarring. Four strategies:

```rust
pub struct BlackLevelConfig {
    /// Luminance threshold below which content is "black" (0.0–1.0).
    pub threshold: f32, // Default: 0.03

    /// What to do when a zone is below threshold.
    pub strategy: BlackStrategy,

    /// For DimAmbient: the warm white color.
    pub ambient_color: Rgb, // Default: Rgb(255, 200, 150)

    /// For DimAmbient: brightness of the ambient glow (0.0–1.0).
    pub ambient_brightness: f32, // Default: 0.05

    /// For LastColorFade: seconds to fade from last color to black.
    pub fade_duration: f32, // Default: 3.0

    /// For BiasLighting: static color (usually D65 white).
    pub bias_color: Rgb, // Default: Rgb(255, 255, 255)
    pub bias_brightness: f32, // Default: 0.05
}

pub enum BlackStrategy {
    /// LEDs turn off. Clean, but can be jarring.
    Off,
    /// Hold a very dim warm white. Cozy, prevents total darkness.
    DimAmbient,
    /// Fade from the last non-black color to off over `fade_duration` seconds.
    LastColorFade,
    /// Static neutral light for eye strain reduction.
    BiasLighting,
}
```

**Implementation in the processing pipeline:**

```rust
impl BlackLevelProcessor {
    pub fn process(&mut self, zone: &mut ZoneColor, dt: f32) {
        let luminance = zone.color.luminance(); // 0.0–1.0

        if luminance < self.config.threshold {
            match self.config.strategy {
                BlackStrategy::Off => {
                    zone.color = Rgb::BLACK;
                }
                BlackStrategy::DimAmbient => {
                    zone.color = self.config.ambient_color
                        .scale(self.config.ambient_brightness);
                }
                BlackStrategy::LastColorFade => {
                    // Track time spent below threshold
                    self.black_duration += dt;
                    let fade_t = (self.black_duration / self.config.fade_duration)
                        .clamp(0.0, 1.0);
                    zone.color = self.last_bright_color.lerp(&Rgb::BLACK, fade_t);
                }
                BlackStrategy::BiasLighting => {
                    zone.color = self.config.bias_color
                        .scale(self.config.bias_brightness);
                }
            }
        } else {
            self.last_bright_color = zone.color;
            self.black_duration = 0.0;
        }
    }
}
```

### 6.5 Temporal Smoothing — Adaptive EMA with Scene-Cut Detection

The most critical processing stage. Without smoothing, LEDs flicker madly during action scenes, UI transitions, and fast camera pans. The smoother uses an exponential moving average (EMA) with an adaptive alpha that responds to the magnitude of change.

```rust
pub struct TemporalSmoother {
    /// Base smoothing factor. 0.0 = frozen, 1.0 = no smoothing.
    /// Typical range: 0.08 (cinema) to 0.50 (gaming).
    alpha: f32,

    /// Scene-cut detection threshold (normalized color distance).
    /// When the per-zone change exceeds this, alpha is temporarily boosted
    /// to let the new scene "snap in" quickly.
    scene_cut_threshold: f32, // Default: 0.55

    /// Multiplier applied to alpha during a detected scene cut.
    scene_cut_boost: f32, // Default: 3.0 (clamped to 1.0 max)

    /// Below this delta, reduce alpha further for ultra-stable output
    /// during static scenes.
    static_threshold: f32, // Default: 0.04

    /// Multiplier applied to alpha during static scenes.
    static_dampen: f32, // Default: 0.5

    /// Previous frame's smoothed colors, one per zone.
    prev_colors: Vec<Rgb>,
}
```

**The smoothing algorithm:**

```rust
impl TemporalSmoother {
    /// Apply temporal smoothing to all zone colors in place.
    ///
    /// # Algorithm: Adaptive EMA with scene-cut detection
    ///
    /// For each zone i:
    ///
    ///   delta_i = color_distance(prev[i], new[i])   // 0.0–1.0 Euclidean in RGB
    ///
    ///   adaptive_alpha_i =
    ///     if delta_i > scene_cut_threshold:
    ///       min(alpha * scene_cut_boost, 1.0)        // snap to new scene
    ///     else if delta_i < static_threshold:
    ///       alpha * static_dampen                    // extra smooth for static
    ///     else:
    ///       alpha                                    // normal smoothing
    ///
    ///   smoothed[i] = lerp(prev[i], new[i], adaptive_alpha_i)
    ///   prev[i] = smoothed[i]
    ///
    /// Scene-cut detection uses a global heuristic: if >60% of zones
    /// exceed the scene_cut_threshold simultaneously, ALL zones get the
    /// boosted alpha. This prevents half the screen snapping while the
    /// other half lags — the entire frame transitions together.
    ///
    pub fn apply(&mut self, zone_colors: &mut Vec<ZoneColor>) {
        // First pass: check if this is a global scene cut
        if self.prev_colors.len() != zone_colors.len() {
            // Zone count changed (config reload, monitor hot-plug).
            // Reset state — no smoothing this frame.
            self.prev_colors = zone_colors.iter().map(|zc| zc.color).collect();
            return;
        }

        let mut scene_cut_zones = 0u32;
        let total_zones = zone_colors.len() as u32;

        // Compute per-zone deltas
        let deltas: Vec<f32> = zone_colors.iter()
            .zip(self.prev_colors.iter())
            .map(|(new, prev)| color_distance(*prev, new.color))
            .collect();

        for &d in &deltas {
            if d > self.scene_cut_threshold {
                scene_cut_zones += 1;
            }
        }

        // Global scene cut: >60% of zones changed dramatically
        let global_scene_cut = total_zones > 0
            && (scene_cut_zones as f32 / total_zones as f32) > 0.6;

        // Second pass: apply adaptive alpha per zone
        for (i, zc) in zone_colors.iter_mut().enumerate() {
            let delta = deltas[i];

            let effective_alpha = if global_scene_cut {
                (self.alpha * self.scene_cut_boost).min(1.0)
            } else if delta > self.scene_cut_threshold {
                (self.alpha * self.scene_cut_boost).min(1.0)
            } else if delta < self.static_threshold {
                self.alpha * self.static_dampen
            } else {
                self.alpha
            };

            let prev = self.prev_colors[i];
            let smoothed = Rgb::new(
                lerp_u8(prev.r, zc.color.r, effective_alpha),
                lerp_u8(prev.g, zc.color.g, effective_alpha),
                lerp_u8(prev.b, zc.color.b, effective_alpha),
            );

            zc.color = smoothed;
            self.prev_colors[i] = smoothed;
        }
    }
}

/// Normalized Euclidean distance in RGB space.
/// Returns 0.0 (identical) to 1.0 (black vs white).
fn color_distance(a: Rgb, b: Rgb) -> f32 {
    let dr = (a.r as f32 - b.r as f32) / 255.0;
    let dg = (a.g as f32 - b.g as f32) / 255.0;
    let db = (a.b as f32 - b.b as f32) / 255.0;
    (dr * dr + dg * dg + db * db).sqrt() / 3.0_f32.sqrt()
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t) as u8
}
```

**Smoothing profiles (recommended defaults):**

| Profile | Alpha | Scene-Cut Threshold | Boost | Static Dampen | Feel |
|---|---|---|---|---|---|
| Cinema | 0.08–0.12 | 0.55 | 3.0 | 0.4 | Glacial, dreamy transitions |
| TV | 0.15–0.25 | 0.55 | 3.0 | 0.5 | Smooth but responsive |
| Gaming | 0.30–0.50 | 0.50 | 2.5 | 0.6 | Snappy, tracks action |
| Reactive | 0.60–0.80 | 0.40 | 2.0 | 0.8 | Music visualizer, maximum response |
| Instant | 1.0 | — | — | — | Raw output, debug only |

### 6.6 Letterbox Detection

Detects black bars from non-native aspect ratio content (21:9 movies on 16:9 monitors, 4:3 content with pillarboxing). When detected, edge sampling regions shift inward to sample actual content.

```rust
pub struct LetterboxDetector {
    /// Frames a bar must persist before it's trusted. Default: 15 (~0.5s at 30fps).
    pub confidence_frames: u32,
    /// Maximum average luminance for a row/column to count as "black". Default: 0.02.
    pub black_threshold: f32,
    /// Minimum bar size as fraction of screen dimension. Default: 0.05.
    pub min_bar_fraction: f32,

    // Internal state
    top_consecutive: u32,
    bottom_consecutive: u32,
    left_consecutive: u32,
    right_consecutive: u32,

    /// Currently confirmed bar sizes (in sector grid rows/columns).
    pub detected: LetterboxBars,
}

pub struct LetterboxBars {
    pub top: u32,    // Rows of black at top
    pub bottom: u32, // Rows of black at bottom
    pub left: u32,   // Columns of black at left
    pub right: u32,  // Columns of black at right
}

impl LetterboxDetector {
    /// Analyze the sector grid for black bars. Updates internal confidence
    /// counters and returns the current detected bars.
    pub fn analyze(&mut self, grid: &SectorGrid) -> &LetterboxBars {
        let min_rows = (self.min_bar_fraction * grid.rows as f32) as u32;
        let min_cols = (self.min_bar_fraction * grid.cols as f32) as u32;

        // Scan from top: count consecutive rows where avg luminance < threshold
        let top_black = self.count_black_rows_from_top(grid);
        self.update_confidence(&mut self.top_consecutive, &mut self.detected.top,
            top_black, min_rows);

        // Repeat for bottom, left, right
        let bottom_black = self.count_black_rows_from_bottom(grid);
        self.update_confidence(&mut self.bottom_consecutive, &mut self.detected.bottom,
            bottom_black, min_rows);

        let left_black = self.count_black_cols_from_left(grid);
        self.update_confidence(&mut self.left_consecutive, &mut self.detected.left,
            left_black, min_cols);

        let right_black = self.count_black_cols_from_right(grid);
        self.update_confidence(&mut self.right_consecutive, &mut self.detected.right,
            right_black, min_cols);

        &self.detected
    }

    fn update_confidence(
        &self,
        consecutive: &mut u32,
        detected: &mut u32,
        measured: u32,
        minimum: u32,
    ) {
        if measured >= minimum {
            *consecutive += 1;
            if *consecutive >= self.confidence_frames {
                *detected = measured;
            }
        } else {
            *consecutive = 0;
            *detected = 0; // Bars gone — immediately clear
        }
    }

    fn count_black_rows_from_top(&self, grid: &SectorGrid) -> u32 {
        let mut count = 0;
        for row in 0..grid.rows {
            let avg_lum = (0..grid.cols)
                .map(|col| grid.get(col, row).luminance())
                .sum::<f32>() / grid.cols as f32;
            if avg_lum < self.black_threshold {
                count += 1;
            } else {
                break;
            }
        }
        count
    }
    // count_black_rows_from_bottom, count_black_cols_from_left/right follow
    // the same pattern scanning from their respective edges.
}
```

---

## 7. ScreenData — Output Type

`ScreenData` is the output of the screen capture pipeline, consumed either by effects (as a reactive input) or pushed directly to device backends (bypassing the effect engine).

```rust
/// Output of the screen capture pipeline for a single frame.
pub struct ScreenData {
    /// Per-zone colors, ready for device output.
    /// Each entry maps to a specific device zone (edge LED, sector, custom region).
    pub zone_colors: Vec<ZoneColor>,

    /// Timestamp of the captured frame (from the display server).
    pub capture_timestamp: Instant,

    /// Resolution of the frame that produced these colors.
    pub frame_resolution: (u32, u32),
}

pub struct ZoneColor {
    /// Target device and zone this color is for.
    pub target: DeviceZoneRef,
    /// Processed, smoothed color ready for output.
    pub color: Rgb,
}

impl ScreenData {
    /// Get the color for a specific device zone. Returns None if the zone
    /// isn't mapped in this capture configuration.
    pub fn color_for(&self, device_id: &str, zone_name: &str) -> Option<Rgb> {
        self.zone_colors.iter()
            .find(|zc| zc.target.device_id == device_id && zc.target.zone_name == zone_name)
            .map(|zc| zc.color)
    }

    /// Get all colors as a flat slice, ordered by zone_colors index.
    /// Useful for bulk output to WLED DDP (which takes an ordered color array).
    pub fn as_color_slice(&self) -> Vec<Rgb> {
        self.zone_colors.iter().map(|zc| zc.color).collect()
    }

    /// The full sector grid snapshot (optional, for effects that want to
    /// use screen content as a reactive input rather than direct output).
    pub fn sector_grid(&self) -> Option<&SectorGrid> {
        // Available when the pipeline retains the grid for effect consumption.
        // None in direct-to-device mode (grid is discarded after region mapping).
        None // Implementation stores this optionally based on config
    }
}
```

### Integration Paths

```
Path 1: Direct-to-device (bypass effect engine)
  ScreenCaptureInput::sample()
    → ScreenData { zone_colors }
    → Device backends consume zone_colors directly
    → Effect engine is idle

Path 2: Reactive input (screen content feeds an effect)
  ScreenCaptureInput::sample()
    → InputData::ScreenCapture(ScreenData)
    → Effect engine reads ScreenData as a reactive input
    → Effect renders to canvas using screen colors as parameters
    → Normal spatial sampling → device output

Path 3: Hybrid (edge zones direct, interior zones via effect)
  ScreenCaptureInput produces ScreenData
    → Edge zone_colors → device backends directly
    → Sector grid → effect engine as reactive input
    → Effect output → spatial sampling → ambient device output
```

---

## 8. Performance Budget

### 8.1 Target: <5% CPU at 1080p/30fps

The screen capture pipeline must not visibly affect system performance, especially during gaming. The budget applies to the entire pipeline from frame capture through LED color output.

| Stage | Target | Hard Limit | Notes |
|---|---|---|---|
| Frame capture (PipeWire DMA-BUF) | ~0.1ms | 1.0ms | Zero-copy — just a pointer swap |
| Frame capture (PipeWire MemPtr) | ~0.5ms | 2.0ms | Shared memory read |
| Frame capture (XShm, 640x360) | ~0.5ms | 2.0ms | Shared memory blit at reduced resolution |
| Frame capture (xcap, 1080p→640x360) | ~2.0ms | 4.0ms | Full capture + CPU resize |
| Sector grid computation (GPU) | ~0.2ms | 0.5ms | Compute shader, 9KB readback |
| Sector grid computation (CPU) | ~0.3ms | 1.0ms | Box filter over 230K pixels |
| Region mapping | ~0.05ms | 0.1ms | Trivial arithmetic over ~200 zones |
| Color processing | ~0.05ms | 0.2ms | Saturation, black level, white balance |
| Temporal smoothing | ~0.02ms | 0.05ms | EMA over ~200 color values |
| **Total (PipeWire + GPU)** | **~0.42ms** | **1.85ms** | **<0.5% CPU at 30fps** |
| **Total (XShm + CPU)** | **~0.92ms** | **3.35ms** | **<3% CPU at 30fps** |
| **Total (xcap + CPU)** | **~2.42ms** | **6.35ms** | **<5% CPU at 30fps** |

### 8.2 Memory Budget

| Component | Size | Notes |
|---|---|---|
| Staging buffer (640x360 BGRA) | 921 KB | One allocation, reused every frame |
| Sector grid (64x36 RGB) | 6.9 KB | Reused |
| Zone colors (~200 zones) | 0.6 KB | Reused |
| Smoother prev_colors (~200) | 0.6 KB | Reused |
| PipeWire stream buffers (x4) | ~3.6 MB | Owned by PipeWire, not us |
| **Total owned** | **~1 MB** | Well under 20 MB target |

### 8.3 Frame Timing

The capture pipeline runs on its own thread, decoupled from the main render loop. Frame delivery uses a lock-free single-producer/single-consumer double buffer:

```
Capture thread:     [capture] → [process] → [write to back buffer] → [swap]
                         ↕ (30fps cadence)
Render thread:      [read front buffer] → [output to devices]
                         ↕ (60fps cadence, or same as capture fps)
```

The render loop never blocks on capture. If a capture frame isn't ready, the render loop reuses the last frame. If the capture thread falls behind, it drops frames (not queues them — freshness beats completeness for ambient lighting).

---

## 9. Adaptive Quality

Under high system load (typically during gaming), the capture pipeline automatically degrades quality to stay within its performance budget. Quality reduction is imperceptible for ambient lighting — LEDs don't need pixel-accurate sampling.

### Quality Tiers

```rust
pub struct AdaptiveConfig {
    pub enabled: bool,

    /// Resolution steps, ordered from highest to lowest quality.
    /// Default: [(640, 360), (320, 180), (160, 90), (80, 45)]
    pub resolution_steps: Vec<(u32, u32)>,

    /// FPS steps, ordered from highest to lowest.
    /// Default: [60, 30, 15]
    pub fps_steps: Vec<u32>,

    /// If average frame capture time exceeds this, step down quality.
    pub high_latency_threshold: Duration, // Default: 4ms

    /// If average frame capture time is below this for 30+ frames, step up.
    pub low_latency_threshold: Duration, // Default: 2ms

    /// Number of frames to average before making a decision.
    pub evaluation_window: u32, // Default: 30
}
```

### Quality Controller

```rust
pub struct QualityController {
    config: AdaptiveConfig,
    frame_times: VecDeque<Duration>,

    current_resolution_idx: usize,
    current_fps_idx: usize,

    /// Frames since last quality change. Prevents oscillation.
    cooldown: u32,
}

pub enum QualityAdjustment {
    /// No change needed.
    Hold,
    /// Reduce resolution one step.
    ReduceResolution { new: (u32, u32) },
    /// Reduce FPS one step.
    ReduceFps { new: u32 },
    /// Increase resolution one step (system has headroom).
    IncreaseResolution { new: (u32, u32) },
    /// Increase FPS one step.
    IncreaseFps { new: u32 },
}

impl QualityController {
    pub fn evaluate(&mut self, frame_time: Duration) -> Option<QualityAdjustment> {
        if !self.config.enabled { return None; }

        self.frame_times.push_back(frame_time);
        if self.frame_times.len() > self.config.evaluation_window as usize {
            self.frame_times.pop_front();
        }

        // Don't decide until we have a full window
        if self.frame_times.len() < self.config.evaluation_window as usize {
            return None;
        }

        // Cooldown prevents oscillation (minimum 60 frames between changes)
        self.cooldown = self.cooldown.saturating_sub(1);
        if self.cooldown > 0 { return None; }

        let avg = self.average_frame_time();

        if avg > self.config.high_latency_threshold {
            self.cooldown = 60;
            // Reduce resolution first, then FPS
            if self.current_resolution_idx < self.config.resolution_steps.len() - 1 {
                self.current_resolution_idx += 1;
                let new = self.config.resolution_steps[self.current_resolution_idx];
                Some(QualityAdjustment::ReduceResolution { new })
            } else if self.current_fps_idx < self.config.fps_steps.len() - 1 {
                self.current_fps_idx += 1;
                let new = self.config.fps_steps[self.current_fps_idx];
                Some(QualityAdjustment::ReduceFps { new })
            } else {
                None // Already at minimum quality
            }
        } else if avg < self.config.low_latency_threshold {
            self.cooldown = 60;
            // Recover: increase FPS first, then resolution
            if self.current_fps_idx > 0 {
                self.current_fps_idx -= 1;
                let new = self.config.fps_steps[self.current_fps_idx];
                Some(QualityAdjustment::IncreaseFps { new })
            } else if self.current_resolution_idx > 0 {
                self.current_resolution_idx -= 1;
                let new = self.config.resolution_steps[self.current_resolution_idx];
                Some(QualityAdjustment::IncreaseResolution { new })
            } else {
                None // Already at maximum quality
            }
        } else {
            None // Within acceptable range
        }
    }

    fn average_frame_time(&self) -> Duration {
        let sum: Duration = self.frame_times.iter().copied().sum();
        sum / self.frame_times.len() as u32
    }
}
```

### Degradation Sequence

```
Normal:       640x360 @ 60fps  →  barely any CPU
Mild load:    640x360 @ 30fps  →  halved capture rate
Moderate:     320x180 @ 30fps  →  4x fewer pixels
Heavy:        160x90  @ 30fps  →  16x fewer pixels
Extreme:      160x90  @ 15fps  →  minimum viable ambient
Gaming:        80x45  @ 15fps  →  absolute minimum (still fine for LEDs)
```

At every tier, the ambient lighting quality remains perceptually good. The human eye doesn't distinguish "64x36 sector grid sampled from 640x360" from "32x18 sector grid sampled from 160x90" when the output is 60 LEDs.

---

## 10. Cross-Platform Strategy

Hypercolor is a Linux-first project, but screen capture should work on Windows (for development) and eventually macOS.

### Platform Matrix

| Capability | Linux (Wayland) | Linux (X11) | Windows | macOS |
|---|---|---|---|---|
| **Primary backend** | PipeWire + Portal | XShm | xcap (DXGI/WGC) | xcap (SCKit) |
| **DMA-BUF zero-copy** | Yes | No | No | No |
| **Streaming mode** | Yes (PipeWire) | No | No | No |
| **Permission model** | Portal dialog + restore token | None (open access) | App capability | Screen Recording permission |
| **Multi-monitor** | Portal multi-select | Root window spans all | Per-monitor via xcap | Per-monitor via xcap |
| **GPU downsample** | Yes (wgpu + DMA-BUF) | No (CPU only) | Possible (wgpu) | Possible (wgpu) |
| **Feature gate** | `screen-pipewire` | `screen-x11` | Default (xcap) | Default (xcap) |
| **Performance** | Excellent (<1% CPU) | Good (2-5% CPU) | Moderate (3-7% CPU) | Moderate (3-7% CPU) |

### What Works on Windows (Development)

Windows developers can work on the full capture pipeline using the `xcap` backend:

- `XcapCapture` works out of the box on Windows via DXGI desktop duplication or Windows Graphics Capture.
- `SectorGrid`, `RegionMapper`, `ColorProcessor`, `TemporalSmoother` are all pure Rust, platform-independent.
- `ScreenData` output type is identical across platforms.
- WLED DDP output works over the network (no USB dependencies).

**What doesn't work on Windows:**
- PipeWire backend (`#[cfg(target_os = "linux")]`).
- XShm backend (`#[cfg(target_os = "linux")]`).
- Portal restore tokens (Linux-specific D-Bus).
- DMA-BUF GPU zero-copy path.

### Conditional Compilation

```rust
// In the backend module:
#[cfg(target_os = "linux")]
mod pipewire;

#[cfg(target_os = "linux")]
mod xshm;

// Always available:
mod xcap_backend;

// Re-export the auto-detection function (handles cfg internally)
pub use auto_detect::auto_detect_backend;
```

### Build Profiles

```toml
# Linux production build — all backends
[target.'cfg(target_os = "linux")'.dependencies]
pipewire = { version = "0.8", optional = true }
x11 = { version = "2.21", optional = true }

# All platforms — universal fallback
[dependencies]
xcap = "0.0.13"
image = "0.25"

# Dev profile: minimal dependencies for fast iteration
[profile.dev.package."*"]
opt-level = 2  # Optimize deps even in debug (image processing is slow at -O0)
```

---

## Appendix A: Error Types

```rust
pub enum CaptureError {
    /// No suitable backend found for the current environment.
    NoBackendAvailable,
    /// User-specified backend not compiled in (missing feature flag).
    BackendNotCompiled(String),
    /// Unknown backend name in config.
    UnknownBackend(String),
    /// PipeWire portal denied screen access.
    PortalDenied,
    /// PipeWire portal timed out waiting for user approval.
    PortalTimeout,
    /// PipeWire stream disconnected (monitor hot-unplug, compositor crash).
    StreamDisconnected,
    /// Monitor specified in config not found.
    MonitorNotFound(String),
    /// XShm extension not available.
    XShmUnavailable,
    /// Generic backend failure with message.
    BackendFailed(String),
}
```

## Appendix B: Transition Speed Quick Reference

For tuning the temporal smoother across different content types:

```
Alpha   Behavior                       Frames to 90% settle (at 30fps)
─────   ─────────────────────────────  ─────────────────────────────────
0.05    Near-frozen, glacial drift     ~45 frames (1.5s)
0.10    Very slow, cinematic           ~22 frames (0.7s)
0.15    Smooth, standard ambient       ~15 frames (0.5s)
0.25    Responsive, TV viewing         ~9 frames  (0.3s)
0.35    Snappy, gaming                 ~6 frames  (0.2s)
0.50    Quick, action-reactive         ~4 frames  (0.13s)
0.80    Near-instant                   ~2 frames  (0.07s)
1.00    Raw, no smoothing              Immediate
```

Formula: frames to reach X% = `log(1 - X) / log(1 - alpha)`
For 90% settle: `frames = log(0.1) / log(1 - alpha)`
