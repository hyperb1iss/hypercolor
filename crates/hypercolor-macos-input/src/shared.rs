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

/// Event masks actually requested for one session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EffectiveEventMasks {
    pub keyboard: u64,
    pub pointer: u64,
}

/// Liveness of the event-tap worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosWorkerState {
    Running,
    Degraded(String),
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
    #[error("failed to spawn the macOS input worker: {0}")]
    WorkerSpawn(String),
    #[error("timed out waiting for the macOS input worker to become ready")]
    WorkerReadyTimeout,
    #[error("failed to create the {0} event tap")]
    TapCreation(&'static str),
    #[error("failed to create the {0} event-tap run-loop source")]
    RunLoopSource(&'static str),
}

pub type MacosInputResult<T> = Result<T, MacosInputError>;
