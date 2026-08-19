//! Linux host input acquisition through evdev.
//!
//! This crate owns `/dev/input` discovery, device lifetime tracking, evdev
//! synchronization, and native event decoding. Its public boundary contains
//! plain Rust values only. Held state, repeat classification, synthesized
//! releases, pointer projection, and frame snapshots belong to the shared
//! host-input fold.

mod shared;

pub use shared::{
    DeviceCapabilities, DeviceOpenState, DeviceOpenStatus, EvdevDeviceDescriptor, EvdevInputBatch,
    EvdevInputConfig, EvdevInputError, EvdevInputEvent, EvdevInputResult, EvdevKeyState,
    EvdevPointerButton, EvdevStateGapReason, EvdevWorkerState, PendingEvents,
};

#[doc(hidden)]
pub use hypercolor_worker_retention::retention_service_identity as worker_retention_service_identity;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::EvdevInputSession;

#[cfg(not(target_os = "linux"))]
mod stubs;
#[cfg(not(target_os = "linux"))]
pub use stubs::EvdevInputSession;

/// Start one evdev acquisition session.
///
/// # Errors
///
/// Returns a platform, configuration, worker-start, or readiness error.
pub fn start_evdev_input(
    config: EvdevInputConfig,
    sink: impl FnMut(EvdevInputBatch<'_>) + Send + 'static,
) -> EvdevInputResult<EvdevInputSession> {
    EvdevInputSession::start(config, sink)
}
