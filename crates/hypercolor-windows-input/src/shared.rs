//! The platform-independent vocabulary this crate speaks to `hypercolor-core`.
//!
//! Nothing here names a Windows type, so core can build against it on every
//! target and the decoding arithmetic in [`crate::decode`] stays unit-testable
//! on Linux CI.

use std::sync::Arc;

use crate::decode::unknown_key_name;
use hypercolor_types::host_input::{HostInputBatch, HostInputEvent, HostPointerSnapshot};
use hypercolor_types::host_input::{
    HostInputDevice, HostKeyIdentity, HostKeySignal, HostRepeatEvidence, HostScanCodePrefix,
    host_key_name_from_windows, host_media_key_name_from_windows,
};

/// Which scan-code prefix a key report carried.
///
/// A boolean `extended` cannot distinguish `E0` (the extended block: arrows,
/// right-hand modifiers, the navigation cluster) from `E1` (Pause/Break), and
/// collapsing them would let an unrecognized `E1` sequence collide with an
/// unprefixed code of the same value.
pub use hypercolor_types::host_input::HostScanCodePrefix as RawKeyPrefix;

/// Which top-level HID collection a device was registered under.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RawDeviceKind {
    Keyboard,
    Mouse,
}

/// Immutable identity and diagnostic metadata for one native device lifetime.
///
/// Raw Input handles are recycled. The session and device generations make
/// each native lifetime distinct, while the interned path and label let every
/// hot event clone one [`Arc`] instead of cloning diagnostic strings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(target_os = "windows")]
pub(crate) struct RawDeviceDescriptor {
    pub source_id: Arc<str>,
    pub interface_path: Option<Arc<str>>,
    pub label: Arc<str>,
    pub kind: RawDeviceKind,
    pub session_generation: u64,
    pub device_generation: u64,
    pub host_device: Arc<HostInputDevice>,
}

#[must_use]
pub fn normalize_windows_key_event(
    device: &Arc<HostInputDevice>,
    make_code: u16,
    prefix: HostScanCodePrefix,
    vkey: u16,
    pressed: bool,
) -> HostInputEvent {
    const VK_PAUSE: u16 = 0x13;
    let key = if prefix == HostScanCodePrefix::E1 && vkey == VK_PAUSE {
        "Pause".to_owned()
    } else if let Some(name) = host_media_key_name_from_windows(vkey) {
        name.to_owned()
    } else if let Some(name) = host_key_name_from_windows(make_code, prefix) {
        name.to_owned()
    } else if make_code == 0 {
        logical_vkey_name(vkey).map_or_else(|| unknown_key_name(make_code, prefix), str::to_owned)
    } else {
        unknown_key_name(make_code, prefix)
    };
    let prefix_name = match prefix {
        HostScanCodePrefix::None => "none",
        HostScanCodePrefix::E0 => "e0",
        HostScanCodePrefix::E1 => "e1",
    };
    HostInputEvent::Key {
        device: Some(Arc::clone(device)),
        identity: HostKeyIdentity {
            key: Arc::from(key),
            physical_code: Arc::from(format!("windows:set1:{prefix_name}:{make_code:02x}")),
        },
        signal: HostKeySignal::Edge {
            pressed,
            repeat: HostRepeatEvidence::Unknown,
        },
    }
}

fn logical_vkey_name(vkey: u16) -> Option<&'static str> {
    match vkey {
        0xA6 => Some("BrowserBack"),
        0xA7 => Some("BrowserForward"),
        0xA8 => Some("BrowserRefresh"),
        0xA9 => Some("BrowserStop"),
        0xAA => Some("BrowserSearch"),
        0xAB => Some("BrowserFavorites"),
        0xAC => Some("BrowserHome"),
        0xB4 => Some("LaunchMail"),
        0xB6 => Some("LaunchApp1"),
        0xB7 => Some("LaunchApp2"),
        _ => None,
    }
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
    /// Monotonic identity for native device lifetimes in this session.
    pub session_generation: u64,
}

impl std::fmt::Debug for RawInputConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawInputConfig")
            .field("keyboard", &self.keyboard)
            .field("mouse", &self.mouse)
            .field("session_generation", &self.session_generation)
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
    events: Vec<HostInputEvent>,
}

impl PendingEvents {
    #[must_use]
    pub const fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn push(&mut self, event: HostInputEvent) {
        self.events.push(event);
    }

    pub fn extend(&mut self, events: impl IntoIterator<Item = HostInputEvent>) {
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
        device_catalog_generation: u64,
        pointer: Option<HostPointerSnapshot>,
        sink: &mut impl FnMut(HostInputBatch<'_>),
    ) -> bool {
        if self.events.is_empty() {
            return false;
        }
        sink(HostInputBatch {
            events: &self.events,
            pointer,
            at_ms,
            device_catalog_generation,
        });
        self.events.clear();
        true
    }
}
