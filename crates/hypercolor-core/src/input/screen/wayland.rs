//! Wayland screen capture source powered by XDG Desktop Portal + PipeWire.
//!
//! This source keeps the portal session and PipeWire stream on a dedicated
//! worker thread. The render loop only clones the latest processed
//! [`ScreenData`] snapshot, while capture demand is toggled at runtime by the
//! daemon depending on the active effect.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Cursor;
use std::num::{NonZeroU32, NonZeroUsize};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
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
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::input::screen::{
    AnalyzedScreenSnapshot, CaptureCadence, CaptureColorimetry, CaptureConfig, CaptureCursor,
    CaptureDamage, CaptureEpoch, CaptureFrame, CaptureFrameError, CaptureFrameMetadata,
    CaptureGeometry, CapturePacer, CapturePixelFormat, CapturePlanePool, CaptureRotation,
    CaptureSourceId, CaptureStorage, CpuCaptureStorage, CpuExactReductionWorkPlan,
    CpuReductionExecutor, ExactBoxList, ExactBoxNode, PhysicalOrigin, PixelExtent, PixelRect,
    PooledCapturePlane, PreparedCpuPublicationFanout, PreparedCpuPublicationFanoutCandidate,
    RawCaptureSurface, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity,
    ScreenAnalysisAdmissionError, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan,
    ScreenAnalysisWorkPlan, ScreenBackendResourceIdentity, ScreenByteAdmissionCoordinator,
    ScreenByteLease, ScreenCaptureBackend, ScreenCaptureDemand, ScreenCaptureInput,
    ScreenColorTransformCapabilities, ScreenComputeCapacityPolicy, ScreenPreparedWorkerToken,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenRequiredResourceMinimum,
    ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime, ScreenSourceReflection,
    ScreenSourceSelector, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale, analyze_screen_frame,
};
use crate::input::traits::{InputData, InputSource};
use crate::input::worker_retention::{retain_input_worker, spawn_input_worker};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceSessionWriter, SourceStatusHandle,
    SourceStatusReporter,
};

const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const FORMAT_ADOPTION_TIMEOUT: Duration = Duration::from_secs(5);
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

#[derive(Debug)]
struct DoubleBufferInner {
    available: Mutex<Vec<Vec<u8>>>,
    capacity: usize,
    _admission: Option<ScreenByteLease>,
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
        Self::try_with_capacity_and_lease(capacity, None)
    }

    fn try_with_capacity_and_admission(
        capacity: usize,
        admission_coordinator: &ScreenByteAdmissionCoordinator,
    ) -> Result<Self, CaptureFrameError> {
        let bytes = u64::try_from(capacity)
            .ok()
            .and_then(|capacity| capacity.checked_mul(2))
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let reservation =
            admission_coordinator
                .try_acquire(bytes)
                .map_err(|error| match error {
                    crate::input::screen::ScreenByteAdmissionError::CapacityExceeded {
                        requested_bytes,
                        available_bytes,
                    } => CaptureFrameError::PlaneCapacityExceeded {
                        requested_bytes,
                        available_bytes,
                    },
                    crate::input::screen::ScreenByteAdmissionError::CapacityShrinkRejected {
                        ..
                    }
                    | crate::input::screen::ScreenByteAdmissionError::RevisionExhausted => {
                        CaptureFrameError::PlaneAllocationFailed { byte_len: capacity }
                    }
                })?;
        Self::try_with_capacity_and_lease(capacity, Some(reservation.freeze()))
    }

    fn try_with_capacity_and_lease(
        capacity: usize,
        admission: Option<ScreenByteLease>,
    ) -> Result<Self, CaptureFrameError> {
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
                _admission: admission,
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

    fn capacity(&self) -> usize {
        self.inner.capacity
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
    pool: Arc<DoubleBufferInner>,
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
        let Some(buffer) = self.buffer.take() else {
            return;
        };
        self.pool
            .available
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
            pool: Arc::clone(&buffers.inner),
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
    admission_coordinator: ScreenByteAdmissionCoordinator,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    generation: AtomicU64,
    frame_generation: AtomicU64,
    topology_generation: AtomicU64,
    topology: Mutex<Option<WaylandTopologyState>>,
    session_generation: AtomicU64,
    expected_epoch: Mutex<Option<CaptureEpoch>>,
    exact: WaylandExactPublicationShared,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WaylandPublicationSource {
    epoch: CaptureEpoch,
    config: ResolvedScreenSourceConfig,
}

impl WaylandPublicationSource {
    fn matches_selector(&self, selector: &ScreenSourceSelector) -> bool {
        match selector {
            ScreenSourceSelector::Configured | ScreenSourceSelector::Primary => true,
            ScreenSourceSelector::Exact(source_id) => source_id == &self.epoch.source_id,
        }
    }

    fn resolved(&self, selector: ScreenSourceSelector) -> ResolvedScreenSource {
        ResolvedScreenSource::new(selector, self.epoch.clone(), self.config.clone())
    }
}

struct WaylandOwnedSource {
    source_id: CaptureSourceId,
    session_generation: u64,
    binding: ScreenWorkerBinding,
    _runtime_lifetime: ScreenResourceLifetime,
}

#[derive(Default)]
struct WaylandExactPublicationShared {
    source: Mutex<Option<WaylandPublicationSource>>,
    owned_sources: Mutex<ExactBoxList<WaylandOwnedSource>>,
    hub: Mutex<Option<Arc<ScreenPublicationHub>>>,
    cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
    resolution_revision: AtomicU64,
}

impl WaylandExactPublicationShared {
    fn replace_source(&self, next: Option<WaylandPublicationSource>) {
        let mut source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *source == next {
            return;
        }
        *source = next;
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("Wayland screen publication resolution revision exhausted");
    }

    fn source(&self) -> Option<WaylandPublicationSource> {
        self.source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn hub(&self) -> Option<Arc<ScreenPublicationHub>> {
        self.hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn owns_source(&self, source_id: &CaptureSourceId) -> bool {
        self.source()
            .is_some_and(|source| &source.epoch.source_id == source_id)
            || self
                .owned_sources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|owned| &owned.source_id == source_id)
    }

    fn register_owned_source(&self, source: Box<ExactBoxNode<WaylandOwnedSource>>) {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_boxed(source);
    }

    fn reap_owned_sources(&self) {
        let authority = self.hub().map(|hub| hub.committed_state());
        let mut sources = self
            .owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sources.retain(|source| {
            authority
                .as_ref()
                .is_some_and(|authority| authority.owns_runtime_binding(&source.binding))
        });
    }

    fn clear_owned_sources_for_session(&self, session_generation: u64) {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|source| source.session_generation != session_generation);
    }

    fn clear_owned_sources(&self) {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn cpu_executor(&self) -> anyhow::Result<Arc<CpuReductionExecutor>> {
        let mut executor = self
            .cpu_executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(executor) = executor.as_ref() {
            return Ok(Arc::clone(executor));
        }
        let prepared = Arc::new(CpuReductionExecutor::new(
            thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
            NonZeroU32::new(16).expect("CPU reduction tile height is nonzero"),
        )?);
        *executor = Some(Arc::clone(&prepared));
        Ok(prepared)
    }

    fn cpu_worker_count(&self) -> NonZeroUsize {
        self.cpu_executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or_else(
                || thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
                |executor| executor.worker_count(),
            )
    }
}

struct CaptureRuntimeSettings {
    config: CaptureConfig,
    demand: ScreenCaptureDemand,
}

struct PreparedWaylandSettings {
    config: CaptureConfig,
    cadence: CaptureCadence,
    demand: ScreenCaptureDemand,
    analyzer: ScreenCaptureInput,
    pipewire_format: Option<PreparedPipeWireFormat>,
}

struct PreparedPipeWireFormat {
    callback_buffers: DoubleBuffer,
    format_bytes: Vec<u8>,
    request: PipeWireFormatRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PipeWireFormatRequest {
    extent: PixelExtent,
    target_fps: u32,
    analysis_work_plan: ScreenAnalysisWorkPlan,
}

impl PipeWireFormatRequest {
    fn new_with_compute_policy(
        extent: PixelExtent,
        requested_output_extent: PixelExtent,
        config: &CaptureConfig,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> anyhow::Result<Self> {
        let work_plan = ScreenAnalysisWorkPlan::try_new(extent, requested_output_extent, config)?;
        let analysis_work_plan = match compute_capacity_policy.analysis() {
            Some(capacity) => work_plan.admit(capacity)?,
            None => work_plan,
        };
        Ok(Self {
            extent,
            target_fps: config.target_fps,
            analysis_work_plan,
        })
    }

    #[cfg(test)]
    fn new_with_compute_capacity(
        extent: PixelExtent,
        requested_output_extent: PixelExtent,
        config: &CaptureConfig,
        capacity: ScreenAnalysisComputeCapacity,
    ) -> anyhow::Result<Self> {
        let analysis_work_plan =
            ScreenAnalysisWorkPlan::try_new(extent, requested_output_extent, config)?
                .admit(capacity)?;
        Ok(Self {
            extent,
            target_fps: config.target_fps,
            analysis_work_plan,
        })
    }

    fn matches(self, negotiated: NegotiatedPipeWireFormat) -> bool {
        let rate = negotiated.framerate;
        self.analysis_work_plan.input_extent() == self.extent
            && self.analysis_work_plan.target_fps() == self.target_fps
            && negotiated.frame.width == self.extent.width()
            && negotiated.frame.height == self.extent.height()
            && rate.denom != 0
            && u64::from(rate.num) == u64::from(self.target_fps) * u64::from(rate.denom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PipeWireFormatAcknowledgment {
    Pending,
    Current,
    Restored,
    Restoring,
    CancelledCurrent,
    Cancelled,
    Rejected,
}

struct PendingPipeWireAdoption {
    id: u64,
    request: PipeWireFormatRequest,
    format_bytes: Vec<u8>,
    callback_buffers: DoubleBuffer,
    analysis_decision: mpsc::SyncSender<SettingsDecision>,
    analysis_done: mpsc::Receiver<bool>,
    done: mpsc::SyncSender<Result<(), String>>,
    authority: Arc<AdoptionAuthority>,
}

struct RestoringPipeWireAdoption {
    pending: PendingPipeWireAdoption,
    failure: String,
}

struct PipeWireFormatState {
    current: PipeWireFormatRequest,
    current_format_bytes: Vec<u8>,
    current_acknowledged: bool,
    pending: Option<PendingPipeWireAdoption>,
    restoring: Option<RestoringPipeWireAdoption>,
}

impl PipeWireFormatState {
    fn acknowledgment(&self, negotiated: NegotiatedPipeWireFormat) -> PipeWireFormatAcknowledgment {
        if self.restoring.is_some() {
            return if self.current.matches(negotiated) {
                PipeWireFormatAcknowledgment::Restored
            } else {
                PipeWireFormatAcknowledgment::Restoring
            };
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.authority.is_cancelled())
        {
            return if self.current.matches(negotiated) {
                PipeWireFormatAcknowledgment::CancelledCurrent
            } else {
                PipeWireFormatAcknowledgment::Cancelled
            };
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request.matches(negotiated))
        {
            PipeWireFormatAcknowledgment::Pending
        } else if self.current.matches(negotiated) {
            PipeWireFormatAcknowledgment::Current
        } else {
            PipeWireFormatAcknowledgment::Rejected
        }
    }

    fn cancel(&mut self, adoption_id: u64) -> Option<PendingPipeWireAdoption> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == adoption_id)
        {
            self.pending.take()
        } else {
            None
        }
    }

    fn begin_restoring(&mut self, pending: PendingPipeWireAdoption, failure: String) -> Vec<u8> {
        self.restoring = Some(RestoringPipeWireAdoption { pending, failure });
        self.current_format_bytes.clone()
    }

    fn restoring_id(&self) -> Option<u64> {
        self.restoring
            .as_ref()
            .map(|restoring| restoring.pending.id)
    }

    fn can_begin_adoption(&self) -> bool {
        self.current_acknowledged && self.pending.is_none() && self.restoring.is_none()
    }

    fn settle_restoration(&mut self) -> Option<RestoringPipeWireAdoption> {
        let restoring = self.restoring.take()?;
        self.current_acknowledged = true;
        Some(restoring)
    }
}

struct PreparedAnalysisSettings {
    config: CaptureConfig,
    cadence: CaptureCadence,
    demand: ScreenCaptureDemand,
    analyzer: ScreenCaptureInput,
}

enum SettingsDecision {
    Commit,
}

const ADOPTION_OPEN: u8 = 0;
const ADOPTION_COMMITTING: u8 = 1;
const ADOPTION_CANCELLED: u8 = 2;
const ADOPTION_COMMITTED: u8 = 3;
const ADOPTION_ANALYSIS_APPLIED: u8 = 4;

#[derive(Default)]
struct AdoptionAuthority {
    phase: AtomicU8,
    transition: Mutex<()>,
    settled: Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdoptionSettlement {
    Cancelled,
    Committed,
}

impl AdoptionAuthority {
    fn claim_commit(&self) -> bool {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.phase
            .compare_exchange(
                ADOPTION_OPEN,
                ADOPTION_COMMITTING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn cancel(&self) -> bool {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cancelled = self
            .phase
            .compare_exchange(
                ADOPTION_OPEN,
                ADOPTION_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if cancelled {
            self.settled.notify_all();
        }
        cancelled
    }

    fn prepare_if_open<R>(&self, prepare: impl FnOnce() -> R) -> Option<R> {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (self.phase.load(Ordering::Acquire) == ADOPTION_OPEN).then(prepare)
    }

    fn is_cancelled(&self) -> bool {
        self.phase.load(Ordering::Acquire) == ADOPTION_CANCELLED
    }

    fn complete_commit(&self) {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let phase = self.phase.load(Ordering::Acquire);
        let result = if matches!(phase, ADOPTION_COMMITTING | ADOPTION_ANALYSIS_APPLIED) {
            self.phase.compare_exchange(
                phase,
                ADOPTION_COMMITTED,
                Ordering::Release,
                Ordering::Acquire,
            )
        } else {
            Err(phase)
        };
        debug_assert!(
            result.is_ok(),
            "only the commit winner can complete adoption"
        );
        if result.is_ok() {
            self.settled.notify_all();
        }
    }

    fn complete_analysis(&self) {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = self.phase.compare_exchange(
            ADOPTION_COMMITTING,
            ADOPTION_ANALYSIS_APPLIED,
            Ordering::Release,
            Ordering::Acquire,
        );
        debug_assert!(
            result.is_ok(),
            "only the commit winner can complete analysis adoption"
        );
        if result.is_ok() {
            self.settled.notify_all();
        }
    }

    #[cfg(test)]
    fn committed(&self) -> bool {
        self.phase.load(Ordering::Acquire) == ADOPTION_COMMITTED
    }

    fn is_committing(&self) -> bool {
        matches!(
            self.phase.load(Ordering::Acquire),
            ADOPTION_COMMITTING | ADOPTION_ANALYSIS_APPLIED
        )
    }

    fn cancel_or_wait_for_commit(&self) -> AdoptionSettlement {
        let mut transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            match self.phase.load(Ordering::Acquire) {
                ADOPTION_OPEN => {
                    self.phase.store(ADOPTION_CANCELLED, Ordering::Release);
                    self.settled.notify_all();
                    return AdoptionSettlement::Cancelled;
                }
                ADOPTION_CANCELLED => return AdoptionSettlement::Cancelled,
                ADOPTION_COMMITTED => return AdoptionSettlement::Committed,
                ADOPTION_COMMITTING | ADOPTION_ANALYSIS_APPLIED => {
                    transition = self
                        .settled
                        .wait(transition)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                phase => unreachable!("invalid adoption authority phase {phase}"),
            }
        }
    }

    fn cancel_or_wait_for_analysis(&self) -> AdoptionSettlement {
        let mut transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            match self.phase.load(Ordering::Acquire) {
                ADOPTION_OPEN => {
                    self.phase.store(ADOPTION_CANCELLED, Ordering::Release);
                    self.settled.notify_all();
                    return AdoptionSettlement::Cancelled;
                }
                ADOPTION_CANCELLED => return AdoptionSettlement::Cancelled,
                ADOPTION_ANALYSIS_APPLIED | ADOPTION_COMMITTED => {
                    return AdoptionSettlement::Committed;
                }
                ADOPTION_COMMITTING => {
                    transition = self
                        .settled
                        .wait(transition)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                phase => unreachable!("invalid adoption authority phase {phase}"),
            }
        }
    }
}

#[cfg(test)]
fn commit_if_authorized(
    authority: &AdoptionAuthority,
    finalize: bool,
    commit: impl FnOnce(),
) -> bool {
    if !authority.claim_commit() {
        return false;
    }
    commit();
    if finalize {
        authority.complete_commit();
    } else {
        authority.complete_analysis();
    }
    true
}

fn commit_claimed(authority: &AdoptionAuthority, finalize: bool, commit: impl FnOnce()) -> bool {
    if !authority.is_committing() {
        return false;
    }
    commit();
    if finalize {
        authority.complete_commit();
    } else {
        authority.complete_analysis();
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdoptionWaitError {
    Disconnected,
    CancellationUnsettled(mpsc::RecvTimeoutError),
}

fn wait_for_adoption_result(
    done: &mpsc::Receiver<Result<(), String>>,
    adoption_timeout: Duration,
    cancellation_timeout: Duration,
    authority: &AdoptionAuthority,
    cancel: impl FnOnce(),
) -> Result<Result<(), String>, AdoptionWaitError> {
    match done.recv_timeout(adoption_timeout) {
        Ok(result) => match authority.cancel_or_wait_for_commit() {
            AdoptionSettlement::Committed => Ok(Ok(())),
            AdoptionSettlement::Cancelled => Ok(result.and_then(|()| {
                Err("Wayland capture reported adoption success without commit authority".to_owned())
            })),
        },
        Err(first_error) => match authority.cancel_or_wait_for_commit() {
            AdoptionSettlement::Committed => Ok(Ok(())),
            AdoptionSettlement::Cancelled => {
                cancel();
                if first_error == mpsc::RecvTimeoutError::Disconnected {
                    return Err(AdoptionWaitError::Disconnected);
                }
                done.recv_timeout(cancellation_timeout)
                    .map_err(AdoptionWaitError::CancellationUnsettled)
            }
        },
    }
}

const WORKER_DEMAND_INACTIVE: u64 = 0;
const WORKER_DEMAND_ACTIVE: u64 = 1;
const WORKER_DEMAND_PARKED: u64 = 2;
const WORKER_DEMAND_MODE_MASK: u64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnavailablePark {
    Parked,
    Rearmed,
    Inactive,
}

fn worker_demand_mode(state: u64) -> u64 {
    state & WORKER_DEMAND_MODE_MASK
}

fn worker_demand_epoch(state: u64) -> u64 {
    state >> 2
}

fn initial_worker_demand(active: bool) -> u64 {
    u64::from(active)
}

fn request_active_worker_demand(state: &AtomicU64) -> bool {
    let mut current = state.load(Ordering::Acquire);
    loop {
        let next_epoch = worker_demand_epoch(current).wrapping_add(1);
        let next = (next_epoch << 2) | WORKER_DEMAND_ACTIVE;
        match state.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return worker_demand_mode(current) == WORKER_DEMAND_PARKED,
            Err(observed) => current = observed,
        }
    }
}

fn set_worker_demand(state: &AtomicU64, active: bool) {
    if active {
        request_active_worker_demand(state);
        return;
    }
    let mut current = state.load(Ordering::Acquire);
    loop {
        let next_epoch = worker_demand_epoch(current).wrapping_add(1);
        let next = (next_epoch << 2) | WORKER_DEMAND_INACTIVE;
        match state.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

fn worker_demanded(state: &AtomicU64) -> bool {
    worker_demand_mode(state.load(Ordering::Acquire)) == WORKER_DEMAND_ACTIVE
}

fn park_unavailable_worker(state: &AtomicU64, session_epoch: u64) -> UnavailablePark {
    let mut current = state.load(Ordering::Acquire);
    loop {
        match worker_demand_mode(current) {
            WORKER_DEMAND_INACTIVE => return UnavailablePark::Inactive,
            WORKER_DEMAND_PARKED => return UnavailablePark::Parked,
            WORKER_DEMAND_ACTIVE if worker_demand_epoch(current) != session_epoch => {
                return UnavailablePark::Rearmed;
            }
            WORKER_DEMAND_ACTIVE => {
                let parked = (current & !WORKER_DEMAND_MODE_MASK) | WORKER_DEMAND_PARKED;
                match state.compare_exchange(current, parked, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return UnavailablePark::Parked,
                    Err(observed) => current = observed,
                }
            }
            mode => unreachable!("invalid worker demand mode {mode}"),
        }
    }
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
    fn config_snapshot(&self) -> CaptureConfig {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn commit_runtime(&self, next: &PreparedAnalysisSettings) -> u64 {
        self.commit_values(&next.config, next.demand)
    }

    fn commit_values(&self, next_config: &CaptureConfig, demand: ScreenCaptureDemand) -> u64 {
        {
            let mut config = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let granted_token = config.restore_token.take();
            config.clone_from(next_config);
            if config.restore_token.is_none() {
                config.restore_token = granted_token;
            }
        }
        *self
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = demand;
        self.generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }

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
        let config = self.config_snapshot();
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
        self.exact.replace_source(None);
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
        self.exact.replace_source(None);
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
        self.exact.replace_source(None);
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
        self.exact.replace_source(None);
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
    next_adoption_id: u64,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
}

impl WaylandScreenCaptureInput {
    /// Create a new Wayland screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        let capacity = ScreenAdmissionCapacity::new(
            config.analysis_memory_bytes,
            config.analysis_memory_bytes,
        );
        Self::with_admission_and_compute_capacity(
            config,
            ScreenByteAdmissionCoordinator::new(capacity),
            ScreenComputeCapacityPolicy::UNBOUNDED,
        )
    }

    /// Create a Wayland source inside an existing process-wide screen byte fence.
    #[must_use]
    pub fn with_admission_coordinator(
        config: CaptureConfig,
        admission_coordinator: ScreenByteAdmissionCoordinator,
    ) -> Self {
        Self::with_admission_and_compute_capacity(
            config,
            admission_coordinator,
            ScreenComputeCapacityPolicy::UNBOUNDED,
        )
    }

    /// Create a source with caller-calibrated compatibility and exact CPU fences.
    #[must_use]
    pub fn with_compute_capacity_policy(
        config: CaptureConfig,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> Self {
        let capacity = ScreenAdmissionCapacity::new(
            config.analysis_memory_bytes,
            config.analysis_memory_bytes,
        );
        Self::with_admission_and_compute_capacity(
            config,
            ScreenByteAdmissionCoordinator::new(capacity),
            compute_capacity_policy,
        )
    }

    /// Create a source with shared memory and caller-calibrated CPU fences.
    #[must_use]
    pub fn with_admission_and_compute_capacity(
        config: CaptureConfig,
        admission_coordinator: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> Self {
        Self {
            settings: Arc::new(SharedSettings {
                config: Mutex::new(config),
                demand: Mutex::new(ScreenCaptureDemand::Inactive),
                admission_coordinator,
                compute_capacity_policy,
                generation: AtomicU64::new(0),
                frame_generation: AtomicU64::new(0),
                topology_generation: AtomicU64::new(0),
                topology: Mutex::new(None),
                session_generation: AtomicU64::new(0),
                expected_epoch: Mutex::new(None),
                exact: WaylandExactPublicationShared::default(),
            }),
            running: false,
            capture_demand: ScreenCaptureDemand::Inactive,
            latest_snapshot: Arc::new(Mutex::new(None)),
            status_snapshot_generation: 0,
            worker: None,
            retiring_workers: Vec::new(),
            token_sink: None,
            next_adoption_id: 0,
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

    fn prepare_active_settings(
        &self,
        config: CaptureConfig,
        demand: ScreenCaptureDemand,
        format_changed: bool,
    ) -> anyhow::Result<PreparedWaylandSettings> {
        let requested_extent = demand
            .requested_extent()
            .context("active Wayland capture settings must carry an extent")?;
        let cadence = CaptureCadence::new(config.target_fps)?;
        let source = self.settings.exact.source();
        let acquisition_extent = source
            .as_ref()
            .map_or(requested_extent, |source| source.config.logical_extent());
        if source.is_some()
            && let Some(capacity) = self.settings.compute_capacity_policy.analysis()
        {
            ScreenAnalysisWorkPlan::try_new(acquisition_extent, requested_extent, &config)?
                .admit(capacity)?;
        }
        let mut analyzer = build_wayland_analyzer_for_extent(
            config.clone(),
            requested_extent,
            self.settings.admission_coordinator.clone(),
            self.settings.compute_capacity_policy,
        )?;
        if source.is_some() {
            analyzer.admit_frame_extent(acquisition_extent)?;
        }
        let pipewire_format = if format_changed {
            let callback_capacity = NegotiatedFormat {
                width: acquisition_extent.width(),
                height: acquisition_extent.height(),
                format: SpaVideoFormat::Rgba,
            }
            .byte_len()
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
            Some(PreparedPipeWireFormat {
                callback_buffers: DoubleBuffer::try_with_capacity_and_admission(
                    callback_capacity,
                    &self.settings.admission_coordinator,
                )?,
                format_bytes: build_format_params(config.target_fps, acquisition_extent)?,
                request: PipeWireFormatRequest::new_with_compute_policy(
                    acquisition_extent,
                    requested_extent,
                    &config,
                    self.settings.compute_capacity_policy,
                )?,
            })
        } else {
            None
        };
        Ok(PreparedWaylandSettings {
            config,
            cadence,
            demand,
            analyzer,
            pipewire_format,
        })
    }

    fn adopt_worker_settings(&mut self, prepared: PreparedWaylandSettings) -> anyhow::Result<()> {
        self.next_adoption_id = self.next_adoption_id.wrapping_add(1).max(1);
        let adoption_id = self.next_adoption_id;
        let worker = self
            .worker
            .as_ref()
            .ok_or_else(|| anyhow!("Wayland capture worker is unavailable for live adoption"))?;
        if worker.portal_pending.load(Ordering::SeqCst) {
            anyhow::bail!("Wayland capture worker cannot adopt settings while the portal is open");
        }
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (decision_tx, decision_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let authority = Arc::new(AdoptionAuthority::default());
        worker
            .command_tx
            .send(WorkerCommand::AdoptSettings {
                adoption_id,
                prepared,
                ready: ready_tx,
                decision: decision_rx,
                done: done_tx,
                authority: Arc::clone(&authority),
            })
            .map_err(|_| anyhow!("Wayland capture worker rejected prepared settings"))?;
        if let Err(error) = ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            authority.cancel();
            let _ = worker
                .command_tx
                .send(WorkerCommand::CancelAdoption { adoption_id });
            anyhow::bail!("Wayland capture worker adoption timed out: {error}");
        }
        if decision_tx.send(SettingsDecision::Commit).is_err() {
            authority.cancel();
            anyhow::bail!("Wayland capture worker exited before settings commit");
        }
        let cancellation_sent = Cell::new(false);
        match wait_for_adoption_result(
            &done_rx,
            FORMAT_ADOPTION_TIMEOUT,
            WORKER_STOP_TIMEOUT,
            &authority,
            || {
                cancellation_sent.set(self.worker.as_ref().is_some_and(|worker| {
                    worker
                        .command_tx
                        .send(WorkerCommand::CancelAdoption { adoption_id })
                        .is_ok()
                }));
            },
        ) {
            Ok(result) => result.map_err(anyhow::Error::msg),
            Err(error) => {
                self.shutdown_worker();
                let restart_error = self.restart_worker().err();
                let mut reason = match error {
                    AdoptionWaitError::Disconnected => {
                        "Wayland capture worker exited during settings commit".to_owned()
                    }
                    AdoptionWaitError::CancellationUnsettled(cancel_error) => format!(
                        "Wayland capture adoption was cancelled but did not settle: {cancel_error}"
                    ),
                };
                if !cancellation_sent.get() {
                    reason.push_str("; cancellation command was rejected");
                }
                if let Some(error) = restart_error {
                    write!(reason, "; failed to restart prior capture: {error}")
                        .expect("writing to a String cannot fail");
                }
                Err(anyhow!(reason))
            }
        }
    }

    /// Apply new capture settings to the running pipeline.
    ///
    /// Analysis settings and PipeWire format changes are prepared together,
    /// then adopted by both workers without interrupting the portal session.
    fn reconfigure(&mut self, config: CaptureConfig) -> anyhow::Result<()> {
        CaptureCadence::new(config.target_fps)?;
        let current = self.settings.config_snapshot();
        let reconnecting =
            self.running && self.capture_demand.is_active() && self.request_active_worker_demand();
        if reconnecting {
            let prepared = self.prepare_active_settings(config, self.capture_demand, false)?;
            let next = PreparedAnalysisSettings {
                config: prepared.config,
                cadence: prepared.cadence,
                demand: prepared.demand,
                analyzer: prepared.analyzer,
            };
            self.settings.commit_runtime(&next);
            return Ok(());
        }
        if current == config {
            return Ok(());
        }
        if self.capture_demand.is_active() {
            let format_changed = current.target_fps != config.target_fps;
            let prepared =
                self.prepare_active_settings(config, self.capture_demand, format_changed)?;
            if self.running {
                self.adopt_worker_settings(prepared)?;
            } else {
                let next = PreparedAnalysisSettings {
                    config: prepared.config,
                    cadence: prepared.cadence,
                    demand: prepared.demand,
                    analyzer: prepared.analyzer,
                };
                self.settings.commit_runtime(&next);
            }
            return Ok(());
        }
        self.settings.commit_values(&config, self.capture_demand);
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
        self.spawn_worker()?;
        self.send_worker_command(WorkerCommand::SetDemand(self.capture_demand))
    }

    fn set_capture_demand_state(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let previous = self.capture_demand;
        if previous == demand {
            if demand.is_active() && self.running {
                self.request_active_worker_demand();
            }
            if demand.is_active() && self.running && self.worker.is_none() {
                self.spawn_worker()?;
                self.send_worker_command(WorkerCommand::SetDemand(demand))?;
            }
            return Ok(());
        }

        if previous.is_active() && demand.is_active() {
            let config = self.settings.config_snapshot();
            let prepared = self.prepare_active_settings(config, demand, false)?;
            if self.running {
                self.adopt_worker_settings(prepared)?;
            } else {
                let next = PreparedAnalysisSettings {
                    config: prepared.config,
                    cadence: prepared.cadence,
                    demand: prepared.demand,
                    analyzer: prepared.analyzer,
                };
                self.settings.commit_runtime(&next);
            }
            self.capture_demand = demand;
            return Ok(());
        }

        let _admission = demand
            .requested_extent()
            .map(|requested_extent| -> anyhow::Result<_> {
                let config = self.settings.config_snapshot();
                let cadence = CaptureCadence::new(config.target_fps)?;
                let analyzer = ScreenCaptureInput::with_requested_extent(config, requested_extent)?;
                Ok((analyzer, cadence))
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
            self.spawn_worker()
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
                self.spawn_worker()
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

    fn request_active_worker_demand(&self) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        request_active_worker_demand(&worker.demand_state)
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        self.reap_workers(false);
        if self.worker.is_some() {
            return Ok(());
        }

        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let settings = Arc::clone(&self.settings);
        let token_sink = self.token_sink.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let demand_state = Arc::new(AtomicU64::new(initial_worker_demand(false)));
        // Born true: the worker is portal-bound from its first instruction,
        // and a shutdown landing before the thread even stores the flag must
        // detach rather than join into the picker freeze.
        let portal_pending = Arc::new(AtomicBool::new(true));
        let worker_flags = WorkerFlags {
            cancel: Arc::clone(&cancel),
            portal_pending: Arc::clone(&portal_pending),
            demand_state: Arc::clone(&demand_state),
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
            demand_state,
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
        let WorkerCommand::SetDemand(demand) = command else {
            anyhow::bail!("only demand commands use the restartable Wayland dispatch path");
        };
        let Some(worker) = &self.worker else {
            return Ok(());
        };

        set_worker_demand(&worker.demand_state, demand.is_active());
        if worker
            .command_tx
            .send(WorkerCommand::SetDemand(demand))
            .is_ok()
        {
            return Ok(());
        }

        warn!("Wayland screen capture worker is no longer accepting commands");
        self.shutdown_worker();

        if demand.is_active() {
            self.spawn_worker()?;
            let replacement_accepted = self.worker.as_ref().is_some_and(|worker| {
                set_worker_demand(&worker.demand_state, true);
                worker
                    .command_tx
                    .send(WorkerCommand::SetDemand(demand))
                    .is_ok()
            });
            if !replacement_accepted {
                self.shutdown_worker();
                anyhow::bail!("failed to restart Wayland screen capture worker");
            }
        }

        Ok(())
    }

    fn shutdown_worker(&mut self) {
        let Some(worker) = self.worker.take() else {
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
        for worker in self.retiring_workers.drain(..) {
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
        CaptureCadence::new(self.settings.config_snapshot().target_fps)?;

        if self.capture_demand.is_active() {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            if let Err(error) = self.spawn_worker().and_then(|()| {
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
        self.settings.exact.clear_owned_sources();

        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }
        self.settings.exact.replace_source(None);
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let worker_exited =
            self.observe_worker_exit(self.running && self.capture_demand.is_active());
        if !self.running || !self.capture_demand.is_active() {
            return Ok(InputData::None);
        }
        if worker_exited {
            self.spawn_worker()?;
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
                let cadence = CaptureCadence::new(self.settings.config_snapshot().target_fps)?;
                status.record_sample(
                    metadata.captured_at,
                    cadence.freshness_deadline(metadata.captured_at)?,
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

    fn screen_analysis_resource_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisResourcePlan>> {
        let Some(requested_extent) = demand.requested_extent() else {
            return Ok(None);
        };
        let config = self.settings.config_snapshot();
        Ok(Some(ScreenAnalysisResourcePlan::try_new_for_extent(
            config.grid_cols,
            config.grid_rows,
            config.target_fps,
            requested_extent,
            u64::MAX,
        )?))
    }

    fn screen_analysis_work_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisWorkPlan>> {
        let Some(requested_extent) = demand.requested_extent() else {
            return Ok(None);
        };
        let input_extent = self
            .settings
            .exact
            .source()
            .map_or(requested_extent, |source| source.config.logical_extent());
        let config = self.settings.config_snapshot();
        Ok(Some(ScreenAnalysisWorkPlan::try_new(
            input_extent,
            requested_extent,
            &config,
        )?))
    }

    fn screen_analysis_compute_capacity(&self) -> Option<ScreenAnalysisComputeCapacity> {
        self.settings.compute_capacity_policy.analysis()
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let previous = self.capture_demand;
        let active = demand.is_active();
        self.status.set_policy(true, true, active)?;
        if previous.is_active() != active {
            if !active {
                self.status_session.clear();
            }
            if active
                && self.running
                && let Some(session) = self.status.begin_session()?
            {
                self.status_session.store(session);
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

    fn set_screen_publication_hub(&mut self, hub: Arc<ScreenPublicationHub>) {
        *self
            .settings
            .exact
            .hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.settings
            .exact
            .resolution_revision
            .load(Ordering::Acquire)
    }

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        let Some(source) = self.settings.exact.source() else {
            return Ok(None);
        };
        if !source.matches_selector(demand.request().selector()) {
            return Ok(None);
        }
        let resolved = source.resolved(demand.request().selector().clone());
        Ok(Some(demand.resolve_with_color_capabilities(
            &resolved,
            ScreenColorTransformCapabilities::new(true, false, false, NonZeroU32::MIN),
        )?))
    }

    fn owns_screen_publication_source(&self, source_id: &CaptureSourceId) -> bool {
        self.settings.exact.owns_source(source_id)
    }

    fn begin_screen_publication_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        let worker = self.worker.as_ref().ok_or_else(|| {
            anyhow!("Wayland capture worker is unavailable for exact publication preparation")
        })?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let (completion_tx, completion_rx) = oneshot::channel();
        worker
            .command_tx
            .send(WorkerCommand::PrepareExact {
                ticket,
                cancelled: Arc::clone(&cancelled),
                completion: completion_tx,
            })
            .map_err(|_| {
                anyhow!("Wayland capture worker rejected exact publication preparation")
            })?;
        let abort_tx = worker.command_tx.clone();
        Ok(ScreenWorkerPreparation::with_abort(
            async move {
                completion_rx.await.map_err(|_| {
                    anyhow!("Wayland capture worker exited during exact publication preparation")
                })?
            },
            move || {
                cancelled.store(true, Ordering::Release);
                let _ = abort_tx.send(WorkerCommand::ReapExact { completion: None });
            },
        ))
    }

    fn begin_screen_publication_retirement(&mut self) -> Option<ScreenWorkerRetirement> {
        let worker = self.worker.as_ref()?;
        let (completion_tx, completion_rx) = oneshot::channel();
        if worker
            .command_tx
            .send(WorkerCommand::ReapExact {
                completion: Some(completion_tx),
            })
            .is_err()
        {
            return Some(ScreenWorkerRetirement::new(async {
                Err(anyhow!(
                    "Wayland capture worker rejected exact publication retirement"
                ))
            }));
        }
        Some(ScreenWorkerRetirement::new(async move {
            completion_rx.await.map_err(|_| {
                anyhow!("Wayland capture worker exited during exact publication retirement")
            })?
        }))
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
    demand_state: Arc<AtomicU64>,
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
    demand_state: Arc<AtomicU64>,
}

enum WorkerCommand {
    SetDemand(ScreenCaptureDemand),
    PrepareExact {
        ticket: ScreenWorkerPreparationTicket,
        cancelled: Arc<AtomicBool>,
        completion: oneshot::Sender<anyhow::Result<ScreenPreparedWorkerToken>>,
    },
    ReapExact {
        completion: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
    AdoptSettings {
        adoption_id: u64,
        prepared: PreparedWaylandSettings,
        ready: mpsc::SyncSender<()>,
        decision: mpsc::Receiver<SettingsDecision>,
        done: mpsc::SyncSender<Result<(), String>>,
        authority: Arc<AdoptionAuthority>,
    },
    CancelAdoption {
        adoption_id: u64,
    },
    Stop,
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
    decoding_enabled: Arc<AtomicBool>,
    admission_coordinator: ScreenByteAdmissionCoordinator,
}

impl WaylandCaptureUserData {
    #[cfg(test)]
    fn new(exchange: Arc<AnalysisExchange>, metrics: Arc<CaptureCallbackMetrics>) -> Self {
        Self {
            format: spa::param::video::VideoInfoRaw::default(),
            negotiated: None,
            buffers: DoubleBuffer::try_with_capacity(0)
                .expect("empty callback planes require no pixel allocation"),
            exchange,
            metrics,
            decoding_enabled: Arc::new(AtomicBool::new(false)),
            admission_coordinator: ScreenByteAdmissionCoordinator::default(),
        }
    }

    fn with_buffers(
        exchange: Arc<AnalysisExchange>,
        metrics: Arc<CaptureCallbackMetrics>,
        buffers: DoubleBuffer,
        decoding_enabled: Arc<AtomicBool>,
        admission_coordinator: ScreenByteAdmissionCoordinator,
    ) -> Self {
        Self {
            format: spa::param::video::VideoInfoRaw::default(),
            negotiated: None,
            buffers,
            exchange,
            metrics,
            decoding_enabled,
            admission_coordinator,
        }
    }

    fn set_negotiated_format(&mut self, format: NegotiatedFormat) -> Result<(), CaptureFrameError> {
        let Some(capacity) = format.byte_len() else {
            return Err(CaptureFrameError::StorageSizeOverflow);
        };
        if self.buffers.capacity() < capacity {
            self.buffers = DoubleBuffer::try_with_capacity_and_admission(
                capacity,
                &self.admission_coordinator,
            )?;
        }
        self.negotiated = Some(format);
        Ok(())
    }

    fn install_prepared_format(&mut self, format: NegotiatedFormat, buffers: DoubleBuffer) {
        self.buffers = buffers;
        self.negotiated = Some(format);
        self.decoding_enabled.store(true, Ordering::Release);
    }

    fn activate_negotiated_format(
        &mut self,
        format: NegotiatedFormat,
    ) -> Result<(), CaptureFrameError> {
        self.set_negotiated_format(format)?;
        self.decoding_enabled.store(true, Ordering::Release);
        Ok(())
    }

    fn fence_decoding(&mut self) {
        self.decoding_enabled.store(false, Ordering::Release);
        self.negotiated = None;
        self.exchange.discard_latest_frame();
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

#[derive(Clone, Copy, Debug)]
struct NegotiatedPipeWireFormat {
    frame: NegotiatedFormat,
    framerate: spa::utils::Fraction,
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
    adoption: Option<AnalysisAdoption>,
    exact_commands: VecDeque<AnalysisExactCommand>,
    stopped: bool,
}

struct AnalysisAdoption {
    prepared: PreparedAnalysisSettings,
    ready: mpsc::SyncSender<()>,
    decision: mpsc::Receiver<SettingsDecision>,
    done: mpsc::SyncSender<bool>,
    authority: Arc<AdoptionAuthority>,
    finalize_authority: bool,
}

enum AnalysisEvent {
    Frame(DecodedChunk),
    Adoption(AnalysisAdoption),
    Exact(AnalysisExactCommand),
}

enum AnalysisExactCommand {
    Prepare {
        ticket: ScreenWorkerPreparationTicket,
        cancelled: Arc<AtomicBool>,
        completion: oneshot::Sender<anyhow::Result<ScreenPreparedWorkerToken>>,
    },
    Reap {
        completion: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
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

    fn prepare_adoption(&self, adoption: AnalysisAdoption) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped {
            return Err("Wayland analysis worker is stopped".to_owned());
        }
        if state.adoption.is_some() {
            return Err("Wayland analysis worker already has a pending adoption".to_owned());
        }
        state.adoption = Some(adoption);
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    fn send_exact(&self, command: AnalysisExactCommand) -> Result<(), Box<AnalysisExactCommand>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped {
            return Err(Box::new(command));
        }
        state.exact_commands.push_back(command);
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    fn discard_latest_frame(&self) {
        let discarded = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take();
        drop(discarded);
    }

    fn wait_for_event(&self, deadline: Instant, cancel: &AtomicBool) -> Option<AnalysisEvent> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.stopped || cancel.load(Ordering::Acquire) {
                return None;
            }
            if let Some(adoption) = state.adoption.take() {
                return Some(AnalysisEvent::Adoption(adoption));
            }
            if let Some(command) = state.exact_commands.pop_front() {
                return Some(AnalysisEvent::Exact(command));
            }
            let now = Instant::now();
            if now >= deadline
                && let Some(frame) = state.latest.take()
            {
                return Some(AnalysisEvent::Frame(frame));
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
        let (discarded_frame, discarded_adoption, discarded_exact) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.stopped = true;
            (
                state.latest.take(),
                state.adoption.take(),
                std::mem::take(&mut state.exact_commands),
            )
        };
        drop(discarded_frame);
        drop(discarded_adoption);
        drop(discarded_exact);
        self.wake.notify_all();
    }
}

struct WaylandExactRuntime {
    source: WaylandPublicationSource,
    binding: ScreenWorkerBinding,
    _lifetimes: Box<[ScreenResourceLifetime]>,
    fanout_candidate: Option<PreparedCpuPublicationFanoutCandidate>,
    fanout: Option<PreparedCpuPublicationFanout>,
}

type WaylandExactRuntimes = ExactBoxList<WaylandExactRuntime>;

impl WaylandExactRuntime {
    fn bind_if_current(&mut self, hub: &ScreenPublicationHub) -> anyhow::Result<()> {
        if self.fanout.is_some() {
            return Ok(());
        }
        let authority = hub.committed_state();
        if !authority.owns_runtime_binding(&self.binding) {
            return Ok(());
        }
        match self.binding.state() {
            ScreenWorkerBindingState::Active | ScreenWorkerBindingState::Retired => {}
            ScreenWorkerBindingState::Prepared | ScreenWorkerBindingState::Armed => return Ok(()),
            ScreenWorkerBindingState::Aborted => {
                anyhow::bail!("Wayland exact runtime binding was aborted after commit")
            }
        }
        let candidate = self
            .fanout_candidate
            .take()
            .ok_or_else(|| anyhow!("Wayland CPU fanout candidate was already consumed"))?;
        self.fanout = Some(candidate.bind(&authority, &self.binding)?);
        Ok(())
    }
}

fn build_wayland_analyzer_for_extent(
    config: CaptureConfig,
    requested_extent: PixelExtent,
    admission_coordinator: ScreenByteAdmissionCoordinator,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
) -> Result<ScreenCaptureInput, ScreenAnalysisAdmissionError> {
    match compute_capacity_policy.analysis() {
        Some(capacity) => ScreenCaptureInput::with_requested_extent_admission_and_compute_capacity(
            config,
            requested_extent,
            admission_coordinator,
            capacity,
        ),
        None => ScreenCaptureInput::with_requested_extent_and_admission(
            config,
            requested_extent,
            admission_coordinator,
        ),
    }
}

fn checked_wayland_metadata_bytes<T>(count: usize, resource: &str) -> anyhow::Result<u64> {
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(std::mem::size_of::<T>())
                .ok()
                .and_then(|size| count.checked_mul(size))
        })
        .ok_or_else(|| anyhow!("Wayland exact {resource} metadata accounting overflow"))
}

fn preflight_wayland_scope_bytes(
    ledger: &mut ScreenWorkerExactLedgerBuilder,
    minimum_remaining: &mut u64,
    bytes: u64,
) -> anyhow::Result<()> {
    let modeled = bytes.min(*minimum_remaining);
    *minimum_remaining -= modeled;
    let additional = bytes - modeled;
    if additional > 0 {
        ledger.preflight_additional_bytes(additional)?;
    }
    Ok(())
}

fn prepare_wayland_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    source: Option<&WaylandPublicationSource>,
    exact: &WaylandExactPublicationShared,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
) -> anyhow::Result<(
    ScreenPreparedWorkerToken,
    Option<(WaylandExactRuntime, WaylandOwnedSource)>,
)> {
    let candidate = ticket.candidate_plan().clone();
    let has_source_branches = candidate
        .branches()
        .iter()
        .any(|branch| branch.descriptor().source_epoch().source_id == *ticket.source_id());
    if !has_source_branches {
        let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
        let resource_count = ledger.ticket().required_minimums().len();
        for index in 0..resource_count {
            let (name, bytes) = {
                let minimum = &ledger.ticket().required_minimums()[index];
                (Arc::clone(minimum.name()), minimum.minimum_bytes())
            };
            ledger.report(&name, bytes)?;
        }
        let (token, _) = ledger.finish()?.into_parts();
        return Ok((token, None));
    }

    let source = source
        .filter(|source| &source.epoch.source_id == ticket.source_id())
        .ok_or_else(|| anyhow!("Wayland exact publication source changed before preparation"))?;
    let worker_count = exact.cpu_worker_count();
    let compute_plan =
        CpuExactReductionWorkPlan::try_for_source(&candidate, ticket.source_id(), |_| true)?;
    let capacity = compute_capacity_policy.exact(worker_count);
    let compute_plan = match capacity {
        Some(capacity) => compute_plan.admit(capacity)?,
        None => compute_plan,
    };
    debug!(
        cpu_reductions = compute_plan.cpu_reduction_count(),
        weighted_work_units_per_second = compute_plan.weighted_work_units_per_second(),
        workers = worker_count.get(),
        capacity_enforced = capacity.is_some(),
        "planned Wayland exact CPU reduction compute"
    );

    let resolved_source =
        source.resolved(ScreenSourceSelector::Exact(source.epoch.source_id.clone()));
    let executor = exact.cpu_executor()?;
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
    let mut processing_minimum_remaining = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::ProcessingProfileState)
        .map_or(0, ScreenRequiredResourceMinimum::minimum_bytes);
    let mut worker_minimum_remaining = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::WorkerAdditional)
        .map_or(0, ScreenRequiredResourceMinimum::minimum_bytes);
    let plane_minimum_bytes = ledger
        .ticket()
        .required_minimums()
        .iter()
        .filter(|minimum| minimum.resource() == ScreenResourceKind::PhysicalPlane)
        .try_fold(0_u64, |total, minimum| {
            total
                .checked_add(minimum.minimum_bytes())
                .ok_or_else(|| anyhow!("Wayland exact physical-plane accounting overflow"))
        })?;
    let runtime_node_bytes =
        checked_wayland_metadata_bytes::<ExactBoxNode<WaylandExactRuntime>>(1, "runtime node")?
            .checked_add(checked_wayland_metadata_bytes::<
                ExactBoxNode<WaylandOwnedSource>,
            >(1, "owned source node")?)
            .ok_or_else(|| anyhow!("Wayland exact runtime node accounting overflow"))?;
    preflight_wayland_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        runtime_node_bytes,
    )?;

    let batch_quote = executor.batch_allocation_quote(&resolved_source, &candidate)?;
    preflight_wayland_scope_bytes(&mut ledger, &mut processing_minimum_remaining, batch_quote)?;
    let batch = executor.prepare_batch(&resolved_source, &candidate)?;
    let workspace_quote = batch.materialization_workspace_allocation_quote(&candidate)?;
    let workspace_additional_bytes = workspace_quote
        .checked_sub(plane_minimum_bytes)
        .ok_or_else(|| anyhow!("Wayland workspace quote understates physical-plane minima"))?;
    preflight_wayland_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        workspace_additional_bytes,
    )?;
    let workspace = batch.prepare_materialization_workspace(&candidate)?;
    let workspace_bytes = workspace.allocation_byte_len();
    let fanout_quote =
        PreparedCpuPublicationFanout::candidate_allocation_quote(&batch, &workspace, &candidate)?;
    let fanout_additional_bytes = fanout_quote
        .checked_sub(batch_quote)
        .ok_or_else(|| anyhow!("Wayland fanout quote understates retained batch backing"))?;
    preflight_wayland_scope_bytes(
        &mut ledger,
        &mut processing_minimum_remaining,
        fanout_additional_bytes,
    )?;
    let fanout_candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor, &batch, workspace, &candidate,
    )?;
    let fanout_bytes = fanout_candidate.allocation_byte_len();
    let processing_scope = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::ProcessingProfileState)
        .map(|minimum| Arc::clone(minimum.name()));
    if fanout_bytes > 0 && processing_scope.is_none() {
        ledger.report_scoped("wayland-cpu-fanout", "worker-runtime-total", fanout_bytes)?;
    }
    let expected_lifetime_count = ledger.prospective_resource_count()?;
    let lifetime_metadata_bytes = checked_wayland_metadata_bytes::<ScreenResourceLifetime>(
        expected_lifetime_count,
        "runtime lifetimes",
    )?;
    preflight_wayland_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        lifetime_metadata_bytes,
    )?;
    let worker_metadata_bytes = workspace_bytes
        .checked_sub(plane_minimum_bytes)
        .ok_or_else(|| anyhow!("Wayland workspace accounting understates physical-plane minima"))?
        .checked_add(runtime_node_bytes)
        .and_then(|bytes| bytes.checked_add(lifetime_metadata_bytes))
        .ok_or_else(|| anyhow!("Wayland exact worker accounting overflow"))?;
    let resource_count = ledger.ticket().required_minimums().len();
    for index in 0..resource_count {
        let (name, resource, minimum) = {
            let minimum = &ledger.ticket().required_minimums()[index];
            (
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        };
        let actual = match resource {
            ScreenResourceKind::ProcessingProfileState
                if processing_scope.as_ref() == Some(&name) =>
            {
                fanout_bytes.max(minimum)
            }
            ScreenResourceKind::WorkerAdditional => worker_metadata_bytes.max(minimum),
            _ => minimum,
        };
        ledger.report(&name, actual)?;
    }
    let exact = ledger.finish()?;
    if exact.lifetimes().len() != expected_lifetime_count {
        anyhow::bail!("Wayland exact lifetime metadata accounting changed during preparation");
    }
    let binding = exact.token().binding().clone();
    let (token, lifetimes) = exact.into_parts();
    let runtime_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "worker-runtime-total")
        .cloned()
        .ok_or_else(|| anyhow!("Wayland worker runtime lifetime is missing from exact ledger"))?;
    Ok((
        token,
        Some((
            WaylandExactRuntime {
                source: source.clone(),
                binding: binding.clone(),
                _lifetimes: lifetimes,
                fanout_candidate: Some(fanout_candidate),
                fanout: None,
            },
            WaylandOwnedSource {
                source_id: source.epoch.source_id.clone(),
                session_generation: source.epoch.session_generation,
                binding,
                _runtime_lifetime: runtime_lifetime,
            },
        )),
    ))
}

fn reap_wayland_exact_runtimes(
    runtimes: &mut WaylandExactRuntimes,
    exact: &WaylandExactPublicationShared,
) {
    exact.reap_owned_sources();
    let authority = exact.hub().map(|hub| hub.committed_state());
    runtimes.retain(|runtime| {
        authority
            .as_ref()
            .is_some_and(|authority| authority.owns_runtime_binding(&runtime.binding))
    });
}

fn bind_current_wayland_exact_runtime<'a>(
    runtimes: &'a mut WaylandExactRuntimes,
    source: &WaylandPublicationSource,
    hub: &ScreenPublicationHub,
) -> anyhow::Result<Option<&'a mut WaylandExactRuntime>> {
    let authority = hub.committed_state();
    let Some(current_binding) = authority.runtime_binding(&source.epoch.source_id) else {
        return Ok(None);
    };
    let runtime = runtimes
        .iter_mut()
        .find(|runtime| runtime.source == *source && runtime.binding.is_same(current_binding));
    let Some(runtime) = runtime else {
        return Ok(None);
    };
    runtime.bind_if_current(hub)?;
    Ok(runtime.fanout.is_some().then_some(runtime))
}

struct WaylandAnalysisState {
    analyzer: ScreenCaptureInput,
    cadence: CaptureCadence,
    pacer: CapturePacer,
    next_analysis_at: Instant,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    plane_pool: CapturePlanePool,
    settings: Arc<SharedSettings>,
    applied_generation: u64,
    applied_demand: ScreenCaptureDemand,
    source: WaylandSourceMetadata,
    sequence: u64,
    exact_runtimes: WaylandExactRuntimes,
}

impl WaylandAnalysisState {
    fn new(
        settings: Arc<SharedSettings>,
        latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
        source: WaylandSourceMetadata,
        config: CaptureConfig,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Self> {
        let applied_generation = settings.generation.load(Ordering::Acquire);
        let requested_extent = demand
            .requested_extent()
            .expect("an active Wayland analysis worker carries an extent");
        let source_extent = source.signature.logical_extent.unwrap_or(requested_extent);
        let cadence = CaptureCadence::new(config.target_fps)?;
        if let Some(capacity) = settings.compute_capacity_policy.analysis() {
            ScreenAnalysisWorkPlan::try_new(source_extent, requested_extent, &config)?
                .admit(capacity)?;
        }
        let mut analyzer = build_wayland_analyzer_for_extent(
            config,
            requested_extent,
            settings.admission_coordinator.clone(),
            settings.compute_capacity_policy,
        )?;
        analyzer.admit_frame_extent(source_extent)?;
        analyzer.start()?;

        Ok(Self {
            analyzer,
            cadence,
            pacer: cadence.pacer(),
            next_analysis_at: Instant::now(),
            latest_snapshot,
            plane_pool: CapturePlanePool::with_admission_coordinator(
                settings.admission_coordinator.clone(),
            ),
            settings,
            applied_generation,
            applied_demand: demand,
            source,
            sequence: 0,
            exact_runtimes: WaylandExactRuntimes::default(),
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
        let next_cadence = match CaptureCadence::new(runtime.config.target_fps) {
            Ok(cadence) => cadence,
            Err(error) => {
                warn!(%error, generation, "Retaining prior Wayland capture cadence");
                return true;
            }
        };
        if let Some(requested_extent) = runtime.demand.requested_extent()
            && let Err(error) = self.analyzer.set_requested_extent(requested_extent)
        {
            warn!(%error, generation, previous_demand = ?self.applied_demand, next_demand = ?runtime.demand, "Retaining prior Wayland screen analysis settings");
            return true;
        }
        if let Err(error) = self.analyzer.apply_settings(runtime.config) {
            warn!(%error, generation, "Retaining prior Wayland capture settings");
            return true;
        }
        self.cadence = next_cadence;
        self.pacer = next_cadence.pacer();
        self.next_analysis_at = Instant::now();
        self.applied_demand = runtime.demand;
        self.applied_generation = generation;
        debug!(generation, "Applied live screen capture settings");
        true
    }

    fn adopt_settings(&mut self, adoption: AnalysisAdoption) {
        if adoption.ready.send(()).is_err()
            || !matches!(
                adoption
                    .decision
                    .recv_timeout(FORMAT_ADOPTION_TIMEOUT + WORKER_STOP_TIMEOUT),
                Ok(SettingsDecision::Commit)
            )
        {
            return;
        }
        let PreparedAnalysisSettings {
            mut config,
            cadence,
            demand,
            analyzer,
        } = adoption.prepared;
        let mut current_config = self
            .settings
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut current_demand = self
            .settings
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut latest_snapshot = self
            .latest_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = Cell::new(0);
        let committed = adoption.authority.claim_commit()
            && commit_claimed(&adoption.authority, adoption.finalize_authority, || {
                let granted_token = current_config.restore_token.take();
                if config.restore_token.is_none() {
                    config.restore_token = granted_token;
                }
                *current_config = config;
                *current_demand = demand;
                let committed_generation = self
                    .settings
                    .generation
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                generation.set(committed_generation);
                self.analyzer = analyzer;
                self.cadence = cadence;
                self.pacer = cadence.pacer();
                self.next_analysis_at = Instant::now();
                self.applied_demand = demand;
                self.applied_generation = committed_generation;
                fence_previous_publication(&mut latest_snapshot);
            });
        let _ = adoption.done.send(committed);
        if !committed {
            return;
        }
        debug!(
            generation = generation.get(),
            "Adopted prepared Wayland screen analysis settings"
        );
    }

    fn handle_exact_command(&mut self, command: AnalysisExactCommand) {
        match command {
            AnalysisExactCommand::Prepare {
                ticket,
                cancelled,
                completion,
            } => {
                if cancelled.load(Ordering::Acquire) {
                    let _ = completion.send(Err(anyhow!(
                        "Wayland exact publication preparation was cancelled"
                    )));
                    return;
                }
                let source = self.settings.exact.source();
                let result = prepare_wayland_exact_runtime(
                    ticket,
                    source.as_ref(),
                    &self.settings.exact,
                    self.settings.compute_capacity_policy,
                );
                match result {
                    Ok((token, runtime)) if !cancelled.load(Ordering::Acquire) => {
                        if let Some((runtime, owned_source)) = runtime {
                            let runtime = WaylandExactRuntimes::boxed_node(runtime);
                            let owned_source = ExactBoxList::boxed_node(owned_source);
                            self.settings.exact.register_owned_source(owned_source);
                            self.exact_runtimes.push_boxed(runtime);
                        }
                        if completion.send(Ok(token)).is_err() {
                            reap_wayland_exact_runtimes(
                                &mut self.exact_runtimes,
                                &self.settings.exact,
                            );
                        }
                    }
                    Ok((_token, _runtime)) => {
                        let _ = completion.send(Err(anyhow!(
                            "Wayland exact publication preparation was cancelled"
                        )));
                    }
                    Err(error) => {
                        let _ = completion.send(Err(error));
                    }
                }
            }
            AnalysisExactCommand::Reap { completion } => {
                reap_wayland_exact_runtimes(&mut self.exact_runtimes, &self.settings.exact);
                if let Some(completion) = completion {
                    let _ = completion.send(Ok(()));
                }
            }
        }
    }

    fn publish_exact(&mut self, frame: &CaptureFrame<RawCaptureSurface>) -> anyhow::Result<()> {
        let Some(hub) = self.settings.exact.hub() else {
            return Ok(());
        };
        let Some(source) = self.settings.exact.source() else {
            return Ok(());
        };
        let Some(runtime) =
            bind_current_wayland_exact_runtime(&mut self.exact_runtimes, &source, &hub)?
        else {
            return Ok(());
        };
        runtime
            .fanout
            .as_mut()
            .ok_or_else(|| anyhow!("Wayland exact CPU fanout is not bound"))?
            .publish_due(
                &hub,
                Some(frame),
                Instant::now(),
                ScreenPublicationHealth::Healthy,
            )?;
        Ok(())
    }

    fn capture_frame(
        &mut self,
        captured_at: Instant,
        width: u32,
        height: u32,
        crop: Option<PixelRect>,
        transform: CaptureRotation,
        plane: PooledCapturePlane,
        colorimetry: CaptureColorimetry,
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
        let freshness_deadline = self.cadence.freshness_deadline(captured_at)?;
        let geometry = CaptureGeometry::new(
            self.source.signature.origin,
            topology.native_extent,
            storage_extent,
            transform,
            crop,
            self.source.source_scale(topology.native_extent.width()),
        )?;
        let epoch = CaptureEpoch {
            source_id: self.source.signature.source_id.clone(),
            topology_generation: topology.generation,
            session_generation: self.source.session_generation,
        };
        let publication_source = WaylandPublicationSource {
            epoch: epoch.clone(),
            config: ResolvedScreenSourceConfig::new(
                geometry,
                self.source
                    .signature
                    .logical_extent
                    .unwrap_or(topology.native_extent),
                ScreenSourceReflection::None,
                CapturePixelFormat::Rgba8,
                colorimetry,
                ScreenBackendResourceIdentity::new(
                    ScreenCaptureBackend::WaylandPipeWire,
                    ScreenResourceApi::Cpu,
                    self.source.session_generation,
                    topology.generation,
                ),
            ),
        };
        let frame = CaptureFrame::new(
            CaptureFrameMetadata {
                source_id: epoch.source_id,
                topology_generation: epoch.topology_generation,
                session_generation: epoch.session_generation,
                sequence: self.sequence,
                captured_at,
                fresh_until: freshness_deadline,
                geometry,
                colorimetry,
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
        self.settings.exact.replace_source(Some(publication_source));
        Ok(frame)
    }

    fn advance_deadline(&mut self, now: Instant) -> anyhow::Result<()> {
        self.next_analysis_at = self.pacer.advance_deadline(self.next_analysis_at, now)?;
        Ok(())
    }
}

impl Drop for WaylandAnalysisState {
    fn drop(&mut self) {
        self.settings
            .exact
            .clear_owned_sources_for_session(self.source.session_generation);
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
    status_writer: Option<SourceSessionWriter>,
) {
    let mut state =
        match WaylandAnalysisState::new(settings, latest_snapshot, source, config, demand) {
            Ok(state) => state,
            Err(error) => {
                warn!(%error, "Failed to admit Wayland screen analysis extent");
                return;
            }
        };
    let mut analysis_failure_latched = false;
    let mut exact_failure_latched = false;
    while let Some(event) = exchange.wait_for_event(state.next_analysis_at, cancel) {
        let decoded = match event {
            AnalysisEvent::Frame(decoded) => decoded,
            AnalysisEvent::Adoption(adoption) => {
                state.adopt_settings(adoption);
                continue;
            }
            AnalysisEvent::Exact(command) => {
                state.handle_exact_command(command);
                continue;
            }
        };
        if !state.sync_settings(cancel) {
            return;
        }
        if let Err(error) = state.advance_deadline(Instant::now()) {
            warn!(%error, "Wayland screen analysis cadence deadline is unrepresentable");
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

        let frame = match state.capture_frame(
            captured_at,
            width,
            height,
            crop,
            transform,
            plane.freeze(),
            CaptureColorimetry::unknown(),
        ) {
            Ok(frame) => frame,
            Err(error) => {
                latch_wayland_analysis_failure(
                    status_writer.as_ref(),
                    &mut analysis_failure_latched,
                    &error,
                );
                continue;
            }
        };
        let exact_failure = match state.publish_exact(&frame) {
            Ok(()) => {
                exact_failure_latched = false;
                None
            }
            Err(error) => {
                if !exact_failure_latched {
                    warn!(%error, "Wayland exact publication rejected a frame");
                    exact_failure_latched = true;
                }
                Some(error)
            }
        };
        let analysis = match analyze_screen_frame(&mut state.analyzer, frame) {
            Ok(analysis) => analysis,
            Err(error) => {
                latch_wayland_analysis_failure(
                    status_writer.as_ref(),
                    &mut analysis_failure_latched,
                    &error,
                );
                continue;
            }
        };
        let metadata = analysis.geometry_frame().metadata();
        let captured_at = metadata.captured_at;
        let fresh_until = metadata.fresh_until;
        if state
            .settings
            .publish_snapshot(&state.latest_snapshot, analysis)
        {
            analysis_failure_latched = false;
            if let Some(status) = status_writer.as_ref() {
                if let Some(error) = exact_failure.as_ref() {
                    let _ = status.record_degraded_sample(
                        captured_at,
                        fresh_until,
                        1,
                        SourceIssue::new(
                            "wayland_exact_publication_rejected",
                            error.to_string(),
                            true,
                        ),
                    );
                } else {
                    let _ = status.record_sample(captured_at, fresh_until, 1);
                }
            }
        }
    }
}

fn latch_wayland_analysis_failure(
    status_writer: Option<&SourceSessionWriter>,
    latched: &mut bool,
    error: &anyhow::Error,
) {
    if *latched {
        return;
    }
    warn!(%error, "Wayland screen analysis rejected a frame; retaining last good publication");
    if let Some(status) = status_writer {
        status.degraded(SourceIssue::new(
            "wayland_screen_analysis_rejected",
            error.to_string(),
            true,
        ));
    }
    *latched = true;
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
        let session_demand_epoch = worker_demand_epoch(flags.demand_state.load(Ordering::Acquire));

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

        if restore_token != startup.config.restore_token
            && !settings.persist_restore_token_for_session(
                session_generation,
                &flags.cancel,
                restore_token,
                token_sink.as_ref(),
            )
        {
            return;
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
            status_writer.clone(),
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
            Ok(PipeWireLoopExit::Unavailable(reason)) => {
                let parking = park_unavailable_worker(&flags.demand_state, session_demand_epoch);
                warn!(%reason, "Wayland screen capture format is unavailable");
                if let Some(status) = status_writer.as_ref() {
                    settings.publish_status_for_session(
                        session_generation,
                        &flags.cancel,
                        status,
                        |status| {
                            status.unavailable(
                                SourceIssue::new(
                                    "wayland_exact_format_unavailable",
                                    reason,
                                    true,
                                )
                                .with_remediation(
                                    "select a capture extent and cadence supported by the PipeWire source",
                                ),
                            )
                        },
                    );
                }
                debug!(?parking, "Settled unavailable Wayland capture demand");
                continue;
            }
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
        if worker_demanded(&flags.demand_state) {
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
        && worker_demanded(&flags.demand_state)
        && Instant::now() < deadline
    {
        thread::park_timeout(
            WORKER_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    !flags.cancel.load(Ordering::Acquire) && worker_demanded(&flags.demand_state)
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
    while !flags.cancel.load(Ordering::Acquire) && worker_demanded(&flags.demand_state) {
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
    Unavailable(String),
}

fn unavailable_format_outcome(current_acknowledged: bool, reason: String) -> PipeWireLoopExit {
    let phase = if current_acknowledged {
        "authoritative"
    } else {
        "initial"
    };
    PipeWireLoopExit::Unavailable(format!(
        "PipeWire rejected the {phase} exact screen format: {reason}"
    ))
}

fn request_pipewire_restoration(
    stream: &pw::stream::Stream,
    format_state: &Mutex<PipeWireFormatState>,
    pending: PendingPipeWireAdoption,
    reason: String,
) -> Result<(), String> {
    if !pending.authority.is_cancelled() && !pending.authority.cancel() {
        return Err("PipeWire adoption claimed commit authority before restoration".to_owned());
    }
    let restore = format_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .begin_restoring(pending, reason);
    update_pipewire_format(stream, &restore)
        .map_err(|error| format!("failed to request prior PipeWire format: {error}"))
}

fn reject_pipewire_format(
    stream: &pw::stream::Stream,
    user_data: &mut WaylandCaptureUserData,
    format_state: &Mutex<PipeWireFormatState>,
    reason: String,
) -> Option<PipeWireLoopExit> {
    user_data.fence_decoding();
    let (pending, restoring, current_acknowledged) = {
        let mut state = format_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            state.pending.take(),
            state.restoring.is_some(),
            state.current_acknowledged,
        )
    };
    if let Some(pending) = pending {
        return request_pipewire_restoration(stream, format_state, pending, reason)
            .err()
            .map(PipeWireLoopExit::Terminal);
    }
    if restoring {
        debug!(%reason, "Ignored stale PipeWire acknowledgment while restoring format");
        return None;
    }
    Some(unavailable_format_outcome(current_acknowledged, reason))
}

fn settle_pipewire_restoration(
    user_data: &mut WaylandCaptureUserData,
    format_state: &Mutex<PipeWireFormatState>,
    frame: NegotiatedFormat,
) -> Result<(), String> {
    let restoring = format_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .settle_restoration()
        .ok_or_else(|| "PipeWire restoration acknowledgment had no owner".to_owned())?;
    user_data
        .activate_negotiated_format(frame)
        .map_err(|error| format!("failed to reactivate prior PipeWire format: {error}"))?;
    let PendingPipeWireAdoption { done, .. } = restoring.pending;
    let _ = done.send(Err(restoring.failure));
    Ok(())
}

fn fence_previous_publication(latest_snapshot: &mut Option<CapturedScreenSnapshot>) {
    *latest_snapshot = None;
}

fn terminate_pipewire_loop(
    mainloop: &pw::main_loop::MainLoopRc,
    loop_exit: &Mutex<Option<PipeWireLoopExit>>,
    outcome: PipeWireLoopExit,
) {
    let mut exit = loop_exit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if exit.is_none() {
        *exit = Some(outcome);
        mainloop.quit();
    }
}

fn commit_pending_pipewire_adoption(
    stream: &pw::stream::Stream,
    user_data: &mut WaylandCaptureUserData,
    format_state: &Mutex<PipeWireFormatState>,
    negotiated: NegotiatedPipeWireFormat,
) -> Result<(), String> {
    let Some(pending) = format_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pending
        .take()
    else {
        return Ok(());
    };
    if pending.authority.is_cancelled() {
        user_data.fence_decoding();
        return request_pipewire_restoration(
            stream,
            format_state,
            pending,
            "PipeWire format adoption timed out".to_owned(),
        );
    }
    let Some(required_capacity) = negotiated.frame.byte_len() else {
        user_data.fence_decoding();
        return request_pipewire_restoration(
            stream,
            format_state,
            pending,
            "negotiated PipeWire extent overflowed callback storage".to_owned(),
        );
    };
    if pending.callback_buffers.capacity() < required_capacity {
        user_data.fence_decoding();
        return request_pipewire_restoration(
            stream,
            format_state,
            pending,
            "prepared PipeWire callback storage did not fit the acknowledged format".to_owned(),
        );
    }

    let analysis_committed = if pending
        .analysis_decision
        .send(SettingsDecision::Commit)
        .is_err()
    {
        pending.authority.cancel();
        false
    } else {
        match pending.analysis_done.recv_timeout(WORKER_STOP_TIMEOUT) {
            Ok(committed) => committed,
            Err(_) => {
                pending.authority.cancel_or_wait_for_analysis() == AdoptionSettlement::Committed
            }
        }
    };
    if !analysis_committed {
        user_data.fence_decoding();
        return request_pipewire_restoration(
            stream,
            format_state,
            pending,
            "Wayland analysis worker exited during acknowledged adoption".to_owned(),
        );
    }

    let PendingPipeWireAdoption {
        request,
        format_bytes,
        callback_buffers,
        done,
        authority,
        ..
    } = pending;
    if !commit_claimed(&authority, true, || {
        user_data.install_prepared_format(negotiated.frame, callback_buffers);
        let mut state = format_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.current = request;
        state.current_format_bytes = format_bytes;
        state.current_acknowledged = true;
    }) {
        return Err("PipeWire format install lost its claimed commit authority".to_owned());
    }
    info!(
        width = negotiated.frame.width,
        height = negotiated.frame.height,
        target_fps = request.target_fps,
        "Adopted acknowledged Wayland screen capture format"
    );
    let _ = done.send(Ok(()));
    Ok(())
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
    status_writer: Option<SourceSessionWriter>,
) -> anyhow::Result<PipeWireLoopExit> {
    pw::init();
    let source = WaylandSourceMetadata::from_stream(&portal_stream, session_generation)?;
    let exchange = Arc::new(AnalysisExchange::default());
    let callback_metrics = Arc::new(CaptureCallbackMetrics::default());
    let loop_exit = Arc::new(Mutex::new(None::<PipeWireLoopExit>));
    let requested_extent = source.signature.logical_extent.unwrap_or(
        demand
            .requested_extent()
            .context("active Wayland capture demand must carry an extent")?,
    );
    let requested_output_extent = demand
        .requested_extent()
        .context("active Wayland capture demand must carry an extent")?;
    let initial_request = PipeWireFormatRequest::new_with_compute_policy(
        requested_extent,
        requested_output_extent,
        config,
        settings.compute_capacity_policy,
    )?;
    let format_bytes = build_format_params(config.target_fps, requested_extent)?;
    let callback_capacity = NegotiatedFormat {
        width: requested_extent.width(),
        height: requested_extent.height(),
        format: SpaVideoFormat::Rgba,
    }
    .byte_len()
    .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let callback_buffers = DoubleBuffer::try_with_capacity_and_admission(
        callback_capacity,
        &settings.admission_coordinator,
    )?;
    let decoding_enabled = Arc::new(AtomicBool::new(false));
    let format_state = Arc::new(Mutex::new(PipeWireFormatState {
        current: initial_request,
        current_format_bytes: format_bytes.clone(),
        current_acknowledged: false,
        pending: None,
        restoring: None,
    }));

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
        .add_local_listener_with_user_data(WaylandCaptureUserData::with_buffers(
            Arc::clone(&exchange),
            Arc::clone(&callback_metrics),
            callback_buffers,
            Arc::clone(&decoding_enabled),
            settings.admission_coordinator.clone(),
        ))
        .param_changed({
            let format_state = Arc::clone(&format_state);
            let mainloop = mainloop.clone();
            let loop_exit = Arc::clone(&loop_exit);
            move |stream, user_data, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    if let Some(outcome) = reject_pipewire_format(
                        stream,
                        user_data,
                        &format_state,
                        "PipeWire removed the negotiated video format".to_owned(),
                    ) {
                        terminate_pipewire_loop(&mainloop, &loop_exit, outcome);
                    }
                    return;
                };
                let Ok((media_type, media_subtype)) =
                    spa::param::format_utils::parse_format(param)
                else {
                    if let Some(outcome) = reject_pipewire_format(
                        stream,
                        user_data,
                        &format_state,
                        "PipeWire returned an unreadable video format".to_owned(),
                    ) {
                        terminate_pipewire_loop(&mainloop, &loop_exit, outcome);
                    }
                    return;
                };
                if media_type != spa::param::format::MediaType::Video
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    if let Some(outcome) = reject_pipewire_format(
                        stream,
                        user_data,
                        &format_state,
                        "PipeWire returned a non-raw video format".to_owned(),
                    ) {
                        terminate_pipewire_loop(&mainloop, &loop_exit, outcome);
                    }
                    return;
                }
                if user_data.format.parse(param).is_err() {
                    if let Some(outcome) = reject_pipewire_format(
                        stream,
                        user_data,
                        &format_state,
                        "PipeWire returned an invalid raw video format".to_owned(),
                    ) {
                        terminate_pipewire_loop(&mainloop, &loop_exit, outcome);
                    }
                    return;
                }

                let format = user_data.format.format();
                let size = user_data.format.size();
                let Some(frame) = spa_video_format(format).map(|format| NegotiatedFormat {
                    width: size.width,
                    height: size.height,
                    format,
                }) else {
                    if let Some(outcome) = reject_pipewire_format(
                        stream,
                        user_data,
                        &format_state,
                        format!("PipeWire negotiated unsupported video format {format:?}"),
                    ) {
                        terminate_pipewire_loop(&mainloop, &loop_exit, outcome);
                    }
                    return;
                };
                let negotiated = NegotiatedPipeWireFormat {
                    frame,
                    framerate: user_data.format.framerate(),
                };
                let acknowledgment = format_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .acknowledgment(negotiated);
                match acknowledgment {
                    PipeWireFormatAcknowledgment::Current => {
                        if let Err(error) = user_data.activate_negotiated_format(frame) {
                            terminate_pipewire_loop(
                                &mainloop,
                                &loop_exit,
                                PipeWireLoopExit::Unavailable(format!(
                                    "failed to activate authoritative PipeWire format: {error}"
                                )),
                            );
                            return;
                        }
                        format_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .current_acknowledged = true;
                        debug!(
                            ?format,
                            width = size.width,
                            height = size.height,
                            "Accepted authoritative Wayland screen capture format"
                        );
                    }
                    PipeWireFormatAcknowledgment::Pending => {
                        if let Err(reason) = commit_pending_pipewire_adoption(
                            stream,
                            user_data,
                            &format_state,
                            negotiated,
                        ) {
                            terminate_pipewire_loop(
                                &mainloop,
                                &loop_exit,
                                PipeWireLoopExit::Terminal(reason),
                            );
                        }
                    }
                    PipeWireFormatAcknowledgment::Restored => {
                        if let Err(reason) =
                            settle_pipewire_restoration(user_data, &format_state, frame)
                        {
                            terminate_pipewire_loop(
                                &mainloop,
                                &loop_exit,
                                PipeWireLoopExit::Terminal(reason),
                            );
                        }
                    }
                    PipeWireFormatAcknowledgment::Restoring => {
                        user_data.fence_decoding();
                        debug!(
                            ?format,
                            width = size.width,
                            height = size.height,
                            "Ignored stale PipeWire format while awaiting restoration"
                        );
                    }
                    PipeWireFormatAcknowledgment::CancelledCurrent => {
                        user_data.fence_decoding();
                        let pending = format_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pending
                            .take();
                        let Some(pending) = pending else {
                            terminate_pipewire_loop(
                                &mainloop,
                                &loop_exit,
                                PipeWireLoopExit::Terminal(
                                    "cancelled PipeWire adoption had no owner".to_owned(),
                                ),
                            );
                            return;
                        };
                        let _ = format_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .begin_restoring(
                                pending,
                                "PipeWire format adoption timed out".to_owned(),
                            );
                        if let Err(reason) =
                            settle_pipewire_restoration(user_data, &format_state, frame)
                        {
                            terminate_pipewire_loop(
                                &mainloop,
                                &loop_exit,
                                PipeWireLoopExit::Terminal(reason),
                            );
                        }
                    }
                    PipeWireFormatAcknowledgment::Cancelled
                    | PipeWireFormatAcknowledgment::Rejected => {
                        let reason = format!(
                            "PipeWire negotiated {size:?} at {:?} instead of the exact requested format",
                            user_data.format.framerate()
                        );
                        if let Some(outcome) = reject_pipewire_format(
                            stream,
                            user_data,
                            &format_state,
                            reason,
                        ) {
                            terminate_pipewire_loop(&mainloop, &loop_exit, outcome);
                        }
                    }
                }
            }
        })
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
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                user_data.record_drop(ChunkDropReason::MissingBuffer);
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                user_data.record_drop(ChunkDropReason::MissingPlane);
                return;
            };

            if !user_data.decoding_enabled.load(Ordering::Acquire) {
                user_data.record_drop(ChunkDropReason::MissingFormat);
                return;
            }
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
                    status_writer,
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
        let command_exchange = Arc::clone(&exchange);
        let command_format_state = Arc::clone(&format_state);
        let command_decoding_enabled = Arc::clone(&decoding_enabled);
        move |command| match command {
            WorkerCommand::SetDemand(demand) => {
                let active = demand.is_active();
                if let Err(error) = stream.set_active(active) {
                    warn!(active, %error, "Failed to update PipeWire stream active state");
                }
            }
            WorkerCommand::PrepareExact {
                ticket,
                cancelled,
                completion,
            } => {
                if let Err(command) = command_exchange.send_exact(AnalysisExactCommand::Prepare {
                    ticket,
                    cancelled,
                    completion,
                }) {
                    let AnalysisExactCommand::Prepare { completion, .. } = *command else {
                        unreachable!("the rejected exact command preserves its variant")
                    };
                    let _ = completion.send(Err(anyhow!(
                        "Wayland analysis worker rejected exact publication preparation"
                    )));
                }
            }
            WorkerCommand::ReapExact { completion } => {
                if let Err(command) =
                    command_exchange.send_exact(AnalysisExactCommand::Reap { completion })
                {
                    let AnalysisExactCommand::Reap { completion } = *command else {
                        unreachable!("the rejected exact command preserves its variant")
                    };
                    if let Some(completion) = completion {
                        let _ = completion.send(Err(anyhow!(
                            "Wayland analysis worker rejected exact publication retirement"
                        )));
                    }
                }
            }
            WorkerCommand::AdoptSettings {
                adoption_id,
                prepared,
                ready,
                decision,
                done,
                authority,
            } => {
                if ready.send(()).is_err() {
                    authority.cancel();
                    return;
                }
                if !matches!(
                    decision.recv_timeout(WORKER_READY_TIMEOUT),
                    Ok(SettingsDecision::Commit)
                ) {
                    authority.cancel();
                    return;
                }
                if authority.is_cancelled() {
                    return;
                }
                let PreparedWaylandSettings {
                    config,
                    cadence,
                    demand,
                    analyzer,
                    pipewire_format,
                } = prepared;
                let (analysis_ready_tx, analysis_ready_rx) = mpsc::sync_channel(1);
                let (analysis_decision_tx, analysis_decision_rx) = mpsc::sync_channel(1);
                let (analysis_done_tx, analysis_done_rx) = mpsc::sync_channel(1);
                let finalize_authority = pipewire_format.is_none();
                let adoption = AnalysisAdoption {
                    prepared: PreparedAnalysisSettings {
                        config,
                        cadence,
                        demand,
                        analyzer,
                    },
                    ready: analysis_ready_tx,
                    decision: analysis_decision_rx,
                    done: analysis_done_tx,
                    authority: Arc::clone(&authority),
                    finalize_authority,
                };
                if let Err(error) = command_exchange.prepare_adoption(adoption) {
                    authority.cancel();
                    let _ = done.send(Err(error));
                    return;
                }
                if analysis_ready_rx
                    .recv_timeout(WORKER_READY_TIMEOUT)
                    .is_err()
                {
                    authority.cancel();
                    let _ = done.send(Err(
                        "Wayland analysis worker exited before adoption".to_owned()
                    ));
                    return;
                }

                if let Some(PreparedPipeWireFormat {
                    callback_buffers,
                    format_bytes,
                    request,
                }) = pipewire_format
                {
                    let cancellation_done = done.clone();
                    let pending = PendingPipeWireAdoption {
                        id: adoption_id,
                        request,
                        format_bytes,
                        callback_buffers,
                        analysis_decision: analysis_decision_tx,
                        analysis_done: analysis_done_rx,
                        done,
                        authority: Arc::clone(&authority),
                    };
                    let update_bytes = pending.format_bytes.clone();
                    {
                        let state = command_format_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if !state.current_acknowledged {
                            pending.authority.cancel();
                            let _ = pending.done.send(Err(
                                "PipeWire has not acknowledged the initial exact format".to_owned(),
                            ));
                            return;
                        }
                        if !state.can_begin_adoption() {
                            pending.authority.cancel();
                            let _ =
                                pending
                                    .done
                                    .send(Err("PipeWire already has an unsettled format adoption"
                                        .to_owned()));
                            return;
                        }
                    }
                    let armed = authority.prepare_if_open(|| {
                        command_decoding_enabled.store(false, Ordering::Release);
                        command_exchange.discard_latest_frame();
                        command_format_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pending = Some(pending);
                    });
                    if armed.is_none() {
                        let _ = cancellation_done.send(Err(
                            "Wayland format adoption was cancelled before negotiation".to_owned(),
                        ));
                        return;
                    }
                    if let Err(error) = update_pipewire_format(&stream, &update_bytes) {
                        let pending = command_format_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .cancel(adoption_id)
                            .expect("failed PipeWire update retains pending adoption");
                        command_decoding_enabled.store(false, Ordering::Release);
                        command_exchange.discard_latest_frame();
                        if let Err(restore_error) = request_pipewire_restoration(
                            &stream,
                            &command_format_state,
                            pending,
                            error.to_string(),
                        ) {
                            terminate_pipewire_loop(
                                &mainloop,
                                &loop_exit,
                                PipeWireLoopExit::Terminal(restore_error),
                            );
                        }
                    }
                    return;
                }

                if analysis_decision_tx.send(SettingsDecision::Commit).is_err() {
                    authority.cancel();
                    let _ = done.send(Err(
                        "Wayland analysis worker exited during adoption".to_owned()
                    ));
                    return;
                }
                let committed = match analysis_done_rx.recv_timeout(WORKER_STOP_TIMEOUT) {
                    Ok(committed) => committed,
                    Err(_) => {
                        authority.cancel_or_wait_for_commit() == AdoptionSettlement::Committed
                    }
                };
                if committed {
                    let _ = done.send(Ok(()));
                } else {
                    let _ = done.send(Err(
                        "Wayland analysis adoption lost commit authority".to_owned()
                    ));
                }
            }
            WorkerCommand::CancelAdoption { adoption_id } => {
                let (pending, already_restoring) = {
                    let mut state = command_format_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let pending = state.cancel(adoption_id);
                    (pending, state.restoring_id() == Some(adoption_id))
                };
                if let Some(pending) = pending {
                    command_decoding_enabled.store(false, Ordering::Release);
                    command_exchange.discard_latest_frame();
                    if let Err(reason) = request_pipewire_restoration(
                        &stream,
                        &command_format_state,
                        pending,
                        "PipeWire format adoption timed out".to_owned(),
                    ) {
                        terminate_pipewire_loop(
                            &mainloop,
                            &loop_exit,
                            PipeWireLoopExit::Terminal(reason),
                        );
                    }
                } else if !already_restoring {
                    debug!(
                        adoption_id,
                        "Ignored stale Wayland format-adoption cancellation"
                    );
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

fn update_pipewire_format(stream: &pw::stream::Stream, format_bytes: &[u8]) -> anyhow::Result<()> {
    let pod = spa::pod::Pod::from_bytes(format_bytes)
        .context("failed to deserialize PipeWire format pod")?;
    stream
        .update_params(&mut [pod])
        .context("failed to update PipeWire format")
}

fn build_format_params(target_fps: u32, requested_extent: PixelExtent) -> anyhow::Result<Vec<u8>> {
    CaptureCadence::new(target_fps)?;
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
            Rectangle,
            spa::utils::Rectangle {
                width: requested_extent.width(),
                height: requested_extent.height(),
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Fraction,
            spa::utils::Fraction {
                num: target_fps,
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
