//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use hypercolor_types::device::DeviceId;

use super::traits::{DeviceBackend, DeviceFrameSink};

mod backend_io;
mod brightness;
mod direct_output;
mod display_output;
mod lifecycle;
mod mapping;
mod output_color;
mod output_coordinator;
mod output_frame;
mod output_lanes;
mod output_telemetry;
mod registry;
mod routing;
mod warnings;

pub use backend_io::BackendIo;
use brightness::DeviceOutputBrightness;
use output_coordinator::DeviceOutputCoordinator;
pub use output_coordinator::DirectControlGuard;
use routing::{DeviceMapping, RoutingPlan};
use warnings::DeviceOutputWarnings;

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type DeviceFrameSinkHandle = Arc<dyn DeviceFrameSink>;
type BackendDeviceKey = (String, DeviceId);
const UNMAPPED_LAYOUT_WARN_INTERVAL: Duration = Duration::from_secs(5);

/// Contiguous LED range on a physical device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRange {
    /// Inclusive start LED index.
    pub start: usize,
    /// Number of LEDs in this range.
    pub length: usize,
}

impl SegmentRange {
    /// Create a new range.
    #[must_use]
    pub const fn new(start: usize, length: usize) -> Self {
        Self { start, length }
    }

    /// Exclusive end LED index.
    #[must_use]
    pub const fn end(self) -> usize {
        self.start.saturating_add(self.length)
    }
}

pub use super::output_queue::{
    AsyncWriteFailure, BackendManagerDebugSnapshot, BackendRoutingDebugSnapshot,
    DeviceOutputStatistics, LayoutRoutingDebugEntry, OrphanedQueueDebugEntry,
    OutputQueueDebugSnapshot,
};

// ── BackendManager ──────────────────────────────────────────────────────────

/// Routes per-zone color data to the correct device backends.
///
/// On each frame, [`write_frame`](Self::write_frame) groups zone colors
/// by target device (using the spatial layout mapping) and dispatches
/// one payload per device to a non-blocking output queue.
#[derive(Default)]
pub struct BackendManager {
    /// Registered backends, keyed by `BackendInfo.id`.
    backends: HashMap<String, BackendHandle>,

    /// Maps spatial layout `Output.device_id` strings to `(backend_id, DeviceId)`.
    ///
    /// Populated during device discovery/connection. Entries are added via
    /// [`map_device`](Self::map_device) when a zone's device reference is
    /// resolved to an actual connected device.
    device_map: HashMap<String, DeviceMapping>,

    /// Per-target output queue, frame-sink, staging, FPS, and direct-control state.
    output: DeviceOutputCoordinator,

    /// User-configured per-device software output brightness state.
    output_brightness: DeviceOutputBrightness,

    /// Frame-output warning dedupe and throttling state.
    warnings: DeviceOutputWarnings,

    /// Incremented whenever routing-relevant device mappings change.
    routing_mapping_generation: u64,

    /// Number of times the cached routing plan has been rebuilt.
    routing_plan_rebuild_count: u64,

    /// Cached routing metadata for the current layout + mapping generation.
    routing_plan: Option<Arc<RoutingPlan>>,
}

// ── FrameWriteStats ─────────────────────────────────────────────────────────

/// Statistics from a single frame's device push.
#[derive(Debug, Clone, Default)]
pub struct FrameWriteStats {
    /// Number of devices that received color data.
    pub devices_written: usize,

    /// Total LEDs written across all devices.
    pub total_leds: usize,

    /// Errors encountered during writes (non-fatal — every device still gets its data).
    pub errors: Vec<String>,
}

impl BackendManager {
    /// Create an empty backend manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
