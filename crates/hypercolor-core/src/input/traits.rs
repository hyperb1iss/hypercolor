//! Input source abstraction — trait and data types for audio, screen, and future inputs.
//!
//! [`InputSource`] is the polymorphic entry point for anything that feeds data
//! into the effect pipeline. Each source produces [`InputData`] snapshots that
//! the render loop consumes per frame.

use crate::types::audio::{AudioData, AudioPipelineConfig};
use crate::types::canvas::PublishedSurface;
use crate::types::event::{TimedInputEvent, ZoneColors};
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
    /// Now-playing media snapshot from the MPRIS source.
    Media(Arc<hypercolor_types::media::MediaState>),
    /// Network throughput snapshot from the net source, refreshed at 1 Hz.
    Net(Arc<hypercolor_types::net::NetStats>),
    /// Screen capture zone colors — grabbed from display regions.
    Screen(ScreenData),
    /// System telemetry snapshot — CPU/GPU/memory/components.
    Sensors(Arc<SystemSnapshot>),
    /// No data available this frame (source idle or warming up).
    None,
}

// ── InteractionData ────────────────────────────────────────────────────────

/// Snapshot of host keyboard and mouse state for one frame.
///
/// Splits into stable held-state (`keyboard`, `mouse`, versioned by
/// `generation`) and a transient per-frame event `batch`. Renderer dirty
/// checks compare `generation` and batch emptiness rather than deep
/// equality, so noisy per-frame values never defeat idle skipping.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteractionData {
    /// Keyboard state including currently pressed keys and edge-triggered presses.
    pub keyboard: KeyboardData,
    /// Mouse position and pressed buttons.
    pub mouse: MouseData,
    /// Ordered, timestamped input edges captured since the previous frame.
    pub batch: InteractionBatch,
    /// Bumps whenever `keyboard` or `mouse` held-state actually changes.
    pub generation: u64,
}

impl InteractionData {
    /// Whether a renderer consuming this snapshot has anything new to see.
    #[must_use]
    pub fn is_dirty_against(&self, last_generation: Option<u64>) -> bool {
        last_generation != Some(self.generation) || !self.batch.is_empty()
    }
}

/// Keyboard snapshot for one frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyboardData {
    /// Keys currently held down.
    pub pressed_keys: Vec<String>,
    /// Keys newly pressed since the last frame sample.
    pub recent_keys: Vec<String>,
}

/// How a pointer position in [`MouseData`] should be interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PointerMode {
    /// No pointer source is available; position fields are meaningless.
    #[default]
    None,
    /// Position is a real cursor location reported by the platform.
    Absolute,
    /// Position is a virtual cursor accumulated from relative motion.
    Virtual,
}

/// Mouse snapshot for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MouseData {
    /// Global X position in pixels (meaning depends on `mode`).
    pub x: i32,
    /// Global Y position in pixels (meaning depends on `mode`).
    pub y: i32,
    /// Buttons currently held down.
    pub buttons: Vec<String>,
    /// Whether any button is currently pressed.
    pub down: bool,
    /// Normalized position in `[0, 1]²`, valid unless `mode` is `None`.
    pub norm_x: f32,
    /// Normalized position in `[0, 1]²`, valid unless `mode` is `None`.
    pub norm_y: f32,
    /// How the position fields were produced.
    pub mode: PointerMode,
}

/// Transient per-frame input edges and aggregates.
///
/// Contents are consumed by the frame that carries them and never persist
/// across frames. Pointer motion is aggregated here rather than queued
/// per hardware event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteractionBatch {
    /// Ordered key/button/wheel edges, capture-timestamped and sequenced.
    pub events: Vec<TimedInputEvent>,
    /// Accumulated wheel travel since last frame, in 1/120-notch units.
    pub wheel_hi_res: i32,
    /// Aggregate pointer motion since last frame.
    pub motion: MotionAggregate,
    /// Events discarded due to queue bounds since last frame.
    pub dropped_events: u32,
}

impl InteractionBatch {
    /// Upper bound on events carried by one frame batch after coalescing.
    pub const MAX_EVENTS: usize = 256;

    /// Whether this batch carries anything a renderer could react to.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.wheel_hi_res == 0
            && self.motion == MotionAggregate::default()
            && self.dropped_events == 0
    }

    /// Fold a superseded frame's batch into this one, oldest events first.
    ///
    /// Renderers that coalesce queued frames use this so no input edge is
    /// lost when a frame is replaced before rendering. Ordering and
    /// timestamps survive; overflow drops the oldest events and counts them.
    pub fn absorb_prior(&mut self, mut prior: Self) {
        if prior.is_empty() {
            return;
        }

        prior.events.append(&mut self.events);
        self.events = prior.events;
        if self.events.len() > Self::MAX_EVENTS {
            let overflow = self.events.len() - Self::MAX_EVENTS;
            self.events.drain(..overflow);
            self.dropped_events = self
                .dropped_events
                .saturating_add(u32::try_from(overflow).unwrap_or(u32::MAX));
        }

        self.wheel_hi_res = self.wheel_hi_res.saturating_add(prior.wheel_hi_res);
        self.motion.dx += prior.motion.dx;
        self.motion.dy += prior.motion.dy;
        self.motion.distance += prior.motion.distance;
        self.dropped_events = self.dropped_events.saturating_add(prior.dropped_events);
    }
}

/// Summed pointer motion for one frame, in normalized canvas units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MotionAggregate {
    /// Net horizontal travel.
    pub dx: f32,
    /// Net vertical travel.
    pub dy: f32,
    /// Total path length (always ≥ the net displacement).
    pub distance: f32,
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
    /// Detected letterbox bars in grid units: top, bottom, left, right.
    pub letterbox: [u32; 4],
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
            letterbox: [0; 4],
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
    /// Events are capture-timestamped by the source; delivery sequence
    /// numbers are assigned at the frame fan-out point. Sources that only
    /// expose sampled state can keep the default empty implementation.
    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
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

    /// Whether this source captures host keyboard/mouse interaction.
    fn is_interaction_source(&self) -> bool {
        false
    }

    /// Toggle whether an interaction source should actively capture host input.
    ///
    /// Interaction sources close their device handles and clear all held
    /// state when capture goes inactive, so no keys or buttons stay stuck.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot update its capture state.
    fn set_interaction_capture_active(&mut self, _active: bool) -> anyhow::Result<()> {
        Ok(())
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

    /// Reconfigure a running screen source without rebuilding the input manager.
    ///
    /// Non-screen sources can ignore this by keeping the default implementation.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot apply the new capture settings.
    fn reconfigure_screen_capture(
        &mut self,
        _config: &crate::input::screen::CaptureConfig,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Discard any persisted source selection and prompt the user to pick again.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot restart its capture session.
    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
