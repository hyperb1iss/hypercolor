//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;
use tracing::debug;

use hypercolor_types::device::{DeviceId, DeviceInfo, OwnedDisplayFramePayload};

use super::traits::{DeviceBackend, DeviceFrameSink};

mod backend_io;
mod output_color;
mod output_frame;
mod output_telemetry;
mod routing;

pub use backend_io::BackendIo;
use routing::{DeviceMapping, RoutingPlan, device_output_len, zone_segments_from_device_info};

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
use super::output_queue::{DeviceStagingBuffer, OutputQueue};

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

    /// Per-target latest-frame output queues.
    output_queues: HashMap<BackendDeviceKey, OutputQueue>,

    /// Per-target output lanes that bypass backend-wide locks on the frame hot path.
    device_frame_sinks: HashMap<BackendDeviceKey, DeviceFrameSinkHandle>,

    /// Reusable per-device color staging for steady-state frame routing.
    device_staging: HashMap<BackendDeviceKey, DeviceStagingBuffer>,

    /// Device staging keys touched during recent frames.
    active_staging_keys: Vec<BackendDeviceKey>,
    active_staging_len: usize,

    /// Monotonic frame generation for staging reset bookkeeping.
    staging_generation: u64,

    /// Preferred output FPS for connected devices, captured at connect time.
    device_fps_cache: HashMap<BackendDeviceKey, u32>,

    /// User-configured per-device output brightness scalar.
    device_brightness: HashMap<DeviceId, f32>,

    /// Incremented whenever software output brightness state changes.
    device_brightness_generation: u64,

    /// Reference-counted direct-control locks that suppress queued frame writes.
    direct_control_locks: HashMap<BackendDeviceKey, usize>,

    /// Layout device IDs already warned as unmapped in the current layout state.
    warned_unmapped_layout_devices: HashSet<String>,

    /// Whether unmapped layout targets should warn instead of being skipped quietly.
    unmapped_layout_warnings_enabled: bool,

    /// Number of unmapped-layout warnings emitted since process start.
    unmapped_layout_warning_count: u64,

    /// Last warning time for zone-to-segment color length mismatches.
    last_segment_mismatch_warn_at: HashMap<String, Instant>,

    /// Connected devices already reported as unused by the active layout.
    warned_inactive_layout_devices: HashSet<BackendDeviceKey>,

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
        self.output_queues
            .retain(|(queued_backend_id, _), _| queued_backend_id != &backend_id);
        self.device_frame_sinks
            .retain(|(sink_backend_id, _), _| sink_backend_id != &backend_id);
        self.device_staging
            .retain(|(staged_backend_id, _), _| staged_backend_id != &backend_id);
        self.device_fps_cache
            .retain(|(cached_backend_id, _), _| cached_backend_id != &backend_id);
        self.direct_control_locks
            .retain(|(locked_backend_id, _), _| locked_backend_id != &backend_id);
        self.warned_inactive_layout_devices
            .retain(|(warn_backend_id, _)| warn_backend_id != &backend_id);

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

    /// Map a spatial layout `device_id` to a `(backend_id, DeviceId)` pair.
    ///
    /// Call this after device discovery to link a zone's device reference
    /// to an actual connected device.
    pub fn map_device(
        &mut self,
        layout_device_id: impl Into<String>,
        backend_id: impl Into<String>,
        device_id: DeviceId,
    ) {
        self.map_device_with_segment(layout_device_id, backend_id, device_id, None);
    }

    /// Map a spatial layout `device_id` with an explicit physical LED range.
    ///
    /// When `segment` is `Some`, zone colors targeting this mapping are
    /// written into that slice of the physical output buffer.
    pub fn map_device_with_segment(
        &mut self,
        layout_device_id: impl Into<String>,
        backend_id: impl Into<String>,
        device_id: DeviceId,
        segment: Option<SegmentRange>,
    ) {
        let layout_id = layout_device_id.into();
        let backend = backend_id.into();
        debug!(
            layout_device_id = %layout_id,
            backend_id = %backend,
            %device_id,
            segment_start = segment.map(|s| s.start),
            segment_length = segment.map(|s| s.length),
            "mapped device"
        );
        self.warned_unmapped_layout_devices.remove(&layout_id);
        self.device_map.insert(
            layout_id,
            DeviceMapping {
                backend_id: backend,
                device_id,
                segment,
                zone_segments: HashMap::new(),
                physical_led_count: None,
            },
        );
        self.invalidate_routing_plan();
    }

    /// Attach hardware zone segment metadata to an existing layout-device mapping.
    ///
    /// This lets spatial zones that share one `device_id` but differ by
    /// `zone_name` target the correct physical LED ranges on multi-zone devices.
    #[must_use]
    pub fn set_device_zone_segments(
        &mut self,
        layout_device_id: &str,
        device_info: &DeviceInfo,
    ) -> bool {
        {
            let Some(mapping) = self.device_map.get_mut(layout_device_id) else {
                return false;
            };

            mapping.zone_segments = zone_segments_from_device_info(device_info);
            mapping.physical_led_count = device_output_len(device_info);
        }
        self.invalidate_routing_plan();
        true
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
        self.device_fps_cache
            .insert((backend_id.to_owned(), device_id), target_fps);
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

    /// Write one immediate color payload to a specific physical device.
    ///
    /// This bypasses spatial routing and output queues, and is intended for
    /// short, direct control operations like identify/flash actions.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_colors(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        colors: &[[u8; 3]],
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_colors(device_id, colors).await
    }

    /// Adjust hardware brightness for a specific physical device.
    ///
    /// This bypasses spatial routing and targets the backend directly.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn set_device_brightness(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        brightness: u8,
    ) -> Result<()> {
        let Some(backend) = self.backends.get(backend_id).cloned() else {
            bail!("backend '{backend_id}' is not registered");
        };

        let mut backend = backend.lock().await;
        backend
            .set_brightness(&device_id, brightness)
            .await
            .with_context(|| {
                format!(
                    "failed to set brightness {brightness} on device {device_id} using backend '{backend_id}'"
                )
            })
    }

    /// Configure software output brightness for a physical device.
    pub fn set_device_output_brightness(&mut self, device_id: DeviceId, brightness: f32) {
        let normalized = brightness.clamp(0.0, 1.0);
        let changed = if normalized >= 0.999 {
            self.device_brightness.remove(&device_id).is_some()
        } else {
            self.device_brightness
                .insert(device_id, normalized)
                .is_none_or(|previous| previous.to_bits() != normalized.to_bits())
        };
        if changed {
            self.device_brightness_generation = self.device_brightness_generation.saturating_add(1);
        }
    }

    /// Read the configured software output brightness for a physical device.
    #[must_use]
    pub fn device_output_brightness(&self, device_id: DeviceId) -> f32 {
        self.device_brightness
            .get(&device_id)
            .copied()
            .unwrap_or(1.0)
    }

    /// Monotonic generation for software output-brightness state changes.
    #[must_use]
    pub fn output_brightness_generation(&self) -> u64 {
        self.device_brightness_generation
    }

    /// Write one immediate JPEG display payload to a specific physical device.
    ///
    /// This bypasses spatial routing and targets display-capable backends
    /// directly for screen/LCD updates.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: &[u8],
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_frame(device_id, jpeg_data).await
    }

    /// Write one owned JPEG display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_frame_owned(device_id, jpeg_data).await
    }

    /// Write one owned display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_payload_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_payload_owned(device_id, payload).await
    }

    /// Cache a backend-provided output FPS for a physical device.
    pub fn set_cached_target_fps(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        target_fps: u32,
    ) {
        self.device_fps_cache
            .insert((backend_id.to_owned(), device_id), target_fps);
    }

    /// Cache a backend-provided hot-path frame sink for a physical device.
    pub fn set_device_frame_sink(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        frame_sink: Option<DeviceFrameSinkHandle>,
    ) {
        let key = (backend_id.to_owned(), device_id);
        self.output_queues.remove(&key);
        if let Some(frame_sink) = frame_sink {
            self.device_frame_sinks.insert(key, frame_sink);
        } else {
            self.device_frame_sinks.remove(&key);
        }
    }

    /// Suppress queued frame writes for a specific physical device.
    ///
    /// Returns the active direct-control lock count after incrementing.
    pub fn begin_direct_control(&mut self, backend_id: &str, device_id: DeviceId) -> usize {
        let key = (backend_id.to_owned(), device_id);
        let count = self.direct_control_locks.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    /// Release one direct-control lock for a specific physical device.
    ///
    /// Returns the remaining lock count after decrementing.
    pub fn end_direct_control(&mut self, backend_id: &str, device_id: DeviceId) -> usize {
        let key = (backend_id.to_owned(), device_id);
        let Some(count) = self.direct_control_locks.get_mut(&key) else {
            return 0;
        };

        *count = count.saturating_sub(1);
        let remaining = *count;
        if remaining == 0 {
            self.direct_control_locks.remove(&key);
        }

        remaining
    }

    /// Whether queued frame writes are currently suppressed for a device.
    #[must_use]
    pub fn is_direct_control_active(&self, backend_id: &str, device_id: DeviceId) -> bool {
        self.is_direct_control_active_key(&(backend_id.to_owned(), device_id))
    }

    fn is_direct_control_active_key(&self, key: &BackendDeviceKey) -> bool {
        self.direct_control_locks
            .get(key)
            .is_some_and(|count| *count > 0)
    }

    /// Remove a device mapping.
    pub fn unmap_device(&mut self, layout_device_id: &str) -> bool {
        let Some(mapping) = self.device_map.remove(layout_device_id) else {
            return false;
        };

        // If no other mapping targets this physical device, tear down its queue.
        let still_used = self.device_map.values().any(|candidate| {
            candidate.backend_id == mapping.backend_id && candidate.device_id == mapping.device_id
        });

        if !still_used {
            let key = (mapping.backend_id, mapping.device_id);
            self.remove_device_target_state(&key);
        }

        self.invalidate_routing_plan();
        true
    }

    /// Remove all mappings for a physical target and drop its queue.
    pub fn remove_device_mappings_for_physical(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> usize {
        let before = self.device_map.len();
        self.device_map.retain(|_, mapping| {
            !(mapping.backend_id == backend_id && mapping.device_id == device_id)
        });
        let removed = before.saturating_sub(self.device_map.len());

        if !self
            .device_map
            .values()
            .any(|mapping| mapping.backend_id == backend_id && mapping.device_id == device_id)
        {
            let key = (backend_id.to_owned(), device_id);
            self.remove_device_target_state(&key);
        }

        if removed > 0 {
            self.invalidate_routing_plan();
        }
        removed
    }

    /// Remove layout mappings for a connected physical target while keeping
    /// its output queue, frame sink, FPS cache, and direct-control state.
    pub fn clear_device_mappings_for_physical(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> usize {
        let before = self.device_map.len();
        self.device_map.retain(|_, mapping| {
            !(mapping.backend_id == backend_id && mapping.device_id == device_id)
        });
        let removed = before.saturating_sub(self.device_map.len());

        if removed > 0 {
            self.invalidate_routing_plan();
        }
        removed
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn routing_plan_rebuild_count(&self) -> u64 {
        self.routing_plan_rebuild_count
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn unmapped_layout_warning_count(&self) -> u64 {
        self.unmapped_layout_warning_count
    }

    /// Enable warnings for layout targets that still lack a connected device mapping.
    pub fn enable_unmapped_layout_warnings(&mut self) {
        self.unmapped_layout_warnings_enabled = true;
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

    /// Number of mapped devices.
    #[must_use]
    pub fn mapped_device_count(&self) -> usize {
        self.device_map.len()
    }

    fn remove_device_target_state(&mut self, key: &BackendDeviceKey) {
        self.output_queues.remove(key);
        self.device_frame_sinks.remove(key);
        self.device_staging.remove(key);
        self.device_fps_cache.remove(key);
        self.direct_control_locks.remove(key);
        self.warned_inactive_layout_devices.remove(key);
    }

    /// Return the cached target FPS for a connected physical device, if present.
    #[must_use]
    pub fn cached_target_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.device_fps_cache
            .get(&(backend_id.to_owned(), device_id))
            .copied()
    }
}
