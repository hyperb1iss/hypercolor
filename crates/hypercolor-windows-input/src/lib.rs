//! Windows host input capture for Hypercolor, via the Raw Input API.
//!
//! Raw Input is the right primitive for a lighting daemon: with
//! `RIDEV_INPUTSINK` it observes keyboard and mouse activity while the daemon
//! sits in the background with no window focus, it needs no permission grant
//! and shows no prompt, and it reports the physical scan code rather than the
//! layout-translated character — which is what keeps a French keyboard naming
//! the same physical keys as the Linux daemon does.
//!
//! This crate is deliberately thin. It owns the message-only window, the
//! registration, `RAWINPUT` decoding, device identity, hotplug, and cursor
//! sampling, and hands out batches in a plain Rust vocabulary. Everything
//! downstream — key naming, held-state folding, repeat classification, the
//! pointer model — lives in `hypercolor-core` where it is pure and testable on
//! every platform. It exists as its own crate because the workspace forbids
//! `unsafe_code` and Win32 interop cannot.
//!
//! One structural note worth carrying into any change here: the sink runs on
//! the message-pump thread and folds directly, with no queue in between. That
//! mirrors `poll_devices` in core's evdev backend, which locks and folds on
//! its poll thread. A queue between pump and sink would need loss-barrier
//! semantics, epoch guards for a detached folding thread, and a wire format
//! for coalesced motion; none of that buys anything, because the sink is not
//! arbitrary user code — it is core's fold, and the only thing it blocks on is
//! the same mutex the render thread holds briefly once per frame.

// The record-walk arithmetic aligns offsets to `sizeof(QWORD)`, which is only
// the correct stride on a 64-bit target. We ship `x86_64-pc-windows-msvc`
// exclusively, so a 32-bit build is a mistake worth catching at compile time
// rather than a silently wrong 4-byte stride at runtime.
#[cfg(all(target_os = "windows", target_pointer_width = "32"))]
compile_error!(
    "hypercolor-windows-input requires a 64-bit target: RAWINPUT_ALIGN is \
     sizeof(QWORD) under _WIN64 and sizeof(DWORD) otherwise, and the record \
     walk assumes the former"
);

pub mod claim;
pub mod decode;
mod shared;

pub use shared::{
    PendingEvents, RawButton, RawCursor, RawDeviceKind, RawInputBatch, RawInputConfig,
    RawInputError, RawInputEvent, RawInputResult, RawKeyPrefix, SessionState, WorkerState,
};

#[cfg(target_os = "windows")]
mod devices;
#[cfg(target_os = "windows")]
mod metrics;
#[cfg(target_os = "windows")]
mod probe;
#[cfg(target_os = "windows")]
mod pump;
#[cfg(target_os = "windows")]
mod session;
#[cfg(target_os = "windows")]
mod worker_retention;

#[cfg(target_os = "windows")]
pub use probe::interactive_session_state;
#[cfg(target_os = "windows")]
pub use session::RawInputSession;

#[cfg(not(target_os = "windows"))]
mod stubs;

#[cfg(not(target_os = "windows"))]
pub use stubs::{RawInputSession, interactive_session_state};
