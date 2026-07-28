//! Screen capture input source — ambient lighting driven by screen content.
//!
//! Implements [`InputSource`] for screen capture, producing [`ScreenData`]
//! with per-zone colors extracted from a sector grid overlay. The actual
//! screen capture backend (xcap, `PipeWire`, etc.) is external — this module
//! provides the pure analysis pipeline: sector grid computation, letterbox
//! detection, temporal smoothing, and zone mapping.
//!
//! # Architecture
//!
//! ```text
//! Raw RGBA pixels ──> SectorGrid ──> LetterboxDetect ──> TemporalSmoother ──> ZoneColors
//! ```
//!
//! The capture backend feeds raw pixel buffers. Everything downstream is
//! backend-agnostic and testable with synthetic data.

mod frame;
mod process;
pub mod sector;
pub mod smooth;
pub mod tune;
#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(target_os = "windows")]
pub mod windows;

pub use frame::{
    CaptureColorSpace, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame, CaptureFrameError,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CapturePlaneLease, CapturePlanePool,
    CaptureRotation, CaptureSourceId, CaptureStageKind, CaptureStorage, CaptureTransferFunction,
    CpuCaptureStorage, GeometryNormalizedCaptureSurface, MoveRegion, PhysicalOrigin, PixelExtent,
    PixelRect, PlatformGpuApi, PlatformGpuSurface, PooledCapturePlane, RawCaptureSurface,
    SourceScale,
};
pub use process::CaptureFrameProcessor;
pub use sector::{LetterboxBars, SectorGrid};
pub use smooth::TemporalSmoother;
pub use tune::ColorTuning;
#[cfg(target_os = "linux")]
pub use wayland::WaylandScreenCaptureInput;
#[cfg(target_os = "windows")]
pub use windows::{CaptureSourceSink, ResolvedCaptureSource, WindowsScreenCaptureInput};

use crate::input::traits::{InputData, InputSource, ScreenData};
use crate::input::{SourceKind, SourceStatusHandle, SourceStatusReporter};
use crate::types::canvas::{
    DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, PublishedSurface, RenderSurfacePool,
    SurfaceDescriptor,
};
use crate::types::event::ZoneColors;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct AnalyzedScreenSnapshot {
    geometry_frame: CaptureFrame<GeometryNormalizedCaptureSurface>,
    data: ScreenData,
}

impl AnalyzedScreenSnapshot {
    pub(crate) const fn geometry_frame(&self) -> &CaptureFrame<GeometryNormalizedCaptureSurface> {
        &self.geometry_frame
    }

    pub(crate) const fn data(&self) -> &ScreenData {
        &self.data
    }
}

pub(crate) fn analyze_screen_frame(
    analyzer: &mut ScreenCaptureInput,
    frame: CaptureFrame<RawCaptureSurface>,
) -> anyhow::Result<AnalyzedScreenSnapshot> {
    let captured_at = frame.metadata().captured_at;
    let frame = analyzer.capture_processor.process(frame)?;
    let geometry = &frame.metadata().geometry;
    let CaptureStorage::Cpu(storage) = frame.storage() else {
        anyhow::bail!("legacy screen analysis requires CPU storage");
    };
    let extent = geometry.storage_extent();
    let pixels = storage
        .tightly_packed_rgba8(extent)
        .ok_or_else(|| anyhow::anyhow!("legacy screen analysis requires tightly packed RGBA8"))?;
    analyzer.push_frame_at(pixels, extent.width(), extent.height(), captured_at);
    let InputData::Screen(data) = analyzer.sample()? else {
        anyhow::bail!("legacy screen analysis did not produce a snapshot");
    };
    Ok(AnalyzedScreenSnapshot {
        geometry_frame: frame,
        data,
    })
}

// ── CaptureConfig ─────────────────────────────────────────────────────────

/// Runtime configuration for the screen capture input source.
///
/// The capture source itself is chosen through the desktop portal picker;
/// `restore_token` carries the persisted choice back into the portal.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    /// Target capture frames per second. Default: 30.
    pub target_fps: u32,

    /// Sector grid columns (horizontal divisions). Default: 8.
    pub grid_cols: u32,

    /// Sector grid rows (vertical divisions). Default: 6.
    pub grid_rows: u32,

    /// Temporal smoothing factor (0.0 = frozen, 1.0 = raw). Default: 0.3.
    pub smoothing_alpha: f32,

    /// Scene-cut detection threshold for the temporal smoother. Default: 100.0.
    pub scene_cut_threshold: f32,

    /// Luminance threshold for letterbox detection (0.0 - 1.0). Default: 0.02.
    pub letterbox_threshold: f32,

    /// Whether letterbox detection is enabled. Default: false, matching
    /// `config::CaptureConfig` — ambient lighting mirrors desktops far more
    /// often than letterboxed film, and dark desktop content trips the
    /// detector into cropping real picture away.
    pub letterbox_enabled: bool,

    /// Color tuning applied to zone colors after smoothing.
    pub tuning: ColorTuning,

    /// XDG portal restore token from a previous session, if any.
    pub restore_token: Option<String>,

    /// Persisted capture source. Windows accepts `auto`, stable monitor ids,
    /// and legacy numeric indices; the XDG portal owns selection on Linux.
    pub source: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 30,
            grid_cols: 8,
            grid_rows: 6,
            smoothing_alpha: 0.3,
            scene_cut_threshold: 100.0,
            letterbox_threshold: 0.02,
            letterbox_enabled: false,
            tuning: ColorTuning::default(),
            restore_token: None,
            source: "auto".to_owned(),
        }
    }
}

/// Largest size within `max_width` x `max_height` that keeps `width` x
/// `height`'s aspect ratio.
///
/// The screen downscale used to target the canvas bounds directly, which
/// squashed a 16:9 desktop into 4:3 — a 1.33x vertical stretch that no
/// downstream fit mode can undo, because the distortion is already baked
/// into the published surface. Circles came out as ellipses on every
/// screen-mirroring effect.
#[must_use]
pub fn fit_within(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 || max_width == 0 || max_height == 0 {
        return (max_width.max(1), max_height.max(1));
    }

    // Integer comparison of width/height against max_width/max_height,
    // cross-multiplied to avoid floating point entirely.
    let source_is_wider =
        u64::from(width) * u64::from(max_height) >= u64::from(height) * u64::from(max_width);

    if source_is_wider {
        let scaled_height = (u64::from(height) * u64::from(max_width) / u64::from(width))
            .try_into()
            .unwrap_or(max_height);
        (max_width, u32::max(scaled_height, 1))
    } else {
        let scaled_width = (u64::from(width) * u64::from(max_height) / u64::from(height))
            .try_into()
            .unwrap_or(max_width);
        (u32::max(scaled_width, 1), max_height)
    }
}

/// One display output the capture backend can address directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenMonitor {
    /// Legacy zero-based enumeration index for display ordering.
    pub index: usize,
    /// Stable capture source id suitable for persistence.
    pub id: String,
    /// OS device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Desktop width in pixels.
    pub width: u32,
    /// Desktop height in pixels.
    pub height: u32,
    /// Whether this output anchors the virtual desktop origin.
    pub primary: bool,
}

/// Display outputs the capture backend can address by index.
///
/// Empty where the backend owns source selection instead (the XDG portal
/// on Linux) or no backend exists. Emptiness is the capability signal: a
/// UI with monitors shows a picker, a UI without falls back to whatever
/// selection flow the platform provides.
#[must_use]
pub fn available_monitors() -> Vec<ScreenMonitor> {
    #[cfg(target_os = "windows")]
    {
        hypercolor_windows_capture::list_monitors()
            .into_iter()
            .map(|monitor| ScreenMonitor {
                index: monitor.index,
                id: monitor.id,
                name: monitor.name,
                width: monitor.width,
                height: monitor.height,
                primary: monitor.primary,
            })
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Parse a configured capture source into a Windows monitor selector.
///
/// `capture.source` is a free-form string shared across backends. The XDG
/// portal picks its own source and leaves the value at "auto", so this only
/// matters on Windows, which addresses display outputs directly. Stable ids
/// survive adapter/output enumeration reorder; numeric values remain accepted
/// for configuration compatibility.
#[must_use]
pub fn monitor_selector_from_source(source: &str) -> hypercolor_windows_capture::MonitorSelector {
    hypercolor_windows_capture::MonitorSelector::parse(source)
}

// ── ScreenCaptureInput ────────────────────────────────────────────────────

/// Screen capture input source implementing [`InputSource`].
///
/// Owns the sector grid configuration, temporal smoother, and latest frame
/// state. The actual pixel data is pushed in via [`push_frame`] — the
/// capture backend lives outside this struct (behind a feature flag).
///
/// # Usage
///
/// ```rust,ignore
/// let mut input = ScreenCaptureInput::new(CaptureConfig::default());
/// input.start()?;
///
/// // Backend captures a frame and pushes raw RGBA pixels:
/// input.push_frame(&rgba_pixels, width, height);
///
/// // Render loop samples the latest data:
/// let data = input.sample()?;
/// ```
pub struct ScreenCaptureInput {
    /// Runtime configuration.
    config: CaptureConfig,

    /// Temporal smoother for flicker reduction.
    smoother: TemporalSmoother,

    capture_processor: CaptureFrameProcessor,

    /// Latest processed zone colors (after grid + smoothing).
    latest_colors: Option<Vec<[u8; 3]>>,

    /// Latest zone IDs corresponding to `latest_colors`.
    latest_zone_ids: Vec<String>,

    /// Effective sector dimensions after letterbox cropping.
    latest_grid_width: u32,
    latest_grid_height: u32,

    /// Latest downscaled capture frame for screen-reactive effects.
    latest_canvas_downscale: Option<PublishedSurface>,

    downscale_pool: RenderSurfacePool,

    /// Whether the source is actively capturing.
    running: bool,

    /// Frame dimensions from the most recent push.
    frame_width: u32,
    frame_height: u32,

    /// Detected letterbox bars from the most recent frame.
    letterbox: LetterboxBars,
    frame_generation: u64,
    status_frame_generation: u64,
    latest_acquired_at: Option<Instant>,
    status: SourceStatusReporter,
}

impl ScreenCaptureInput {
    /// Create a new screen capture input with the given configuration.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        let smoother = TemporalSmoother::new(config.smoothing_alpha, config.scene_cut_threshold);

        Self {
            config,
            smoother,
            capture_processor: CaptureFrameProcessor::default(),
            latest_colors: None,
            latest_zone_ids: Vec::new(),
            latest_grid_width: 0,
            latest_grid_height: 0,
            latest_canvas_downscale: None,
            downscale_pool: RenderSurfacePool::with_slot_count(
                SurfaceDescriptor::rgba8888(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT),
                2,
            ),
            running: false,
            frame_width: 0,
            frame_height: 0,
            letterbox: LetterboxBars::default(),
            frame_generation: 0,
            status_frame_generation: 0,
            latest_acquired_at: None,
            status: SourceStatusReporter::new(
                "screen_analysis",
                SourceKind::Screen,
                "in_process",
                true,
                true,
                true,
            ),
        }
    }

    /// Push a raw RGBA8 frame into the pipeline.
    ///
    /// Computes the sector grid, detects letterbox bars, applies temporal
    /// smoothing, and stores the result for the next `sample()` call.
    ///
    /// # Arguments
    ///
    /// * `frame` — Raw RGBA8 pixel data, row-major, 4 bytes per pixel.
    /// * `width` — Frame width in pixels.
    /// * `height` — Frame height in pixels.
    pub fn push_frame(&mut self, frame: &[u8], width: u32, height: u32) {
        self.push_frame_at(frame, width, height, Instant::now());
    }

    fn push_frame_at(&mut self, frame: &[u8], width: u32, height: u32, acquired_at: Instant) {
        self.frame_generation = self.frame_generation.wrapping_add(1);
        self.frame_width = width;
        self.frame_height = height;

        // 1. Compute sector grid from raw pixels.
        let grid = SectorGrid::compute(
            frame,
            width,
            height,
            self.config.grid_cols,
            self.config.grid_rows,
        );

        // 2. Detect letterbox bars (if enabled). Stale bars must clear when
        // detection is switched off live, or cropping would continue forever.
        if self.config.letterbox_enabled {
            self.letterbox = grid.detect_letterbox(self.config.letterbox_threshold);
        } else {
            self.letterbox = LetterboxBars::default();
        }

        // 3. Get zone colors — crop letterbox if bars detected, else use full grid.
        let (effective_grid, region) = if self.letterbox.has_bars() {
            grid.crop_letterbox(&self.letterbox).map_or_else(
                || (grid.clone(), FrameRegion::full(width, height)),
                |cropped| {
                    let region = FrameRegion::from_letterbox(
                        width,
                        height,
                        grid.cols(),
                        grid.rows(),
                        self.letterbox,
                    )
                    .unwrap_or_else(|| FrameRegion::full(width, height));
                    (cropped, region)
                },
            )
        } else {
            (grid, FrameRegion::full(width, height))
        };
        let (downscale_width, downscale_height) = fit_within(
            region.width,
            region.height,
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
        );
        self.latest_canvas_downscale = downscale_frame(
            frame,
            width,
            height,
            region,
            downscale_width,
            downscale_height,
            &mut self.downscale_pool,
        );

        let zone_data = effective_grid.to_zone_colors();
        self.latest_grid_width = effective_grid.cols();
        self.latest_grid_height = effective_grid.rows();
        let mut colors: Vec<[u8; 3]> = zone_data.iter().map(|(_, c)| *c).collect();
        self.latest_zone_ids = zone_data.into_iter().map(|(id, _)| id).collect();

        // 4. Apply temporal smoothing, then color tuning on the smoothed output.
        let elapsed = self.latest_acquired_at.map_or(Duration::ZERO, |previous| {
            acquired_at.saturating_duration_since(previous)
        });
        self.smoother.apply_for_elapsed(&mut colors, elapsed);
        self.config.tuning.apply(&mut colors);

        self.latest_colors = Some(colors);
        self.latest_acquired_at = Some(acquired_at);
    }

    /// Current configuration.
    #[must_use]
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// Apply new analysis settings to a running pipeline.
    ///
    /// Grid, smoothing, letterbox, and tuning changes take effect on the next
    /// pushed frame. The smoother resets when the grid shape changes so stale
    /// zone state never blends into the new layout.
    pub fn apply_settings(&mut self, config: CaptureConfig) {
        if config.grid_cols != self.config.grid_cols || config.grid_rows != self.config.grid_rows {
            self.smoother.reset();
        }
        self.smoother.set_alpha(config.smoothing_alpha);
        self.smoother
            .set_scene_cut_threshold(config.scene_cut_threshold);
        self.config = config;
    }

    /// Most recently detected letterbox bars.
    #[must_use]
    pub fn letterbox_bars(&self) -> &LetterboxBars {
        &self.letterbox
    }

    /// Frame dimensions from the most recent push.
    #[must_use]
    pub fn frame_dimensions(&self) -> (u32, u32) {
        (self.frame_width, self.frame_height)
    }
}

impl InputSource for ScreenCaptureInput {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        self.status.begin_session()?;
        self.running = true;
        self.smoother.reset();
        self.latest_colors = None;
        self.latest_grid_width = 0;
        self.latest_grid_height = 0;
        self.latest_canvas_downscale = None;
        self.latest_acquired_at = None;
        self.status_frame_generation = self.frame_generation;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.latest_colors = None;
        self.latest_grid_width = 0;
        self.latest_grid_height = 0;
        self.latest_canvas_downscale = None;
        self.latest_acquired_at = None;
        self.smoother.reset();
        self.status.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let Some(ref colors) = self.latest_colors else {
            return Ok(InputData::None);
        };

        let zone_colors: Vec<ZoneColors> = self
            .latest_zone_ids
            .iter()
            .zip(colors.iter())
            .map(|(zone_id, rgb)| ZoneColors {
                zone_id: zone_id.clone(),
                colors: vec![*rgb],
            })
            .collect();

        if self.status_frame_generation != self.frame_generation {
            if let (Some(status), Some(acquired_at)) =
                (self.status.session(), self.latest_acquired_at)
            {
                let frame_period =
                    Duration::from_secs_f64(1.0 / f64::from(self.config.target_fps.max(1)));
                status.record_sample(acquired_at, acquired_at + frame_period + frame_period, 1)?;
            }
            self.status_frame_generation = self.frame_generation;
        }

        Ok(InputData::Screen(ScreenData {
            zone_colors,
            grid_width: self.latest_grid_width,
            grid_height: self.latest_grid_height,
            canvas_downscale: self.latest_canvas_downscale.clone(),
            source_width: self.frame_width,
            source_height: self.frame_height,
            letterbox: [
                self.letterbox.top,
                self.letterbox.bottom,
                self.letterbox.left,
                self.letterbox.right,
            ],
        }))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

#[derive(Clone, Copy)]
struct FrameRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl FrameRegion {
    const fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn from_letterbox(
        width: u32,
        height: u32,
        cols: u32,
        rows: u32,
        bars: LetterboxBars,
    ) -> Option<Self> {
        let sector_width = width.checked_div(cols)?;
        let sector_height = height.checked_div(rows)?;
        let x = bars.left.checked_mul(sector_width)?;
        let y = bars.top.checked_mul(sector_height)?;
        let right = if bars.right == 0 {
            width
        } else {
            cols.checked_sub(bars.right)?.checked_mul(sector_width)?
        };
        let bottom = if bars.bottom == 0 {
            height
        } else {
            rows.checked_sub(bars.bottom)?.checked_mul(sector_height)?
        };
        let width = right.checked_sub(x)?;
        let height = bottom.checked_sub(y)?;
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }
}

fn downscale_frame(
    frame: &[u8],
    width: u32,
    height: u32,
    region: FrameRegion,
    target_width: u32,
    target_height: u32,
    surface_pool: &mut RenderSurfacePool,
) -> Option<PublishedSurface> {
    if width == 0 || height == 0 || target_width == 0 || target_height == 0 {
        return None;
    }

    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(4))?;
    if frame.len() < expected_len {
        return None;
    }

    let descriptor = SurfaceDescriptor::rgba8888(target_width, target_height);
    if surface_pool.descriptor() != descriptor {
        *surface_pool = RenderSurfacePool::with_slot_count(descriptor, 2);
    }

    let mut lease = surface_pool.dequeue()?;
    let bytes = lease.canvas_mut().as_rgba_bytes_mut();
    let src_width = usize::try_from(width).ok()?;
    let target_width_usize = usize::try_from(target_width).ok()?;

    for y in 0..target_height {
        let region_y =
            u32::try_from(u64::from(y) * u64::from(region.height) / u64::from(target_height))
                .ok()?;
        let src_y = u32::min(
            region.y.checked_add(region_y)?,
            region.y.saturating_add(region.height).saturating_sub(1),
        );
        let src_row = usize::try_from(src_y).ok()?;
        for x in 0..target_width {
            let region_x =
                u32::try_from(u64::from(x) * u64::from(region.width) / u64::from(target_width))
                    .ok()?;
            let src_x = u32::min(
                region.x.checked_add(region_x)?,
                region.x.saturating_add(region.width).saturating_sub(1),
            );
            let src_col = usize::try_from(src_x).ok()?;
            let src_idx = src_row
                .checked_mul(src_width)?
                .checked_add(src_col)?
                .checked_mul(4)?;
            let dst_idx = usize::try_from(y)
                .ok()?
                .checked_mul(target_width_usize)?
                .checked_add(usize::try_from(x).ok()?)?
                .checked_mul(4)?;
            bytes[dst_idx..dst_idx + 4].copy_from_slice(&frame[src_idx..src_idx + 4]);
        }
    }

    Some(lease.submit(0, 0))
}
