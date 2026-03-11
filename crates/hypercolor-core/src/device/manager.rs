//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use hypercolor_types::canvas::{linear_to_output_u8, srgb_to_linear};
use hypercolor_types::device::{DeviceId, DeviceInfo, ZoneInfo};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{SpatialLayout, ZoneAttachment};

use super::traits::DeviceBackend;

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type BackendDeviceKey = (String, DeviceId);
type ZoneRoute<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a [u32]>,
    Option<&'a ZoneAttachment>,
);
const UNMAPPED_LAYOUT_WARN_INTERVAL: Duration = Duration::from_secs(5);
const LED_PERCEPTUAL_COMPENSATION_STRENGTH: f32 = 0.22;
const LED_NEUTRAL_COMPENSATION_WEIGHT: f32 = 0.25;
const LED_HEADROOM_WEIGHT_FLOOR: f32 = 0.1;

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
    /// Connect a device, retrying once after backend discovery refresh.
    ///
    /// Returns the backend's preferred output FPS for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails both before and
    /// after discovery refresh.
    pub async fn connect_with_refresh(&self, device_id: DeviceId) -> Result<u32> {
        let mut backend = self.backend.lock().await;

        if let Err(initial_error) = backend.connect(&device_id).await {
            let initial_message = initial_error.to_string();
            debug!(
                backend_id = %self.backend_id,
                %device_id,
                error = %initial_message,
                "initial connect failed; refreshing backend discovery state and retrying"
            );

            backend.discover().await.with_context(|| {
                format!(
                    "backend '{}' discovery refresh failed after initial connect failure for device {device_id}: {initial_message}",
                    self.backend_id
                )
            })?;

            if let Err(retry_error) = backend.connect(&device_id).await {
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

// ── Debug Snapshot ──────────────────────────────────────────────────────────

/// Snapshot of backend dispatch internals for reverse-engineering and tuning.
#[derive(Debug, Clone, Serialize)]
pub struct BackendManagerDebugSnapshot {
    /// Number of active output queues.
    pub queue_count: usize,

    /// Number of mapped layout devices.
    pub mapped_device_count: usize,

    /// Per-queue diagnostics.
    pub queues: Vec<OutputQueueDebugSnapshot>,
}

/// Snapshot of layout-to-backend routing state.
#[derive(Debug, Clone, Serialize)]
pub struct BackendRoutingDebugSnapshot {
    /// Registered backend IDs.
    pub backend_ids: Vec<String>,

    /// Number of layout-device mappings.
    pub mapping_count: usize,

    /// Number of active output queues.
    pub queue_count: usize,

    /// Detailed routing entries for each mapped layout device.
    pub mappings: Vec<LayoutRoutingDebugEntry>,

    /// Active queues with no corresponding layout mapping.
    pub orphaned_queues: Vec<OrphanedQueueDebugEntry>,
}

/// One layout-device routing entry.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutRoutingDebugEntry {
    /// Layout-level device reference.
    pub layout_device_id: String,

    /// Target backend ID.
    pub backend_id: String,

    /// Target backend device ID.
    pub device_id: String,

    /// Whether the target backend is currently registered.
    pub backend_registered: bool,

    /// Whether a queue is active for this mapping.
    pub queue_active: bool,
}

/// Queue entry that currently has no layout mapping.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanedQueueDebugEntry {
    /// Backend ID for the orphaned queue.
    pub backend_id: String,

    /// Device ID for the orphaned queue.
    pub device_id: String,
}

/// Debug stats for a single output queue.
#[derive(Debug, Clone, Serialize)]
pub struct OutputQueueDebugSnapshot {
    /// Backend ID this queue targets.
    pub backend_id: String,

    /// Device ID this queue targets.
    pub device_id: String,

    /// Layout device IDs currently routed to this queue.
    pub mapped_layout_ids: Vec<String>,

    /// Configured target frame rate for this queue.
    pub target_fps: u32,

    /// Total frames accepted from the render loop.
    pub frames_received: u64,

    /// Total frames successfully written by the worker.
    pub frames_sent: u64,

    /// Frames dropped due to latest-frame replacement while I/O was busy.
    pub frames_dropped: u64,

    /// Average latency from enqueue to write completion.
    pub avg_latency_ms: u64,

    /// Last async write error observed by this queue worker.
    pub last_error: Option<String>,

    /// Milliseconds since last worker write attempt.
    pub last_sent_ago_ms: Option<u64>,

    /// Most recent frame sequence seen by this queue.
    pub last_sequence: u64,
}

/// One async device write failure observed by an output queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncWriteFailure {
    /// Backend ID that owns the queue.
    pub backend_id: String,
    /// Physical device ID targeted by the queue.
    pub device_id: DeviceId,
    /// Most recent async write error string.
    pub error: String,
}

#[derive(Debug, Clone, Default)]
struct OutputQueueMetrics {
    frames_received: u64,
    frames_sent: u64,
    frames_dropped: u64,
    total_latency: Duration,
    last_error: Option<String>,
    last_sent_at: Option<Instant>,
}

// ── OutputQueue ─────────────────────────────────────────────────────────────

/// Frame payload queued for asynchronous backend writes.
#[derive(Debug, Clone)]
struct FramePayload {
    /// LED colors for the target device.
    colors: Vec<[u8; 3]>,
    /// Monotonic sequence for dropped-frame diagnostics.
    sequence: u64,
    /// Timestamp when this payload was queued by the render loop.
    produced_at: Instant,
}

/// Latest-frame queue for a single `(backend_id, device_id)` target.
///
/// Internally uses a `watch` channel so stale queued payloads are replaced
/// atomically and the sender never blocks the render loop.
struct OutputQueue {
    tx: watch::Sender<Option<Arc<FramePayload>>>,
    _io_task: JoinHandle<()>,
    target_fps: u32,
    metrics: Arc<StdMutex<OutputQueueMetrics>>,
    next_sequence: u64,
}

impl OutputQueue {
    /// Spawn an output worker for one physical target.
    fn spawn(
        backend_id: String,
        device_id: DeviceId,
        backend: BackendHandle,
        target_fps: u32,
    ) -> Self {
        let (tx, mut rx) = watch::channel(None::<Arc<FramePayload>>);
        let metrics = Arc::new(StdMutex::new(OutputQueueMetrics::default()));
        let metrics_for_task = Arc::clone(&metrics);

        let io_task = tokio::spawn(async move {
            let send_interval = target_interval_for_fps(target_fps);
            let mut next_send_at = Instant::now();
            let mut last_sent_sequence = 0_u64;
            let mut pending = None::<Arc<FramePayload>>;

            loop {
                if pending.is_none() {
                    // Sender dropped => manager shutdown or queue removed.
                    if rx.changed().await.is_err() {
                        break;
                    }
                    pending.clone_from(&rx.borrow_and_update());
                    continue;
                }

                if send_interval.is_some() {
                    tokio::select! {
                        changed = rx.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            pending.clone_from(&rx.borrow_and_update());
                            continue;
                        }
                        () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {}
                    }
                }

                let Some(frame) = pending.take() else {
                    continue;
                };

                if frame.sequence > last_sent_sequence + 1 {
                    let dropped = frame.sequence - last_sent_sequence - 1;
                    if let Ok(mut snapshot) = metrics_for_task.lock() {
                        snapshot.frames_dropped = snapshot.frames_dropped.saturating_add(dropped);
                    }

                    trace!(
                        backend_id = %backend_id,
                        device_id = %device_id,
                        dropped,
                        "dropping stale device frames"
                    );
                }

                let result = {
                    let mut backend = backend.lock().await;
                    backend.write_colors(&device_id, &frame.colors).await
                };
                let send_completed = Instant::now();

                if let Ok(mut snapshot) = metrics_for_task.lock() {
                    snapshot.last_sent_at = Some(send_completed);

                    match &result {
                        Ok(()) => {
                            snapshot.frames_sent = snapshot.frames_sent.saturating_add(1);
                            snapshot.total_latency +=
                                send_completed.saturating_duration_since(frame.produced_at);
                            snapshot.last_error = None;
                        }
                        Err(error) => {
                            snapshot.last_error = Some(error.to_string());
                        }
                    }
                }

                if let Err(error) = result {
                    warn!(
                        backend_id = %backend_id,
                        device_id = %device_id,
                        error = %error,
                        "device output worker write failed"
                    );
                }

                last_sent_sequence = frame.sequence;

                if let Some(interval) = send_interval {
                    next_send_at = advance_deadline(next_send_at, interval, Instant::now());
                }
            }
        });

        Self {
            tx,
            _io_task: io_task,
            target_fps,
            metrics,
            next_sequence: 0,
        }
    }

    /// Push the latest payload for this device.
    fn push(&mut self, colors: Vec<[u8; 3]>) {
        if let Ok(mut snapshot) = self.metrics.lock() {
            snapshot.frames_received = snapshot.frames_received.saturating_add(1);
        }

        self.next_sequence = self.next_sequence.saturating_add(1);

        self.tx.send_replace(Some(Arc::new(FramePayload {
            colors,
            sequence: self.next_sequence,
            produced_at: Instant::now(),
        })));
    }

    fn snapshot(
        &self,
        backend_id: &str,
        device_id: DeviceId,
        mapped_layout_ids: Vec<String>,
    ) -> OutputQueueDebugSnapshot {
        let metrics = self
            .metrics
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let avg_latency_ms = if metrics.frames_sent == 0 {
            0
        } else {
            let divisor = u32::try_from(metrics.frames_sent).unwrap_or(u32::MAX);
            let average = metrics
                .total_latency
                .checked_div(divisor)
                .unwrap_or_default()
                .as_millis();
            u64::try_from(average).unwrap_or(u64::MAX)
        };
        let now = Instant::now();
        let last_sent_ago_ms = metrics.last_sent_at.map(|last| {
            let ms = now.saturating_duration_since(last).as_millis();
            u64::try_from(ms).unwrap_or(u64::MAX)
        });

        OutputQueueDebugSnapshot {
            backend_id: backend_id.to_owned(),
            device_id: device_id.to_string(),
            mapped_layout_ids,
            target_fps: self.target_fps,
            frames_received: metrics.frames_received,
            frames_sent: metrics.frames_sent,
            frames_dropped: metrics.frames_dropped,
            avg_latency_ms,
            last_error: metrics.last_error,
            last_sent_ago_ms,
            last_sequence: self.next_sequence,
        }
    }
}

// ── BackendManager ──────────────────────────────────────────────────────────

/// Routes per-zone color data to the correct device backends.
///
/// On each frame, [`write_frame`](Self::write_frame) groups zone colors
/// by target device (using the spatial layout mapping) and dispatches
/// one payload per device to a non-blocking output queue.
#[derive(Default)]
pub struct BackendManager {
    /// Registered backends, keyed by `BackendInfo.id` (e.g., `"wled"`, `"usb"`).
    backends: HashMap<String, BackendHandle>,

    /// Maps spatial layout `DeviceZone.device_id` strings to `(backend_id, DeviceId)`.
    ///
    /// Populated during device discovery/connection. Entries are added via
    /// [`map_device`](Self::map_device) when a zone's device reference is
    /// resolved to an actual connected device.
    device_map: HashMap<String, DeviceMapping>,

    /// Per-target latest-frame output queues.
    output_queues: HashMap<BackendDeviceKey, OutputQueue>,

    /// Preferred output FPS for connected devices, captured at connect time.
    device_fps_cache: HashMap<BackendDeviceKey, u32>,

    /// User-configured per-device output brightness scalar.
    device_brightness: HashMap<DeviceId, f32>,

    /// Reference-counted direct-control locks that suppress queued frame writes.
    direct_control_locks: HashMap<BackendDeviceKey, usize>,

    /// Layout device IDs already warned as unmapped in the current layout state.
    warned_unmapped_layout_devices: HashSet<String>,

    /// Last warning time for zone-to-segment color length mismatches.
    last_segment_mismatch_warn_at: HashMap<String, Instant>,

    /// Connected devices already reported as unused by the active layout.
    warned_inactive_layout_devices: HashSet<BackendDeviceKey>,
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
        let Some(mapping) = self.device_map.get_mut(layout_device_id) else {
            return false;
        };

        mapping.zone_segments = zone_segments_from_device_info(device_info);
        mapping.physical_led_count = device_output_len(device_info);
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
        self.device_fps_cache
            .insert((backend_id.to_owned(), device_id), target_fps);

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
        io.disconnect(device_id).await?;

        let _ = self.remove_device_mappings_for_physical(backend_id, device_id);
        Ok(())
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
        if normalized >= 0.999 {
            self.device_brightness.remove(&device_id);
            return;
        }
        self.device_brightness.insert(device_id, normalized);
    }

    /// Read the configured software output brightness for a physical device.
    #[must_use]
    pub fn device_output_brightness(&self, device_id: DeviceId) -> f32 {
        self.device_brightness
            .get(&device_id)
            .copied()
            .unwrap_or(1.0)
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
        self.direct_control_locks
            .get(&(backend_id.to_owned(), device_id))
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
            self.output_queues.remove(&key);
            self.device_fps_cache.remove(&key);
            self.direct_control_locks.remove(&key);
            self.warned_inactive_layout_devices.remove(&key);
        }

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
            self.output_queues.remove(&key);
            self.device_fps_cache.remove(&key);
            self.direct_control_locks.remove(&key);
            self.warned_inactive_layout_devices.remove(&key);
        }

        removed
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
        let active_layout_device_ids = layout
            .zones
            .iter()
            .map(|zone| zone.device_id.as_str())
            .collect::<HashSet<_>>();
        self.warned_unmapped_layout_devices
            .retain(|layout_device_id| {
                active_layout_device_ids.contains(layout_device_id.as_str())
            });

        let mut stats = FrameWriteStats::default();

        // Build zone_id → routing metadata lookup from the spatial layout.
        let zone_routes: HashMap<&str, ZoneRoute<'_>> = layout
            .zones
            .iter()
            .map(|zone| {
                (
                    zone.id.as_str(),
                    (
                        zone.device_id.as_str(),
                        zone.zone_name.as_deref(),
                        zone.led_mapping.as_deref(),
                        zone.attachment.as_ref(),
                    ),
                )
            })
            .collect();

        let inactive_devices = self.connected_devices_without_layout_targets(layout);
        let inactive_keys = inactive_devices.iter().cloned().collect::<HashSet<_>>();
        let mut newly_inactive = inactive_devices
            .into_iter()
            .filter(|key| !self.warned_inactive_layout_devices.contains(key))
            .collect::<Vec<_>>();
        newly_inactive.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });

        if !newly_inactive.is_empty() {
            let devices = newly_inactive
                .iter()
                .map(|(backend_id, device_id)| format!("{backend_id}:{device_id}"))
                .collect::<Vec<_>>();
            let mapped_layout_ids_by_device = newly_inactive
                .iter()
                .map(|(backend_id, device_id)| {
                    let mut aliases = self
                        .device_map
                        .iter()
                        .filter(|(_, mapping)| {
                            mapping.backend_id == *backend_id && mapping.device_id == *device_id
                        })
                        .map(|(layout_device_id, _)| layout_device_id.clone())
                        .collect::<Vec<_>>();
                    aliases.sort_unstable();
                    format!("{backend_id}:{device_id} => [{}]", aliases.join(", "))
                })
                .collect::<Vec<_>>();
            warn!(
                inactive_device_count = devices.len(),
                devices = ?devices,
                layout_zone_count = layout.zones.len(),
                mapped_layout_ids = ?mapped_layout_ids_by_device,
                "connected devices have no active layout zones; frames will not be sent"
            );
        }
        self.warned_inactive_layout_devices = inactive_keys;

        #[allow(clippy::items_after_statements)]
        #[derive(Default)]
        struct AccumulatedColors {
            values: Vec<[u8; 3]>,
            has_segmented_write: bool,
            required_len: usize,
        }

        // Group colors by (backend_id, device_id). Owned keys to avoid
        // borrow conflicts with `self.backends` during the write phase.
        let mut device_colors: HashMap<(String, DeviceId), AccumulatedColors> = HashMap::new();

        for zc in zone_colors {
            let Some((layout_device_id, zone_name, led_mapping, attachment)) =
                zone_routes.get(zc.zone_id.as_str()).copied()
            else {
                warn!(zone_id = %zc.zone_id, "zone not found in spatial layout");
                continue;
            };
            let remapped_colors = remap_zone_colors(&zc.zone_id, &zc.colors, led_mapping);

            let Some(mapping) = self.device_map.get(layout_device_id) else {
                if self
                    .warned_unmapped_layout_devices
                    .insert(layout_device_id.to_owned())
                {
                    warn!(
                        zone_id = %zc.zone_id,
                        layout_device_id = %layout_device_id,
                        "zone skipped because the target layout device is not mapped to a connected backend device"
                    );
                }
                continue;
            };

            self.warned_unmapped_layout_devices.remove(layout_device_id);

            let key = (mapping.backend_id.clone(), mapping.device_id);
            let entry = device_colors.entry(key).or_default();
            entry.required_len = entry
                .required_len
                .max(mapping.physical_led_count.unwrap_or_default());

            let segment = attachment_segment_for_zone(
                &zc.zone_id,
                mapped_segment_for_zone_name(&zc.zone_id, zone_name, mapping),
                attachment,
                remapped_colors.len(),
            );

            if let Some(segment) = segment {
                entry.has_segmented_write = true;
                let required_len = segment.end();
                if entry.values.len() < required_len {
                    entry.values.resize(required_len, [0, 0, 0]);
                }

                let copy_len = segment.length.min(remapped_colors.len());
                if copy_len > 0 {
                    let start = segment.start;
                    let end = start.saturating_add(copy_len);
                    entry.values[start..end].copy_from_slice(&remapped_colors[..copy_len]);
                }

                if copy_len != segment.length {
                    let warn_key = format!("{layout_device_id}:{}", zc.zone_id);
                    let should_warn = self
                        .last_segment_mismatch_warn_at
                        .get(&warn_key)
                        .is_none_or(|last_warn_at| {
                            last_warn_at.elapsed() >= UNMAPPED_LAYOUT_WARN_INTERVAL
                        });

                    if should_warn {
                        warn!(
                            zone_id = %zc.zone_id,
                            layout_device_id = %layout_device_id,
                            segment_start = segment.start,
                            expected = segment.length,
                            received = remapped_colors.len(),
                            "zone color count does not match mapped segment length"
                        );
                        self.last_segment_mismatch_warn_at
                            .insert(warn_key, Instant::now());
                    }
                }
            } else {
                if entry.has_segmented_write {
                    warn!(
                        zone_id = %zc.zone_id,
                        "mixed segmented and non-segmented mappings for the same physical device"
                    );
                }
                entry.values.extend_from_slice(&remapped_colors);
            }
        }

        // Dispatch to output queues.
        for ((backend_id, device_id), colors) in device_colors {
            let AccumulatedColors {
                mut values,
                has_segmented_write: _,
                required_len,
            } = colors;
            if values.len() < required_len {
                values.resize(required_len, [0, 0, 0]);
            }

            if self.is_direct_control_active(backend_id.as_str(), device_id) {
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

            let device_output_brightness = self.device_output_brightness(device_id);
            let per_frame_brightness = device_brightness
                .and_then(|settings| settings.get(&device_id).copied())
                .unwrap_or(1.0);
            let brightness = (global_brightness * per_frame_brightness * device_output_brightness)
                .clamp(0.0, 1.0);
            let Some(queue) = self.ensure_output_queue(backend_id.as_str(), device_id) else {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            };

            // Convert screen-referred sRGB into LED PWM in one pass so we keep
            // more low-end detail and can compensate dark, saturated colors
            // before the final 8-bit quantization step.
            prepare_output_for_leds(&mut values, brightness);

            stats.devices_written += 1;
            stats.total_leds += values.len();
            queue.push(values);
        }

        stats
    }

    fn ensure_output_queue(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Option<&mut OutputQueue> {
        let key = (backend_id.to_owned(), device_id);

        if !self.output_queues.contains_key(&key) {
            let backend = self.backends.get(backend_id)?.clone();
            let target_fps = self.device_fps_cache.get(&key).copied().unwrap_or(60);
            let queue = OutputQueue::spawn(backend_id.to_owned(), device_id, backend, target_fps);
            self.output_queues.insert(key.clone(), queue);
        }

        self.output_queues.get_mut(&key)
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
                let error = queue
                    .metrics
                    .lock()
                    .ok()
                    .and_then(|metrics| metrics.last_error.clone())?;

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

    /// Build a debug snapshot of queue and routing internals.
    #[must_use]
    pub fn debug_snapshot(&self) -> BackendManagerDebugSnapshot {
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
            queues.push(queue.snapshot(backend_id, *device_id, mapped_layout_ids));
        }

        queues.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.cmp(&right.device_id))
        });

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

fn prepare_output_for_leds(colors: &mut [[u8; 3]], brightness: f32) {
    let brightness = brightness.clamp(0.0, 1.0);
    if brightness <= 0.0 {
        colors.fill([0, 0, 0]);
        return;
    }

    for color in colors {
        let linear = [
            decode_srgb_channel(color[0]),
            decode_srgb_channel(color[1]),
            decode_srgb_channel(color[2]),
        ];
        let compensated = apply_led_perceptual_compensation(linear);
        *color = encode_led_pwm(scale_linear_rgb(compensated, brightness));
    }
}

fn scale_linear_rgb(mut color: [f32; 3], brightness: f32) -> [f32; 3] {
    color[0] *= brightness;
    color[1] *= brightness;
    color[2] *= brightness;
    color
}

fn apply_led_perceptual_compensation(mut color: [f32; 3]) -> [f32; 3] {
    let max_channel = color[0].max(color[1]).max(color[2]);
    if max_channel <= f32::EPSILON {
        return color;
    }

    let min_channel = color[0].min(color[1]).min(color[2]);
    let luma = color[0].mul_add(0.2126, color[1].mul_add(0.7152, color[2] * 0.0722));
    let headroom = 1.0 - max_channel;
    if headroom <= f32::EPSILON {
        return color;
    }

    // Point-light LEDs under-represent low-luma chromatic colors, especially
    // blue/cyan/magenta. Lift those gently while keeping neutrals closer to
    // the source and never exceeding the available channel headroom.
    let whiteness = min_channel / max_channel;
    let colorfulness = LED_NEUTRAL_COMPENSATION_WEIGHT
        + (1.0 - LED_NEUTRAL_COMPENSATION_WEIGHT) * (1.0 - whiteness);
    let shadow_bias = 1.0 - luma;
    let headroom_weight = LED_HEADROOM_WEIGHT_FLOOR + (1.0 - LED_HEADROOM_WEIGHT_FLOOR) * headroom;
    let gain = 1.0
        + LED_PERCEPTUAL_COMPENSATION_STRENGTH
            * shadow_bias
            * shadow_bias
            * headroom_weight
            * colorfulness;
    let gain = gain.min(1.0 / max_channel);

    if gain <= 1.0 {
        return color;
    }

    color[0] = (color[0] * gain).min(1.0);
    color[1] = (color[1] * gain).min(1.0);
    color[2] = (color[2] * gain).min(1.0);
    color
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "the LUT generator clamps the computed transfer value into the valid 16-bit range"
)]
fn decode_srgb_channel(channel: u8) -> f32 {
    static SRGB_TO_LED_LINEAR_LUT: OnceLock<[u16; 256]> = OnceLock::new();

    let linear = SRGB_TO_LED_LINEAR_LUT.get_or_init(|| {
        std::array::from_fn(|index| {
            let srgb = f32::from(u8::try_from(index).expect("LUT index must fit in u8")) / 255.0;
            (srgb_to_linear(srgb) * 65535.0).round().clamp(0.0, 65535.0) as u16
        })
    })[usize::from(channel)];

    f32::from(linear) / 65535.0
}

fn encode_led_pwm(color: [f32; 3]) -> [u8; 3] {
    [
        linear_to_output_u8(color[0]),
        linear_to_output_u8(color[1]),
        linear_to_output_u8(color[2]),
    ]
}

fn target_interval_for_fps(target_fps: u32) -> Option<Duration> {
    if target_fps == 0 {
        return None;
    }

    Some(Duration::from_secs_f64(1.0 / f64::from(target_fps)))
}

fn advance_deadline(previous_deadline: Instant, interval: Duration, now: Instant) -> Instant {
    previous_deadline
        .checked_add(interval)
        .unwrap_or(now)
        .max(now)
}

fn remap_zone_colors<'a>(
    zone_id: &str,
    colors: &'a [[u8; 3]],
    led_mapping: Option<&[u32]>,
) -> Cow<'a, [[u8; 3]]> {
    let Some(led_mapping) = led_mapping else {
        return Cow::Borrowed(colors);
    };

    if led_mapping.len() != colors.len() {
        warn!(
            zone_id = %zone_id,
            mapping_len = led_mapping.len(),
            color_len = colors.len(),
            "ignoring zone LED mapping because it does not match the sampled LED count"
        );
        return Cow::Borrowed(colors);
    }

    let mut reordered = vec![[0, 0, 0]; colors.len()];
    for (spatial_index, &physical_index) in led_mapping.iter().enumerate() {
        let Ok(physical_index) = usize::try_from(physical_index) else {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                "ignoring zone LED mapping because one physical index does not fit in usize"
            );
            return Cow::Borrowed(colors);
        };
        if physical_index >= reordered.len() {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                color_len = colors.len(),
                "ignoring zone LED mapping because one physical index is out of bounds"
            );
            return Cow::Borrowed(colors);
        }
        reordered[physical_index] = colors[spatial_index];
    }

    Cow::Owned(reordered)
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
    let Some(zone_segment) = mapping.zone_segments.get(zone_name).copied() else {
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

fn attachment_segment_for_zone(
    zone_id: &str,
    base_segment: Option<SegmentRange>,
    attachment: Option<&ZoneAttachment>,
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

    if let Some(base_segment) = base_segment {
        if led_start.saturating_add(resolved_led_count) > base_segment.length {
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

        return Some(SegmentRange::new(
            base_segment.start.saturating_add(led_start),
            resolved_led_count,
        ));
    }

    Some(SegmentRange::new(led_start, resolved_led_count))
}
