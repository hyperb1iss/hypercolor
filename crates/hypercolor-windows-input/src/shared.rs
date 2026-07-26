//! The platform-independent vocabulary this crate speaks to `hypercolor-core`.
//!
//! Nothing here names a Windows type, so core can build against it on every
//! target and the decoding arithmetic in [`crate::decode`] stays unit-testable
//! on Linux CI.

use std::sync::Arc;

/// Which scan-code prefix a key report carried.
///
/// A boolean `extended` cannot distinguish `E0` (the extended block: arrows,
/// right-hand modifiers, the navigation cluster) from `E1` (Pause/Break), and
/// collapsing them would let an unrecognized `E1` sequence collide with an
/// unprefixed code of the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RawKeyPrefix {
    /// No prefix byte: the make code is a plain set-1 scan code.
    None,
    /// `E0`-prefixed: the extended key block.
    E0,
    /// `E1`-prefixed: in practice Pause/Break.
    E1,
}

/// A mouse button, named in the evdev vocabulary so effects stay portable.
///
/// `Side` and `Extra` are Windows' logical `XBUTTON1`/`XBUTTON2`. Windows
/// guarantees nothing about where those physically sit, so this mapping is a
/// naming convention chosen to match evdev's own names, not a claim about
/// hardware placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RawButton {
    Left,
    Right,
    Middle,
    Side,
    Extra,
}

impl RawButton {
    /// Canonical cross-platform name, matching `pointer_button_name` on Linux.
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
            Self::Side => "side",
            Self::Extra => "extra",
        }
    }
}

/// Which top-level HID collection a device was registered under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawDeviceKind {
    Keyboard,
    Mouse,
}

/// One decoded hardware edge, or a device-lifecycle marker.
#[derive(Debug, Clone, PartialEq)]
pub enum RawInputEvent {
    /// A key edge. `pressed` is the raw hardware edge; auto-repeat carries no
    /// marker on Windows, so repeat classification happens in core against the
    /// held set for this `source_id`.
    Key {
        source_id: Arc<str>,
        make_code: u16,
        prefix: RawKeyPrefix,
        /// Logical virtual-key code. Layout-dependent, so it is only consulted
        /// when `make_code` is unusable.
        vkey: u16,
        pressed: bool,
    },
    Button {
        source_id: Arc<str>,
        button: RawButton,
        pressed: bool,
    },
    /// Vertical wheel travel in 1/120-notch units, matching evdev's
    /// `REL_WHEEL_HI_RES`. Horizontal wheel is dropped rather than folded in.
    Wheel {
        source_id: Arc<str>,
        delta_hi_res: i32,
    },
    /// Relative counts from a normal mouse.
    MotionRelative {
        source_id: Arc<str>,
        dx: i32,
        dy: i32,
    },
    /// Absolute position from a tablet, RDP, or VM pointer, already normalized
    /// to `[0,1]²` against whichever rect `MOUSE_VIRTUAL_DESKTOP` selected.
    MotionAbsolute {
        source_id: Arc<str>,
        norm_x: f32,
        norm_y: f32,
        /// Which rect the device's raw range covered, from
        /// `MOUSE_VIRTUAL_DESKTOP`.
        ///
        /// Carried through rather than consumed at normalization time because
        /// a device can switch spaces mid-session, and the two normalizations
        /// are not comparable: differencing a primary-monitor position against
        /// a virtual-desktop one invents a jump the pointer never made. Core
        /// resets its baseline when this changes.
        virtual_desktop: bool,
    },
    DeviceArrived {
        source_id: Arc<str>,
        label: String,
        kind: RawDeviceKind,
    },
    DeviceRemoved {
        source_id: Arc<str>,
    },
    /// Ordered barrier: everything this source had held is now unknown.
    ///
    /// Core releases that source's held keys and buttons and resets its
    /// absolute baseline before applying any later event in the batch. Emitted
    /// only for a keyboard overrun report, where the keyboard's own view of
    /// held state has become unreliable.
    StateGap {
        source_id: Arc<str>,
    },
}

impl RawInputEvent {
    /// The device bucket this event belongs to.
    #[must_use]
    pub fn source_id(&self) -> &Arc<str> {
        match self {
            Self::Key { source_id, .. }
            | Self::Button { source_id, .. }
            | Self::Wheel { source_id, .. }
            | Self::MotionRelative { source_id, .. }
            | Self::MotionAbsolute { source_id, .. }
            | Self::DeviceArrived { source_id, .. }
            | Self::DeviceRemoved { source_id }
            | Self::StateGap { source_id } => source_id,
        }
    }
}

/// Cursor position as of one drain, in virtual-desktop pixels plus normalized
/// canvas coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawCursor {
    /// Signed: the virtual desktop starts at negative coordinates whenever a
    /// monitor sits left of the primary.
    pub x: i32,
    pub y: i32,
    pub norm_x: f32,
    pub norm_y: f32,
}

/// One coherent drain: the events, the cursor as of the same drain, and the
/// capture stamp taken before the drain read anything.
///
/// Delivered by reference so a drain costs no allocation, and as one call per
/// drain rather than one per event — the core side takes exactly one lock to
/// fold the whole batch.
#[derive(Debug)]
pub struct RawInputBatch<'a> {
    pub events: &'a [RawInputEvent],
    pub cursor: Option<RawCursor>,
    pub at_ms: u64,
    /// The epoch core handed to `start()`, echoed back so core can reject a
    /// batch from a session it no longer owns.
    pub epoch: u64,
}

/// Capture configuration for one session.
#[derive(Clone)]
pub struct RawInputConfig {
    /// Register for the keyboard usage. Declining means the process is never
    /// registered for keyboards at all, rather than filtering after the fact.
    pub keyboard: bool,
    /// Register for the mouse usage. Also gates cursor sampling, so a
    /// keyboard-only session never reads the pointer position.
    pub mouse: bool,
    /// Core's monotonic clock, called on the pump immediately before each
    /// drain. Injected because `input_mono_ms` lives in core and the
    /// dependency cannot run the other way.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Allocated by core; echoed in every batch.
    pub epoch: u64,
}

impl std::fmt::Debug for RawInputConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawInputConfig")
            .field("keyboard", &self.keyboard)
            .field("mouse", &self.mouse)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Whether this process can see user input at all.
///
/// The probe is the window station rather than the session id: a service or a
/// scheduled task set to "run whether the user is logged on or not" gets a
/// non-interactive window station inside an ordinary non-zero session and sees
/// exactly as little input as a session-0 service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Process has a visible window station; Raw Input can reach it.
    Interactive,
    /// No visible window station. `CreateWindowExW` and
    /// `RegisterRawInputDevices` would both still succeed and `WM_INPUT` would
    /// simply never arrive, so this is detected explicitly rather than by
    /// waiting for input that cannot come.
    NoInteractiveSession,
}

/// Liveness of a session's pump thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerState {
    Running,
    /// The pump stopped or its fold panicked. Core reports the source
    /// unavailable rather than silently flatlining.
    Failed(String),
}

/// Errors from starting or running a Raw Input session.
#[derive(Debug, thiserror::Error)]
pub enum RawInputError {
    #[error("Raw Input is only available on Windows")]
    UnsupportedPlatform,
    #[error("no interactive window station; Raw Input cannot observe user input here")]
    NoInteractiveSession,
    #[error("no device kinds enabled for capture")]
    NothingToCapture,
    #[error("failed to spawn the Raw Input worker: {0}")]
    WorkerSpawn(String),
    #[error("timed out waiting for the Raw Input worker to become ready")]
    WorkerReadyTimeout,
    #[error("failed to create the message-only window: {0}")]
    WindowCreation(String),
    #[error("failed to register for Raw Input: {0}")]
    Registration(String),
    #[error("another window in this process owns the Raw Input registration")]
    RegistrationStolen,
    #[error("the session was stopped before it finished starting")]
    Cancelled,
}

pub type RawInputResult<T> = Result<T, RawInputError>;
