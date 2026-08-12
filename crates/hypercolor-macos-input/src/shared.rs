//! Platform-neutral values crossing the macOS input boundary.

use std::sync::Arc;

/// Capture configuration for one event-tap session.
#[derive(Clone)]
pub struct MacosInputConfig {
    pub keyboard: bool,
    pub pointer: bool,
    pub epoch: u64,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for MacosInputConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacosInputConfig")
            .field("keyboard", &self.keyboard)
            .field("pointer", &self.pointer)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Aggregate Core Graphics modifier flags retained without native types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MacosModifierFlags(u64);

impl MacosModifierFlags {
    pub const ALPHA_SHIFT: Self = Self(1 << 16);
    pub const SHIFT: Self = Self(1 << 17);
    pub const CONTROL: Self = Self(1 << 18);
    pub const ALTERNATE: Self = Self(1 << 19);
    pub const COMMAND: Self = Self(1 << 20);
    pub const NUMERIC_PAD: Self = Self(1 << 21);
    pub const HELP: Self = Self(1 << 22);
    pub const SECONDARY_FN: Self = Self(1 << 23);

    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Pointer button reported by a Core Graphics event tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosPointerButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// Unit of the signed 16.16 values in a wheel event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosScrollUnit {
    /// Physical wheel notches before core scales them into `Line120` units.
    Notches,
    /// Continuous trackpad or Magic Mouse movement in pixels.
    Pixels,
}

/// Gesture phase attached to exact scroll motion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MacosScrollPhase {
    #[default]
    None,
    Began,
    Stationary,
    Changed,
    Ended,
    Cancelled,
    MayBegin,
}

/// Why native state can no longer be treated as a complete edge stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosInputGapReason {
    TapDisabledTimeout,
    TapDisabledUserInput,
    PermissionRevoked,
    SessionInterrupted,
    WorkerExited,
    SourceStopped,
    QueueOverflow,
}

/// One decoded edge or ordered state barrier.
#[derive(Debug, Clone, PartialEq)]
pub enum MacosInputEvent {
    Key {
        virtual_keycode: u16,
        pressed: bool,
        autorepeat: bool,
    },
    ModifierFlags {
        virtual_keycode: u16,
        flags: MacosModifierFlags,
    },
    Button {
        button: MacosPointerButton,
        pressed: bool,
    },
    Motion {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    Wheel {
        fixed_delta_x: i64,
        fixed_delta_y: i64,
        unit: MacosScrollUnit,
        phase: MacosScrollPhase,
        momentum_phase: MacosScrollPhase,
    },
    MediaKey {
        nx_key_type: u16,
        pressed: bool,
        repeat: bool,
    },
    StateGap {
        reason: MacosInputGapReason,
    },
}

/// Decoded subtype-8 media-key payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosMediaKey {
    pub nx_key_type: u16,
    pub pressed: bool,
    pub repeat: bool,
}

/// Union of active macOS display bounds for one topology generation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacosVirtualDesktop {
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub topology_generation: u64,
}

impl MacosVirtualDesktop {
    /// Construct validated virtual-desktop bounds.
    ///
    /// # Errors
    ///
    /// Returns [`MacosInputError::InvalidVirtualDesktop`] for non-finite or
    /// non-positive dimensions and non-finite origins.
    pub fn new(
        origin_x: f64,
        origin_y: f64,
        width: f64,
        height: f64,
        topology_generation: u64,
    ) -> MacosInputResult<Self> {
        if !origin_x.is_finite()
            || !origin_y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return Err(MacosInputError::InvalidVirtualDesktop);
        }
        Ok(Self {
            origin_x,
            origin_y,
            width,
            height,
            topology_generation,
        })
    }

    /// Normalize a signed global point into the current display union.
    #[must_use]
    pub fn normalize(self, x: f64, y: f64) -> (f64, f64) {
        let nx = ((x - self.origin_x) / self.width).clamp(0.0, 1.0);
        let ny = ((y - self.origin_y) / self.height).clamp(0.0, 1.0);
        (nx, ny)
    }
}

/// One coherent native queue drain.
#[derive(Debug)]
pub struct MacosInputBatch<'a> {
    pub epoch: u64,
    pub at_ms: u64,
    pub events: &'a [MacosInputEvent],
    pub virtual_desktop: MacosVirtualDesktop,
}

/// Whether a native input batch reached the canonical core publication state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosInputPublicationOutcome {
    Published,
    Rejected,
}

/// Monotonic native diagnostics for one session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosInputDiagnostics {
    pub queue_capacity: usize,
    pub queue_depth: usize,
    pub events_received: u64,
    pub events_published: u64,
    pub dropped_events: u64,
    pub tap_disable_count: u64,
    pub tap_disabled_timeout: u64,
    pub tap_disabled_user_input: u64,
    pub tap_reenabled: u64,
    pub state_gaps: u64,
    pub unsupported_system_events: u64,
    pub invalid_scroll_phases: u64,
    pub last_point_delta_x: i64,
    pub last_point_delta_y: i64,
    pub callback_to_publication_sample_count: u64,
    pub callback_to_publication_total_ns: u64,
    pub callback_to_publication_max_ns: u64,
    pub callback_to_publication_p95_ns: u64,
    pub callback_to_publication_p99_ns: u64,
}

/// Event masks actually requested for one session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EffectiveEventMasks {
    pub keyboard: u64,
    pub pointer: u64,
}

/// A recoverable event-tap worker failure with stable native meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosWorkerDegradation {
    TapDisabled(MacosInputGapReason),
    DisplayTopology(String),
}

impl std::fmt::Display for MacosWorkerDegradation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TapDisabled(MacosInputGapReason::TapDisabledTimeout) => {
                f.write_str("event tap was repeatedly disabled by timeout")
            }
            Self::TapDisabled(MacosInputGapReason::TapDisabledUserInput) => {
                f.write_str("event tap was repeatedly disabled by user input")
            }
            Self::TapDisabled(reason) => write!(f, "event tap was repeatedly disabled: {reason:?}"),
            Self::DisplayTopology(reason) => {
                write!(f, "display topology refresh failed: {reason}")
            }
        }
    }
}

/// Liveness of the event-tap worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosWorkerState {
    Running,
    Degraded(MacosWorkerDegradation),
    PermissionRevoked,
    Failed(String),
}

/// Errors from validating or starting macOS host input capture.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MacosInputError {
    #[error("macOS host input is only available on macOS")]
    UnsupportedPlatform,
    #[error("no input kinds enabled for capture")]
    NothingToCapture,
    #[error("keyboard capture requires Input Monitoring permission")]
    PermissionDenied,
    #[error("invalid virtual desktop bounds")]
    InvalidVirtualDesktop,
    #[error("Core Graphics display enumeration failed with error {0}")]
    DisplayTopology(i32),
    #[error("Core Graphics reported no active displays")]
    NoActiveDisplays,
    #[error("failed to spawn the macOS input worker: {0}")]
    WorkerSpawn(String),
    #[error("timed out waiting for the macOS input worker to become ready")]
    WorkerReadyTimeout,
    #[error("failed to create the {0} event tap")]
    TapCreation(&'static str),
    #[error("failed to create the {0} event-tap run-loop source")]
    RunLoopSource(&'static str),
    #[error("failed to inspect the installed event taps: Core Graphics error {0}")]
    TapInspection(i32),
    #[error("failed to read the current process audit token: Mach error {0}")]
    AuditToken(i32),
}

pub type MacosInputResult<T> = Result<T, MacosInputError>;
