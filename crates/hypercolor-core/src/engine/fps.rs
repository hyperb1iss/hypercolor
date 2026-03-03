//! Adaptive FPS controller with tiered frame rate management.
//!
//! [`FpsController`] manages frame timing, budget tracking, and automatic
//! tier transitions based on measured render performance. The controller
//! operates across five performance tiers from [`FpsTier::Minimal`] (10 fps)
//! to [`FpsTier::Full`] (60 fps).
//!
//! **Downshift is fast** (2 consecutive budget misses triggers an immediate
//! drop), while **upshift is slow** (sustained headroom required over a
//! configurable window) to prevent oscillation between tiers.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ── FPS Tiers ────────────────────────────────────────────────────────────────

/// Performance tiers controlling the target frame rate.
///
/// The system automatically shifts between tiers based on actual frame
/// render times. Lower tiers reduce system load by rendering fewer
/// frames per second.
///
/// Ordered from lowest to highest frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum FpsTier {
    /// 10 fps, 100ms budget. Absolute minimum — idle/standby.
    Minimal = 0,
    /// 20 fps, 50ms budget. Low-power or heavy system load.
    Low = 1,
    /// 30 fps, ~33.3ms budget. Balanced performance.
    Medium = 2,
    /// 45 fps, ~22.2ms budget. High quality with moderate headroom.
    High = 3,
    /// 60 fps, ~16.6ms budget. Full fidelity, desktop idle.
    Full = 4,
}

impl FpsTier {
    /// Target frames per second for this tier.
    #[must_use]
    pub const fn fps(self) -> u32 {
        match self {
            Self::Minimal => 10,
            Self::Low => 20,
            Self::Medium => 30,
            Self::High => 45,
            Self::Full => 60,
        }
    }

    /// Frame time budget (target interval between frames).
    #[must_use]
    pub const fn frame_interval(self) -> Duration {
        match self {
            Self::Minimal => Duration::from_millis(100),
            Self::Low => Duration::from_millis(50),
            Self::Medium => Duration::from_nanos(33_333_333),
            Self::High => Duration::from_nanos(22_222_222),
            Self::Full => Duration::from_nanos(16_666_666),
        }
    }

    /// The next tier up, if one exists.
    #[must_use]
    pub const fn upshift(self) -> Option<Self> {
        match self {
            Self::Minimal => Some(Self::Low),
            Self::Low => Some(Self::Medium),
            Self::Medium => Some(Self::High),
            Self::High => Some(Self::Full),
            Self::Full => None,
        }
    }

    /// The next tier down, if one exists.
    #[must_use]
    pub const fn downshift(self) -> Option<Self> {
        match self {
            Self::Minimal => None,
            Self::Low => Some(Self::Minimal),
            Self::Medium => Some(Self::Low),
            Self::High => Some(Self::Medium),
            Self::Full => Some(Self::High),
        }
    }

    /// All tiers in ascending order.
    pub const ALL: [Self; 5] = [
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Full,
    ];

    /// Resolve a target FPS value to the nearest tier.
    ///
    /// Picks the tier whose fps value is closest to the requested rate,
    /// preferring the higher tier on a tie.
    #[must_use]
    pub fn from_fps(target: u32) -> Self {
        // Walk tiers from lowest to highest. Pick the last one whose fps
        // is at most `target`, or the closest overall if `target` is below
        // Minimal. On exact boundary between two tiers, prefer the higher.
        let mut best = Self::Minimal;
        let mut best_dist = u32::MAX;

        for tier in Self::ALL {
            let fps = tier.fps();
            let dist = fps.abs_diff(target);
            // `<=` so ties resolve toward higher tiers (iterated ascending)
            if dist <= best_dist {
                best_dist = dist;
                best = tier;
            }
        }
        best
    }
}

impl std::fmt::Display for FpsTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}fps ({})",
            self.fps(),
            match self {
                Self::Minimal => "minimal",
                Self::Low => "low",
                Self::Medium => "medium",
                Self::High => "high",
                Self::Full => "full",
            }
        )
    }
}

// ── Tier Transition Config ───────────────────────────────────────────────────

/// Thresholds and hysteresis parameters for tier transitions.
///
/// These knobs control how aggressively the controller reacts to
/// performance changes. The defaults favor stability — downshift is
/// immediate on sustained overruns, upshift requires prolonged headroom.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierTransitionConfig {
    /// Consecutive frame budget misses before downshifting.
    pub downshift_miss_threshold: u32,

    /// Seconds of sustained headroom required before upshifting.
    pub upshift_sustain_secs: f64,

    /// EWMA headroom ratio below which upshift becomes eligible.
    /// A value of 0.7 means the smoothed frame time must be under
    /// 70% of the current tier's budget.
    pub upshift_headroom_ratio: f64,

    /// EWMA smoothing factor (alpha). Lower values give more weight
    /// to history, dampening spikes.
    pub ewma_alpha: f64,
}

impl Default for TierTransitionConfig {
    fn default() -> Self {
        Self {
            downshift_miss_threshold: 2,
            upshift_sustain_secs: 5.0,
            upshift_headroom_ratio: 0.7,
            ewma_alpha: 0.05,
        }
    }
}

// ── Frame Stats ──────────────────────────────────────────────────────────────

/// Timing statistics for a single completed frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameStats {
    /// Total wall-clock time for this frame.
    pub frame_time: Duration,

    /// Time remaining in the budget (zero if overrun).
    pub headroom: Duration,

    /// Whether this frame exceeded the budget.
    pub budget_exceeded: bool,

    /// Current EWMA-smoothed frame time.
    pub ewma_frame_time: Duration,

    /// Active FPS tier when this frame completed.
    pub tier: FpsTier,

    /// Consecutive budget misses so far.
    pub consecutive_misses: u32,

    /// Total frames rendered since the last tier change.
    pub frames_since_tier_change: u64,
}

// ── FPS Controller ───────────────────────────────────────────────────────────

/// Adaptive frame rate controller with automatic tier transitions.
///
/// Tracks actual frame render times, maintains an EWMA (exponentially
/// weighted moving average) of frame duration, and manages transitions
/// between [`FpsTier`] levels based on performance headroom.
///
/// The controller is purely a timing state machine — it does not own
/// any threads or perform any I/O. The render loop drives it by calling
/// [`begin_frame`](Self::begin_frame) and [`end_frame`](Self::end_frame)
/// each iteration.
pub struct FpsController {
    /// Current performance tier.
    tier: FpsTier,

    /// Maximum tier automatic upshifts are allowed to reach.
    max_tier: FpsTier,

    /// Tier transition configuration.
    config: TierTransitionConfig,

    /// When the current frame started rendering.
    frame_start: Option<Instant>,

    /// Number of consecutive frames that exceeded the budget.
    consecutive_misses: u32,

    /// EWMA of frame time in seconds.
    ewma_frame_time: f64,

    /// Total frames rendered since the last tier change.
    frames_since_tier_change: u64,

    /// When the controller first became eligible for upshift.
    /// Reset whenever headroom drops below the threshold.
    upshift_eligible_since: Option<Instant>,

    /// Total frames rendered across all tiers.
    total_frames: u64,
}

impl FpsController {
    /// Create a new controller targeting the given FPS tier.
    #[must_use]
    pub fn new(tier: FpsTier) -> Self {
        Self {
            tier,
            max_tier: FpsTier::Full,
            config: TierTransitionConfig::default(),
            frame_start: None,
            consecutive_misses: 0,
            ewma_frame_time: tier.frame_interval().as_secs_f64() * 0.5,
            frames_since_tier_change: 0,
            upshift_eligible_since: None,
            total_frames: 0,
        }
    }

    /// Create a controller with custom transition thresholds.
    #[must_use]
    pub fn with_config(tier: FpsTier, config: TierTransitionConfig) -> Self {
        Self {
            config,
            ..Self::new(tier)
        }
    }

    /// Current active FPS tier.
    #[must_use]
    pub fn tier(&self) -> FpsTier {
        self.tier
    }

    /// Maximum tier automatic upshifts may reach.
    #[must_use]
    pub fn max_tier(&self) -> FpsTier {
        self.max_tier
    }

    /// Set the maximum allowed upshift tier.
    pub fn set_max_tier(&mut self, max_tier: FpsTier) {
        self.max_tier = max_tier;
        if self.tier > self.max_tier {
            self.tier = self.max_tier;
        }
        self.upshift_eligible_since = None;
    }

    /// Target frame interval for the current tier.
    #[must_use]
    pub fn target_interval(&self) -> Duration {
        self.tier.frame_interval()
    }

    /// Target FPS for the current tier.
    #[must_use]
    pub fn target_fps(&self) -> u32 {
        self.tier.fps()
    }

    /// Number of consecutive frames that exceeded the budget.
    #[must_use]
    pub fn consecutive_misses(&self) -> u32 {
        self.consecutive_misses
    }

    /// EWMA-smoothed frame time.
    #[must_use]
    pub fn ewma_frame_time(&self) -> Duration {
        Duration::from_secs_f64(self.ewma_frame_time)
    }

    /// Total frames rendered since the last tier change.
    #[must_use]
    pub fn frames_since_tier_change(&self) -> u64 {
        self.frames_since_tier_change
    }

    /// Total frames rendered across all tiers.
    #[must_use]
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Reference to the active transition config.
    #[must_use]
    pub fn config(&self) -> &TierTransitionConfig {
        &self.config
    }

    /// Mark the beginning of a new frame. Call this at the top of each
    /// render loop iteration.
    pub fn begin_frame(&mut self) {
        self.frame_start = Some(Instant::now());
    }

    /// Mark the end of the current frame.
    ///
    /// Updates the EWMA, consecutive miss counter, and checks for tier
    /// transitions. Returns [`FrameStats`] with timing data and the
    /// sleep duration before the next frame.
    ///
    /// Returns `None` if [`begin_frame`](Self::begin_frame) was not called.
    pub fn end_frame(&mut self) -> Option<FrameStats> {
        let start = self.frame_start.take()?;
        let elapsed = start.elapsed();
        self.record_frame(elapsed)
    }

    /// Record a frame with an explicit duration (useful for testing or
    /// when the caller measures time externally).
    ///
    /// Performs all the same bookkeeping as [`end_frame`](Self::end_frame).
    pub fn record_frame(&mut self, frame_time: Duration) -> Option<FrameStats> {
        let budget = self.tier.frame_interval();

        // Update EWMA
        let alpha = self.config.ewma_alpha;
        self.ewma_frame_time =
            (1.0 - alpha) * self.ewma_frame_time + alpha * frame_time.as_secs_f64();

        self.frames_since_tier_change += 1;
        self.total_frames += 1;

        let budget_exceeded = frame_time > budget;

        if budget_exceeded {
            self.consecutive_misses += 1;
            self.upshift_eligible_since = None;
        } else {
            self.consecutive_misses = 0;
        }

        let headroom = if budget_exceeded {
            Duration::ZERO
        } else {
            budget.saturating_sub(frame_time)
        };

        Some(FrameStats {
            frame_time,
            headroom,
            budget_exceeded,
            ewma_frame_time: Duration::from_secs_f64(self.ewma_frame_time),
            tier: self.tier,
            consecutive_misses: self.consecutive_misses,
            frames_since_tier_change: self.frames_since_tier_change,
        })
    }

    /// Check whether a downshift should occur based on consecutive misses.
    #[must_use]
    pub fn should_downshift(&self) -> bool {
        self.consecutive_misses >= self.config.downshift_miss_threshold
            && self.tier.downshift().is_some()
    }

    /// Check whether an upshift should occur based on sustained headroom.
    ///
    /// Mutates internal state to track when headroom first became sufficient.
    pub fn should_upshift(&mut self) -> bool {
        // Can't upshift past the configured ceiling.
        if self.tier >= self.max_tier || self.tier.upshift().is_none() {
            self.upshift_eligible_since = None;
            return false;
        }

        // Check EWMA headroom ratio
        let budget_secs = self.tier.frame_interval().as_secs_f64();
        if budget_secs <= 0.0 {
            return false;
        }

        let headroom_ratio = self.ewma_frame_time / budget_secs;
        if headroom_ratio > self.config.upshift_headroom_ratio {
            self.upshift_eligible_since = None;
            return false;
        }

        // Track sustained headroom duration
        let now = Instant::now();
        match self.upshift_eligible_since {
            None => {
                self.upshift_eligible_since = Some(now);
                false
            }
            Some(since) => {
                now.duration_since(since).as_secs_f64() >= self.config.upshift_sustain_secs
            }
        }
    }

    /// Attempt an automatic tier transition. Returns `Some(new_tier)` if
    /// a transition occurred.
    pub fn maybe_transition(&mut self) -> Option<FpsTier> {
        if self.should_downshift() {
            return self.downshift();
        }
        if self.should_upshift() {
            return self.upshift();
        }
        None
    }

    /// Force a transition to a specific tier.
    pub fn set_tier(&mut self, tier: FpsTier) {
        let clamped = tier.min(self.max_tier);
        if self.tier != clamped {
            self.tier = clamped;
            self.reset_transition_state();
        }
    }

    /// Downshift one tier. Returns the new tier, or `None` if already at minimum.
    pub fn downshift(&mut self) -> Option<FpsTier> {
        let new = self.tier.downshift()?;
        self.tier = new;
        self.reset_transition_state();
        Some(new)
    }

    /// Upshift one tier. Returns the new tier, or `None` if already at maximum.
    pub fn upshift(&mut self) -> Option<FpsTier> {
        let new = self.tier.upshift()?;
        if new > self.max_tier {
            return None;
        }
        self.tier = new;
        self.reset_transition_state();
        Some(new)
    }

    /// Compute the sleep duration before the next frame, given a measured
    /// frame time.
    #[must_use]
    pub fn sleep_duration(&self, frame_time: Duration) -> Duration {
        self.tier.frame_interval().saturating_sub(frame_time)
    }

    /// Reset transition tracking state after a tier change.
    fn reset_transition_state(&mut self) {
        self.consecutive_misses = 0;
        self.frames_since_tier_change = 0;
        self.upshift_eligible_since = None;
    }
}

impl std::fmt::Debug for FpsController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FpsController")
            .field("tier", &self.tier)
            .field("max_tier", &self.max_tier)
            .field("config", &self.config)
            .field("frame_start", &self.frame_start)
            .field("consecutive_misses", &self.consecutive_misses)
            .field("ewma_frame_time_ms", &(self.ewma_frame_time * 1000.0))
            .field("frames_since_tier_change", &self.frames_since_tier_change)
            .field("upshift_eligible_since", &self.upshift_eligible_since)
            .field("total_frames", &self.total_frames)
            .finish()
    }
}
