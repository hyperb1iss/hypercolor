//! Render engine — the frame pipeline orchestrator.
//!
//! This module contains the [`RenderLoop`] which drives the core render
//! pipeline, and the [`FpsController`] which manages adaptive frame rate
//! tiers. Together they form the heartbeat of the Hypercolor engine.
//!
//! The render loop sequences five pipeline stages within a frame budget:
//!
//! ```text
//! Input Sampling → Effect Render → Spatial Sample → Device Push → Bus Publish
//!     (1.0ms)         (8.0ms)        (0.5ms)         (2.0ms)       (0.1ms)
//! ```
//!
//! The [`FpsController`] automatically adjusts the target frame rate across
//! five tiers (10/20/30/45/60 fps) based on measured performance, downshifting
//! quickly on overruns and upshifting slowly when sustained headroom is detected.

mod fps;

pub use fps::{FpsController, FpsTier, FrameStats, TierTransitionConfig};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{debug, info, trace};

// ── Render Loop State ────────────────────────────────────────────────────────

/// Lifecycle states for the render loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderLoopState {
    /// The loop has been created but not yet started.
    Created,

    /// Actively rendering frames at the current FPS tier.
    Running,

    /// Rendering is paused — last frame held on devices.
    Paused,

    /// The loop has been stopped and will not produce more frames.
    Stopped,
}

impl std::fmt::Display for RenderLoopState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

// ── Frame Timing ─────────────────────────────────────────────────────────────

/// Accumulated timing statistics across the render loop's lifetime.
#[derive(Debug, Clone, Copy)]
pub struct RenderLoopStats {
    /// Total frames rendered.
    pub total_frames: u64,

    /// Current FPS tier.
    pub tier: FpsTier,

    /// Current runtime ceiling for automatic upshifts.
    pub max_tier: FpsTier,

    /// EWMA-smoothed frame time.
    pub avg_frame_time: Duration,

    /// Consecutive budget misses in the current window.
    pub consecutive_misses: u32,

    /// Current loop state.
    pub state: RenderLoopState,
}

// ── Render Loop ──────────────────────────────────────────────────────────────

/// The main render loop orchestrator.
///
/// Owns the [`FpsController`] for timing and tier management, and provides
/// the frame pipeline structure. The render loop is designed to run on a
/// dedicated OS thread, calling [`tick`](Self::tick) in a tight loop.
///
/// The loop itself is decoupled from the actual rendering — it provides
/// the timing skeleton. Callers drive it by:
///
/// 1. Calling [`tick`](Self::tick) each iteration
/// 2. Performing their render work
/// 3. Calling [`frame_complete`](Self::frame_complete) to finalize timing
///
/// The `running` flag is atomic and can be used to signal stop from
/// any thread.
pub struct RenderLoop {
    /// Adaptive frame rate controller.
    fps_controller: FpsController,

    /// Atomic stop signal, shareable across threads.
    running: Arc<AtomicBool>,

    /// Current lifecycle state.
    state: RenderLoopState,

    /// Monotonically increasing frame counter.
    frame_number: u64,

    /// When the loop was started (for elapsed time tracking).
    start_time: Option<Instant>,
}

impl RenderLoop {
    /// Create a new render loop targeting the given FPS.
    ///
    /// Resolves the FPS value to the nearest [`FpsTier`]. The loop starts
    /// in the [`Created`](RenderLoopState::Created) state — call
    /// [`start`](Self::start) to begin.
    #[must_use]
    pub fn new(fps: u32) -> Self {
        let tier = FpsTier::from_fps(fps);
        info!(target_fps = fps, resolved_tier = %tier, "Render loop created");
        let mut fps_controller = FpsController::new(tier);
        fps_controller.set_max_tier(tier);
        Self {
            fps_controller,
            running: Arc::new(AtomicBool::new(false)),
            state: RenderLoopState::Created,
            frame_number: 0,
            start_time: None,
        }
    }

    /// Create a render loop with a specific tier and transition config.
    #[must_use]
    pub fn with_config(tier: FpsTier, config: TierTransitionConfig) -> Self {
        info!(tier = %tier, "Render loop created with custom config");
        Self {
            fps_controller: FpsController::with_config(tier, config),
            running: Arc::new(AtomicBool::new(false)),
            state: RenderLoopState::Created,
            frame_number: 0,
            start_time: None,
        }
    }

    /// Transition the loop to the running state.
    pub fn start(&mut self) {
        if self.state == RenderLoopState::Stopped {
            debug!("Cannot start a stopped render loop — create a new one");
            return;
        }
        self.running.store(true, Ordering::Release);
        self.state = RenderLoopState::Running;
        self.start_time = Some(Instant::now());
        info!(tier = %self.fps_controller.tier(), "Render loop started");
    }

    /// Signal the loop to stop. Safe to call from any thread via the
    /// shared [`stop_handle`](Self::stop_handle).
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.state = RenderLoopState::Stopped;
        info!(total_frames = self.frame_number, "Render loop stopped");
    }

    /// Returns `true` if the loop is in the running state.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire) && self.state == RenderLoopState::Running
    }

    /// Returns the current loop state.
    #[must_use]
    pub fn state(&self) -> RenderLoopState {
        self.state
    }

    /// Pause rendering. Devices hold the last frame.
    pub fn pause(&mut self) {
        if self.state == RenderLoopState::Running {
            self.state = RenderLoopState::Paused;
            debug!("Render loop paused");
        }
    }

    /// Resume from paused state.
    pub fn resume(&mut self) {
        if self.state == RenderLoopState::Paused {
            self.state = RenderLoopState::Running;
            debug!("Render loop resumed");
        }
    }

    /// Get an `Arc<AtomicBool>` handle that can be used to stop the loop
    /// from another thread.
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// Begin a new frame. Call this at the top of each loop iteration.
    ///
    /// Returns `false` if the loop is not running (caller should skip
    /// the frame).
    pub fn tick(&mut self) -> bool {
        if !self.is_running() {
            return false;
        }
        self.fps_controller.begin_frame();
        true
    }

    /// Finalize the current frame. Updates timing stats and checks for
    /// automatic tier transitions.
    ///
    /// Returns the [`FrameStats`] for this frame, including the recommended
    /// sleep duration before the next frame. Returns `None` if no frame
    /// was in progress.
    pub fn frame_complete(&mut self) -> Option<FrameStats> {
        let stats = self.fps_controller.end_frame()?;
        self.frame_number += 1;

        trace!(
            frame = self.frame_number,
            frame_time_us = stats.frame_time.as_micros(),
            budget_exceeded = stats.budget_exceeded,
            tier = %stats.tier,
        );

        // Check for automatic tier transitions
        if let Some(new_tier) = self.fps_controller.maybe_transition() {
            info!(
                old_tier = %stats.tier,
                new_tier = %new_tier,
                consecutive_misses = stats.consecutive_misses,
                "FPS tier transition"
            );
        }

        Some(stats)
    }

    /// Current frame number (monotonically increasing).
    #[must_use]
    pub fn frame_number(&self) -> u64 {
        self.frame_number
    }

    /// Elapsed time since the loop was started.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start_time.map_or(Duration::ZERO, |t| t.elapsed())
    }

    /// Reference to the FPS controller for direct inspection or config changes.
    #[must_use]
    pub fn fps_controller(&self) -> &FpsController {
        &self.fps_controller
    }

    /// Mutable reference to the FPS controller.
    pub fn fps_controller_mut(&mut self) -> &mut FpsController {
        &mut self.fps_controller
    }

    /// Collect a snapshot of current render loop statistics.
    #[must_use]
    pub fn stats(&self) -> RenderLoopStats {
        RenderLoopStats {
            total_frames: self.frame_number,
            tier: self.fps_controller.tier(),
            max_tier: self.fps_controller.max_tier(),
            avg_frame_time: self.fps_controller.ewma_frame_time(),
            consecutive_misses: self.fps_controller.consecutive_misses(),
            state: self.state,
        }
    }

    /// Force a tier change on the FPS controller.
    pub fn set_tier(&mut self, tier: FpsTier) {
        info!(new_tier = %tier, "Manual FPS tier override");
        self.fps_controller.set_tier(tier);
    }

    /// Target frame interval for the current tier.
    #[must_use]
    pub fn target_interval(&self) -> Duration {
        self.fps_controller.target_interval()
    }
}

impl std::fmt::Debug for RenderLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderLoop")
            .field("state", &self.state)
            .field("frame_number", &self.frame_number)
            .field("running", &self.running.load(Ordering::Relaxed))
            .field("start_time", &self.start_time)
            .field("fps_controller", &self.fps_controller)
            .finish()
    }
}
