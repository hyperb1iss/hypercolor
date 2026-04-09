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

pub mod sector;
pub mod smooth;
#[cfg(target_os = "linux")]
pub mod wayland;

pub use sector::{LetterboxBars, SectorGrid};
pub use smooth::TemporalSmoother;
#[cfg(target_os = "linux")]
pub use wayland::WaylandScreenCaptureInput;

use crate::input::traits::{InputData, InputSource, ScreenData};
use crate::types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, PublishedSurface};
use crate::types::event::ZoneColors;

// ── CaptureConfig ─────────────────────────────────────────────────────────

/// Runtime configuration for the screen capture input source.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Which monitor to capture. Default: `MonitorSelect::Primary`.
    pub monitor: MonitorSelect,

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

    /// Whether letterbox detection is enabled. Default: true.
    pub letterbox_enabled: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            monitor: MonitorSelect::Primary,
            target_fps: 30,
            grid_cols: 8,
            grid_rows: 6,
            smoothing_alpha: 0.3,
            scene_cut_threshold: 100.0,
            letterbox_threshold: 0.02,
            letterbox_enabled: true,
        }
    }
}

/// Which display to capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorSelect {
    /// The compositor's primary/focused output.
    Primary,
    /// A specific output by name (e.g., `"DP-1"`, `"HDMI-A-1"`).
    ByName(String),
    /// A specific output by index (0-based).
    ByIndex(u32),
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

    /// Latest processed zone colors (after grid + smoothing).
    latest_colors: Option<Vec<[u8; 3]>>,

    /// Latest zone IDs corresponding to `latest_colors`.
    latest_zone_ids: Vec<String>,

    /// Latest downscaled capture frame for screen-reactive effects.
    latest_canvas_downscale: Option<PublishedSurface>,

    /// Whether the source is actively capturing.
    running: bool,

    /// Frame dimensions from the most recent push.
    frame_width: u32,
    frame_height: u32,

    /// Detected letterbox bars from the most recent frame.
    letterbox: LetterboxBars,
}

impl ScreenCaptureInput {
    /// Create a new screen capture input with the given configuration.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        let smoother = TemporalSmoother::new(config.smoothing_alpha, config.scene_cut_threshold);

        Self {
            config,
            smoother,
            latest_colors: None,
            latest_zone_ids: Vec::new(),
            latest_canvas_downscale: None,
            running: false,
            frame_width: 0,
            frame_height: 0,
            letterbox: LetterboxBars::default(),
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
        self.frame_width = width;
        self.frame_height = height;
        self.latest_canvas_downscale = downscale_frame(
            frame,
            width,
            height,
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
        );

        // 1. Compute sector grid from raw pixels.
        let grid = SectorGrid::compute(
            frame,
            width,
            height,
            self.config.grid_cols,
            self.config.grid_rows,
        );

        // 2. Detect letterbox bars (if enabled).
        if self.config.letterbox_enabled {
            self.letterbox = grid.detect_letterbox(self.config.letterbox_threshold);
        }

        // 3. Get zone colors — crop letterbox if bars detected, else use full grid.
        let effective_grid = if self.letterbox.has_bars() {
            grid.crop_letterbox(&self.letterbox).unwrap_or(grid)
        } else {
            grid
        };

        let zone_data = effective_grid.to_zone_colors();
        let mut colors: Vec<[u8; 3]> = zone_data.iter().map(|(_, c)| *c).collect();
        self.latest_zone_ids = zone_data.into_iter().map(|(id, _)| id).collect();

        // 4. Apply temporal smoothing.
        self.smoother.apply(&mut colors);

        self.latest_colors = Some(colors);
    }

    /// Current configuration.
    #[must_use]
    pub fn config(&self) -> &CaptureConfig {
        &self.config
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
        self.running = true;
        self.smoother.reset();
        self.latest_colors = None;
        self.latest_canvas_downscale = None;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.latest_colors = None;
        self.latest_canvas_downscale = None;
        self.smoother.reset();
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

        Ok(InputData::Screen(ScreenData {
            zone_colors,
            grid_width: self.config.grid_cols,
            grid_height: self.config.grid_rows,
            canvas_downscale: self.latest_canvas_downscale.clone(),
            source_width: self.frame_width,
            source_height: self.frame_height,
        }))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

fn downscale_frame(
    frame: &[u8],
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
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

    let mut canvas = Canvas::new(target_width, target_height);
    let bytes = canvas.as_rgba_bytes_mut();
    let src_width = usize::try_from(width).ok()?;
    let target_width_usize = usize::try_from(target_width).ok()?;

    for y in 0..target_height {
        let src_y = u32::min(
            (u64::from(y) * u64::from(height) / u64::from(target_height))
                .try_into()
                .ok()
                .unwrap_or_default(),
            height.saturating_sub(1),
        );
        let src_row = usize::try_from(src_y).ok()?;
        for x in 0..target_width {
            let src_x = u32::min(
                (u64::from(x) * u64::from(width) / u64::from(target_width))
                    .try_into()
                    .ok()
                    .unwrap_or_default(),
                width.saturating_sub(1),
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

    Some(PublishedSurface::from_owned_canvas(canvas, 0, 0))
}
