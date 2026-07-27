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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawDeviceKind {
    Keyboard,
    Mouse,
}

/// Immutable identity and diagnostic metadata for one native device lifetime.
///
/// Raw Input handles are recycled. The session and device generations make
/// each native lifetime distinct, while the interned path and label let every
/// hot event clone one [`Arc`] instead of cloning diagnostic strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDeviceDescriptor {
    pub source_id: Arc<str>,
    pub interface_path: Option<Arc<str>>,
    pub label: Arc<str>,
    pub kind: RawDeviceKind,
    pub session_generation: u64,
    pub device_generation: u64,
}

/// One decoded hardware edge, or a device-lifecycle marker.
#[derive(Debug, Clone, PartialEq)]
pub enum RawInputEvent {
    /// A key edge. `pressed` is the raw hardware edge; auto-repeat carries no
    /// marker on Windows, so repeat classification happens in core against the
    /// held set for this `source_id`.
    Key {
        device: Arc<RawDeviceDescriptor>,
        make_code: u16,
        prefix: RawKeyPrefix,
        /// Logical virtual-key code. Layout-dependent, so it is only consulted
        /// when `make_code` is unusable.
        vkey: u16,
        pressed: bool,
    },
    Button {
        device: Arc<RawDeviceDescriptor>,
        button: RawButton,
        pressed: bool,
    },
    /// Vertical wheel travel in 1/120-notch units, matching evdev's
    /// `REL_WHEEL_HI_RES`. Horizontal wheel is dropped rather than folded in.
    Wheel {
        device: Arc<RawDeviceDescriptor>,
        delta_hi_res: i32,
    },
    /// Relative counts from a normal mouse.
    MotionRelative {
        device: Arc<RawDeviceDescriptor>,
        dx: i32,
        dy: i32,
    },
    /// Absolute position from a tablet, RDP, or VM pointer, already normalized
    /// to `[0,1]²` against whichever rect `MOUSE_VIRTUAL_DESKTOP` selected.
    MotionAbsolute {
        device: Arc<RawDeviceDescriptor>,
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
        device: Arc<RawDeviceDescriptor>,
    },
    DeviceRemoved {
        device: Arc<RawDeviceDescriptor>,
    },
    /// Ordered barrier: everything this source had held is now unknown.
    ///
    /// Core releases that source's held keys and buttons and resets its
    /// absolute baseline before applying any later event in the batch. Emitted
    /// only for a keyboard overrun report, where the keyboard's own view of
    /// held state has become unreliable.
    StateGap {
        device: Arc<RawDeviceDescriptor>,
    },
}

impl RawInputEvent {
    /// The device bucket this event belongs to.
    #[must_use]
    pub fn source_id(&self) -> &Arc<str> {
        match self {
            Self::Key { device, .. }
            | Self::Button { device, .. }
            | Self::Wheel { device, .. }
            | Self::MotionRelative { device, .. }
            | Self::MotionAbsolute { device, .. }
            | Self::DeviceArrived { device }
            | Self::DeviceRemoved { device }
            | Self::StateGap { device } => &device.source_id,
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
    #[error("failed to enumerate physical monitor topology: {0}")]
    MonitorTopology(String),
    #[error("failed to register for Raw Input: {0}")]
    Registration(String),
    #[error("another window in this process owns the Raw Input registration")]
    RegistrationStolen,
    #[error("the session was stopped before it finished starting")]
    Cancelled,
}

pub type RawInputResult<T> = Result<T, RawInputError>;

/// The events waiting to be handed to the sink, and the rule that delivering
/// them clears them.
///
/// This exists as its own type because the rule was once spread across two
/// call sites and one of them forgot. The drain cleared at the *start* of a
/// slice rather than after delivering, so whatever the last slice produced was
/// still buffered when the worker flushed, and every key edge, button edge and
/// motion delta reached core twice. Both halves were individually correct; the
/// composition was not, and no test could see it because the buffer discipline
/// lived inside a loop that only runs on Windows against real hardware.
///
/// Owning the buffer here puts that discipline on a type that compiles and
/// tests everywhere.
#[derive(Debug, Default)]
pub struct PendingEvents {
    events: Vec<RawInputEvent>,
}

impl PendingEvents {
    #[must_use]
    pub const fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn push(&mut self, event: RawInputEvent) {
        self.events.push(event);
    }

    pub fn extend(&mut self, events: impl IntoIterator<Item = RawInputEvent>) {
        self.events.extend(events);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Hand the pending events to `sink` and clear them.
    ///
    /// Returns whether anything was delivered, so a caller can tell an empty
    /// buffer from a delivered one. Delivering twice without pushing in
    /// between cannot repeat a batch: the second call finds nothing.
    pub fn deliver(
        &mut self,
        at_ms: u64,
        epoch: u64,
        cursor: Option<RawCursor>,
        sink: &mut impl FnMut(RawInputBatch<'_>),
    ) -> bool {
        if self.events.is_empty() {
            return false;
        }
        sink(RawInputBatch {
            events: &self.events,
            cursor,
            at_ms,
            epoch,
        });
        self.events.clear();
        true
    }
}
