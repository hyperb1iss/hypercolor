//! Input source abstraction — trait and data types for audio, screen, and future inputs.
//!
//! [`InputSource`] is the polymorphic entry point for anything that feeds data
//! into the effect pipeline. Each source produces [`InputData`] snapshots that
//! the render loop consumes per frame.

use crate::types::audio::{AudioData, AudioPipelineConfig};
use crate::types::canvas::PublishedSurface;
use crate::types::event::{InputEvent, ZoneColors};
use hypercolor_types::sensor::SystemSnapshot;
use std::sync::Arc;

// ── InputData ──────────────────────────────────────────────────────────────

/// A single sample from an input source.
///
/// The render loop pattern-matches on this to route data into the correct
/// pipeline stage (audio analysis, screen capture zones, etc.).
#[derive(Debug, Clone)]
pub enum InputData {
    /// Audio analysis snapshot — spectrum, beats, levels.
    Audio(AudioData),
    /// Global keyboard and mouse state for interactive HTML effects.
    Interaction(InteractionData),
    /// Screen capture zone colors — grabbed from display regions.
    Screen(ScreenData),
    /// System telemetry snapshot — CPU/GPU/memory/components.
    Sensors(Arc<SystemSnapshot>),
    /// No data available this frame (source idle or warming up).
    None,
}

// ── InteractionData ────────────────────────────────────────────────────────

/// Snapshot of host keyboard and mouse state for one frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InteractionData {
    /// Keyboard state including currently pressed keys and edge-triggered presses.
    pub keyboard: KeyboardData,
    /// Mouse position and pressed buttons.
    pub mouse: MouseData,
}

/// Keyboard snapshot for one frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyboardData {
    /// Keys currently held down.
    pub pressed_keys: Vec<String>,
    /// Keys newly pressed since the last frame sample.
    pub recent_keys: Vec<String>,
}

/// Mouse snapshot for one frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MouseData {
    /// Global X position in pixels.
    pub x: i32,
    /// Global Y position in pixels.
    pub y: i32,
    /// Buttons currently held down.
    pub buttons: Vec<String>,
    /// Whether any button is currently pressed.
    pub down: bool,
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
    /// Grid width used when deriving `zone_colors`.
    pub grid_width: u32,
    /// Grid height used when deriving `zone_colors`.
    pub grid_height: u32,
    /// Downscaled screen image suitable for screen-reactive effects.
    pub canvas_downscale: Option<PublishedSurface>,
    /// Source frame width in pixels.
    pub source_width: u32,
    /// Source frame height in pixels.
    pub source_height: u32,
}

impl ScreenData {
    /// Build screen data from zone colors only.
    #[must_use]
    pub fn from_zones(zone_colors: Vec<ZoneColors>, grid_width: u32, grid_height: u32) -> Self {
        Self {
            zone_colors,
            grid_width,
            grid_height,
            canvas_downscale: None,
            source_width: 0,
            source_height: 0,
        }
    }
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

    /// Pull the latest data snapshot for this frame using the current frame delta.
    ///
    /// The default implementation falls back to [`sample`](Self::sample) so
    /// sources that do not care about frame timing do not need custom logic.
    fn sample_with_delta_secs(&mut self, delta_secs: f32) -> anyhow::Result<InputData> {
        let _ = delta_secs;
        self.sample()
    }

    /// Whether the source is actively capturing.
    fn is_running(&self) -> bool;

    /// Drain any discrete input events captured since the last frame.
    ///
    /// Sources that only expose sampled state can keep the default empty
    /// implementation.
    fn drain_events(&mut self) -> Vec<InputEvent> {
        Vec::new()
    }

    /// Whether this source supports runtime audio reconfiguration.
    fn is_audio_source(&self) -> bool {
        false
    }

    /// Whether this source supports runtime screen capture demand control.
    fn is_screen_source(&self) -> bool {
        false
    }

    /// Reconfigure a running audio source without rebuilding the full input manager.
    ///
    /// Non-audio sources can ignore this by keeping the default implementation.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot apply the new audio configuration.
    fn reconfigure_audio(
        &mut self,
        _config: &AudioPipelineConfig,
        _name: &str,
        _capture_active: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Toggle whether an audio source should actively capture from hardware.
    ///
    /// Audio sources can use this to pause their underlying stream while
    /// remaining registered with the input manager.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot update its capture state.
    fn set_audio_capture_active(&mut self, _active: bool) -> anyhow::Result<()> {
        Ok(())
    }

    /// Toggle whether a screen source should actively capture from the compositor.
    ///
    /// Screen sources can use this to pause their underlying capture session
    /// while remaining registered with the input manager.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot update its capture state.
    fn set_screen_capture_active(&mut self, _active: bool) -> anyhow::Result<()> {
        Ok(())
    }
}
