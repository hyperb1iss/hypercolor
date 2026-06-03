//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hypercolor_driver_api::DiscoveredDevice;
use tokio::sync::Mutex;
use tracing::{debug, trace, warn};

use hypercolor_types::attachment::zone_name_matches_slot_alias;
use hypercolor_types::device::{DeviceId, DeviceInfo, OwnedDisplayFramePayload, ZoneInfo};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    LedTopology, NormalizedPosition, Output, OutputComponent, SpatialLayout, StripDirection,
};

use crate::spatial::is_led_sampled_zone;

use super::traits::{DeviceBackend, DeviceDisplaySink, DeviceFrameSink};

mod output_color;

use output_color::{apply_zone_brightness, prepare_output_for_led_ranges};

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type DeviceFrameSinkHandle = Arc<dyn DeviceFrameSink>;
type DeviceDisplaySinkHandle = Arc<dyn DeviceDisplaySink>;
type BackendDeviceKey = (String, DeviceId);
const UNMAPPED_LAYOUT_WARN_INTERVAL: Duration = Duration::from_secs(5);

/// Lightweight handle for backend I/O that can outlive the manager lock.
///
/// Clone this from [`BackendManager::backend_io`] while holding the manager
/// briefly, then perform the awaited backend call after releasing the outer
/// manager mutex.
#[derive(Clone)]
pub struct BackendIo {
    backend_id: String,
    backend: BackendHandle,
}

impl BackendIo {
    /// Connect a device, retrying once after cleanup and backend discovery refresh.
    ///
    /// Returns the backend's preferred output FPS for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails both before and
    /// after discovery refresh.
    pub async fn connect_with_refresh(&self, device_id: DeviceId) -> Result<u32> {
        self.connect_with_refresh_inner(device_id, None).await
    }

    /// Connect a device, applying timeout only to backend operations after
    /// this handle acquires the backend lock.
    ///
    /// Returns the backend's preferred output FPS for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails or times out.
    pub async fn connect_with_refresh_timeout(
        &self,
        device_id: DeviceId,
        timeout: Duration,
    ) -> Result<u32> {
        self.connect_with_refresh_inner(device_id, Some(timeout))
            .await
    }

    async fn connect_with_refresh_inner(
        &self,
        device_id: DeviceId,
        timeout: Option<Duration>,
    ) -> Result<u32> {
        let mut backend = self.backend.lock().await;

        if let Err(initial_error) = run_backend_operation(
            timeout,
            &self.backend_id,
            device_id,
            "connect",
            backend.connect(&device_id),
        )
        .await
        {
            let initial_message = initial_error.to_string();
            if is_backend_operation_timeout(&initial_error) {
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %initial_message,
                    "backend connect timed out; preserving discovery state for reconnect"
                );
                return Err(initial_error);
            } else if is_missing_discovery_descriptor(&initial_message) {
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %initial_message,
                    "backend discovery state missing; refreshing before connect retry"
                );
            } else {
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %initial_message,
                    "initial connect failed; refreshing backend discovery state and retrying"
                );

                match run_backend_operation(
                    timeout,
                    &self.backend_id,
                    device_id,
                    "disconnect cleanup",
                    backend.disconnect(&device_id),
                )
                .await
                {
                    Ok(()) => debug!(
                        backend_id = %self.backend_id,
                        %device_id,
                        "best-effort cleanup after failed connect completed"
                    ),
                    Err(cleanup_error) => debug!(
                        backend_id = %self.backend_id,
                        %device_id,
                        error = %cleanup_error,
                        "best-effort cleanup after failed connect could not release an existing session"
                    ),
                }
            }

            run_backend_operation(
                timeout,
                &self.backend_id,
                device_id,
                "discovery refresh",
                backend.discover(),
            )
            .await
            .with_context(|| {
                format!(
                    "backend '{}' discovery refresh failed after initial connect failure for device {device_id}: {initial_message}",
                    self.backend_id
                )
            })?;

            if let Err(retry_error) = run_backend_operation(
                timeout,
                &self.backend_id,
                device_id,
                "connect retry",
                backend.connect(&device_id),
            )
            .await
            {
                let retry_message = retry_error.to_string();
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %retry_message,
                    "connect still failing after discovery refresh"
                );
                return Err(retry_error).with_context(|| {
                    format!(
                        "failed to connect device {device_id} using backend '{}' after discovery refresh (initial error: {initial_message})",
                        self.backend_id
                    )
                });
            }

            debug!(
                backend_id = %self.backend_id,
                %device_id,
                "connect succeeded after discovery refresh"
            );
        }

        Ok(backend.target_fps(&device_id).unwrap_or(60))
    }

    /// Prime the backend's discovery cache from a scanner result.
    pub async fn remember_discovered_device(&self, discovered: &DiscoveredDevice) {
        let mut backend = self.backend.lock().await;
        backend.remember_discovered_device(discovered);
    }

    /// Fetch refreshed metadata for a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata retrieval fails.
    pub async fn connected_device_info(&self, device_id: DeviceId) -> Result<Option<DeviceInfo>> {
        let backend = self.backend.lock().await;
        backend
            .connected_device_info(&device_id)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch connected device metadata for {device_id} using backend '{}'",
                    self.backend_id
                )
            })
    }

    /// Clone the hot-path frame sink for a connected device, if the backend exposes one.
    pub async fn frame_sink(&self, device_id: DeviceId) -> Option<DeviceFrameSinkHandle> {
        let backend = self.backend.lock().await;
        backend.frame_sink(&device_id)
    }

    /// Clone the hot-path display sink for a connected device, if the backend exposes one.
    pub async fn display_sink(&self, device_id: DeviceId) -> Option<DeviceDisplaySinkHandle> {
        let backend = self.backend.lock().await;
        backend.display_sink(&device_id)
    }

    /// Whether this backend can briefly connect an idle device for direct control.
    pub async fn supports_temporary_direct_control(&self, info: &DeviceInfo) -> bool {
        let backend = self.backend.lock().await;
        backend.supports_temporary_direct_control(info)
    }

    /// Whether this backend consumes host-managed attachment profiles.
    pub async fn supports_host_attachment_profiles(&self, info: &DeviceInfo) -> bool {
        let backend = self.backend.lock().await;
        backend.supports_host_attachment_profiles(info)
    }

    /// Disconnect a device from the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend disconnect call fails.
    pub async fn disconnect(&self, device_id: DeviceId) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend.disconnect(&device_id).await.with_context(|| {
            format!(
                "failed to disconnect device {device_id} using backend '{}'",
                self.backend_id
            )
        })
    }

    /// Write immediate LED colors directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend write fails.
    pub async fn write_colors(&self, device_id: DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .write_colors(&device_id, colors)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} colors to device {device_id} using backend '{}'",
                    colors.len(),
                    self.backend_id
                )
            })
    }

    /// Write immediate display bytes directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame(&self, device_id: DeviceId, jpeg_data: &[u8]) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .write_display_frame(&device_id, jpeg_data)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} display bytes to device {device_id} using backend '{}'",
                    jpeg_data.len(),
                    self.backend_id
                )
            })
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame_owned(
        &self,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let byte_len = jpeg_data.len();
        let mut backend = self.backend.lock().await;
        backend
            .write_display_frame_owned(&device_id, jpeg_data)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} display bytes to device {device_id} using backend '{}'",
                    byte_len, self.backend_id
                )
            })
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_payload_owned(
        &self,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        let byte_len = payload.data.len();
        let format = payload.format;
        let mut backend = self.backend.lock().await;
        backend
            .write_display_payload_owned(&device_id, payload)
            .await
            .with_context(|| {
                format!(
                    "failed to write {byte_len} {format} display bytes to device {device_id} using backend '{}'",
                    self.backend_id
                )
            })
    }
}

fn is_missing_discovery_descriptor(message: &str) -> bool {
    message.contains(" has no pending ") && message.contains(" descriptor; run discover()")
}

fn is_backend_operation_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("transport timeout after")
            || message.contains(" timed out after ") && message.contains(" using backend ")
    })
}

async fn run_backend_operation<T, F>(
    timeout: Option<Duration>,
    backend_id: &str,
    device_id: DeviceId,
    operation: &'static str,
    future: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let Some(timeout) = timeout else {
        return future.await;
    };

    let Ok(result) = tokio::time::timeout(timeout, future).await else {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        bail!(
            "device {operation} timed out after {timeout_ms}ms using backend '{backend_id}' for device {device_id}"
        );
    };

    result
}

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

/// Internal mapping from a layout device identifier to a backend + device.
#[derive(Debug, Clone)]
struct DeviceMapping {
    backend_id: String,
    device_id: DeviceId,
    segment: Option<SegmentRange>,
    zone_segments: HashMap<String, SegmentRange>,
    physical_led_count: Option<usize>,
}

#[derive(Debug)]
struct RoutingPlan {
    layout_signature: u64,
    mapping_generation: u64,
    active_layout_device_ids: HashSet<String>,
    active_target_keys: Vec<BackendDeviceKey>,
    zone_routes: HashMap<String, PlannedZoneRoute>,
    ordered_zone_routes: Vec<OrderedZoneRoute>,
    inactive_devices: Vec<BackendDeviceKey>,
    mapped_layout_ids_by_device: HashMap<BackendDeviceKey, Vec<String>>,
}

#[derive(Debug, Clone)]
enum PlannedZoneRoute {
    Mapped(CompiledZoneRoute),
    Unmapped { layout_device_id: String },
}

#[derive(Debug, Clone)]
struct CompiledZoneRoute {
    layout_device_id: String,
    target_key: BackendDeviceKey,
    led_mapping: Option<Box<[u32]>>,
    segment: Option<SegmentRange>,
    attachment: Option<OutputComponent>,
    physical_led_count: Option<usize>,
    zone_brightness: f32,
}

#[derive(Debug, Clone)]
struct OrderedZoneRoute {
    zone_id: String,
    route: PlannedZoneRoute,
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

    fn invalidate_routing_plan(&mut self) {
        self.routing_mapping_generation = self.routing_mapping_generation.saturating_add(1);
        self.routing_plan = None;
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
            .map(|backend| BackendIo {
                backend_id: backend_id.to_owned(),
                backend,
            })
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
    pub fn ordered_routing_zone_count(&mut self, layout: &SpatialLayout) -> usize {
        self.routing_plan(layout).ordered_zone_routes.len()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn unmapped_layout_warning_count(&self) -> u64 {
        self.unmapped_layout_warning_count
    }

    /// Monotonic generation for routing-relevant device mappings.
    #[doc(hidden)]
    #[must_use]
    pub const fn routing_mapping_generation(&self) -> u64 {
        self.routing_mapping_generation
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

    /// Physical devices that are connected and mapped, but not referenced by the active layout.
    ///
    /// This is a diagnostic view over the current routing table. Devices in this list
    /// will not receive queued frame writes until a layout zone targets one of their aliases.
    #[doc(hidden)]
    #[must_use]
    pub fn connected_devices_without_layout_targets(
        &self,
        layout: &SpatialLayout,
    ) -> Vec<(String, DeviceId)> {
        let targeted = layout
            .zones
            .iter()
            .filter_map(|zone| {
                self.device_map
                    .get(zone.device_id.as_str())
                    .map(|mapping| (mapping.backend_id.clone(), mapping.device_id))
            })
            .collect::<HashSet<_>>();

        let mut inactive = self
            .device_map
            .values()
            .map(|mapping| (mapping.backend_id.clone(), mapping.device_id))
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|key| !targeted.contains(key))
            .collect::<Vec<_>>();

        inactive.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });
        inactive
    }

    /// Build transient layout zones for connected outputs absent from `layout`.
    ///
    /// These zones are never persisted. The render thread uses them to route
    /// `UnassignedBehavior::Off` and `Fallback` through the same segmented
    /// backend path as normal scene-owned zones.
    #[doc(hidden)]
    #[must_use]
    pub fn unassigned_output_zones(&self, layout: &SpatialLayout) -> Vec<Output> {
        let coverage = layout_output_coverage(layout);
        let mut layout_ids = self.device_map.keys().cloned().collect::<Vec<_>>();
        layout_ids.sort_unstable();

        let mut zones = Vec::new();
        for layout_device_id in layout_ids {
            let Some(mapping) = self.device_map.get(layout_device_id.as_str()) else {
                continue;
            };
            let coverage = coverage.get(layout_device_id.as_str());

            if !mapping.zone_segments.is_empty() {
                if coverage.is_some_and(LayoutOutputCoverage::covers_whole_device) {
                    continue;
                }

                let assigned_zone_names = coverage.map(|coverage| &coverage.zone_names);
                let mut segment_names = mapping.zone_segments.keys().cloned().collect::<Vec<_>>();
                segment_names.sort_unstable();
                for zone_name in segment_names {
                    if assigned_zone_names
                        .is_some_and(|names| zone_name_covered_by_layout(names, &zone_name))
                    {
                        continue;
                    }
                    let Some(segment) = mapping.zone_segments.get(&zone_name).copied() else {
                        continue;
                    };
                    if segment.length == 0 {
                        continue;
                    }
                    zones.push(unassigned_output_zone(
                        layout_device_id.as_str(),
                        Some(zone_name.as_str()),
                        segment.length,
                    ));
                }
                continue;
            }

            if coverage.is_some() {
                continue;
            }

            let led_count = mapping
                .segment
                .map_or(mapping.physical_led_count.unwrap_or_default(), |segment| {
                    segment.length
                });
            if led_count == 0 {
                continue;
            }
            zones.push(unassigned_output_zone(
                layout_device_id.as_str(),
                None,
                led_count,
            ));
        }

        zones
    }

    fn routing_plan(&mut self, layout: &SpatialLayout) -> Arc<RoutingPlan> {
        let layout_signature = layout_routing_signature(layout);
        let needs_rebuild = self.routing_plan.as_ref().is_none_or(|plan| {
            plan.layout_signature != layout_signature
                || plan.mapping_generation != self.routing_mapping_generation
        });

        if needs_rebuild {
            let plan = Arc::new(self.compile_routing_plan(layout, layout_signature));
            self.routing_plan = Some(Arc::clone(&plan));
            self.routing_plan_rebuild_count = self.routing_plan_rebuild_count.saturating_add(1);
            return plan;
        }

        Arc::clone(
            self.routing_plan
                .as_ref()
                .expect("routing plan should exist when cache is valid"),
        )
    }

    fn compile_routing_plan(&self, layout: &SpatialLayout, layout_signature: u64) -> RoutingPlan {
        let mut active_layout_device_ids = HashSet::with_capacity(layout.zones.len());
        let mut active_target_keys = HashSet::with_capacity(layout.zones.len());
        let mut zone_routes = HashMap::with_capacity(layout.zones.len());
        let mut ordered_zone_routes = Vec::with_capacity(layout.zones.len());

        for zone in &layout.zones {
            active_layout_device_ids.insert(zone.device_id.clone());
            let zone_brightness = normalized_zone_brightness(zone.brightness);

            let route = if let Some(mapping) = self.device_map.get(zone.device_id.as_str()) {
                let target_key = (mapping.backend_id.clone(), mapping.device_id);
                active_target_keys.insert(target_key.clone());
                PlannedZoneRoute::Mapped(CompiledZoneRoute {
                    layout_device_id: zone.device_id.clone(),
                    target_key,
                    led_mapping: normalized_led_mapping(zone.led_mapping.as_deref()),
                    segment: mapped_segment_for_zone_name(
                        &zone.id,
                        zone.zone_name.as_deref(),
                        mapping,
                    ),
                    attachment: zone.attachment.clone(),
                    physical_led_count: mapping.physical_led_count,
                    zone_brightness,
                })
            } else {
                PlannedZoneRoute::Unmapped {
                    layout_device_id: zone.device_id.clone(),
                }
            };

            if should_use_ordered_routing(zone) {
                ordered_zone_routes.push(OrderedZoneRoute {
                    zone_id: zone.id.clone(),
                    route: route.clone(),
                });
            }
            zone_routes.insert(zone.id.clone(), route);
        }

        let mut mapped_layout_ids_by_device: HashMap<BackendDeviceKey, Vec<String>> =
            HashMap::new();
        for (layout_device_id, mapping) in &self.device_map {
            mapped_layout_ids_by_device
                .entry((mapping.backend_id.clone(), mapping.device_id))
                .or_default()
                .push(layout_device_id.clone());
        }
        for ids in mapped_layout_ids_by_device.values_mut() {
            ids.sort_unstable();
        }

        let mut inactive_devices = mapped_layout_ids_by_device
            .keys()
            .filter(|key| !active_target_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        inactive_devices.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });

        RoutingPlan {
            layout_signature,
            mapping_generation: self.routing_mapping_generation,
            active_layout_device_ids,
            active_target_keys: {
                let mut active_target_keys = active_target_keys.into_iter().collect::<Vec<_>>();
                active_target_keys.sort_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
                });
                active_target_keys
            },
            zone_routes,
            ordered_zone_routes,
            inactive_devices,
            mapped_layout_ids_by_device,
        }
    }

    fn begin_staging_frame(&mut self) {
        self.staging_generation = self.staging_generation.saturating_add(1);
        self.active_staging_len = 0;
    }

    fn staging_buffer(&mut self, key: &BackendDeviceKey) -> &mut DeviceStagingBuffer {
        let generation = self.staging_generation;
        let mut became_active = false;

        if let Some(staging) = self.device_staging.get_mut(key) {
            if staging.frame_generation != generation {
                staging.output.clear();
                staging.required_len = 0;
                staging.written_ranges.clear();
                staging.has_segmented_write = false;
                staging.frame_generation = generation;
                became_active = true;
            }
        } else {
            let staging = self.device_staging.entry(key.clone()).or_default();
            staging.output.clear();
            staging.required_len = 0;
            staging.written_ranges.clear();
            staging.has_segmented_write = false;
            staging.frame_generation = generation;
            became_active = true;
        }

        if became_active {
            if self.active_staging_len < self.active_staging_keys.len() {
                self.active_staging_keys[self.active_staging_len].clone_from(key);
            } else {
                self.active_staging_keys.push(key.clone());
            }
            self.active_staging_len += 1;
        }

        self.device_staging
            .get_mut(key)
            .expect("staging buffer must exist after entry initialization")
    }

    fn remove_device_target_state(&mut self, key: &BackendDeviceKey) {
        self.output_queues.remove(key);
        self.device_frame_sinks.remove(key);
        self.device_staging.remove(key);
        self.device_fps_cache.remove(key);
        self.direct_control_locks.remove(key);
        self.warned_inactive_layout_devices.remove(key);
    }

    /// Push frame color data to all mapped devices.
    ///
    /// For each zone in `zone_colors`, looks up the target device via the
    /// spatial layout's zone-to-device mapping, groups colors by device,
    /// and enqueues one payload per device. Errors are
    /// collected but do not halt processing — every mapped device gets
    /// its data.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::too_many_lines,
        reason = "frame routing keeps mapping, remap, segmented writes, and queue dispatch together so the hot-path ordering stays readable"
    )]
    pub async fn write_frame(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
    ) -> FrameWriteStats {
        self.write_frame_with_brightness(zone_colors, layout, 1.0, None)
            .await
    }

    /// Push frame color data to all mapped devices with optional per-device
    /// output brightness scalars.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::too_many_lines,
        reason = "frame routing keeps mapping, remap, segmented writes, queue dispatch together so the hot-path ordering stays readable"
    )]
    pub async fn write_frame_with_brightness(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
        global_brightness: f32,
        device_brightness: Option<&HashMap<DeviceId, f32>>,
    ) -> FrameWriteStats {
        self.begin_staging_frame();
        let plan = self.routing_plan(layout);
        self.warned_unmapped_layout_devices
            .retain(|layout_device_id| plan.active_layout_device_ids.contains(layout_device_id));

        let mut stats = FrameWriteStats::default();

        let newly_inactive = plan
            .inactive_devices
            .iter()
            .filter(|key| !self.warned_inactive_layout_devices.contains(*key))
            .cloned()
            .collect::<Vec<_>>();

        if !newly_inactive.is_empty() {
            let devices = newly_inactive
                .iter()
                .take(8)
                .map(|(backend_id, device_id)| format!("{backend_id}:{device_id}"))
                .collect::<Vec<_>>();
            let mapped_layout_ids_by_device = newly_inactive
                .iter()
                .take(8)
                .map(|(backend_id, device_id)| {
                    let aliases = plan
                        .mapped_layout_ids_by_device
                        .get(&(backend_id.clone(), *device_id))
                        .cloned()
                        .unwrap_or_default();
                    format!("{backend_id}:{device_id} => [{}]", aliases.join(", "))
                })
                .collect::<Vec<_>>();
            let inactive_device_count = newly_inactive.len();
            let omitted_device_count = inactive_device_count.saturating_sub(devices.len());
            if layout.zones.is_empty() {
                debug!(
                    inactive_device_count,
                    sample_devices = ?devices,
                    omitted_device_count,
                    layout_zone_count = layout.zones.len(),
                    "connected devices are not in the empty active layout; frames will not be sent"
                );
            } else {
                warn!(
                    inactive_device_count,
                    sample_devices = ?devices,
                    omitted_device_count,
                    layout_zone_count = layout.zones.len(),
                    sample_mapped_layout_ids = ?mapped_layout_ids_by_device,
                    "connected devices have no active layout zones; frames will not be sent"
                );
            }
        }
        self.warned_inactive_layout_devices.clear();
        self.warned_inactive_layout_devices
            .extend(plan.inactive_devices.iter().cloned());

        if zone_colors.len() == plan.ordered_zone_routes.len()
            && zone_colors
                .iter()
                .zip(&plan.ordered_zone_routes)
                .all(|(zone_colors, ordered)| zone_colors.zone_id == ordered.zone_id)
        {
            for (zone_colors, ordered) in zone_colors.iter().zip(&plan.ordered_zone_routes) {
                self.route_zone_colors(
                    zone_colors.zone_id.as_str(),
                    &zone_colors.colors,
                    &ordered.route,
                );
            }
        } else {
            for zc in zone_colors {
                let Some(route) = plan.zone_routes.get(zc.zone_id.as_str()) else {
                    warn!(zone_id = %zc.zone_id, "zone not found in spatial layout");
                    continue;
                };
                self.route_zone_colors(&zc.zone_id, &zc.colors, route);
            }
        }

        let active_staging_len = self.active_staging_len;
        let mut active_staging_keys = Vec::new();
        std::mem::swap(&mut active_staging_keys, &mut self.active_staging_keys);
        self.active_staging_len = 0;

        for key in active_staging_keys.iter().take(active_staging_len) {
            let (backend_id, device_id) = key;

            if self.is_direct_control_active_key(key) {
                trace!(
                    backend_id = %backend_id,
                    device_id = %device_id,
                    "skipping queued device frame while direct control is active"
                );
                continue;
            }

            if !self.backends.contains_key(backend_id.as_str()) {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            }

            let device_output_brightness = self.device_output_brightness(*device_id);
            let per_frame_brightness = device_brightness
                .and_then(|settings| settings.get(device_id).copied())
                .unwrap_or(1.0);
            let brightness = (global_brightness * per_frame_brightness * device_output_brightness)
                .clamp(0.0, 1.0);

            let values = {
                let staging = self
                    .device_staging
                    .get_mut(key)
                    .expect("active staging key should resolve to a staging buffer");
                if staging.output.len() < staging.required_len {
                    staging.output.resize(staging.required_len, [0, 0, 0]);
                }

                prepare_output_for_led_ranges(
                    &mut staging.output,
                    &staging.written_ranges,
                    brightness,
                );

                let mut values = Vec::new();
                std::mem::swap(&mut values, &mut staging.output);
                values
            };

            let Some(queue) = self.ensure_output_queue_for_key(key) else {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                if let Some(staging) = self.device_staging.get_mut(key) {
                    staging.output = values;
                }
                continue;
            };

            stats.devices_written += 1;
            stats.total_leds += values.len();
            let recycled = queue.push(values);
            if let (Some(staging), Some(recycled)) = (self.device_staging.get_mut(key), recycled) {
                staging.output = recycled;
            }
        }

        self.active_staging_keys = active_staging_keys;

        stats
    }

    /// Whether existing queued outputs can be reused for the active layout.
    ///
    /// Returns `true` when every active physical target already has an output
    /// queue, so a retained frame can skip re-routing and reuse the latest
    /// queued payloads.
    #[must_use]
    pub fn can_reuse_routed_frame_outputs(&mut self, layout: &SpatialLayout) -> bool {
        let plan = self.routing_plan(layout);
        plan.active_target_keys.iter().all(|key| {
            self.is_direct_control_active_key(key) || self.output_queues.contains_key(key)
        })
    }

    /// Stable identity for the active routed-output lane.
    #[must_use]
    pub fn routed_output_signature(&mut self, layout: &SpatialLayout) -> u64 {
        let plan = self.routing_plan(layout);
        let mut hasher = DefaultHasher::new();
        plan.layout_signature.hash(&mut hasher);
        plan.mapping_generation.hash(&mut hasher);
        plan.active_target_keys.hash(&mut hasher);
        hasher.finish()
    }

    /// Reuse the latest routed outputs for the active layout.
    ///
    /// This only nudges queues that need a retry after an asynchronous write
    /// failure; successfully queued or in-flight identical payloads are left
    /// untouched.
    pub fn reuse_routed_frame_outputs(&mut self, layout: &SpatialLayout) -> FrameWriteStats {
        let plan = self.routing_plan(layout);
        let mut stats = FrameWriteStats::default();

        for key in &plan.active_target_keys {
            if self.is_direct_control_active_key(key) {
                continue;
            }

            let Some(queue) = self.output_queues.get_mut(key) else {
                continue;
            };
            if let Some(led_count) = queue.retry_latest_after_error() {
                stats.devices_written = stats.devices_written.saturating_add(1);
                stats.total_leds = stats.total_leds.saturating_add(led_count);
            }
        }

        stats
    }

    fn route_zone_colors(&mut self, zone_id: &str, colors: &[[u8; 3]], route: &PlannedZoneRoute) {
        let PlannedZoneRoute::Mapped(route) = route else {
            let PlannedZoneRoute::Unmapped { layout_device_id } = route else {
                unreachable!("only mapped or unmapped zone routes are compiled");
            };
            if !self.unmapped_layout_warnings_enabled {
                return;
            }
            if self
                .warned_unmapped_layout_devices
                .insert(layout_device_id.clone())
            {
                self.unmapped_layout_warning_count =
                    self.unmapped_layout_warning_count.saturating_add(1);
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %layout_device_id,
                    "zone skipped because the target layout device is not mapped to a connected backend device"
                );
            }
            return;
        };

        self.warned_unmapped_layout_devices
            .remove(route.layout_device_id.as_str());

        let segment = attachment_segment_for_zone(
            zone_id,
            route.segment,
            route.attachment.as_ref(),
            colors.len(),
        );
        let mismatch = {
            let staging = self.staging_buffer(&route.target_key);
            staging.required_len = staging
                .required_len
                .max(route.physical_led_count.unwrap_or_default());
            let remapped_colors = remap_zone_colors(
                zone_id,
                colors,
                route.led_mapping.as_deref(),
                &mut staging.remap_scratch,
            );
            let remapped_len = remapped_colors.len();

            if let Some(segment) = segment {
                staging.has_segmented_write = true;
                let segment_end = segment.end();
                if staging.output.len() < segment_end {
                    staging.output.resize(segment_end, [0, 0, 0]);
                }

                let wrote_segment =
                    write_segment_colors(&mut staging.output, segment, remapped_colors);
                if wrote_segment {
                    let start = segment.start;
                    let end = segment.end();
                    apply_zone_brightness(&mut staging.output[start..end], route.zone_brightness);
                    staging.mark_written_range(start, end);
                }

                (!wrote_segment && segment.length > 0).then_some((
                    segment.start,
                    segment.length,
                    remapped_len,
                ))
            } else {
                if staging.has_segmented_write {
                    warn!(
                        zone_id = %zone_id,
                        "mixed segmented and non-segmented mappings for the same physical device"
                    );
                }
                let start = staging.output.len();
                staging.output.extend_from_slice(remapped_colors);
                let end = staging.output.len();
                apply_zone_brightness(&mut staging.output[start..end], route.zone_brightness);
                staging.mark_written_range(start, end);
                None
            }
        };

        if let Some((segment_start, expected, received)) = mismatch {
            let warn_key = format!("{}:{zone_id}", route.layout_device_id);
            let should_warn = self
                .last_segment_mismatch_warn_at
                .get(&warn_key)
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= UNMAPPED_LAYOUT_WARN_INTERVAL);

            if should_warn {
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %route.layout_device_id,
                    segment_start,
                    expected,
                    received,
                    "zone color count does not match mapped segment length"
                );
                self.last_segment_mismatch_warn_at
                    .insert(warn_key, Instant::now());
            }
        }
    }

    fn ensure_output_queue_for_key(&mut self, key: &BackendDeviceKey) -> Option<&mut OutputQueue> {
        let frame_sink = self.device_frame_sinks.get(key).cloned();
        let should_replace_queue = self
            .output_queues
            .get(key)
            .is_some_and(|queue| queue.uses_frame_sink() != frame_sink.is_some());

        if should_replace_queue {
            self.output_queues.remove(key);
        }

        if !self.output_queues.contains_key(key) {
            let backend = self.backends.get(key.0.as_str())?.clone();
            let target_fps = self.device_fps_cache.get(key).copied().unwrap_or(60);
            let queue = OutputQueue::spawn(key.0.clone(), key.1, backend, frame_sink, target_fps);
            self.output_queues.insert(key.clone(), queue);
        }

        self.output_queues.get_mut(key)
    }

    /// Return the cached target FPS for a connected physical device, if present.
    #[must_use]
    pub fn cached_target_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.device_fps_cache
            .get(&(backend_id.to_owned(), device_id))
            .copied()
    }

    /// Snapshot async write failures currently retained by output queues.
    #[must_use]
    pub fn async_write_failures(&self) -> Vec<AsyncWriteFailure> {
        let mut failures = self
            .output_queues
            .iter()
            .filter_map(|((backend_id, device_id), queue)| {
                let error = queue.last_error()?;

                Some(AsyncWriteFailure {
                    backend_id: backend_id.clone(),
                    device_id: *device_id,
                    error,
                })
            })
            .collect::<Vec<_>>();

        failures.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
        });
        failures
    }

    /// Build a typed per-device output telemetry snapshot for collector tasks.
    #[must_use]
    pub fn device_output_statistics(&self) -> Vec<DeviceOutputStatistics> {
        let mut layout_ids_by_key: HashMap<BackendDeviceKey, Vec<String>> = HashMap::new();
        for (layout_device_id, mapping) in &self.device_map {
            layout_ids_by_key
                .entry((mapping.backend_id.clone(), mapping.device_id))
                .or_default()
                .push(layout_device_id.clone());
        }

        for ids in layout_ids_by_key.values_mut() {
            ids.sort_unstable();
        }

        let mut queues = Vec::with_capacity(self.output_queues.len());
        for ((backend_id, device_id), queue) in &self.output_queues {
            let mapped_layout_ids = layout_ids_by_key
                .get(&(backend_id.clone(), *device_id))
                .cloned()
                .unwrap_or_default();
            queues.push(queue.statistics(backend_id, *device_id, mapped_layout_ids));
        }

        queues.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
        });
        queues
    }

    /// Build a debug snapshot of queue and routing internals.
    #[must_use]
    pub fn debug_snapshot(&self) -> BackendManagerDebugSnapshot {
        let queues = self
            .device_output_statistics()
            .into_iter()
            .map(DeviceOutputStatistics::into_debug_snapshot)
            .collect::<Vec<_>>();

        BackendManagerDebugSnapshot {
            queue_count: queues.len(),
            mapped_device_count: self.device_map.len(),
            queues,
        }
    }

    /// Build a routing-focused debug snapshot (layout IDs -> backend targets).
    #[must_use]
    pub fn routing_snapshot(&self) -> BackendRoutingDebugSnapshot {
        let mut backend_ids = self.backends.keys().cloned().collect::<Vec<_>>();
        backend_ids.sort_unstable();

        let mapped_keys = self
            .device_map
            .values()
            .map(|mapping| (mapping.backend_id.clone(), mapping.device_id))
            .collect::<std::collections::HashSet<_>>();

        let mut mappings = self
            .device_map
            .iter()
            .map(|(layout_device_id, mapping)| {
                let key = (mapping.backend_id.clone(), mapping.device_id);
                LayoutRoutingDebugEntry {
                    layout_device_id: layout_device_id.clone(),
                    backend_id: mapping.backend_id.clone(),
                    device_id: mapping.device_id.to_string(),
                    backend_registered: self.backends.contains_key(&mapping.backend_id),
                    queue_active: self.output_queues.contains_key(&key),
                }
            })
            .collect::<Vec<_>>();
        mappings.sort_by(|left, right| left.layout_device_id.cmp(&right.layout_device_id));

        let mut orphaned_queues = self
            .output_queues
            .keys()
            .filter(|key| !mapped_keys.contains(*key))
            .map(|(backend_id, device_id)| OrphanedQueueDebugEntry {
                backend_id: backend_id.clone(),
                device_id: device_id.to_string(),
            })
            .collect::<Vec<_>>();
        orphaned_queues.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.cmp(&right.device_id))
        });

        BackendRoutingDebugSnapshot {
            backend_ids,
            mapping_count: self.device_map.len(),
            queue_count: self.output_queues.len(),
            mappings,
            orphaned_queues,
        }
    }
}

#[derive(Debug, Default)]
struct LayoutOutputCoverage {
    covers_whole_device: bool,
    zone_names: HashSet<String>,
}

impl LayoutOutputCoverage {
    const fn covers_whole_device(&self) -> bool {
        self.covers_whole_device
    }
}

fn layout_output_coverage(layout: &SpatialLayout) -> HashMap<&str, LayoutOutputCoverage> {
    let mut coverage = HashMap::new();
    for zone in layout.zones.iter().filter(|zone| is_led_sampled_zone(zone)) {
        let entry = coverage
            .entry(zone.device_id.as_str())
            .or_insert_with(LayoutOutputCoverage::default);
        if let Some(zone_name) = zone.zone_name.as_ref() {
            entry.zone_names.insert(zone_name.clone());
        } else {
            entry.covers_whole_device = true;
        }
    }
    coverage
}

fn zone_name_covered_by_layout(assigned_zone_names: &HashSet<String>, zone_name: &str) -> bool {
    assigned_zone_names
        .iter()
        .any(|assigned| zone_name_matches_slot_alias(Some(assigned.as_str()), Some(zone_name)))
}

fn unassigned_output_zone(
    layout_device_id: &str,
    zone_name: Option<&str>,
    led_count: usize,
) -> Output {
    let led_count = u32::try_from(led_count).unwrap_or(u32::MAX);
    Output {
        id: unassigned_output_zone_id(layout_device_id, zone_name),
        name: zone_name.map_or_else(
            || format!("{layout_device_id} unassigned"),
            |zone_name| format!("{layout_device_id} {zone_name} unassigned"),
        ),
        device_id: layout_device_id.to_owned(),
        zone_name: zone_name.map(str::to_owned),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        display_order: i32::MAX,
        orientation: None,
        topology: LedTopology::Strip {
            count: led_count,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn unassigned_output_zone_id(layout_device_id: &str, zone_name: Option<&str>) -> String {
    zone_name.map_or_else(
        || format!("__unassigned:{layout_device_id}"),
        |zone_name| format!("__unassigned:{layout_device_id}:{zone_name}"),
    )
}

fn should_use_ordered_routing(zone: &Output) -> bool {
    is_led_sampled_zone(zone)
}

fn remap_zone_colors<'a>(
    zone_id: &str,
    colors: &'a [[u8; 3]],
    led_mapping: Option<&[u32]>,
    scratch: &'a mut Vec<[u8; 3]>,
) -> &'a [[u8; 3]] {
    let Some(led_mapping) = led_mapping else {
        return colors;
    };

    if led_mapping.len() != colors.len() {
        warn!(
            zone_id = %zone_id,
            mapping_len = led_mapping.len(),
            color_len = colors.len(),
            "ignoring zone LED mapping because it does not match the sampled LED count"
        );
        return colors;
    }

    scratch.clear();
    scratch.resize(colors.len(), [0, 0, 0]);
    for (spatial_index, &physical_index) in led_mapping.iter().enumerate() {
        let Ok(physical_index) = usize::try_from(physical_index) else {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                "ignoring zone LED mapping because one physical index does not fit in usize"
            );
            return colors;
        };
        if physical_index >= scratch.len() {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                color_len = colors.len(),
                "ignoring zone LED mapping because one physical index is out of bounds"
            );
            return colors;
        }
        scratch[physical_index] = colors[spatial_index];
    }

    scratch.as_slice()
}

fn write_segment_colors(
    output: &mut Vec<[u8; 3]>,
    segment: SegmentRange,
    colors: &[[u8; 3]],
) -> bool {
    if segment.length == 0 {
        return true;
    }
    if colors.is_empty() {
        return false;
    }

    let start = segment.start;
    let end = segment.end();
    if output.len() < end {
        output.resize(end, [0, 0, 0]);
    }
    let target = &mut output[start..end];

    match colors.len().cmp(&segment.length) {
        std::cmp::Ordering::Equal => target.copy_from_slice(colors),
        std::cmp::Ordering::Less | std::cmp::Ordering::Greater => {
            if colors.len() == 1 {
                target.fill(colors[0]);
            } else {
                let source_len = colors.len();
                let target_len = target.len();
                for (index, color) in target.iter_mut().enumerate() {
                    let source_index = index.saturating_mul(source_len) / target_len;
                    *color = colors[source_index.min(source_len - 1)];
                }
            }
        }
    }

    true
}

fn normalized_led_mapping(led_mapping: Option<&[u32]>) -> Option<Box<[u32]>> {
    let led_mapping = led_mapping?;

    if led_mapping
        .iter()
        .enumerate()
        .all(|(index, &physical_index)| u32::try_from(index).ok() == Some(physical_index))
    {
        return None;
    }

    Some(led_mapping.to_vec().into_boxed_slice())
}

fn zone_segments_from_device_info(device_info: &DeviceInfo) -> HashMap<String, SegmentRange> {
    let mut next_start = 0_usize;
    let mut segments = HashMap::with_capacity(device_info.zones.len());

    for zone in &device_info.zones {
        let Some(segment) = next_zone_segment(zone, next_start) else {
            continue;
        };
        next_start = segment.end();
        segments.insert(zone.name.clone(), segment);
    }

    segments
}

fn device_output_len(device_info: &DeviceInfo) -> Option<usize> {
    let total_leds = device_info
        .total_led_count()
        .max(device_info.capabilities.led_count);
    let Ok(total_leds) = usize::try_from(total_leds) else {
        warn!(
            device = %device_info.name,
            device_led_count = total_leds,
            "ignoring device output length because led_count does not fit in usize"
        );
        return None;
    };

    Some(total_leds)
}

fn next_zone_segment(zone: &ZoneInfo, start: usize) -> Option<SegmentRange> {
    let Ok(length) = usize::try_from(zone.led_count) else {
        warn!(
            zone_name = %zone.name,
            zone_led_count = zone.led_count,
            "ignoring device zone segment because led_count does not fit in usize"
        );
        return None;
    };

    Some(SegmentRange::new(start, length))
}

fn mapped_segment_for_zone_name(
    zone_id: &str,
    zone_name: Option<&str>,
    mapping: &DeviceMapping,
) -> Option<SegmentRange> {
    let Some(zone_name) = zone_name else {
        return mapping.segment;
    };
    let Some(zone_segment) = zone_segment_for_name(&mapping.zone_segments, zone_name) else {
        return mapping.segment;
    };

    let Some(base_segment) = mapping.segment else {
        return Some(zone_segment);
    };

    if zone_segment.start >= base_segment.start && zone_segment.end() <= base_segment.end() {
        return Some(zone_segment);
    }

    if base_segment.start >= zone_segment.start && base_segment.end() <= zone_segment.end() {
        return Some(base_segment);
    }

    let overlap_start = base_segment.start.max(zone_segment.start);
    let overlap_end = base_segment.end().min(zone_segment.end());
    if overlap_start < overlap_end {
        warn!(
            zone_id = %zone_id,
            zone_name = %zone_name,
            base_segment_start = base_segment.start,
            base_segment_length = base_segment.length,
            zone_segment_start = zone_segment.start,
            zone_segment_length = zone_segment.length,
            "using the overlap between the logical device segment and the hardware zone segment"
        );
        return Some(SegmentRange::new(
            overlap_start,
            overlap_end.saturating_sub(overlap_start),
        ));
    }

    warn!(
        zone_id = %zone_id,
        zone_name = %zone_name,
        base_segment_start = base_segment.start,
        base_segment_length = base_segment.length,
        zone_segment_start = zone_segment.start,
        zone_segment_length = zone_segment.length,
        "ignoring hardware zone segment because it does not overlap the mapped logical segment"
    );
    Some(base_segment)
}

fn zone_segment_for_name(
    zone_segments: &HashMap<String, SegmentRange>,
    zone_name: &str,
) -> Option<SegmentRange> {
    zone_segments.get(zone_name).copied().or_else(|| {
        zone_segments.iter().find_map(|(candidate, segment)| {
            zone_name_matches_slot_alias(Some(zone_name), Some(candidate)).then_some(*segment)
        })
    })
}

fn layout_routing_signature(layout: &SpatialLayout) -> u64 {
    let mut hasher = DefaultHasher::new();
    layout.id.hash(&mut hasher);
    layout.zones.len().hash(&mut hasher);

    for zone in &layout.zones {
        zone.id.hash(&mut hasher);
        zone.device_id.hash(&mut hasher);
        zone.zone_name.hash(&mut hasher);
        zone.led_mapping.hash(&mut hasher);
        normalized_zone_brightness(zone.brightness)
            .to_bits()
            .hash(&mut hasher);
        hash_attachment(zone.attachment.as_ref(), &mut hasher);
    }

    hasher.finish()
}

fn normalized_zone_brightness(brightness: Option<f32>) -> f32 {
    brightness.unwrap_or(1.0).clamp(0.0, 1.0)
}

fn hash_attachment(attachment: Option<&OutputComponent>, hasher: &mut DefaultHasher) {
    let Some(attachment) = attachment else {
        0_u8.hash(hasher);
        return;
    };

    1_u8.hash(hasher);
    attachment.template_id.hash(hasher);
    attachment.slot_id.hash(hasher);
    attachment.instance.hash(hasher);
    attachment.led_start.hash(hasher);
    attachment.led_count.hash(hasher);
    attachment.led_mapping.hash(hasher);
}

fn attachment_segment_for_zone(
    zone_id: &str,
    base_segment: Option<SegmentRange>,
    attachment: Option<&OutputComponent>,
    sampled_led_count: usize,
) -> Option<SegmentRange> {
    let Some(attachment) = attachment else {
        return base_segment;
    };
    let (Some(led_start), Some(led_count)) = (attachment.led_start, attachment.led_count) else {
        return base_segment;
    };

    let Ok(led_start) = usize::try_from(led_start) else {
        warn!(
            zone_id = %zone_id,
            attachment_led_start = led_start,
            "ignoring attachment segment override because led_start does not fit in usize"
        );
        return base_segment;
    };
    let Ok(led_count) = usize::try_from(led_count) else {
        warn!(
            zone_id = %zone_id,
            attachment_led_count = led_count,
            "ignoring attachment segment override because led_count does not fit in usize"
        );
        return base_segment;
    };
    let resolved_led_count = if sampled_led_count > 0 && sampled_led_count != led_count {
        debug!(
            zone_id = %zone_id,
            attachment_led_count = led_count,
            sampled_led_count,
            "attachment segment length differs from sampled zone length; using sampled LED count"
        );
        sampled_led_count
    } else {
        led_count
    };
    let attachment_end = led_start.saturating_add(resolved_led_count);

    if let Some(base_segment) = base_segment {
        if led_start >= base_segment.start && attachment_end <= base_segment.end() {
            return Some(SegmentRange::new(led_start, resolved_led_count));
        }

        if attachment_end <= base_segment.length {
            return Some(SegmentRange::new(
                base_segment.start.saturating_add(led_start),
                resolved_led_count,
            ));
        }

        if resolved_led_count == base_segment.length {
            return Some(base_segment);
        }

        warn!(
            zone_id = %zone_id,
            attachment_led_start = led_start,
            attachment_led_count = led_count,
            resolved_led_count,
            base_segment_start = base_segment.start,
            base_segment_length = base_segment.length,
            "ignoring attachment segment override because it exceeds the mapped segment"
        );
        return Some(base_segment);
    }

    Some(SegmentRange::new(led_start, resolved_led_count))
}
