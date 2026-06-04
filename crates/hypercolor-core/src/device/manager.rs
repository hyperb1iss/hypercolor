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

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tracing::debug;

use hypercolor_types::device::{DeviceId, DeviceInfo};

use super::traits::{DeviceBackend, DeviceFrameSink};

mod backend_io;
mod brightness;
mod direct_output;
mod display_output;
mod mapping;
mod output_color;
mod output_coordinator;
mod output_frame;
mod output_lanes;
mod output_telemetry;
mod routing;
mod warnings;

pub use backend_io::BackendIo;
use brightness::DeviceOutputBrightness;
use output_coordinator::DeviceOutputCoordinator;
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

    /// Register a device backend. Uses `backend.info().id` as the key.
    ///
    /// Replaces any existing backend with the same ID.
    pub fn register_backend(&mut self, backend: Box<dyn DeviceBackend>) {
        let info = backend.info();
        let backend_id = info.id.clone();

        debug!(
            backend_id = %backend_id,
            name = %info.name,
            "registered device backend"
        );

        // If a backend gets replaced, drop all output queues bound to that ID.
        // They are lazily recreated on the next frame.
        self.output.remove_backend_state(&backend_id);

        self.backends
            .insert(backend_id, Arc::new(Mutex::new(backend)));
    }

    /// Clone a backend I/O handle without holding the manager across awaits.
    #[must_use]
    pub fn backend_io(&self, backend_id: &str) -> Option<BackendIo> {
        self.backends
            .get(backend_id)
            .cloned()
            .map(|backend| BackendIo::new(backend_id.to_owned(), backend))
    }

    /// Connect a physical device and map it to a layout device identifier.
    ///
    /// This keeps connect + map as a single operation so discovery/lifecycle
    /// code can avoid split-brain states.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or the backend connect call
    /// fails.
    pub async fn connect_device(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        layout_device_id: &str,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        let target_fps = io.connect_with_refresh(device_id).await?;
        let frame_sink = io.frame_sink(device_id).await;
        self.set_cached_target_fps(backend_id, device_id, target_fps);
        self.set_device_frame_sink(backend_id, device_id, frame_sink);

        self.map_device(
            layout_device_id.to_owned(),
            backend_id.to_owned(),
            device_id,
        );
        Ok(())
    }

    /// Query refreshed metadata for a connected physical device.
    ///
    /// Backends can use this to expose connect-time topology discovery back
    /// to the daemon after a successful handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or metadata retrieval fails.
    pub async fn connected_device_info(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Result<Option<DeviceInfo>> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.connected_device_info(device_id).await
    }

    /// Disconnect a physical device and unmap its layout device identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or disconnect fails.
    pub async fn disconnect_device(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        _layout_device_id: &str,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };

        let disconnect_result = io.disconnect(device_id).await;
        let _ = self.remove_device_mappings_for_physical(backend_id, device_id);
        disconnect_result
    }

    /// Cache a backend-provided output FPS for a physical device.
    pub fn set_cached_target_fps(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        target_fps: u32,
    ) {
        self.output
            .set_target_fps(backend_id, device_id, target_fps);
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn routing_plan_rebuild_count(&self) -> u64 {
        self.routing_plan_rebuild_count
    }

    /// List registered backend IDs.
    #[must_use]
    pub fn backend_ids(&self) -> Vec<&str> {
        self.backends.keys().map(String::as_str).collect()
    }

    /// Number of registered backends.
    #[must_use]
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Return the cached target FPS for a connected physical device, if present.
    #[must_use]
    pub fn cached_target_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.output.target_fps(backend_id, device_id)
    }
}
