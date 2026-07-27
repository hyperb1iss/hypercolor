//! Input source abstraction — trait and data types for audio, screen, and future inputs.
//!
//! [`InputSource`] is the polymorphic entry point for anything that feeds data
//! into the effect pipeline. Each source produces [`InputData`] snapshots that
//! the render loop consumes per frame.

use super::status::{SourceStatusError, SourceStatusHandle, SourceStatusReporter};
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
    ///
    /// Producers uphold two invariants that make this sound: `recent_keys`
    /// drain on every sample (the same press edge never repeats across
    /// frames), and the generation bumps whenever held state changed or
    /// recents were delivered.
    #[must_use]
    pub fn is_dirty_against(&self, last_generation: Option<u64>) -> bool {
        last_generation != Some(self.generation) || !self.batch.is_empty()
    }

    /// Fold another source's snapshot into this one.
    ///
    /// Multiple interaction sources (host capture plus browser injection)
    /// each produce a full snapshot per frame; the pipeline merges them so
    /// no source's held state clobbers another's. Held keys and buttons
    /// union, recent keys concatenate, motion sums, and generations add so
    /// the combined counter advances whenever any source changes.
    ///
    /// Pointer position takes an injected pointer over a host one, then falls
    /// back to the first source that actually has one (`mode != None`), so an
    /// idle source never blanks an active pointer. The injected preference
    /// exists because host capture registers before the browser source, and
    /// first-wins alone would make previewing an interactive effect in the UI
    /// track the desktop cursor instead of the canvas. Fixing that by
    /// reordering registration would be worse: `recent_keys` concatenates in
    /// source order right above, so permuting sources would reorder the key
    /// stream an effect sees.
    pub fn merge_from(&mut self, other: InteractionData) {
        for key in other.keyboard.pressed_keys {
            if !self.keyboard.pressed_keys.contains(&key) {
                self.keyboard.pressed_keys.push(key);
            }
        }
        self.keyboard.recent_keys.extend(other.keyboard.recent_keys);
        // Decided before `buttons` is moved out of `other.mouse`.
        let take_pointer = takes_pointer_from(&self.mouse, &other.mouse);
        for button in other.mouse.buttons {
            if !self.mouse.buttons.contains(&button) {
                self.mouse.buttons.push(button);
            }
        }
        self.mouse.down = self.mouse.down || other.mouse.down;
        if take_pointer {
            self.mouse.x = other.mouse.x;
            self.mouse.y = other.mouse.y;
            self.mouse.norm_x = other.mouse.norm_x;
            self.mouse.norm_y = other.mouse.norm_y;
            self.mouse.mode = other.mouse.mode;
            self.mouse.injected = other.mouse.injected;
        }
        self.batch.wheel_hi_res = self
            .batch
            .wheel_hi_res
            .saturating_add(other.batch.wheel_hi_res);
        self.batch.motion.dx += other.batch.motion.dx;
        self.batch.motion.dy += other.batch.motion.dy;
        self.batch.motion.distance += other.batch.motion.distance;
        self.batch.window_secs = self.batch.window_secs.max(other.batch.window_secs);
        self.batch.dropped_events = self
            .batch
            .dropped_events
            .saturating_add(other.batch.dropped_events);
        self.generation = self.generation.wrapping_add(other.generation);
    }

    /// Fold a borrowed source snapshot into reusable aggregate storage.
    ///
    /// Unlike [`Self::merge_from`], this retains the destination vectors so a
    /// warmed route cache can merge stable source shapes without reallocating.
    pub fn merge_from_ref(&mut self, other: &Self) {
        for key in &other.keyboard.pressed_keys {
            if !self.keyboard.pressed_keys.contains(key) {
                self.keyboard.pressed_keys.push(key.clone());
            }
        }
        self.keyboard
            .recent_keys
            .extend(other.keyboard.recent_keys.iter().cloned());
        let take_pointer = takes_pointer_from(&self.mouse, &other.mouse);
        for button in &other.mouse.buttons {
            if !self.mouse.buttons.contains(button) {
                self.mouse.buttons.push(button.clone());
            }
        }
        self.mouse.down |= other.mouse.down;
        if take_pointer {
            self.mouse.x = other.mouse.x;
            self.mouse.y = other.mouse.y;
            self.mouse.norm_x = other.mouse.norm_x;
            self.mouse.norm_y = other.mouse.norm_y;
            self.mouse.mode = other.mouse.mode;
            self.mouse.injected = other.mouse.injected;
        }
        self.batch.wheel_hi_res = self
            .batch
            .wheel_hi_res
            .saturating_add(other.batch.wheel_hi_res);
        self.batch.motion.dx += other.batch.motion.dx;
        self.batch.motion.dy += other.batch.motion.dy;
        self.batch.motion.distance += other.batch.motion.distance;
        self.batch.window_secs = self.batch.window_secs.max(other.batch.window_secs);
        self.batch.dropped_events = self
            .batch
            .dropped_events
            .saturating_add(other.batch.dropped_events);
    }
}

/// Health snapshot for one interaction source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionDiagnostics {
    /// Backend identifier: `"evdev"`, `"device_query"`, or `"browser"`.
    pub backend: &'static str,
    /// Whether this source captures from host hardware (vs injected input).
    pub host_capture: bool,
    /// Whether the source is currently capturing.
    pub capturing: bool,
    /// Device nodes opened and streaming (host backends only).
    pub devices_opened: usize,
    /// Device nodes present but unreadable — udev rules missing.
    pub devices_denied: usize,
    /// Why input is not flowing, when the counters alone cannot say.
    pub degraded: Option<InteractionDegradation>,
}

/// Why an interaction source cannot deliver input.
///
/// `devices_opened`/`devices_denied` could express Linux's failure mode
/// (nodes present but unreadable) and nothing else. On Windows there is no
/// per-device denial and no udev, so the counter shape could only ever produce
/// wrong advice — a remedy banner telling a Windows user to run
/// `sudo just udev-install`. This gives the session-level failure its own typed
/// field instead of smuggling it through a counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionDegradation {
    /// Process has no visible window station — a Windows service, or a
    /// scheduled task running without an interactive desktop. Raw Input
    /// registers happily in that state and simply never delivers a message.
    NoInteractiveSession,
    /// Device nodes present but unreadable — Linux udev rules missing.
    AccessDenied,
    /// The backend could not initialize, or its worker died.
    Unavailable(String),
}

impl InteractionDegradation {
    /// Stable snake_case code for the API and UI surfaces.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoInteractiveSession => "no_interactive_session",
            Self::AccessDenied => "access_denied",
            Self::Unavailable(_) => "unavailable",
        }
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
    /// Whether this pointer was injected by a preview surface rather than read
    /// from host hardware.
    ///
    /// The producer declares its own priority here because the merge point has
    /// no source identity to key on: the render pipeline merges bare
    /// [`InteractionData`] values pulled from `sample_and_drain_with_delta_secs`.
    pub injected: bool,
}

/// Whether `incoming` should supply the merged pointer position.
///
/// An injected pointer outranks a host one; otherwise the first source with a
/// pointer at all wins, which is the behaviour every other case already had.
fn takes_pointer_from(current: &MouseData, incoming: &MouseData) -> bool {
    if incoming.mode == PointerMode::None {
        return false;
    }
    if current.mode == PointerMode::None {
        return true;
    }
    incoming.injected && !current.injected
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
    /// Wall-clock span the motion aggregate covers, in seconds.
    ///
    /// Grows when superseded frames coalesce, so velocity derived from
    /// `motion.distance` stays honest instead of dividing several frames
    /// of travel by a single frame's delta. Not part of emptiness.
    pub window_secs: f32,
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
        self.window_secs += prior.window_secs;
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

    /// Clone the source's independent health handle, when it publishes one.
    ///
    /// The default keeps third-party implementations source-compatible. All
    /// production Hypercolor sources override this seam; `None` explicitly
    /// identifies an uninstrumented compatibility source.
    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        None
    }

    /// Borrow the source-owned lifecycle reporter, when implemented.
    ///
    /// The default `None` is the matching uninstrumented compatibility path.
    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        None
    }

    /// Bind future lifecycle publications to the manager-owned graph generation.
    fn set_source_graph_generation(&mut self, source_graph_generation: u64) {
        if let Some(status) = self.source_status_reporter() {
            status.set_source_graph_generation(source_graph_generation);
        }
    }

    /// Permanently retire this source's status at its removal generation.
    ///
    /// # Errors
    ///
    /// Returns a typed transition error if the source rejects retirement.
    fn retire_source_status(
        &mut self,
        source_graph_generation: u64,
    ) -> Result<(), SourceStatusError> {
        self.source_status_reporter()
            .map_or(Ok(()), |status| status.retire(source_graph_generation))
    }

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
    /// Events are capture-timestamped by the source and retain portable
    /// physical-code and repeat metadata when available; delivery sequence
    /// numbers are assigned at the frame fan-out point. Sources that only
    /// expose sampled state can keep the default empty implementation.
    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        Vec::new()
    }

    /// Sample and drain in one step.
    ///
    /// Sources that produce both a snapshot and events override this to do
    /// both under one internal lock, so an edge arriving between the two
    /// can never appear in the frame's batch while missing from its held
    /// state. The default composes the two calls for single-role sources.
    fn sample_and_drain_with_delta_secs(
        &mut self,
        delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        let sample = self.sample_with_delta_secs(delta_secs);
        let events = self.drain_events();
        (sample, events)
    }

    /// Publish one shared sample and append drained events into reusable storage.
    ///
    /// The compatibility default preserves the atomic sample-and-drain seam but
    /// allocates an [`Arc`] for non-empty samples. Sources with an upstream
    /// latest-value publication can override this and clone their existing Arc.
    fn sample_shared_and_drain_into(
        &mut self,
        delta_secs: f32,
        events: &mut Vec<TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        let (sample, drained) = self.sample_and_drain_with_delta_secs(delta_secs);
        events.extend(drained);
        match sample? {
            InputData::None => Ok(None),
            sample => Ok(Some(Arc::new(sample))),
        }
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

    /// Health snapshot for interaction sources, for status and remediation UX.
    ///
    /// Non-interaction sources return `None`. Interaction sources report
    /// their backend and, for host hardware, how many device nodes opened
    /// versus were denied — so the UI can tell "input is off" apart from
    /// "input is on but the udev rules are missing".
    fn interaction_diagnostics(&self) -> Option<InteractionDiagnostics> {
        None
    }

    /// Whether this source captures from host input hardware (evdev, OS
    /// event tap) rather than an injected feed like the browser preview.
    ///
    /// Consent config only governs host capture; the browser source is
    /// always registered and is not a host source, so live enable/disable
    /// of host input must not add or remove it.
    fn is_host_capture_source(&self) -> bool {
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
