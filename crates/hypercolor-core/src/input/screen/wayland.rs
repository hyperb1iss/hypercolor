//! Wayland screen capture source powered by XDG Desktop Portal + PipeWire.
//!
//! This source keeps the portal session and PipeWire stream on a dedicated
//! worker thread. The render loop only clones the latest processed
//! [`ScreenData`] snapshot, while capture demand is toggled at runtime by the
//! daemon depending on the active effect.

use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use ashpd::desktop::{
    CreateSessionOptions, PersistMode, Session,
    screencast::{
        CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
        StartCastOptions, Stream,
    },
};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use tracing::{debug, info, warn};

use crate::input::screen::{
    AnalyzedScreenSnapshot, CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage,
    CaptureEpoch, CaptureFrame, CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat,
    CapturePlanePool, CaptureRotation, CaptureSourceId, CaptureStorage, CaptureTransferFunction,
    CpuCaptureStorage, PhysicalOrigin, PixelExtent, PixelRect, PooledCapturePlane,
    RawCaptureSurface, ScreenCaptureDemand, ScreenCaptureInput, SourceScale, analyze_screen_frame,
};
use crate::input::traits::{InputData, InputSource};
use crate::input::worker_retention::{retain_input_worker, spawn_input_worker};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceSessionWriter, SourceStatusHandle,
    SourceStatusReporter,
};
use crate::types::canvas::SurfaceResourceError;

const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
const RECONNECT_DELAY: Duration = Duration::from_millis(250);

/// Packed raw pixel formats accepted by the PipeWire callback seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaVideoFormat {
    /// Red, green, blue, alpha.
    Rgba,
    /// Blue, green, red, alpha.
    Bgra,
    /// Red, green, blue, ignored.
    Rgbx,
    /// Blue, green, red, ignored.
    Bgrx,
    /// Alpha, red, green, blue.
    Argb,
    /// Alpha, blue, green, red.
    Abgr,
    /// Ignored, red, green, blue.
    Xrgb,
    /// Ignored, blue, green, red.
    Xbgr,
    /// Red, green, blue.
    Rgb,
    /// Blue, green, red.
    Bgr,
}

impl SpaVideoFormat {
    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb | Self::Bgr => 3,
            Self::Rgba
            | Self::Bgra
            | Self::Rgbx
            | Self::Bgrx
            | Self::Argb
            | Self::Abgr
            | Self::Xrgb
            | Self::Xbgr => 4,
        }
    }
}

/// Borrowed, negotiated SPA chunk presented to the synchronous copy seam.
#[derive(Clone, Copy, Debug)]
pub struct SpaChunkView<'a> {
    data: &'a [u8],
    offset: usize,
    size: usize,
    stride: i32,
    width: u32,
    height: u32,
    format: SpaVideoFormat,
    crop: Option<PixelRect>,
    transform: CaptureRotation,
}

impl<'a> SpaChunkView<'a> {
    /// Construct one negotiated chunk view. [`decode_chunk`] validates every bound.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        data: &'a [u8],
        offset: usize,
        size: usize,
        stride: i32,
        width: u32,
        height: u32,
        format: SpaVideoFormat,
        crop: Option<PixelRect>,
        transform: CaptureRotation,
    ) -> Self {
        Self {
            data,
            offset,
            size,
            stride,
            width,
            height,
            format,
            crop,
            transform,
        }
    }
}

/// Precise reason a callback chunk was rejected without touching published data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkDropReason {
    /// PipeWire had no buffer ready when the process callback fired.
    MissingBuffer,
    /// The dequeued PipeWire buffer did not contain a pixel plane.
    MissingPlane,
    /// A pixel buffer arrived before a supported format was negotiated.
    MissingFormat,
    /// The first PipeWire pixel plane was not mapped into this process.
    UnmappedPlane,
    /// The negotiated dimensions are empty or overflow addressable storage.
    InvalidExtent,
    /// The SPA chunk offset or size escapes the mapped plane.
    InvalidChunkBounds,
    /// The signed stride cannot contain one negotiated row.
    InvalidStride,
    /// The chunk ends before the final row is complete.
    TruncatedChunk,
    /// The SPA crop escapes the negotiated native extent.
    InvalidCrop,
    /// Both preallocated buffers are still owned by analysis or publication.
    BufferUnavailable,
    /// The negotiated frame exceeds the capacity prepared outside the callback.
    BufferTooSmall,
}

/// Allocation-free result counters returned by [`decode_chunk`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CopyStats {
    bytes_copied: usize,
    rows_copied: u32,
    drop_reason: Option<ChunkDropReason>,
}

impl CopyStats {
    /// Number of source bytes copied into tightly packed storage.
    #[must_use]
    pub const fn bytes_copied(self) -> usize {
        self.bytes_copied
    }

    /// Number of complete rows copied.
    #[must_use]
    pub const fn rows_copied(self) -> u32 {
        self.rows_copied
    }

    /// Rejection reason, or `None` when the copy completed.
    #[must_use]
    pub const fn drop_reason(self) -> Option<ChunkDropReason> {
        self.drop_reason
    }

    const fn dropped(reason: ChunkDropReason) -> Self {
        Self {
            bytes_copied: 0,
            rows_copied: 0,
            drop_reason: Some(reason),
        }
    }
}

struct DoubleBufferInner {
    available: Mutex<Vec<Vec<u8>>>,
    capacity: usize,
}

/// Fixed two-plane pool used by the PipeWire process callback.
pub struct DoubleBuffer {
    inner: Arc<DoubleBufferInner>,
    completed: Option<DecodedChunk>,
}

impl DoubleBuffer {
    /// Preallocate both callback planes to the negotiated maximum byte size.
    ///
    /// # Errors
    ///
    /// Returns a typed allocation failure when either callback plane cannot
    /// reserve `capacity` bytes.
    pub fn try_with_capacity(capacity: usize) -> Result<Self, CaptureFrameError> {
        let mut available = Vec::new();
        available
            .try_reserve_exact(2)
            .map_err(|_| CaptureFrameError::PlaneAllocationFailed { byte_len: capacity })?;
        for _ in 0..2 {
            let mut plane = Vec::new();
            plane
                .try_reserve_exact(capacity)
                .map_err(|_| CaptureFrameError::PlaneAllocationFailed { byte_len: capacity })?;
            plane.resize(capacity, 0);
            available.push(plane);
        }
        Ok(Self {
            inner: Arc::new(DoubleBufferInner {
                available: Mutex::new(available),
                capacity,
            }),
            completed: None,
        })
    }

    /// Bytes copied by the most recent successful decode.
    #[must_use]
    pub fn latest_bytes(&self) -> Option<&[u8]> {
        self.completed.as_ref().map(DecodedChunk::bytes)
    }

    /// Negotiated extent attached to the most recent successful decode.
    #[must_use]
    pub fn latest_extent(&self) -> Option<(u32, u32)> {
        self.completed
            .as_ref()
            .map(|chunk| (chunk.width, chunk.height))
    }

    /// Crop metadata retained without applying it in the callback.
    #[must_use]
    pub fn latest_crop(&self) -> Option<PixelRect> {
        self.completed.as_ref().and_then(|chunk| chunk.crop)
    }

    /// Transform metadata retained without applying it in the callback.
    #[must_use]
    pub fn latest_transform(&self) -> Option<CaptureRotation> {
        self.completed.as_ref().map(|chunk| chunk.transform)
    }

    /// Negotiated pixel encoding attached to the most recent successful decode.
    #[must_use]
    pub fn latest_format(&self) -> Option<SpaVideoFormat> {
        self.completed.as_ref().map(|chunk| chunk.format)
    }

    fn acquire(&self) -> Option<Vec<u8>> {
        self.inner
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
    }

    fn take_completed(&mut self) -> Option<DecodedChunk> {
        self.completed.take()
    }
}

#[derive(Debug)]
struct DoubleBufferedPlane {
    buffer: Option<Vec<u8>>,
    pool: Weak<DoubleBufferInner>,
}

impl AsRef<[u8]> for DoubleBufferedPlane {
    fn as_ref(&self) -> &[u8] {
        self.buffer
            .as_deref()
            .expect("double-buffered plane owns its storage")
    }
}

impl Drop for DoubleBufferedPlane {
    fn drop(&mut self) {
        let (Some(buffer), Some(pool)) = (self.buffer.take(), self.pool.upgrade()) else {
            return;
        };
        pool.available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(buffer);
    }
}

#[derive(Debug)]
struct DecodedChunk {
    plane: DoubleBufferedPlane,
    byte_len: usize,
    width: u32,
    height: u32,
    format: SpaVideoFormat,
    crop: Option<PixelRect>,
    transform: CaptureRotation,
    captured_at: Instant,
}

impl DecodedChunk {
    fn bytes(&self) -> &[u8] {
        &self.plane.as_ref()[..self.byte_len]
    }
}

/// Validate and copy one SPA chunk without allocating or performing analysis.
pub fn decode_chunk(view: &SpaChunkView<'_>, buffers: &mut DoubleBuffer) -> CopyStats {
    let Some(width) = usize::try_from(view.width).ok().filter(|width| *width > 0) else {
        return CopyStats::dropped(ChunkDropReason::InvalidExtent);
    };
    let Some(height) = usize::try_from(view.height)
        .ok()
        .filter(|height| *height > 0)
    else {
        return CopyStats::dropped(ChunkDropReason::InvalidExtent);
    };
    let Some(row_bytes) = width.checked_mul(view.format.bytes_per_pixel()) else {
        return CopyStats::dropped(ChunkDropReason::InvalidExtent);
    };
    let Some(total_bytes) = row_bytes.checked_mul(height) else {
        return CopyStats::dropped(ChunkDropReason::InvalidExtent);
    };
    let Some(chunk_end) = view.offset.checked_add(view.size) else {
        return CopyStats::dropped(ChunkDropReason::InvalidChunkBounds);
    };
    if view.size == 0 || chunk_end > view.data.len() {
        return CopyStats::dropped(ChunkDropReason::InvalidChunkBounds);
    }

    let stride = if view.stride == 0 {
        row_bytes
    } else {
        let Some(stride) = view
            .stride
            .checked_abs()
            .and_then(|value| usize::try_from(value).ok())
        else {
            return CopyStats::dropped(ChunkDropReason::InvalidStride);
        };
        stride
    };
    if stride < row_bytes {
        return CopyStats::dropped(ChunkDropReason::InvalidStride);
    }
    let Some(required) = stride
        .checked_mul(height.saturating_sub(1))
        .and_then(|span| span.checked_add(row_bytes))
    else {
        return CopyStats::dropped(ChunkDropReason::InvalidExtent);
    };
    if required > view.size {
        return CopyStats::dropped(ChunkDropReason::TruncatedChunk);
    }
    if let Some(crop) = view.crop {
        let right = crop.x().checked_add(crop.extent().width());
        let bottom = crop.y().checked_add(crop.extent().height());
        if right.is_none_or(|right| right > view.width)
            || bottom.is_none_or(|bottom| bottom > view.height)
        {
            return CopyStats::dropped(ChunkDropReason::InvalidCrop);
        }
    }
    if total_bytes > buffers.inner.capacity {
        return CopyStats::dropped(ChunkDropReason::BufferTooSmall);
    }
    let Some(mut output) = buffers.acquire() else {
        return CopyStats::dropped(ChunkDropReason::BufferUnavailable);
    };
    let chunk = &view.data[view.offset..chunk_end];
    for row in 0..height {
        let source_row = if view.stride < 0 {
            height - 1 - row
        } else {
            row
        };
        let source_start = source_row * stride;
        let destination_start = row * row_bytes;
        output[destination_start..destination_start + row_bytes]
            .copy_from_slice(&chunk[source_start..source_start + row_bytes]);
    }
    buffers.completed = Some(DecodedChunk {
        plane: DoubleBufferedPlane {
            buffer: Some(output),
            pool: Arc::downgrade(&buffers.inner),
        },
        byte_len: total_bytes,
        width: view.width,
        height: view.height,
        format: view.format,
        crop: view.crop,
        transform: view.transform,
        captured_at: Instant::now(),
    });
    CopyStats {
        bytes_copied: total_bytes,
        rows_copied: view.height,
        drop_reason: None,
    }
}

/// Callback invoked when the portal hands back a new restore token (or the
/// token is cleared before a re-pick). The daemon persists it to config so
/// the picked source survives restarts without re-prompting.
pub type RestoreTokenSink = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// Settings shared between the input source handle and the capture worker.
///
/// The config lives behind a mutex while the generation counter is atomic:
/// the worker polls the counter once per frame and only takes the lock when
/// a reconfiguration actually happened.
struct SharedSettings {
    config: Mutex<CaptureConfig>,
    demand: Mutex<ScreenCaptureDemand>,
    generation: AtomicU64,
    frame_generation: AtomicU64,
    topology_generation: AtomicU64,
    topology: Mutex<Option<WaylandTopologyState>>,
    session_generation: AtomicU64,
    expected_epoch: Mutex<Option<CaptureEpoch>>,
}

struct CaptureRuntimeSettings {
    config: CaptureConfig,
    demand: ScreenCaptureDemand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WaylandTopologySignature {
    source_id: CaptureSourceId,
    origin: PhysicalOrigin,
    logical_extent: Option<PixelExtent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedWaylandTopology {
    generation: u64,
    native_extent: PixelExtent,
}

#[derive(Debug)]
struct WaylandTopologyState {
    signature: WaylandTopologySignature,
    resolved: ResolvedWaylandTopology,
}

#[derive(Clone)]
struct CapturedScreenSnapshot {
    analysis: AnalyzedScreenSnapshot,
    generation: u64,
}

impl SharedSettings {
    fn snapshot_for_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
    ) -> Option<CaptureRuntimeSettings> {
        let _session_guard = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire)
            || self.session_generation.load(Ordering::Acquire) != session_generation
        {
            return None;
        }
        let config = self
            .config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_default();
        let demand = *self
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(CaptureRuntimeSettings { config, demand })
    }

    fn expected_epoch(&self) -> Option<CaptureEpoch> {
        self.expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn begin_session(&self) -> u64 {
        let mut expected = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_generation = self
            .session_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        *expected = None;
        session_generation
    }

    fn begin_successor_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
        active_session_generation: &AtomicU64,
    ) -> Option<u64> {
        let mut expected = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let successor_generation = session_generation.wrapping_add(1);
        self.session_generation
            .compare_exchange(
                session_generation,
                successor_generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        *expected = None;
        active_session_generation.store(successor_generation, Ordering::Release);
        Some(successor_generation)
    }

    fn cancel_worker_session(
        &self,
        latest_snapshot: &Mutex<Option<CapturedScreenSnapshot>>,
        cancel: &AtomicBool,
        active_session_generation: &AtomicU64,
    ) {
        let mut expected = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cancel.store(true, Ordering::SeqCst);
        let session_generation = active_session_generation.load(Ordering::Acquire);
        if self.session_generation.load(Ordering::Acquire) != session_generation {
            return;
        }
        if expected
            .as_ref()
            .is_some_and(|epoch| epoch.session_generation == session_generation)
        {
            *expected = None;
        }
        let mut latest = latest_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if latest.as_ref().is_some_and(|snapshot| {
            snapshot
                .analysis
                .geometry_frame()
                .metadata()
                .session_generation
                == session_generation
        }) {
            *latest = None;
        }
    }

    fn persist_restore_token_for_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
        restore_token: Option<String>,
        token_sink: Option<&RestoreTokenSink>,
    ) -> bool {
        let _session_guard = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire)
            || self.session_generation.load(Ordering::Acquire) != session_generation
        {
            return false;
        }
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .restore_token
            .clone_from(&restore_token);
        if let Some(sink) = token_sink {
            sink(restore_token);
        }
        true
    }

    fn session_is_current(&self, session_generation: u64, cancel: &AtomicBool) -> bool {
        !cancel.load(Ordering::Acquire)
            && self.session_generation.load(Ordering::Acquire) == session_generation
    }

    fn publish_status_for_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
        status: &SourceSessionWriter,
        publish: impl FnOnce(&SourceSessionWriter) -> bool,
    ) -> bool {
        let _session_guard = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire)
            || self.session_generation.load(Ordering::Acquire) != session_generation
        {
            return false;
        }
        publish(status)
    }

    fn activate_topology(
        &self,
        signature: &WaylandTopologySignature,
        native_extent: PixelExtent,
        session_generation: u64,
    ) -> Option<ResolvedWaylandTopology> {
        let mut expected = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.session_generation.load(Ordering::Acquire) != session_generation {
            return None;
        }

        let mut topology = self
            .topology
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let resolved = match topology.as_ref() {
            Some(state) if state.signature == *signature => state.resolved,
            _ => {
                let resolved = ResolvedWaylandTopology {
                    generation: self
                        .topology_generation
                        .fetch_add(1, Ordering::AcqRel)
                        .wrapping_add(1),
                    native_extent,
                };
                *topology = Some(WaylandTopologyState {
                    signature: signature.clone(),
                    resolved,
                });
                resolved
            }
        };
        *expected = Some(CaptureEpoch {
            source_id: signature.source_id.clone(),
            topology_generation: resolved.generation,
            session_generation,
        });
        Some(resolved)
    }

    fn publish_snapshot(
        &self,
        latest_snapshot: &Mutex<Option<CapturedScreenSnapshot>>,
        analysis: AnalyzedScreenSnapshot,
    ) -> bool {
        let expected = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(expected) = expected.as_ref() else {
            return false;
        };
        if analysis.geometry_frame().validate_epoch(expected).is_err() {
            return false;
        }
        let Ok(mut latest) = latest_snapshot.lock() else {
            return false;
        };
        let generation = self
            .frame_generation
            .fetch_add(1, Ordering::Release)
            .wrapping_add(1);
        *latest = Some(CapturedScreenSnapshot {
            analysis,
            generation,
        });
        true
    }

    fn clear_expected_epoch(&self) {
        *self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn invalidate_session(
        &self,
        latest_snapshot: &Mutex<Option<CapturedScreenSnapshot>>,
        session_generation: u64,
    ) -> bool {
        let mut expected = self
            .expected_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if expected
            .as_ref()
            .is_none_or(|epoch| epoch.session_generation != session_generation)
        {
            return false;
        }
        *expected = None;
        let mut latest = latest_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if latest.as_ref().is_some_and(|snapshot| {
            snapshot
                .analysis
                .geometry_frame()
                .metadata()
                .session_generation
                == session_generation
        }) {
            *latest = None;
        }
        true
    }
}

/// Wayland-only live screen capture input source.
pub struct WaylandScreenCaptureInput {
    settings: Arc<SharedSettings>,
    running: bool,
    capture_demand: ScreenCaptureDemand,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    status_snapshot_generation: u64,
    worker: Option<WaylandCaptureWorker>,
    retiring_workers: Vec<WaylandCaptureWorker>,
    token_sink: Option<RestoreTokenSink>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
}

impl WaylandScreenCaptureInput {
    /// Create a new Wayland screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            settings: Arc::new(SharedSettings {
                config: Mutex::new(config),
                demand: Mutex::new(ScreenCaptureDemand::Inactive),
                generation: AtomicU64::new(0),
                frame_generation: AtomicU64::new(0),
                topology_generation: AtomicU64::new(0),
                topology: Mutex::new(None),
                session_generation: AtomicU64::new(0),
                expected_epoch: Mutex::new(None),
            }),
            running: false,
            capture_demand: ScreenCaptureDemand::Inactive,
            latest_snapshot: Arc::new(Mutex::new(None)),
            status_snapshot_generation: 0,
            worker: None,
            retiring_workers: Vec::new(),
            token_sink: None,
            status: SourceStatusReporter::new(
                "wayland_screen_capture",
                SourceKind::Screen,
                "pipewire",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
        }
    }

    /// Attach a sink that persists portal restore tokens.
    #[must_use]
    pub fn with_restore_token_sink(mut self, sink: RestoreTokenSink) -> Self {
        self.token_sink = Some(sink);
        self
    }

    fn current_target_fps(&self) -> u32 {
        self.settings
            .config
            .lock()
            .map(|config| config.target_fps)
            .unwrap_or(30)
    }

    /// Apply new capture settings to the running pipeline.
    ///
    /// Analysis settings (grid, smoothing, letterbox, tuning) reach the
    /// worker without interruption. A target FPS change requires stream
    /// re-negotiation, so the worker restarts; with a restore token in
    /// place that restart is silent.
    fn reconfigure(&mut self, config: CaptureConfig) -> anyhow::Result<()> {
        let fps_changed = self.current_target_fps() != config.target_fps;

        if let Ok(mut current) = self.settings.config.lock() {
            // The worker may have written a freshly granted portal token
            // since the caller snapshotted its config; never let a stale
            // None overwrite it. Intentional clears go through
            // `reselect_source`.
            let granted_token = current.restore_token.take();
            *current = config;
            if current.restore_token.is_none() {
                current.restore_token = granted_token;
            }
        }
        self.settings.generation.fetch_add(1, Ordering::Release);

        if fps_changed && self.worker.is_some() {
            if self.portal_pending() {
                warn!(
                    "Portal source picker is open; new capture FPS applies on the next session restart"
                );
                return Ok(());
            }
            info!("Restarting Wayland capture worker for new target FPS");
            self.restart_worker()?;
        }

        Ok(())
    }

    /// Drop the persisted portal token and re-open the source picker.
    fn reselect_source(&mut self) -> anyhow::Result<()> {
        if self.portal_pending() {
            debug!("Portal source picker is already open; ignoring re-pick request");
            return Ok(());
        }

        if let Ok(mut current) = self.settings.config.lock() {
            current.restore_token = None;
        }
        if let Some(sink) = &self.token_sink {
            sink(None);
        }

        if !self.running || !self.capture_demand.is_active() {
            return Ok(());
        }

        info!("Re-opening Wayland screencast source picker");
        self.restart_worker()
    }

    fn portal_pending(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.portal_pending.load(Ordering::SeqCst))
    }

    fn restart_worker(&mut self) -> anyhow::Result<()> {
        self.shutdown_worker();
        if !self.running || !self.capture_demand.is_active() {
            return Ok(());
        }
        self.spawn_worker(self.capture_demand)?;
        self.send_worker_command(WorkerCommand::SetDemand(self.capture_demand))
    }

    fn set_capture_demand_state(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let previous = self.capture_demand;
        if previous == demand {
            if demand.is_active() && self.running && self.worker.is_none() {
                self.spawn_worker(demand)?;
            }
            return Ok(());
        }

        let _admission = demand
            .requested_extent()
            .map(|requested_extent| {
                let config = self
                    .settings
                    .config
                    .lock()
                    .map(|config| config.clone())
                    .unwrap_or_default();
                ScreenCaptureInput::with_requested_extent(config, requested_extent)
            })
            .transpose()?;

        if let Ok(mut current) = self.settings.demand.lock() {
            *current = demand;
        }
        self.settings.generation.fetch_add(1, Ordering::Release);

        if !self.running {
            if !demand.is_active() {
                self.settings.clear_expected_epoch();
            }
            if let Ok(mut latest) = self.latest_snapshot.lock() {
                *latest = None;
            }
            self.capture_demand = demand;
            return Ok(());
        }

        let result = if demand.is_active() {
            self.spawn_worker(demand)
                .and_then(|()| self.send_worker_command(WorkerCommand::SetDemand(demand)))
        } else if self.worker.is_some() {
            self.shutdown_worker();
            Ok(())
        } else {
            self.settings.clear_expected_epoch();
            Ok(())
        };

        if let Err(error) = result {
            if let Ok(mut current) = self.settings.demand.lock() {
                *current = previous;
            }
            self.settings.generation.fetch_add(1, Ordering::Release);
            let rollback = if previous.is_active() {
                self.spawn_worker(previous)
                    .and_then(|()| self.send_worker_command(WorkerCommand::SetDemand(previous)))
            } else {
                self.shutdown_worker();
                Ok(())
            };
            if let Err(rollback_error) = rollback {
                return Err(error.context(format!(
                    "failed to restore previous Wayland capture demand: {rollback_error}"
                )));
            }
            return Err(error);
        }

        if previous.is_active() != demand.is_active()
            && let Ok(mut latest) = self.latest_snapshot.lock()
        {
            *latest = None;
        }
        self.capture_demand = demand;
        Ok(())
    }

    fn spawn_worker(&mut self, initial_demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        self.reap_workers(false);
        if self.worker.is_some() {
            return Ok(());
        }

        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let settings = Arc::clone(&self.settings);
        let token_sink = self.token_sink.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let demanded = Arc::new(AtomicBool::new(initial_demand.is_active()));
        // Born true: the worker is portal-bound from its first instruction,
        // and a shutdown landing before the thread even stores the flag must
        // detach rather than join into the picker freeze.
        let portal_pending = Arc::new(AtomicBool::new(true));
        let worker_flags = WorkerFlags {
            cancel: Arc::clone(&cancel),
            portal_pending: Arc::clone(&portal_pending),
            demanded: Arc::clone(&demanded),
        };
        let (command_tx, command_rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let status_writer = self.status_session.load();
        let worker_status_writer = status_writer.clone();
        let session_generation = Arc::new(AtomicU64::new(settings.begin_session()));
        let capture_session_generation = Arc::clone(&session_generation);
        let worker_settings = Arc::clone(&settings);
        let worker_latest_snapshot = Arc::clone(&latest_snapshot);
        let join_handle = spawn_input_worker(
            thread::Builder::new().name("hypercolor-screen-capture".to_owned()),
            move || {
                let _ = ready_tx.send(());
                run_capture_worker(
                    settings,
                    latest_snapshot,
                    command_rx,
                    token_sink,
                    worker_flags,
                    status_writer,
                    capture_session_generation,
                );
                let _ = exit_tx.send(());
            },
        )
        .context("failed to spawn Wayland screen capture worker")?;

        self.worker = Some(WaylandCaptureWorker {
            command_tx,
            exit_rx,
            join_handle: Some(join_handle),
            cancel,
            portal_pending,
            demanded,
            status_writer: worker_status_writer,
            session_generation,
            settings: worker_settings,
            latest_snapshot: worker_latest_snapshot,
        });
        if let Err(error) = ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            self.shutdown_worker();
            anyhow::bail!("Wayland screen capture worker readiness timed out: {error}");
        }
        if self.observe_worker_exit(true) {
            anyhow::bail!("Wayland screen capture worker exited during startup");
        }
        Ok(())
    }

    fn send_worker_command(&mut self, command: WorkerCommand) -> anyhow::Result<()> {
        let Some(worker) = &self.worker else {
            return Ok(());
        };

        if dispatch_worker_command(&worker.command_tx, &worker.demanded, &command) {
            return Ok(());
        }

        warn!("Wayland screen capture worker is no longer accepting commands");
        self.shutdown_worker();

        if let WorkerCommand::SetDemand(demand) = command
            && demand.is_active()
        {
            self.spawn_worker(demand)?;
            let replacement_accepted = self.worker.as_ref().is_some_and(|worker| {
                dispatch_worker_command(&worker.command_tx, &worker.demanded, &command)
            });
            if !replacement_accepted {
                self.shutdown_worker();
                anyhow::bail!("failed to restart Wayland screen capture worker");
            }
        }

        Ok(())
    }

    fn shutdown_worker(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };

        worker.cancel_session();
        let _ = worker.command_tx.send(WorkerCommand::Stop);

        if !worker.portal_pending.load(Ordering::SeqCst) {
            let _ = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
        }
        if worker.is_finished() {
            let _ = worker.join(false);
        } else {
            debug!("Retaining Wayland capture worker until the portal request terminates");
            self.retiring_workers.push(worker);
        }
    }

    fn observe_worker_exit(&mut self, publish_failure: bool) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            self.reap_workers(false);
            return false;
        };
        if !worker.is_finished() {
            self.reap_workers(false);
            return false;
        }
        let worker = self.worker.take().expect("finished worker remains owned");
        let status_writer = worker.status_writer.clone();
        let session_generation = Arc::clone(&worker.session_generation);
        let cancel = Arc::clone(&worker.cancel);
        let failure_reason = worker.join(publish_failure);
        if let (Some(reason), Some(status)) = (failure_reason, status_writer.as_ref()) {
            publish_unexpected_exit_status(
                &self.settings,
                &session_generation,
                &cancel,
                status,
                reason,
            );
        }
        self.settings.invalidate_session(
            &self.latest_snapshot,
            session_generation.load(Ordering::Acquire),
        );
        self.reap_workers(false);
        true
    }

    fn reap_workers(&mut self, wait: bool) {
        let mut retained = Vec::with_capacity(self.retiring_workers.len());
        for mut worker in self.retiring_workers.drain(..) {
            if wait && !worker.portal_pending.load(Ordering::SeqCst) {
                let _ = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
            }
            if worker.is_finished() {
                let _ = worker.join(false);
            } else {
                retained.push(worker);
            }
        }
        self.retiring_workers = retained;
    }
}

impl InputSource for WaylandScreenCaptureInput {
    fn name(&self) -> &'static str {
        "wayland_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }

        if self.capture_demand.is_active() {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            if let Err(error) = self.spawn_worker(self.capture_demand).and_then(|()| {
                self.send_worker_command(WorkerCommand::SetDemand(self.capture_demand))
            }) {
                self.status_session.clear();
                self.status.stop();
                self.shutdown_worker();
                return Err(error);
            }
        } else {
            debug!(
                "Wayland screen capture armed but idle until a screen-reactive effect requests capture"
            );
        }

        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.running = false;
        self.capture_demand = ScreenCaptureDemand::Inactive;
        if let Ok(mut demand) = self.settings.demand.lock() {
            *demand = ScreenCaptureDemand::Inactive;
        }
        if self.worker.is_some() {
            self.shutdown_worker();
        } else {
            self.settings.clear_expected_epoch();
        }
        self.reap_workers(true);

        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let worker_exited =
            self.observe_worker_exit(self.running && self.capture_demand.is_active());
        if !self.running || !self.capture_demand.is_active() {
            return Ok(InputData::None);
        }
        if worker_exited {
            self.spawn_worker(self.capture_demand)?;
            self.send_worker_command(WorkerCommand::SetDemand(self.capture_demand))?;
            return Ok(InputData::None);
        }

        let latest = self
            .latest_snapshot
            .lock()
            .map_err(|_| anyhow!("wayland screen capture snapshot mutex poisoned"))?;

        let snapshot = latest.clone();
        drop(latest);
        let Some(snapshot) = snapshot else {
            return Ok(InputData::None);
        };
        let metadata = snapshot.analysis.geometry_frame().metadata();
        let Some(expected) = self.settings.expected_epoch() else {
            return Ok(InputData::None);
        };
        if snapshot
            .analysis
            .geometry_frame()
            .validate_epoch(&expected)
            .is_err()
        {
            return Ok(InputData::None);
        }
        if snapshot.generation != self.status_snapshot_generation {
            if let Some(status) = self.status.session() {
                let frame_period =
                    Duration::from_secs_f64(1.0 / f64::from(self.current_target_fps().max(1)));
                status.record_sample(
                    metadata.captured_at,
                    metadata.captured_at + frame_period + frame_period,
                    1,
                )?;
            }
            self.status_snapshot_generation = snapshot.generation;
        }
        Ok(InputData::Screen(snapshot.analysis.data().clone()))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.capture_demand
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let previous = self.capture_demand;
        let active = demand.is_active();
        self.status.set_policy(true, true, active)?;
        if previous.is_active() != active {
            if !active {
                self.status_session.clear();
            }
            if active && self.running {
                if let Some(session) = self.status.begin_session()? {
                    self.status_session.store(session);
                }
            }
        }
        if let Err(error) = self.set_capture_demand_state(demand) {
            self.status_session.clear();
            self.status.stop();
            self.status.set_policy(true, true, previous.is_active())?;
            if previous.is_active()
                && self.running
                && let Some(session) = self.status.begin_session()?
            {
                self.status_session.store(session);
            }
            return Err(error);
        }
        Ok(())
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        self.reconfigure(config.clone())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.reselect_source()
    }
}

struct WaylandCaptureWorker {
    command_tx: pw::channel::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: Option<thread::JoinHandle<()>>,
    /// Tells the worker to exit at its next checkpoint without touching
    /// shared state (snapshot, settings, restore token).
    cancel: Arc<AtomicBool>,
    /// True while the worker is awaiting the portal source picker — the
    /// phase during which it cannot see commands and must not be joined.
    portal_pending: Arc<AtomicBool>,
    demanded: Arc<AtomicBool>,
    status_writer: Option<SourceSessionWriter>,
    session_generation: Arc<AtomicU64>,
    settings: Arc<SharedSettings>,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
}

impl WaylandCaptureWorker {
    fn cancel_session(&self) {
        self.settings.cancel_worker_session(
            &self.latest_snapshot,
            &self.cancel,
            &self.session_generation,
        );
    }

    fn is_finished(&self) -> bool {
        self.join_handle
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    fn join(mut self, publish_failure: bool) -> Option<String> {
        let Some(join_handle) = self.join_handle.take() else {
            return None;
        };
        let failure = join_handle.join().err();
        if publish_failure {
            return Some(failure.map_or_else(
                || "Wayland screen capture worker exited unexpectedly".to_owned(),
                |panic| format!("Wayland screen capture worker panicked: {panic:?}"),
            ));
        } else if let Some(panic) = failure {
            warn!(message = ?panic, "Wayland screen capture worker panicked");
        }
        None
    }
}

impl Drop for WaylandCaptureWorker {
    fn drop(&mut self) {
        let Some(join_handle) = self.join_handle.take() else {
            return;
        };
        self.cancel_session();
        let _ = self.command_tx.send(WorkerCommand::Stop);
        if join_handle.is_finished() {
            let _ = join_handle.join();
            return;
        }
        retain_input_worker(join_handle, "Wayland capture worker");
    }
}

/// Cancellation and phase flags shared with a capture worker thread.
struct WorkerFlags {
    cancel: Arc<AtomicBool>,
    portal_pending: Arc<AtomicBool>,
    demanded: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
enum WorkerCommand {
    SetDemand(ScreenCaptureDemand),
    Stop,
}

fn dispatch_worker_command(
    command_tx: &pw::channel::Sender<WorkerCommand>,
    demanded: &AtomicBool,
    command: &WorkerCommand,
) -> bool {
    if let WorkerCommand::SetDemand(demand) = command {
        demanded.store(demand.is_active(), Ordering::Release);
    }
    command_tx.send(command.clone()).is_ok()
}

fn publish_unexpected_exit_status(
    settings: &SharedSettings,
    active_session_generation: &AtomicU64,
    cancel: &AtomicBool,
    status: &SourceSessionWriter,
    reason: String,
) -> bool {
    settings.publish_status_for_session(
        active_session_generation.load(Ordering::Acquire),
        cancel,
        status,
        |status| {
            status.degraded(SourceIssue::new(
                "wayland_screen_worker_exited",
                reason,
                true,
            ))
        },
    )
}

struct PortalCaptureSession {
    session: Session<Screencast>,
    stream: Stream,
    fd: OwnedFd,
}

#[derive(Clone)]
struct WaylandSourceMetadata {
    signature: WaylandTopologySignature,
    session_generation: u64,
    topology: Option<ResolvedWaylandTopology>,
}

impl WaylandSourceMetadata {
    fn from_stream(stream: &Stream, session_generation: u64) -> anyhow::Result<Self> {
        let source_name = stream
            .id()
            .or_else(|| stream.mapping_id())
            .unwrap_or("monitor");
        let source_id =
            CaptureSourceId::new(Arc::<str>::from(format!("wayland:portal:{source_name}")))?;
        let (x, y) = stream.position().unwrap_or_default();
        let logical_extent = stream.size().and_then(|(width, height)| {
            let width = u32::try_from(width).ok()?;
            let height = u32::try_from(height).ok()?;
            PixelExtent::new(width, height).ok()
        });
        Ok(Self {
            signature: WaylandTopologySignature {
                source_id,
                origin: PhysicalOrigin { x, y },
                logical_extent,
            },
            session_generation,
            topology: None,
        })
    }

    fn source_scale(&self, physical_width: u32) -> SourceScale {
        self.signature
            .logical_extent
            .and_then(|logical_extent| {
                SourceScale::new(logical_extent.width(), physical_width).ok()
            })
            .unwrap_or(SourceScale::ONE)
    }
}

struct WaylandCaptureUserData {
    format: spa::param::video::VideoInfoRaw,
    negotiated: Option<NegotiatedFormat>,
    buffers: DoubleBuffer,
    exchange: Arc<AnalysisExchange>,
    metrics: Arc<CaptureCallbackMetrics>,
}

impl WaylandCaptureUserData {
    fn new(exchange: Arc<AnalysisExchange>, metrics: Arc<CaptureCallbackMetrics>) -> Self {
        Self {
            format: spa::param::video::VideoInfoRaw::default(),
            negotiated: None,
            buffers: DoubleBuffer::try_with_capacity(0)
                .expect("empty callback planes require no pixel allocation"),
            exchange,
            metrics,
        }
    }

    fn set_negotiated_format(&mut self, format: NegotiatedFormat) -> Result<(), CaptureFrameError> {
        let Some(capacity) = format.byte_len() else {
            self.negotiated = None;
            return Err(CaptureFrameError::StorageSizeOverflow);
        };
        if self
            .negotiated
            .is_none_or(|previous| previous.byte_len() != Some(capacity))
        {
            let prepared = match DoubleBuffer::try_with_capacity(capacity) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.negotiated = None;
                    return Err(error);
                }
            };
            self.buffers = prepared;
        }
        self.negotiated = Some(format);
        Ok(())
    }

    fn record_drop(&self, reason: ChunkDropReason) {
        self.metrics.record(CopyStats::dropped(reason));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NegotiatedFormat {
    width: u32,
    height: u32,
    format: SpaVideoFormat,
}

impl NegotiatedFormat {
    fn byte_len(self) -> Option<usize> {
        usize::try_from(self.width)
            .ok()?
            .checked_mul(usize::try_from(self.height).ok()?)?
            .checked_mul(self.format.bytes_per_pixel())
    }
}

#[derive(Default)]
struct CaptureCallbackMetrics {
    copied_frames: AtomicU64,
    dropped_frames: AtomicU64,
    copied_bytes: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CaptureCallbackMetricsSnapshot {
    copied_frames: u64,
    dropped_frames: u64,
    copied_bytes: u64,
}

impl CaptureCallbackMetrics {
    fn record(&self, stats: CopyStats) {
        if stats.drop_reason().is_some() {
            self.dropped_frames.fetch_add(1, Ordering::Relaxed);
        } else {
            self.copied_frames.fetch_add(1, Ordering::Relaxed);
            self.copied_bytes.fetch_add(
                u64::try_from(stats.bytes_copied()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
    }

    fn snapshot(&self) -> CaptureCallbackMetricsSnapshot {
        CaptureCallbackMetricsSnapshot {
            copied_frames: self.copied_frames.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            copied_bytes: self.copied_bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct AnalysisExchangeState {
    latest: Option<DecodedChunk>,
    stopped: bool,
}

#[derive(Default)]
struct AnalysisExchange {
    state: Mutex<AnalysisExchangeState>,
    wake: Condvar,
}

impl AnalysisExchange {
    fn publish(&self, frame: DecodedChunk) {
        let replaced = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.stopped {
                Some(frame)
            } else {
                state.latest.replace(frame)
            }
        };
        drop(replaced);
        self.wake.notify_one();
    }

    fn wait_for_latest(&self, deadline: Instant, cancel: &AtomicBool) -> Option<DecodedChunk> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.stopped || cancel.load(Ordering::Acquire) {
                return None;
            }
            let now = Instant::now();
            if now >= deadline
                && let Some(frame) = state.latest.take()
            {
                return Some(frame);
            }
            let timeout = if now >= deadline {
                WORKER_POLL_INTERVAL
            } else {
                deadline.saturating_duration_since(now)
            };
            let waited = self
                .wake
                .wait_timeout(state, timeout.min(WORKER_POLL_INTERVAL))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waited.0;
        }
    }

    fn stop(&self) {
        let discarded = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.stopped = true;
            state.latest.take()
        };
        drop(discarded);
        self.wake.notify_all();
    }
}

struct WaylandAnalysisState {
    analyzer: ScreenCaptureInput,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    plane_pool: CapturePlanePool,
    settings: Arc<SharedSettings>,
    applied_generation: u64,
    applied_demand: ScreenCaptureDemand,
    source: WaylandSourceMetadata,
    sequence: u64,
}

impl WaylandAnalysisState {
    fn new(
        settings: Arc<SharedSettings>,
        latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
        source: WaylandSourceMetadata,
        config: CaptureConfig,
        demand: ScreenCaptureDemand,
    ) -> Result<Self, SurfaceResourceError> {
        let applied_generation = settings.generation.load(Ordering::Acquire);
        let requested_extent = demand
            .requested_extent()
            .expect("an active Wayland analysis worker carries an extent");
        let mut analyzer = ScreenCaptureInput::with_requested_extent(config, requested_extent)?;
        let _ = analyzer.start();

        Ok(Self {
            analyzer,
            latest_snapshot,
            plane_pool: CapturePlanePool::default(),
            settings,
            applied_generation,
            applied_demand: demand,
            source,
            sequence: 0,
        })
    }

    fn sync_settings(&mut self, cancel: &AtomicBool) -> bool {
        let generation = self.settings.generation.load(Ordering::Acquire);
        if generation == self.applied_generation {
            return self
                .settings
                .session_is_current(self.source.session_generation, cancel);
        }
        let Some(runtime) = self
            .settings
            .snapshot_for_session(self.source.session_generation, cancel)
        else {
            return false;
        };
        if let Some(requested_extent) = runtime.demand.requested_extent() {
            if let Err(error) = self.analyzer.set_requested_extent(requested_extent) {
                warn!(%error, generation, previous_demand = ?self.applied_demand, next_demand = ?runtime.demand, "Retaining prior Wayland screen analysis settings");
                return true;
            }
        }
        self.analyzer.apply_settings(runtime.config);
        self.applied_demand = runtime.demand;
        self.applied_generation = generation;
        debug!(generation, "Applied live screen capture settings");
        true
    }

    fn capture_frame(
        &mut self,
        captured_at: Instant,
        width: u32,
        height: u32,
        crop: Option<PixelRect>,
        transform: CaptureRotation,
        plane: PooledCapturePlane,
    ) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let storage_extent = PixelExtent::new(width, height)?;
        let topology = if let Some(topology) = self.source.topology {
            topology
        } else {
            let topology = self
                .settings
                .activate_topology(
                    &self.source.signature,
                    storage_extent,
                    self.source.session_generation,
                )
                .ok_or_else(|| {
                    anyhow!("Wayland capture session became stale during topology activation")
                })?;
            self.source.topology = Some(topology);
            topology
        };
        let row_stride = i64::from(width)
            .checked_mul(4)
            .ok_or_else(|| anyhow!("Wayland capture row stride overflow"))?;
        let frame_period =
            Duration::from_secs_f64(1.0 / f64::from(self.analyzer.config().target_fps.max(1)));
        let frame = CaptureFrame::new(
            CaptureFrameMetadata {
                source_id: self.source.signature.source_id.clone(),
                topology_generation: topology.generation,
                session_generation: self.source.session_generation,
                sequence: self.sequence,
                captured_at,
                fresh_until: captured_at + frame_period + frame_period,
                geometry: CaptureGeometry::new(
                    self.source.signature.origin,
                    topology.native_extent,
                    storage_extent,
                    transform,
                    crop,
                    self.source.source_scale(topology.native_extent.width()),
                )?,
                color_space: CaptureColorSpace::Unknown,
                transfer_function: CaptureTransferFunction::Unknown,
                cursor: CaptureCursor::default(),
            },
            CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
                plane,
                CapturePixelFormat::Rgba8,
                row_stride,
                0,
            )),
            CaptureDamage::default(),
        )?;
        let expected = self
            .settings
            .expected_epoch()
            .ok_or_else(|| anyhow!("Wayland capture epoch is not active"))?;
        frame.validate_epoch(&expected)?;
        Ok(frame)
    }
}

fn run_analysis_worker(
    exchange: &AnalysisExchange,
    settings: Arc<SharedSettings>,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    source: WaylandSourceMetadata,
    config: CaptureConfig,
    demand: ScreenCaptureDemand,
    cancel: &AtomicBool,
) {
    let mut state =
        match WaylandAnalysisState::new(settings, latest_snapshot, source, config, demand) {
            Ok(state) => state,
            Err(error) => {
                warn!(%error, "Failed to admit Wayland screen analysis extent");
                return;
            }
        };
    let mut deadline = Instant::now();
    while let Some(decoded) = exchange.wait_for_latest(deadline, cancel) {
        if !state.sync_settings(cancel) {
            return;
        }
        let Some(rgba_len) = usize::try_from(decoded.width)
            .ok()
            .and_then(|width| {
                usize::try_from(decoded.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            continue;
        };
        let Ok(mut plane) = state.plane_pool.try_acquire(rgba_len) else {
            warn!(
                rgba_len,
                "Retaining prior Wayland snapshot after plane allocation failure"
            );
            continue;
        };
        plane.resize(rgba_len, 0);
        convert_packed_to_rgba(&decoded, &mut plane);
        let captured_at = decoded.captured_at;
        let width = decoded.width;
        let height = decoded.height;
        let crop = decoded.crop;
        let transform = decoded.transform;
        drop(decoded);

        let Ok(frame) =
            state.capture_frame(captured_at, width, height, crop, transform, plane.freeze())
        else {
            continue;
        };
        let Ok(analysis) = analyze_screen_frame(&mut state.analyzer, frame) else {
            continue;
        };
        state
            .settings
            .publish_snapshot(&state.latest_snapshot, analysis);

        let period =
            Duration::from_secs_f64(1.0 / f64::from(state.analyzer.config().target_fps.max(1)));
        deadline = deadline
            .checked_add(period)
            .unwrap_or_else(Instant::now)
            .max(Instant::now());
    }
}

fn run_capture_worker(
    settings: Arc<SharedSettings>,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    command_rx: pw::channel::Receiver<WorkerCommand>,
    token_sink: Option<RestoreTokenSink>,
    flags: WorkerFlags,
    status_writer: Option<SourceSessionWriter>,
    active_session_generation: Arc<AtomicU64>,
) {
    let mut session_generation = active_session_generation.load(Ordering::Acquire);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!(%error, "Failed to create Wayland capture runtime");
            if let Some(status) = status_writer.as_ref() {
                settings.publish_status_for_session(
                    session_generation,
                    &flags.cancel,
                    status,
                    |status| {
                        status.failed(SourceIssue::new(
                            "wayland_runtime_start_failed",
                            error.to_string(),
                            true,
                        ))
                    },
                );
            }
            return;
        }
    };

    let mut command_rx = Some(command_rx);
    loop {
        flags.portal_pending.store(false, Ordering::SeqCst);
        if !wait_for_demand(&flags) {
            return;
        }

        let Some(startup) = settings.snapshot_for_session(session_generation, &flags.cancel) else {
            return;
        };
        flags.portal_pending.store(true, Ordering::SeqCst);
        let portal_result =
            runtime.block_on(open_portal_session_while_demanded(&startup.config, &flags));
        flags.portal_pending.store(false, Ordering::SeqCst);
        let Some(portal_result) = portal_result else {
            if !settings.session_is_current(session_generation, &flags.cancel) {
                return;
            }
            continue;
        };
        let (portal, restore_token) = match portal_result {
            Ok(portal) => portal,
            Err(error) => {
                if !settings.session_is_current(session_generation, &flags.cancel) {
                    return;
                }
                warn!(%error, "Failed to establish Wayland screencast session");
                if let Some(status) = status_writer.as_ref() {
                    settings.publish_status_for_session(
                        session_generation,
                        &flags.cancel,
                        status,
                        |status| {
                            status.unavailable(
                                SourceIssue::new(
                                    "wayland_portal_unavailable",
                                    error.to_string(),
                                    true,
                                )
                                .with_remediation(
                                    "grant screen-sharing permission in the desktop portal",
                                ),
                            )
                        },
                    );
                }
                if !wait_for_retry(&flags) {
                    return;
                }
                let Some(successor_generation) = settings.begin_successor_session(
                    session_generation,
                    &flags.cancel,
                    &active_session_generation,
                ) else {
                    return;
                };
                session_generation = successor_generation;
                continue;
            }
        };

        if !settings.session_is_current(session_generation, &flags.cancel) {
            return;
        }

        if restore_token != startup.config.restore_token {
            if !settings.persist_restore_token_for_session(
                session_generation,
                &flags.cancel,
                restore_token,
                token_sink.as_ref(),
            ) {
                return;
            }
        }

        let PortalCaptureSession {
            session,
            stream,
            fd,
        } = portal;
        let loop_outcome = run_pipewire_loop(
            &startup.config,
            startup.demand,
            Arc::clone(&settings),
            Arc::clone(&latest_snapshot),
            stream,
            fd,
            &mut command_rx,
            Arc::clone(&flags.cancel),
            session_generation,
        );
        settings.invalidate_session(&latest_snapshot, session_generation);
        if let Err(error) = runtime.block_on(session.close()) {
            warn!(%error, "Wayland screencast session close reported an error");
        }
        if !settings.session_is_current(session_generation, &flags.cancel) {
            return;
        }

        let reason = match loop_outcome {
            Ok(PipeWireLoopExit::Stopped) => return,
            Ok(PipeWireLoopExit::Terminal(reason)) => reason,
            Err(error) => error.to_string(),
        };
        warn!(%reason, "Wayland screen capture stream terminated; reconnecting");
        if let Some(status) = status_writer.as_ref() {
            settings.publish_status_for_session(
                session_generation,
                &flags.cancel,
                status,
                |status| {
                    status.degraded(SourceIssue::new(
                        "wayland_stream_reconnecting",
                        reason,
                        true,
                    ))
                },
            );
        }
        if !wait_for_retry(&flags) {
            return;
        }
        let Some(successor_generation) = settings.begin_successor_session(
            session_generation,
            &flags.cancel,
            &active_session_generation,
        ) else {
            return;
        };
        session_generation = successor_generation;
    }
}

fn wait_for_demand(flags: &WorkerFlags) -> bool {
    while !flags.cancel.load(Ordering::Acquire) {
        if flags.demanded.load(Ordering::Acquire) {
            return true;
        }
        thread::park_timeout(WORKER_POLL_INTERVAL);
    }
    false
}

fn wait_for_retry(flags: &WorkerFlags) -> bool {
    let deadline = Instant::now()
        .checked_add(RECONNECT_DELAY)
        .unwrap_or_else(Instant::now);
    while !flags.cancel.load(Ordering::Acquire)
        && flags.demanded.load(Ordering::Acquire)
        && Instant::now() < deadline
    {
        thread::park_timeout(
            WORKER_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    !flags.cancel.load(Ordering::Acquire) && flags.demanded.load(Ordering::Acquire)
}

async fn open_portal_session_while_demanded(
    config: &CaptureConfig,
    flags: &WorkerFlags,
) -> Option<anyhow::Result<(PortalCaptureSession, Option<String>)>> {
    tokio::select! {
        result = open_portal_session(config) => Some(result),
        () = wait_until_worker_inactive(flags) => None,
    }
}

async fn wait_until_worker_inactive(flags: &WorkerFlags) {
    while !flags.cancel.load(Ordering::Acquire) && flags.demanded.load(Ordering::Acquire) {
        tokio::time::sleep(WORKER_POLL_INTERVAL).await;
    }
}

async fn open_portal_session(
    config: &CaptureConfig,
) -> anyhow::Result<(PortalCaptureSession, Option<String>)> {
    let proxy = Screencast::new()
        .await
        .context("failed to connect to xdg-desktop-portal screencast interface")?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .context("failed to create screencast portal session")?;

    // An invalid or revoked restore token is ignored by the portal, which
    // falls back to showing the picker — no retry path needed.
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(Some(SourceType::Monitor.into()))
                .set_multiple(false)
                .set_restore_token(config.restore_token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await
        .context("failed to open screencast source picker")?;

    let response = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .context("failed to start screencast portal session")?
        .response()
        .context("screen capture request was denied or cancelled")?;
    let restore_token = response.restore_token().map(ToOwned::to_owned);
    let stream = response
        .streams()
        .first()
        .cloned()
        .context("portal did not return a monitor stream")?;
    let fd = proxy
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await
        .context("failed to open PipeWire remote for screencast session")?;

    info!(
        pipewire_node = stream.pipe_wire_node_id(),
        stream = ?stream,
        restored = config.restore_token.is_some(),
        "Wayland screencast session established"
    );

    Ok((
        PortalCaptureSession {
            session,
            stream,
            fd,
        },
        restore_token,
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PipeWireLoopExit {
    Stopped,
    Terminal(String),
}

#[allow(clippy::too_many_arguments)]
fn run_pipewire_loop(
    config: &CaptureConfig,
    demand: ScreenCaptureDemand,
    settings: Arc<SharedSettings>,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    portal_stream: Stream,
    portal_fd: OwnedFd,
    command_rx: &mut Option<pw::channel::Receiver<WorkerCommand>>,
    cancel: Arc<AtomicBool>,
    session_generation: u64,
) -> anyhow::Result<PipeWireLoopExit> {
    pw::init();
    let source = WaylandSourceMetadata::from_stream(&portal_stream, session_generation)?;
    let exchange = Arc::new(AnalysisExchange::default());
    let callback_metrics = Arc::new(CaptureCallbackMetrics::default());
    let loop_exit = Arc::new(Mutex::new(None::<PipeWireLoopExit>));

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("failed to create PipeWire context")?;
    let core = context
        .connect_fd_rc(portal_fd, None)
        .context("failed to connect to screencast PipeWire remote")?;

    let stream = pw::stream::StreamRc::new(
        core,
        "hypercolor-screen-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .context("failed to create PipeWire capture stream")?;

    let _listener = stream
        .add_local_listener_with_user_data(WaylandCaptureUserData::new(
            Arc::clone(&exchange),
            Arc::clone(&callback_metrics),
        ))
        .state_changed({
            let mainloop = mainloop.clone();
            let loop_exit = Arc::clone(&loop_exit);
            move |_, _, old, new| {
                debug!(?old, ?new, "Wayland screen capture stream state changed");
                let terminal = match new {
                    pw::stream::StreamState::Error(error) => {
                        Some(format!("PipeWire stream entered error state: {error}"))
                    }
                    pw::stream::StreamState::Unconnected
                        if old != pw::stream::StreamState::Unconnected =>
                    {
                        Some("PipeWire stream disconnected".to_owned())
                    }
                    _ => None,
                };
                if let Some(reason) = terminal {
                    let mut exit = loop_exit
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if exit.is_none() {
                        *exit = Some(PipeWireLoopExit::Terminal(reason));
                        mainloop.quit();
                    }
                }
            }
        })
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }

            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }

            if user_data.format.parse(param).is_err() {
                warn!("Failed to parse negotiated PipeWire video format");
                return;
            }

            let format = user_data.format.format();
            let size = user_data.format.size();
            let negotiated = spa_video_format(format).map(|format| NegotiatedFormat {
                width: size.width,
                height: size.height,
                format,
            });
            match negotiated {
                Some(negotiated) => match user_data.set_negotiated_format(negotiated) {
                    Ok(()) => info!(
                        ?format,
                        width = size.width,
                        height = size.height,
                        "Negotiated Wayland screen capture format"
                    ),
                    Err(error) => warn!(
                        ?format,
                        width = size.width,
                        height = size.height,
                        %error,
                        "Retaining prior Wayland callback planes"
                    ),
                },
                None => warn!(
                    ?format,
                    width = size.width,
                    height = size.height,
                    "Negotiated unsupported Wayland screen capture format"
                ),
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                user_data.record_drop(ChunkDropReason::MissingBuffer);
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                user_data.record_drop(ChunkDropReason::MissingPlane);
                return;
            };

            let Some(negotiated) = user_data.negotiated else {
                user_data.record_drop(ChunkDropReason::MissingFormat);
                return;
            };
            let (offset, size, stride) = {
                let chunk = data.chunk();
                (
                    usize::try_from(chunk.offset()).ok(),
                    usize::try_from(chunk.size()).ok(),
                    chunk.stride(),
                )
            };
            let (Some(offset), Some(size)) = (offset, size) else {
                user_data.record_drop(ChunkDropReason::InvalidChunkBounds);
                return;
            };
            let Some(mapped) = data.data() else {
                user_data.record_drop(ChunkDropReason::UnmappedPlane);
                return;
            };
            // pipewire-rs 0.9 does not expose SPA buffer metas safely; the
            // pure seam still carries crop/transform until an audited adapter does.
            let view = SpaChunkView::new(
                mapped,
                offset,
                size,
                stride,
                negotiated.width,
                negotiated.height,
                negotiated.format,
                None,
                CaptureRotation::Identity,
            );
            let stats = decode_chunk(&view, &mut user_data.buffers);
            let completed = if stats.drop_reason().is_none() {
                user_data.buffers.take_completed()
            } else {
                None
            };
            drop(buffer);
            user_data.metrics.record(stats);
            if let Some(frame) = completed {
                user_data.exchange.publish(frame);
            }
        })
        .register()
        .context("failed to register PipeWire screen capture listener")?;

    let requested_extent = demand
        .requested_extent()
        .context("active Wayland capture demand must carry an extent")?;
    let format_bytes = build_format_params(config.target_fps.max(1), requested_extent)?;
    let mut params = [spa::pod::Pod::from_bytes(&format_bytes)
        .context("failed to deserialize PipeWire format pod")?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(portal_stream.pipe_wire_node_id()),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .context("failed to connect PipeWire screen capture stream")?;

    let (analysis_exit_tx, analysis_exit_rx) = pw::channel::channel();
    let analysis_exchange = Arc::clone(&exchange);
    let analysis_cancel = Arc::clone(&cancel);
    let analysis_config = config.clone();
    let analysis_demand = demand;
    let analysis_handle = spawn_input_worker(
        thread::Builder::new().name("hypercolor-screen-analysis".to_owned()),
        move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_analysis_worker(
                    &analysis_exchange,
                    settings,
                    latest_snapshot,
                    source,
                    analysis_config,
                    analysis_demand,
                    &analysis_cancel,
                );
            }));
            if result.is_err() {
                let _ = analysis_exit_tx.send(());
            }
        },
    )
    .context("failed to spawn Wayland screen analysis worker")?;
    let _analysis_exit_rx = analysis_exit_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let loop_exit = Arc::clone(&loop_exit);
        move |()| {
            *loop_exit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(
                PipeWireLoopExit::Terminal("Wayland analysis worker panicked".to_owned()),
            );
            mainloop.quit();
        }
    });

    let receiver = command_rx
        .take()
        .context("PipeWire command receiver was not returned by the previous stream")?;
    let attached_command_rx = receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let stream = stream.clone();
        let loop_exit = Arc::clone(&loop_exit);
        let target_fps = config.target_fps.max(1);
        move |command| match command {
            WorkerCommand::SetDemand(demand) => {
                let active = demand.is_active();
                if let Err(error) = stream.set_active(active) {
                    warn!(active, %error, "Failed to update PipeWire stream active state");
                }
                if let Some(requested_extent) = demand.requested_extent() {
                    let update =
                        build_format_params(target_fps, requested_extent).and_then(|bytes| {
                            let pod = spa::pod::Pod::from_bytes(&bytes)
                                .context("failed to deserialize PipeWire format pod")?;
                            stream
                                .update_params(&mut [pod])
                                .context("failed to update PipeWire format")
                        });
                    if let Err(error) = update {
                        warn!(%error, "Failed to update PipeWire capture extent");
                    }
                }
            }
            WorkerCommand::Stop => {
                *loop_exit
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(PipeWireLoopExit::Stopped);
                mainloop.quit();
            }
        }
    });

    mainloop.run();
    *command_rx = Some(attached_command_rx.deattach());
    exchange.stop();

    if let Err(error) = stream.disconnect() {
        debug!(%error, "PipeWire screen capture stream disconnect reported an error");
    }

    analysis_handle
        .join()
        .map_err(|panic| anyhow!("Wayland analysis worker join failed: {panic:?}"))?;
    let metrics = callback_metrics.snapshot();
    debug!(
        copied_frames = metrics.copied_frames,
        dropped_frames = metrics.dropped_frames,
        copied_bytes = metrics.copied_bytes,
        "Wayland capture callback totals"
    );
    Ok(loop_exit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .unwrap_or_else(|| {
            PipeWireLoopExit::Terminal("PipeWire main loop exited unexpectedly".to_owned())
        }))
}

fn build_format_params(target_fps: u32, requested_extent: PixelExtent) -> anyhow::Result<Vec<u8>> {
    let fps = target_fps;
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::ARGB,
            spa::param::video::VideoFormat::ABGR,
            spa::param::video::VideoFormat::xRGB,
            spa::param::video::VideoFormat::xBGR,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: requested_extent.width(),
                height: requested_extent.height(),
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1,
            },
            spa::utils::Rectangle {
                width: requested_extent.width(),
                height: requested_extent.height(),
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: fps, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1,
            }
        ),
    );

    Ok(spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner())
}

fn spa_video_format(format: spa::param::video::VideoFormat) -> Option<SpaVideoFormat> {
    match format {
        spa::param::video::VideoFormat::RGBA => Some(SpaVideoFormat::Rgba),
        spa::param::video::VideoFormat::BGRA => Some(SpaVideoFormat::Bgra),
        spa::param::video::VideoFormat::RGBx => Some(SpaVideoFormat::Rgbx),
        spa::param::video::VideoFormat::BGRx => Some(SpaVideoFormat::Bgrx),
        spa::param::video::VideoFormat::ARGB => Some(SpaVideoFormat::Argb),
        spa::param::video::VideoFormat::ABGR => Some(SpaVideoFormat::Abgr),
        spa::param::video::VideoFormat::xRGB => Some(SpaVideoFormat::Xrgb),
        spa::param::video::VideoFormat::xBGR => Some(SpaVideoFormat::Xbgr),
        spa::param::video::VideoFormat::RGB => Some(SpaVideoFormat::Rgb),
        spa::param::video::VideoFormat::BGR => Some(SpaVideoFormat::Bgr),
        _ => None,
    }
}

fn convert_packed_to_rgba(decoded: &DecodedChunk, rgba: &mut [u8]) {
    let width = usize::try_from(decoded.width).expect("validated width fits usize");
    let height = usize::try_from(decoded.height).expect("validated height fits usize");
    let source_row_bytes = width * decoded.format.bytes_per_pixel();
    let destination_row_bytes = width * 4;
    for row in 0..height {
        let source_start = row * source_row_bytes;
        let destination_start = row * destination_row_bytes;
        convert_row_to_rgba(
            &decoded.bytes()[source_start..source_start + source_row_bytes],
            &mut rgba[destination_start..destination_start + destination_row_bytes],
            decoded.format,
        );
    }
}

fn convert_row_to_rgba(src: &[u8], dst: &mut [u8], format: SpaVideoFormat) {
    match format {
        SpaVideoFormat::Rgba => {
            dst.copy_from_slice(src);
        }
        SpaVideoFormat::Bgra => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = src_px[3];
            }
        }
        SpaVideoFormat::Rgbx => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[0];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[2];
                dst_px[3] = 255;
            }
        }
        SpaVideoFormat::Bgrx => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = 255;
            }
        }
        SpaVideoFormat::Argb => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[1];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[3];
                dst_px[3] = src_px[0];
            }
        }
        SpaVideoFormat::Abgr => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[3];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[1];
                dst_px[3] = src_px[0];
            }
        }
        SpaVideoFormat::Xrgb => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[1];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[3];
                dst_px[3] = 255;
            }
        }
        SpaVideoFormat::Xbgr => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[3];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[1];
                dst_px[3] = 255;
            }
        }
        SpaVideoFormat::Rgb => {
            for (src_px, dst_px) in src.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[0];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[2];
                dst_px[3] = 255;
            }
        }
        SpaVideoFormat::Bgr => {
            for (src_px, dst_px) in src.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests;
