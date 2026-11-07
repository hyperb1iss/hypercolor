//! Device backend system — traits, registry, and discovery orchestration.
//!
//! This module defines the output half of Hypercolor's pipeline: everything
//! needed to discover physical hardware, establish connections, and push
//! LED color data to devices over any transport (USB HID, UDP, TCP, HTTP).

#[cfg(unix)]
pub mod blocks;
pub mod conflicts;
mod discovery;
mod discovery_server;
mod hid_usage;
mod lifecycle;
pub mod manager;
pub mod mock;
mod output_queue;
mod registry;
pub mod smbus_backend;
pub mod smbus_scanner;
mod state_machine;
mod transport_error;
pub mod unclaimed;
pub mod usb_backend;
pub mod usb_hotplug;
pub mod usb_scanner;

#[cfg(unix)]
pub use blocks::{BlocksBackend, BlocksScanner};
pub use conflicts::{
    ConflictChanges, ScanTicket, SoftwareCatalog, SoftwareConflictStore, SoftwareSpec,
};
pub use discovery::{DiscoveryOrchestrator, DiscoveryProgress, DiscoveryReport, ScannerScanReport};
pub use discovery_server::discover_servers;
pub use hid_usage::HidUsageIndex;
pub use lifecycle::{DeviceLifecycleManager, LifecycleAction};
pub use manager::{
    AsyncWriteFailure, BackendIo, BackendManager, DeviceOutputStatistics, DirectControlGuard,
    DisplayOutputLane, DisplayOutputStatistics, SegmentRange,
};
pub use registry::{
    DeviceRegistry, PortableKeyCollision, PortableRebindError, PreparedDeviceOutput,
};
pub use smbus_backend::SmBusBackend;
pub use smbus_scanner::SmBusScanner;
pub use state_machine::{
    DeviceStateMachine, DeviceStateMachineDebugSnapshot, FLAP_ESCALATION_THRESHOLD, FlapEscalation,
    RECONNECT_STABLE_AFTER, ReconnectPolicy, ReconnectStatus, StateTransitionRecord,
};
pub use unclaimed::{UnclaimedDeviceStore, UsbObservation};
pub use usb_backend::{
    UsbActorMetricsSnapshot, UsbBackend, UsbProtocolConfigStore, usb_actor_metrics_snapshot,
};
pub use usb_hotplug::{UsbHotplugEvent, UsbHotplugMonitor};
pub use usb_scanner::UsbScanner;

/// When to offer a held frame again, for a protocol with no pump cadence.
const HELD_FRAME_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

/// The skip for a frame `protocol` holds behind unconfirmed output. The
/// protocol sends the newest frame itself, so the lane comes back at its next
/// pump.
pub(crate) fn held_frame_skip(
    protocol: &dyn hypercolor_hal::protocol::Protocol,
) -> hypercolor_driver_api::SkipReason {
    let wait = protocol.frame_pump_interval().unwrap_or(HELD_FRAME_RETRY);
    hypercolor_driver_api::SkipReason::Deferred {
        retry_at: std::time::Instant::now() + wait,
    }
}
