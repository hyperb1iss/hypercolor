//! Input source abstraction — trait and data types for audio, screen, and future inputs.
//!
//! [`InputSource`] is the polymorphic entry point for anything that feeds data
//! into the effect pipeline. Each source produces [`InputData`] snapshots that
//! the render loop consumes per frame.

use crate::types::audio::AudioData;
use crate::types::event::ZoneColors;

// ── InputData ──────────────────────────────────────────────────────────────

/// A single sample from an input source.
///
/// The render loop pattern-matches on this to route data into the correct
/// pipeline stage (audio analysis, screen capture zones, etc.).
#[derive(Debug, Clone)]
pub enum InputData {
    /// Audio analysis snapshot — spectrum, beats, levels.
    Audio(AudioData),
    /// Screen capture zone colors — grabbed from display regions.
    Screen(ScreenData),
    /// No data available this frame (source idle or warming up).
    None,
}

// ── ScreenData ─────────────────────────────────────────────────────────────

/// Captured screen region colors, one entry per monitored zone.
///
/// Screen capture sources produce this when grabbing display regions
/// for ambient lighting or screen-reactive effects.
#[derive(Debug, Clone)]
pub struct ScreenData {
    /// Per-zone color data extracted from screen regions.
    pub zone_colors: Vec<ZoneColors>,
}

// ── InputSource ────────────────────────────────────────────────────────────

/// A live data source that feeds the effect pipeline.
///
/// Implementations handle their own hardware/OS interaction (cpal for audio,
/// xcap for screen capture, etc.). The engine only sees this trait.
///
/// # Lifecycle
///
/// 1. Create the source (device detection, config parsing)
/// 2. Call [`start`] to begin capture
/// 3. Call [`sample`] each frame to pull the latest data
/// 4. Call [`stop`] to release hardware resources
///
/// Sources must be [`Send`] so the engine can own them across thread boundaries.
pub trait InputSource: Send {
    /// Human-readable name for logging and UI display (e.g., `"PipeWire Monitor"`).
    fn name(&self) -> &str;

    /// Begin capturing. Opens hardware streams, allocates buffers.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying device cannot be opened or configured.
    fn start(&mut self) -> anyhow::Result<()>;

    /// Stop capturing and release hardware resources.
    fn stop(&mut self);

    /// Pull the latest data snapshot for this frame.
    ///
    /// Returns [`InputData::None`] if the source has no new data yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture stream has died or data is corrupted.
    fn sample(&mut self) -> anyhow::Result<InputData>;

    /// Whether the source is actively capturing.
    fn is_running(&self) -> bool;
}
