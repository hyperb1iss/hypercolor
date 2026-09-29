//! Device input capability offered to drivers.
//!
//! Drivers that own hardware with its own input surfaces publish what the
//! hardware reports through a per-device lease. The host folds those edges
//! into the interaction pipeline, attributed to the device.

use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::DeviceInputEdge;

/// Host service that accepts input from driver-owned devices.
pub trait DeviceInputSink: Send + Sync {
    /// Start publishing input for one device.
    ///
    /// The returned publisher is a lease. Dropping it retires the device's
    /// input source, and the host cancels anything it held. Attaching the
    /// same device again supersedes the earlier lease: its source is retired
    /// the same way, and its `publish` and drop become no-ops, so a stale
    /// lease can never retire its successor.
    fn attach(&self, device_id: DeviceId, label: &str) -> Box<dyn DeviceInputPublisher>;
}

/// Per-device input lease returned by [`DeviceInputSink::attach`].
pub trait DeviceInputPublisher: Send + Sync {
    /// Publish edges in the order the device reported them.
    ///
    /// Returns `false` once this lease has been superseded or detached.
    fn publish(&self, edges: &[DeviceInputEdge]) -> bool;
}
