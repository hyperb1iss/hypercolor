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

pub use sector::{LetterboxBars, SectorGrid};
pub use smooth::TemporalSmoother;

use crate::input::traits::{InputData, InputSource, ScreenData};
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
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.latest_colors = None;
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

        Ok(InputData::Screen(ScreenData { zone_colors }))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
