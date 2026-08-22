//! Wayland screen capture source powered by XDG Desktop Portal + PipeWire.
//!
//! This source keeps the portal session and PipeWire stream on a dedicated
//! worker thread. The render loop only clones the latest processed
//! [`ScreenData`] snapshot, while capture demand is toggled at runtime by the
//! daemon depending on the active effect.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::num::{NonZeroU32, NonZeroUsize};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
pub use hypercolor_pipewire_interop::PackedVideoFormat as SpaVideoFormat;
use hypercolor_pipewire_interop::{
    BufferFault, CallbackAction, CaptureFormatRequest, D4Transform, DequeueOutcome, FormatEvent,
    FormatFault, FormatOffer, LoopReceiver, LoopSender, MetaFault, NegotiatedVideoFormat,
    PixelCrop, PortalRemote, PortalRequest, PortalSession, PortalStreamDescriptor, ProcessBuffer,
    StateChange, StreamControl, StreamEventHandler, StreamState, connect_stream, loop_channel,
    open_portal_session,
};
use hypercolor_types::source_status::SourceDiagnosticsEnvelope;
use serde_json::{Map, Value, json};
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
    ScreenColorTransformCapabilities, ScreenCommittedState, ScreenComputeCapacityPolicy,
    ScreenPreparedWorkerToken, ScreenPublicationHealth, ScreenPublicationHub,
    ScreenRequiredResourceMinimum, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerBinding,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale, analyze_screen_frame,
};
use crate::input::traits::{
    CapabilityActionDisposition, CapabilityActionIdentity, InputData, InputSource, ScreenSource,
    ScreenSourcePickerAction, ScreenSourceRole, SourceRoleBinding,
};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceSessionWriter, SourceStatusHandle,
    SourceStatusReporter,
};
use hypercolor_worker_retention::{retain_worker, spawn_worker};

use super::adapter::{
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureExactRuntimeOwner, CaptureOwnedSource,
    CapturePublication, CapturePublicationFence, CapturePublicationSource, CaptureSessionAuthority,
    VersionedCaptureSettings, begin_capture_exact_preparation, begin_capture_exact_retirement,
    bind_current_capture_exact_runtime, execute_capture_exact_command,
};

const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const FORMAT_ADOPTION_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
const CAPTURE_DIAGNOSTICS_INTERVAL: Duration = Duration::from_secs(1);
const RECONNECT_DELAY: Duration = Duration::from_millis(250);

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
    /// The dequeued wrapper did not retain its native buffer.
    MissingNativeBuffer,
    /// The first pixel plane did not retain a chunk descriptor.
    MissingChunk,
    /// A pixel buffer arrived before a supported format was negotiated.
    MissingFormat,
    /// The first PipeWire pixel plane was not mapped into this process.
    UnmappedPlane,
    /// The negotiated dimensions are empty or overflow addressable storage.
    InvalidExtent,
    /// The SPA chunk offset or size escapes the mapped plane.
    InvalidChunkBounds,
    /// Native buffer counts or pointers could not form bounded views.
    InvalidBufferLayout,
    /// A DMA-BUF plane did not have a stable kernel allocation identity.
    InvalidDmaBuf,
    /// The signed stride cannot contain one negotiated row.
    InvalidStride,
    /// The chunk ends before the final row is complete.
    TruncatedChunk,
    /// The SPA crop escapes the negotiated native extent.
    InvalidCrop,
    /// The SPA transform metadata carried an invalid value.
    InvalidTransform,
    /// Core policy panicked inside the guarded native visitor.
    VisitorPanicked,
    /// Both preallocated buffers are still owned by analysis or publication.
    BufferUnavailable,
    /// The negotiated frame exceeds the capacity prepared outside the callback.
    BufferTooSmall,
}

impl ChunkDropReason {
    const ALL: [Self; 17] = [
        Self::MissingBuffer,
        Self::MissingPlane,
        Self::MissingNativeBuffer,
        Self::MissingChunk,
        Self::MissingFormat,
        Self::UnmappedPlane,
        Self::InvalidExtent,
        Self::InvalidChunkBounds,
        Self::InvalidBufferLayout,
        Self::InvalidDmaBuf,
        Self::InvalidStride,
        Self::TruncatedChunk,
        Self::InvalidCrop,
        Self::InvalidTransform,
        Self::VisitorPanicked,
        Self::BufferUnavailable,
        Self::BufferTooSmall,
    ];
    const COUNT: usize = Self::ALL.len();

    const fn index(self) -> usize {
        match self {
            Self::MissingBuffer => 0,
            Self::MissingPlane => 1,
            Self::MissingNativeBuffer => 2,
            Self::MissingChunk => 3,
            Self::MissingFormat => 4,
            Self::UnmappedPlane => 5,
            Self::InvalidExtent => 6,
            Self::InvalidChunkBounds => 7,
            Self::InvalidBufferLayout => 8,
            Self::InvalidDmaBuf => 9,
            Self::InvalidStride => 10,
            Self::TruncatedChunk => 11,
            Self::InvalidCrop => 12,
            Self::InvalidTransform => 13,
            Self::VisitorPanicked => 14,
            Self::BufferUnavailable => 15,
            Self::BufferTooSmall => 16,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::MissingBuffer => "missing_buffer",
            Self::MissingPlane => "missing_plane",
            Self::MissingNativeBuffer => "missing_native_buffer",
            Self::MissingChunk => "missing_chunk",
            Self::MissingFormat => "missing_format",
            Self::UnmappedPlane => "unmapped_plane",
            Self::InvalidExtent => "invalid_extent",
            Self::InvalidChunkBounds => "invalid_chunk_bounds",
            Self::InvalidBufferLayout => "invalid_buffer_layout",
            Self::InvalidDmaBuf => "invalid_dma_buf",
            Self::InvalidStride => "invalid_stride",
            Self::TruncatedChunk => "truncated_chunk",
            Self::InvalidCrop => "invalid_crop",
            Self::InvalidTransform => "invalid_transform",
            Self::VisitorPanicked => "visitor_panicked",
            Self::BufferUnavailable => "buffer_unavailable",
            Self::BufferTooSmall => "buffer_too_small",
        }
    }
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
/// Invoked under the session-epoch guard, which serializes token grants
/// and clears; persistence authorizes tokens by epoch alone.
pub type RestoreTokenSink = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// Settings shared between the input source handle and the capture worker.
///
/// The config lives behind a mutex while the generation counter is atomic:
/// the worker polls the counter once per frame and only takes the lock when
/// a reconfiguration actually happened.
struct SharedSettings {
    values: VersionedCaptureSettings<CaptureConfig>,
    admission_coordinator: ScreenByteAdmissionCoordinator,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    topology_generation: AtomicU64,
    topology: Mutex<Option<WaylandTopologyState>>,
    session_generation: AtomicU64,
    session_guard: Mutex<()>,
    publication: Arc<Mutex<WaylandCapturePublication>>,
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

impl CapturePublicationSource for WaylandPublicationSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.epoch.source_id
    }
}

struct WaylandOwnedSource {
    source_id: CaptureSourceId,
    session_generation: u64,
    binding: ScreenWorkerBinding,
    _runtime_lifetime: ScreenResourceLifetime,
}

impl CaptureOwnedSource for WaylandOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    fn belongs_to_authority(&self, authority: &ScreenCommittedState) -> bool {
        authority.owns_runtime_binding(&self.binding)
    }
}

#[derive(Default)]
struct WaylandExactPublicationShared {
    common: CaptureExactPublicationShared<WaylandPublicationSource, WaylandOwnedSource>,
    cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
}

impl Deref for WaylandExactPublicationShared {
    type Target = CaptureExactPublicationShared<WaylandPublicationSource, WaylandOwnedSource>;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl WaylandExactPublicationShared {
    #[cfg(test)]
    fn register_test_owned_source(&self, source: Box<ExactBoxNode<WaylandOwnedSource>>) -> bool {
        let authority = CaptureSessionAuthority::new(source.value().session_generation);
        drop(self.activate_authority(authority));
        self.register_owned_source_if_current(authority, source)
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
    offer: FormatOffer,
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

    fn matches(self, negotiated: NegotiatedVideoFormat) -> bool {
        // The transport tick rate is advisory: compositors negotiate 0/1
        // (variable) or their own display rate, and CapturePacer governs the
        // capture cadence regardless. Extent stays exact.
        let rate = negotiated.framerate;
        self.analysis_work_plan.input_extent() == self.extent
            && self.analysis_work_plan.target_fps() == self.target_fps
            && negotiated.width == self.extent.width()
            && negotiated.height == self.extent.height()
            && rate.denominator != 0
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
    offer: FormatOffer,
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
    current_offer: FormatOffer,
    current_acknowledged: bool,
    pending: Option<PendingPipeWireAdoption>,
    restoring: Option<RestoringPipeWireAdoption>,
}

impl PipeWireFormatState {
    fn acknowledgment(&self, negotiated: NegotiatedVideoFormat) -> PipeWireFormatAcknowledgment {
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

    fn begin_restoring(
        &mut self,
        pending: PendingPipeWireAdoption,
        failure: String,
    ) -> FormatOffer {
        self.restoring = Some(RestoringPipeWireAdoption { pending, failure });
        self.current_offer
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
    native_extent: Option<PixelExtent>,
    transform: CaptureRotation,
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WaylandPublicationFence(Option<CaptureEpoch>);

impl CapturePublicationFence<CaptureEpoch> for WaylandPublicationFence {
    fn admits(&self, epoch: &CaptureEpoch) -> bool {
        self.0.as_ref() == Some(epoch)
    }
}

type WaylandCapturePublication =
    CapturePublication<WaylandPublicationFence, CaptureEpoch, CapturedScreenSnapshot>;

impl SharedSettings {
    fn config_snapshot(&self) -> CaptureConfig {
        self.values.lock_config().clone()
    }

    fn commit_runtime(&self, next: &PreparedAnalysisSettings) -> u64 {
        self.commit_values(&next.config, next.demand)
    }

    fn commit_values(&self, next_config: &CaptureConfig, demand: ScreenCaptureDemand) -> u64 {
        let mut values = self.values.lock();
        let granted_token = values.config_mut().restore_token.take();
        values.config_mut().clone_from(next_config);
        if values.config().restore_token.is_none() {
            values.config_mut().restore_token = granted_token;
        }
        *values.demand_mut() = demand;
        values.commit()
    }

    fn snapshot_for_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
    ) -> Option<CaptureRuntimeSettings> {
        let _session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire)
            || self.session_generation.load(Ordering::Acquire) != session_generation
        {
            return None;
        }
        let snapshot = self.values.snapshot();
        Some(CaptureRuntimeSettings {
            config: snapshot.config,
            demand: snapshot.demand,
        })
    }

    fn expected_epoch(&self) -> Option<CaptureEpoch> {
        let _session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fence()
            .0
            .clone()
    }

    fn begin_session(&self) -> u64 {
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_generation = self
            .session_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .expect("Wayland capture session generation exhausted")
            + 1;
        let displaced = publication.replace_fence(WaylandPublicationFence(None));
        let displaced_exact = self
            .exact
            .activate_authority(CaptureSessionAuthority::new(session_generation));
        drop(publication);
        drop(session_guard);
        drop((displaced, displaced_exact));
        session_generation
    }

    fn begin_successor_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
        active_session_generation: &AtomicU64,
    ) -> Option<u64> {
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let successor_generation = session_generation.checked_add(1)?;
        self.session_generation
            .compare_exchange(
                session_generation,
                successor_generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        let displaced = publication.replace_fence(WaylandPublicationFence(None));
        let successor_authority = CaptureSessionAuthority::new(successor_generation);
        let displaced_exact = self.exact.activate_authority(successor_authority);
        active_session_generation.store(successor_generation, Ordering::Release);
        drop(publication);
        drop(session_guard);
        drop((displaced, displaced_exact));
        Some(successor_generation)
    }

    fn cancel_worker_session(&self, cancel: &AtomicBool, active_session_generation: &AtomicU64) {
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cancel.store(true, Ordering::SeqCst);
        let session_generation = active_session_generation.load(Ordering::Acquire);
        if self.session_generation.load(Ordering::Acquire) != session_generation {
            return;
        }
        let displaced = if publication
            .fence()
            .0
            .as_ref()
            .is_some_and(|epoch| epoch.session_generation == session_generation)
        {
            Some(publication.replace_fence(WaylandPublicationFence(None)))
        } else {
            None
        };
        self.exact
            .replace_source_if_current(CaptureSessionAuthority::new(session_generation), None);
        drop(publication);
        drop(session_guard);
        drop(displaced);
    }

    fn persist_restore_token_for_session(
        &self,
        session_generation: u64,
        cancel: &AtomicBool,
        restore_token: Option<String>,
        token_sink: Option<&RestoreTokenSink>,
    ) -> bool {
        let _session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancel.load(Ordering::Acquire)
            || self.session_generation.load(Ordering::Acquire) != session_generation
        {
            return false;
        }
        self.values
            .lock_config()
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
            .session_guard
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
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut publication = self
            .publication
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
        let epoch = CaptureEpoch {
            source_id: signature.source_id.clone(),
            topology_generation: resolved.generation,
            session_generation,
        };
        let displaced_fence =
            publication.replace_fence_if_changed(WaylandPublicationFence(Some(epoch.clone())));
        let displaced_activation = publication
            .activate(epoch)
            .expect("the active Wayland epoch matches its installed publication fence");
        drop(topology);
        drop(publication);
        drop(session_guard);
        drop((displaced_fence, displaced_activation));
        Some(resolved)
    }

    fn publish_snapshot(&self, analysis: AnalyzedScreenSnapshot) -> bool {
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(expected) = publication.fence().0.clone() else {
            return false;
        };
        if analysis.geometry_frame().validate_epoch(&expected).is_err() {
            return false;
        }
        let result = publication.publish(&expected, CapturedScreenSnapshot { analysis });
        drop(publication);
        drop(session_guard);
        result.is_ok()
    }

    fn clear_expected_epoch(&self) {
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let displaced = {
            self.publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace_fence(WaylandPublicationFence(None))
        };
        let session_generation = self.session_generation.load(Ordering::Acquire);
        let displaced_source = (session_generation != 0).then(|| {
            self.exact
                .replace_source_if_current(CaptureSessionAuthority::new(session_generation), None)
        });
        drop(session_guard);
        drop((displaced, displaced_source));
    }

    fn invalidate_session(&self, session_generation: u64) -> bool {
        let session_guard = self
            .session_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if publication
            .fence()
            .0
            .as_ref()
            .is_none_or(|epoch| epoch.session_generation != session_generation)
        {
            return false;
        }
        let displaced = publication.replace_fence(WaylandPublicationFence(None));
        self.exact
            .replace_source_if_current(CaptureSessionAuthority::new(session_generation), None);
        drop(publication);
        drop(session_guard);
        drop(displaced);
        true
    }
}

/// Wayland-only live screen capture input source.
pub struct WaylandScreenCaptureInput {
    settings: Arc<SharedSettings>,
    running: bool,
    capture_demand: ScreenCaptureDemand,
    publication: Arc<Mutex<WaylandCapturePublication>>,
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
        let publication = Arc::new(Mutex::new(WaylandCapturePublication::default()));
        Self {
            settings: Arc::new(SharedSettings {
                values: VersionedCaptureSettings::new(config, ScreenCaptureDemand::Inactive),
                admission_coordinator,
                compute_capacity_policy,
                topology_generation: AtomicU64::new(0),
                topology: Mutex::new(None),
                session_generation: AtomicU64::new(0),
                session_guard: Mutex::new(()),
                publication: Arc::clone(&publication),
                exact: WaylandExactPublicationShared::default(),
            }),
            running: false,
            capture_demand: ScreenCaptureDemand::Inactive,
            publication,
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
                offer: FormatOffer::new(CaptureFormatRequest {
                    width: acquisition_extent.width(),
                    height: acquisition_extent.height(),
                    target_fps: config.target_fps,
                })?,
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

        clear_restore_token(&self.settings, self.token_sink.as_ref());

        if !self.running || !self.capture_demand.is_active() {
            return Ok(());
        }

        info!("Re-opening Wayland screencast source picker");
        self.restart_worker()
    }

    fn detached_reselect_action(&self) -> ScreenSourcePickerAction {
        let settings = Arc::clone(&self.settings);
        let token_sink = self.token_sink.clone();
        let worker = self.worker.as_ref().map(|worker| {
            (
                Arc::clone(&worker.portal_pending),
                worker.command_tx.clone(),
            )
        });
        ScreenSourcePickerAction::new(
            Arc::new(move || {
                if worker
                    .as_ref()
                    .is_some_and(|(portal_pending, _)| portal_pending.load(Ordering::SeqCst))
                {
                    debug!("Portal source picker is already open; ignoring re-pick request");
                    return Ok(());
                }
                clear_restore_token(&settings, token_sink.as_ref());
                if let Some((_, command_tx)) = &worker {
                    command_tx
                        .send(WorkerCommand::Reselect)
                        .map_err(|_| anyhow!("Wayland capture worker rejected source reselect"))?;
                }
                Ok(())
            }),
            CapabilityActionIdentity::new("platform_backend", CapabilityActionDisposition::Local),
        )
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

        if let Ok(mut current) = self.settings.values.try_lock_demand() {
            *current = demand;
        }
        self.settings.values.bump_revision();

        if !self.running {
            if !demand.is_active() {
                self.settings.clear_expected_epoch();
            }
            let latest = self
                .publication
                .lock()
                .ok()
                .and_then(|mut publication| publication.clear_latest());
            drop(latest);
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
            if let Ok(mut current) = self.settings.values.try_lock_demand() {
                *current = previous;
            }
            self.settings.values.bump_revision();
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

        let latest = (previous.is_active() != demand.is_active())
            .then(|| {
                self.publication
                    .lock()
                    .ok()
                    .and_then(|mut publication| publication.clear_latest())
            })
            .flatten();
        drop(latest);
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
        let (command_tx, command_rx) = loop_channel();
        let worker_command_tx = command_tx.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let status_writer = self.status_session.load();
        let worker_status_writer = status_writer.clone();
        let session_generation = Arc::new(AtomicU64::new(settings.begin_session()));
        let capture_session_generation = Arc::clone(&session_generation);
        let worker_settings = Arc::clone(&settings);
        let join_handle = spawn_worker(
            thread::Builder::new().name("hypercolor-screen-capture".to_owned()),
            move || {
                let _ = ready_tx.send(());
                run_capture_worker(
                    settings,
                    command_rx,
                    worker_command_tx,
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
        self.settings
            .invalidate_session(session_generation.load(Ordering::Acquire));
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
        if let Ok(mut demand) = self.settings.values.try_lock_demand() {
            *demand = ScreenCaptureDemand::Inactive;
        }
        if self.worker.is_some() {
            self.shutdown_worker();
        } else {
            self.settings.clear_expected_epoch();
        }
        self.reap_workers(true);
        let session_generation = self.settings.session_generation.load(Ordering::Acquire);
        if session_generation != 0 {
            let authority = CaptureSessionAuthority::new(session_generation);
            self.settings
                .exact
                .clear_owned_sources_if_current(authority);
            self.settings
                .exact
                .replace_source_if_current(authority, None);
        }

        let latest = self
            .publication
            .lock()
            .ok()
            .and_then(|mut publication| publication.clear_latest());
        drop(latest);
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

        let publication = self
            .publication
            .lock()
            .map_err(|_| anyhow!("wayland screen capture snapshot mutex poisoned"))?;
        let snapshot = publication.snapshot();
        drop(publication);
        let Some(snapshot) = snapshot else {
            return Ok(InputData::None);
        };
        let metadata = snapshot.value.analysis.geometry_frame().metadata();
        if snapshot
            .value
            .analysis
            .geometry_frame()
            .validate_epoch(&snapshot.epoch)
            .is_err()
        {
            return Ok(InputData::None);
        }
        if snapshot.revision != self.status_snapshot_generation {
            if let Some(status) = self.status.session() {
                let cadence = CaptureCadence::new(self.settings.config_snapshot().target_fps)?;
                status.record_sample(
                    metadata.captured_at,
                    cadence.freshness_deadline(metadata.captured_at)?,
                    1,
                )?;
            }
            self.status_snapshot_generation = snapshot.revision;
        }
        Ok(InputData::Screen(snapshot.value.analysis.data().clone()))
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
}

impl ScreenSource for WaylandScreenCaptureInput {
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
        self.settings.exact.install_hub(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.settings.exact.resolution_revision()
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
        begin_capture_exact_preparation(&worker.exact_command_endpoint(), ticket)
    }

    fn begin_screen_publication_retirement(&mut self) -> Option<ScreenWorkerRetirement> {
        let worker = self.worker.as_ref()?;
        Some(begin_capture_exact_retirement(
            &worker.exact_command_endpoint(),
        ))
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        self.reconfigure(config.clone())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.reselect_source()
    }

    fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        Some(self.detached_reselect_action())
    }
}

impl SourceRoleBinding for WaylandScreenCaptureInput {
    type Role = ScreenSourceRole;
}

fn clear_restore_token(settings: &SharedSettings, token_sink: Option<&RestoreTokenSink>) {
    let _session_guard = settings
        .session_guard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    settings.values.lock_config().restore_token = None;
    if let Some(sink) = token_sink {
        sink(None);
    }
}

struct WaylandCaptureWorker {
    command_tx: LoopSender<WorkerCommand>,
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
}

#[derive(Clone)]
struct WaylandExactCommandEndpoint {
    command_tx: LoopSender<WorkerCommand>,
    session_generation: Arc<AtomicU64>,
}

impl CaptureExactCommandEndpoint for WaylandExactCommandEndpoint {
    const SOURCE_NAME: &'static str = "Wayland capture";

    fn authority(&self) -> CaptureSessionAuthority {
        CaptureSessionAuthority::new(self.session_generation.load(Ordering::Acquire))
    }

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected> {
        self.command_tx
            .send(WorkerCommand::Exact(command))
            .map_err(|_| CaptureExactCommandRejected)
    }
}

impl WaylandCaptureWorker {
    fn exact_command_endpoint(&self) -> WaylandExactCommandEndpoint {
        WaylandExactCommandEndpoint {
            command_tx: self.command_tx.clone(),
            session_generation: Arc::clone(&self.session_generation),
        }
    }
}

impl WaylandCaptureWorker {
    fn cancel_session(&self) {
        self.settings
            .cancel_worker_session(&self.cancel, &self.session_generation);
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
        retain_worker(join_handle, "Wayland capture worker");
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
    Reselect,
    Exact(CaptureExactCommand),
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
    AnalysisExited,
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

#[derive(Clone)]
struct WaylandSourceMetadata {
    signature: WaylandTopologySignature,
    session_generation: u64,
    topology: Option<ResolvedWaylandTopology>,
}

impl WaylandSourceMetadata {
    fn from_stream(
        stream: &PortalStreamDescriptor,
        session_generation: u64,
    ) -> anyhow::Result<Self> {
        let source_name = stream.source_name();
        let source_id =
            CaptureSourceId::new(Arc::<str>::from(format!("wayland:portal:{source_name}")))?;
        let (x, y) = stream.position();
        let logical_extent = stream
            .logical_size()
            .and_then(|(width, height)| PixelExtent::new(width, height).ok());
        Ok(Self {
            signature: WaylandTopologySignature {
                source_id,
                origin: PhysicalOrigin { x, y },
                logical_extent,
                native_extent: None,
                transform: CaptureRotation::Identity,
            },
            session_generation,
            topology: None,
        })
    }

    fn source_scale(&self) -> SourceScale {
        let physical_width = self
            .signature
            .native_extent
            .map(|extent| self.signature.transform.apply_to_extent(extent).width())
            .unwrap_or(1);
        self.signature
            .logical_extent
            .and_then(|logical_extent| {
                SourceScale::new(logical_extent.width(), physical_width).ok()
            })
            .unwrap_or(SourceScale::ONE)
    }
}

struct WaylandCaptureUserData {
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

impl NegotiatedFormat {
    fn byte_len(self) -> Option<usize> {
        usize::try_from(self.width)
            .ok()?
            .checked_mul(usize::try_from(self.height).ok()?)?
            .checked_mul(self.format.bytes_per_pixel())
    }

    const fn from_native(format: NegotiatedVideoFormat) -> Self {
        Self {
            width: format.width,
            height: format.height,
            format: format.format,
        }
    }
}

struct WaylandStreamEvents {
    callback: WaylandCaptureUserData,
    format_state: Arc<Mutex<PipeWireFormatState>>,
    loop_exit: Arc<Mutex<Option<PipeWireLoopExit>>>,
}

impl StreamEventHandler for WaylandStreamEvents {
    fn format_changed(
        &mut self,
        control: &StreamControl<'_>,
        event: FormatEvent,
    ) -> CallbackAction {
        let negotiated = match event {
            FormatEvent::Removed => {
                return self.reject_format(
                    control,
                    "PipeWire removed the negotiated video format".to_owned(),
                );
            }
            FormatEvent::Invalid(fault) => {
                return self.reject_format(control, format_fault_reason(fault));
            }
            FormatEvent::Negotiated(negotiated) => negotiated,
        };
        if let Err(error) = control.acknowledge_format(negotiated) {
            return terminate_pipewire_loop(
                &self.loop_exit,
                PipeWireLoopExit::Terminal(format!(
                    "failed to advertise PipeWire buffer metadata: {error}"
                )),
            );
        }
        let frame = NegotiatedFormat::from_native(negotiated);
        let acknowledgment = self
            .format_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .acknowledgment(negotiated);
        match acknowledgment {
            PipeWireFormatAcknowledgment::Current => {
                if let Err(error) = self.callback.activate_negotiated_format(frame) {
                    return terminate_pipewire_loop(
                        &self.loop_exit,
                        PipeWireLoopExit::Unavailable(format!(
                            "failed to activate authoritative PipeWire format: {error}"
                        )),
                    );
                }
                self.format_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .current_acknowledged = true;
                debug!(
                    format = ?negotiated.format,
                    width = negotiated.width,
                    height = negotiated.height,
                    "Accepted authoritative Wayland screen capture format"
                );
            }
            PipeWireFormatAcknowledgment::Pending => {
                if let Err(reason) = commit_pending_pipewire_adoption(
                    control,
                    &mut self.callback,
                    &self.format_state,
                    negotiated,
                ) {
                    return terminate_pipewire_loop(
                        &self.loop_exit,
                        PipeWireLoopExit::Terminal(reason),
                    );
                }
            }
            PipeWireFormatAcknowledgment::Restored => {
                if let Err(reason) =
                    settle_pipewire_restoration(&mut self.callback, &self.format_state, frame)
                {
                    return terminate_pipewire_loop(
                        &self.loop_exit,
                        PipeWireLoopExit::Terminal(reason),
                    );
                }
            }
            PipeWireFormatAcknowledgment::Restoring => {
                self.callback.fence_decoding();
                debug!(
                    format = ?negotiated.format,
                    width = negotiated.width,
                    height = negotiated.height,
                    "Ignored stale PipeWire format while awaiting restoration"
                );
            }
            PipeWireFormatAcknowledgment::CancelledCurrent => {
                self.callback.fence_decoding();
                let pending = self
                    .format_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pending
                    .take();
                let Some(pending) = pending else {
                    return terminate_pipewire_loop(
                        &self.loop_exit,
                        PipeWireLoopExit::Terminal(
                            "cancelled PipeWire adoption had no owner".to_owned(),
                        ),
                    );
                };
                let _ = self
                    .format_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .begin_restoring(pending, "PipeWire format adoption timed out".to_owned());
                if let Err(reason) =
                    settle_pipewire_restoration(&mut self.callback, &self.format_state, frame)
                {
                    return terminate_pipewire_loop(
                        &self.loop_exit,
                        PipeWireLoopExit::Terminal(reason),
                    );
                }
            }
            PipeWireFormatAcknowledgment::Cancelled | PipeWireFormatAcknowledgment::Rejected => {
                if acknowledgment == PipeWireFormatAcknowledgment::Rejected
                    && let Some(extent) =
                        initial_native_extent_correction(&self.format_state, negotiated)
                {
                    self.callback.fence_decoding();
                    return terminate_pipewire_loop(
                        &self.loop_exit,
                        PipeWireLoopExit::RequiresNativeExtent(extent),
                    );
                }
                return self.reject_format(
                    control,
                    format!(
                        "PipeWire negotiated {}x{} {:?} at {:?} instead of the exact requested format",
                        negotiated.width,
                        negotiated.height,
                        negotiated.format,
                        negotiated.framerate
                    ),
                );
            }
        }
        CallbackAction::Continue
    }

    fn state_changed(&mut self, event: StateChange) -> CallbackAction {
        debug!(
            previous = ?event.previous,
            current = ?event.current,
            "Wayland screen capture stream state changed"
        );
        let terminal = match event.current {
            StreamState::Error(error) => {
                Some(format!("PipeWire stream entered error state: {error}"))
            }
            StreamState::Unconnected if event.previous != StreamState::Unconnected => {
                Some("PipeWire stream disconnected".to_owned())
            }
            StreamState::Unconnected
            | StreamState::Connecting
            | StreamState::Paused
            | StreamState::Streaming => None,
        };
        terminal.map_or(CallbackAction::Continue, |reason| {
            terminate_pipewire_loop(&self.loop_exit, PipeWireLoopExit::Terminal(reason))
        })
    }

    fn process(&mut self, buffer: ProcessBuffer<'_>) -> CallbackAction {
        let outcome = buffer.visit(|native| {
            if !self.callback.decoding_enabled.load(Ordering::Acquire) {
                return (CopyStats::dropped(ChunkDropReason::MissingFormat), None);
            }
            let Some(negotiated) = self.callback.negotiated else {
                return (CopyStats::dropped(ChunkDropReason::MissingFormat), None);
            };
            if native.dma_buf_identity().is_err() {
                return (CopyStats::dropped(ChunkDropReason::InvalidDmaBuf), None);
            }
            let crop = match native.crop() {
                None => None,
                Some(Ok(crop)) => match pixel_rect_from_native(crop) {
                    Ok(crop) => Some(crop),
                    Err(reason) => return (CopyStats::dropped(reason), None),
                },
                Some(Err(error)) => {
                    return (CopyStats::dropped(meta_drop_reason(error, true)), None);
                }
            };
            let transform = match native.transform() {
                None => CaptureRotation::Identity,
                Some(Ok(transform)) => capture_rotation(transform),
                Some(Err(error)) => {
                    return (CopyStats::dropped(meta_drop_reason(error, false)), None);
                }
            };
            let chunk = native.chunk();
            let view = SpaChunkView::new(
                native.bytes(),
                chunk.offset,
                chunk.size,
                chunk.stride,
                negotiated.width,
                negotiated.height,
                negotiated.format,
                crop,
                transform,
            );
            let stats = decode_chunk(&view, &mut self.callback.buffers);
            let completed = if stats.drop_reason().is_none() {
                self.callback.buffers.take_completed()
            } else {
                None
            };
            (stats, completed)
        });
        match outcome {
            DequeueOutcome::Empty => {
                self.callback.record_drop(ChunkDropReason::MissingBuffer);
            }
            DequeueOutcome::Faulted(error) => {
                self.callback.record_drop(buffer_drop_reason(error));
            }
            DequeueOutcome::Visited((stats, completed)) => {
                self.callback.metrics.record(stats);
                if let Some(frame) = completed {
                    self.callback.exchange.publish(frame);
                }
            }
            DequeueOutcome::VisitorPanicked => {
                self.callback.record_drop(ChunkDropReason::VisitorPanicked);
                return terminate_pipewire_loop(
                    &self.loop_exit,
                    PipeWireLoopExit::Terminal(
                        "Wayland frame policy panicked inside the guarded PipeWire visitor"
                            .to_owned(),
                    ),
                );
            }
        }
        CallbackAction::Continue
    }
}

impl WaylandStreamEvents {
    fn reject_format(&mut self, control: &StreamControl<'_>, reason: String) -> CallbackAction {
        reject_pipewire_format(control, &mut self.callback, &self.format_state, reason)
            .map_or(CallbackAction::Continue, |outcome| {
                terminate_pipewire_loop(&self.loop_exit, outcome)
            })
    }
}

fn format_fault_reason(fault: FormatFault) -> String {
    match fault {
        FormatFault::Unreadable => "PipeWire returned an unreadable video format".to_owned(),
        FormatFault::NonRawVideo => "PipeWire returned a non-raw video format".to_owned(),
        FormatFault::InvalidRawVideo => "PipeWire returned an invalid raw video format".to_owned(),
        FormatFault::UnsupportedPixelFormat => {
            "PipeWire negotiated an unsupported packed video format".to_owned()
        }
    }
}

const fn capture_rotation(transform: D4Transform) -> CaptureRotation {
    match transform {
        D4Transform::Identity => CaptureRotation::Identity,
        D4Transform::Clockwise90 => CaptureRotation::Clockwise90,
        D4Transform::Clockwise180 => CaptureRotation::Clockwise180,
        D4Transform::Clockwise270 => CaptureRotation::Clockwise270,
        D4Transform::Flipped => CaptureRotation::Flipped,
        D4Transform::Flipped90 => CaptureRotation::Flipped90,
        D4Transform::Flipped180 => CaptureRotation::Flipped180,
        D4Transform::Flipped270 => CaptureRotation::Flipped270,
    }
}

fn pixel_rect_from_native(crop: PixelCrop) -> Result<PixelRect, ChunkDropReason> {
    PixelRect::new(crop.x, crop.y, crop.width, crop.height)
        .map_err(|_| ChunkDropReason::InvalidCrop)
}

const fn meta_drop_reason(error: MetaFault, crop: bool) -> ChunkDropReason {
    match (crop, error) {
        (true, _) => ChunkDropReason::InvalidCrop,
        (false, _) => ChunkDropReason::InvalidTransform,
    }
}

const fn buffer_drop_reason(error: BufferFault) -> ChunkDropReason {
    match error {
        BufferFault::MissingBuffer => ChunkDropReason::MissingBuffer,
        BufferFault::MissingNativeBuffer => ChunkDropReason::MissingNativeBuffer,
        BufferFault::MissingPlane => ChunkDropReason::MissingPlane,
        BufferFault::MissingChunk => ChunkDropReason::MissingChunk,
        BufferFault::UnmappedPlane => ChunkDropReason::UnmappedPlane,
        BufferFault::InvalidLayout => ChunkDropReason::InvalidBufferLayout,
        BufferFault::InvalidChunkBounds => ChunkDropReason::InvalidChunkBounds,
        BufferFault::InvalidDmaBuf => ChunkDropReason::InvalidDmaBuf,
    }
}

struct CaptureCallbackMetrics {
    copied_frames: AtomicU64,
    copied_bytes: AtomicU64,
    drop_reasons: [AtomicU64; ChunkDropReason::COUNT],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CaptureCallbackMetricsSnapshot {
    copied_frames: u64,
    dropped_frames: u64,
    copied_bytes: u64,
    drop_reasons: [u64; ChunkDropReason::COUNT],
}

impl Default for CaptureCallbackMetrics {
    fn default() -> Self {
        Self {
            copied_frames: AtomicU64::new(0),
            copied_bytes: AtomicU64::new(0),
            drop_reasons: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl CaptureCallbackMetrics {
    fn record(&self, stats: CopyStats) {
        if let Some(reason) = stats.drop_reason() {
            self.drop_reasons[reason.index()].fetch_add(1, Ordering::Relaxed);
        } else {
            self.copied_frames.fetch_add(1, Ordering::Relaxed);
            self.copied_bytes.fetch_add(
                u64::try_from(stats.bytes_copied()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
    }

    fn snapshot(&self) -> CaptureCallbackMetricsSnapshot {
        let drop_reasons =
            std::array::from_fn(|index| self.drop_reasons[index].load(Ordering::Relaxed));
        CaptureCallbackMetricsSnapshot {
            copied_frames: self.copied_frames.load(Ordering::Relaxed),
            dropped_frames: drop_reasons.iter().copied().sum(),
            copied_bytes: self.copied_bytes.load(Ordering::Relaxed),
            drop_reasons,
        }
    }
}

impl CaptureCallbackMetricsSnapshot {
    fn diagnostics(self) -> SourceDiagnosticsEnvelope {
        let mut drop_reasons = Map::with_capacity(ChunkDropReason::COUNT);
        for reason in ChunkDropReason::ALL {
            drop_reasons.insert(
                reason.name().to_owned(),
                Value::from(self.drop_reasons[reason.index()]),
            );
        }
        SourceDiagnosticsEnvelope::try_new(
            "wayland.pipewire.capture",
            1,
            Vec::new(),
            json!({
                "copied_frames": self.copied_frames,
                "dropped_frames": self.dropped_frames,
                "copied_bytes": self.copied_bytes,
                "drop_reasons": drop_reasons,
            }),
        )
        .expect("fixed Wayland callback diagnostics satisfy envelope bounds")
    }
}

fn publish_callback_diagnostics(
    status_writer: Option<&SourceSessionWriter>,
    metrics: &CaptureCallbackMetrics,
) {
    if let Some(status) = status_writer {
        status.publish_status_diagnostics(Some(metrics.snapshot().diagnostics()));
    }
}

#[derive(Default)]
struct AnalysisExchangeState {
    latest: Option<DecodedChunk>,
    adoption: Option<AnalysisAdoption>,
    exact_commands: VecDeque<CaptureExactCommand>,
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
    Exact(CaptureExactCommand),
    Diagnostics,
}

#[derive(Default)]
struct AnalysisExchange {
    state: Mutex<AnalysisExchangeState>,
    wake: Condvar,
}

fn analysis_wait_timeout(
    frame_deadline: Instant,
    diagnostics_deadline: Instant,
    now: Instant,
) -> Duration {
    let next_wake = if now >= frame_deadline {
        diagnostics_deadline
    } else {
        frame_deadline.min(diagnostics_deadline)
    };
    next_wake.saturating_duration_since(now)
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

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), Box<CaptureExactCommand>> {
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

    fn wait_for_event(
        &self,
        deadline: Instant,
        diagnostics_deadline: Instant,
        cancel: &AtomicBool,
    ) -> Option<AnalysisEvent> {
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
            if now >= diagnostics_deadline {
                return Some(AnalysisEvent::Diagnostics);
            }
            let timeout = analysis_wait_timeout(deadline, diagnostics_deadline, now);
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

impl CaptureExactRuntimeOwner for WaylandExactRuntime {
    type Source = WaylandPublicationSource;

    const BACKEND_NAME: &'static str = "Wayland";
    const ABORTED_BINDING_ERROR: &'static str =
        "Wayland exact runtime binding was aborted after commit";

    fn source(&self) -> &Self::Source {
        &self.source
    }

    fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }

    fn bind_routes(&mut self, authority: &ScreenCommittedState) -> anyhow::Result<bool> {
        let was_bound = self.fanout.is_some();
        if was_bound {
            return Ok(false);
        }
        let candidate = self
            .fanout_candidate
            .take()
            .ok_or_else(|| anyhow!("Wayland CPU fanout candidate was already consumed"))?;
        self.fanout = Some(candidate.bind(authority, &self.binding)?);
        Ok(true)
    }

    fn is_bound(&self) -> bool {
        self.fanout.is_some()
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

struct WaylandAnalysisState {
    analyzer: ScreenCaptureInput,
    cadence: CaptureCadence,
    pacer: CapturePacer,
    next_analysis_at: Instant,
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
        source: WaylandSourceMetadata,
        config: CaptureConfig,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Self> {
        let applied_generation = settings.values.revision();
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
        let generation = self.settings.values.revision();
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
        let mut current_values = self.settings.values.lock();
        let mut latest_snapshot = self
            .settings
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = Cell::new(0);
        let mut displaced_snapshot = None;
        let committed = adoption.authority.claim_commit()
            && commit_claimed(&adoption.authority, adoption.finalize_authority, || {
                let granted_token = current_values.config_mut().restore_token.take();
                if config.restore_token.is_none() {
                    config.restore_token = granted_token;
                }
                *current_values.config_mut() = config;
                *current_values.demand_mut() = demand;
                let committed_generation = self.settings.values.commit_revision();
                generation.set(committed_generation);
                self.analyzer = analyzer;
                self.cadence = cadence;
                self.pacer = cadence.pacer();
                self.next_analysis_at = Instant::now();
                self.applied_demand = demand;
                self.applied_generation = committed_generation;
                displaced_snapshot = fence_previous_publication(&mut latest_snapshot);
            });
        drop(latest_snapshot);
        drop(displaced_snapshot);
        let _ = adoption.done.send(committed);
        if !committed {
            return;
        }
        debug!(
            generation = generation.get(),
            "Adopted prepared Wayland screen analysis settings"
        );
    }

    fn handle_exact_command(&mut self, command: CaptureExactCommand) {
        execute_capture_exact_command(
            command,
            &self.settings.exact,
            &mut self.exact_runtimes,
            |ticket, source| {
                prepare_wayland_exact_runtime(
                    ticket,
                    source,
                    &self.settings.exact,
                    self.settings.compute_capacity_policy,
                )
            },
        );
    }

    fn publish_exact(&mut self, frame: &CaptureFrame<RawCaptureSurface>) -> anyhow::Result<()> {
        let Some(hub) = self.settings.exact.hub() else {
            return Ok(());
        };
        let Some(source) = self.settings.exact.source() else {
            return Ok(());
        };
        let Some(runtime) =
            bind_current_capture_exact_runtime(&mut self.exact_runtimes, &source, &hub, |_, _| {
                Ok(())
            })?
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
        let signature = WaylandTopologySignature {
            native_extent: Some(storage_extent),
            transform,
            ..self.source.signature.clone()
        };
        let topology = if self.source.signature == signature {
            self.source
                .topology
                .ok_or_else(|| anyhow!("Wayland topology signature had no resolved generation"))?
        } else {
            let topology = self
                .settings
                .activate_topology(&signature, storage_extent, self.source.session_generation)
                .ok_or_else(|| {
                    anyhow!("Wayland capture session became stale during topology activation")
                })?;
            self.source.signature = signature;
            self.source.topology = Some(topology);
            topology
        };
        if topology.native_extent != storage_extent {
            return Err(anyhow!(
                "Wayland frame storage extent disagreed with its resolved topology"
            ));
        }
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
            self.source.source_scale(),
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
        self.settings.exact.replace_source_if_current(
            CaptureSessionAuthority::new(self.source.session_generation),
            Some(publication_source),
        );
        Ok(frame)
    }

    fn advance_deadline(&mut self, now: Instant) -> anyhow::Result<()> {
        self.next_analysis_at = self.pacer.advance_deadline(self.next_analysis_at, now)?;
        Ok(())
    }
}

impl Drop for WaylandAnalysisState {
    fn drop(&mut self) {
        self.settings.exact.retain_owned_sources_if_current(
            CaptureSessionAuthority::new(self.source.session_generation),
            |source| source.session_generation != self.source.session_generation,
        );
    }
}

fn run_analysis_worker(
    exchange: &AnalysisExchange,
    settings: Arc<SharedSettings>,
    source: WaylandSourceMetadata,
    config: CaptureConfig,
    demand: ScreenCaptureDemand,
    cancel: &AtomicBool,
    status_writer: Option<SourceSessionWriter>,
    callback_metrics: &CaptureCallbackMetrics,
) {
    let mut state = match WaylandAnalysisState::new(settings, source, config, demand) {
        Ok(state) => state,
        Err(error) => {
            warn!(%error, "Failed to admit Wayland screen analysis extent");
            return;
        }
    };
    let mut analysis_failure_latched = false;
    let mut exact_failure_latched = false;
    let mut diagnostics_deadline = Instant::now();
    while let Some(event) =
        exchange.wait_for_event(state.next_analysis_at, diagnostics_deadline, cancel)
    {
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
            AnalysisEvent::Diagnostics => {
                publish_callback_diagnostics(status_writer.as_ref(), callback_metrics);
                diagnostics_deadline = Instant::now() + CAPTURE_DIAGNOSTICS_INTERVAL;
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

        // Every format we offer is 8-bit raw SDR, and PipeWire's raw-video
        // default without negotiated colorimetry metadata is sRGB. Replace
        // with derived values once SPA colorimetry negotiation lands.
        let frame = match state.capture_frame(
            captured_at,
            width,
            height,
            crop,
            transform,
            plane.freeze(),
            CaptureColorimetry::SRGB,
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
        if state.settings.publish_snapshot(analysis) {
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
    publish_callback_diagnostics(status_writer.as_ref(), callback_metrics);
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
    command_rx: LoopReceiver<WorkerCommand>,
    command_tx: LoopSender<WorkerCommand>,
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
    let mut native_extent_override: Option<PixelExtent> = None;
    let mut extent_corrections: u8 = 0;
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
        let Some(portal_result) = portal_result else {
            flags.portal_pending.store(false, Ordering::SeqCst);
            if !settings.session_is_current(session_generation, &flags.cancel) {
                return;
            }
            continue;
        };
        let portal = match portal_result {
            Ok(portal) => portal,
            Err(error) => {
                flags.portal_pending.store(false, Ordering::SeqCst);
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

        let (portal_guard, portal_remote, restore_token) = portal.into_parts();
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
        // Cleared only after the granted token persists: a re-pick arriving
        // between portal grant and persist would otherwise clear state the
        // worker immediately rewrites, silently reconnecting the old source.
        flags.portal_pending.store(false, Ordering::SeqCst);

        let loop_outcome = run_pipewire_loop(
            &startup.config,
            startup.demand,
            Arc::clone(&settings),
            portal_remote,
            &mut command_rx,
            command_tx.clone(),
            Arc::clone(&flags.cancel),
            session_generation,
            status_writer.clone(),
            native_extent_override,
        );
        settings.invalidate_session(session_generation);
        if let Err(error) = runtime.block_on(portal_guard.close()) {
            warn!(%error, "Wayland screencast session close reported an error");
        }
        if !settings.session_is_current(session_generation, &flags.cancel) {
            return;
        }

        let reason = match loop_outcome {
            Ok(PipeWireLoopExit::Stopped) => return,
            Ok(PipeWireLoopExit::Reselect) => {
                extent_corrections = 0;
                native_extent_override = None;
                info!("Re-opening Wayland screencast source picker");
                continue;
            }
            Ok(PipeWireLoopExit::RequiresNativeExtent(extent)) => {
                if extent_corrections >= 3 {
                    let parking =
                        park_unavailable_worker(&flags.demand_state, session_demand_epoch);
                    warn!(
                        ?extent,
                        "Wayland native extent kept changing; parking capture"
                    );
                    if let Some(status) = status_writer.as_ref() {
                        settings.publish_status_for_session(
                            session_generation,
                            &flags.cancel,
                            status,
                            |status| {
                                status.unavailable(
                                    SourceIssue::new(
                                        "wayland_exact_format_unavailable",
                                        "PipeWire kept fixating new native extents \
                                         during initial negotiation"
                                            .to_owned(),
                                        true,
                                    )
                                    .with_remediation(
                                        "check for display configuration churn and retry",
                                    ),
                                )
                            },
                        );
                    }
                    debug!(?parking, "Settled unavailable Wayland capture demand");
                    extent_corrections = 0;
                    native_extent_override = None;
                    continue;
                }
                extent_corrections += 1;
                native_extent_override = Some(extent);
                info!(
                    ?extent,
                    attempt = extent_corrections,
                    "Adopting native extent fixated by PipeWire for the next \
                     capture session"
                );
                continue;
            }
            Ok(PipeWireLoopExit::Unavailable(reason)) => {
                extent_corrections = 0;
                native_extent_override = None;
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
        // Any outcome other than a correction ends the discovery dance: the
        // next session lineage re-derives its extent from portal truth so a
        // stale override cannot burn the correction budget or retry a
        // topology that no longer exists.
        extent_corrections = 0;
        native_extent_override = None;
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
) -> Option<Result<PortalSession, hypercolor_pipewire_interop::PortalError>> {
    let request = PortalRequest {
        restore_token: config.restore_token.clone(),
    };
    tokio::select! {
        result = open_portal_session(&request) => Some(result),
        () = wait_until_worker_inactive(flags) => None,
    }
}

async fn wait_until_worker_inactive(flags: &WorkerFlags) {
    while !flags.cancel.load(Ordering::Acquire) && worker_demanded(&flags.demand_state) {
        tokio::time::sleep(WORKER_POLL_INTERVAL).await;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PipeWireLoopExit {
    Stopped,
    Reselect,
    Terminal(String),
    Unavailable(String),
    /// Initial negotiation fixated a different native extent than requested
    /// (fractionally scaled outputs report logical size through the portal
    /// while the node streams physical pixels). The worker replans the next
    /// session iteration around this extent.
    RequiresNativeExtent(PixelExtent),
}

/// Detect an initial-negotiation fixation whose only disagreement is extent.
///
/// Only fires before the first acknowledgment with no adoption or restoration
/// in flight; renegotiations after acknowledgment keep the strict rejection
/// path so a mid-stream change cannot silently rewrite committed geometry.
fn initial_native_extent_correction(
    format_state: &Mutex<PipeWireFormatState>,
    negotiated: NegotiatedVideoFormat,
) -> Option<PixelExtent> {
    let state = format_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.current_acknowledged || state.pending.is_some() || state.restoring.is_some() {
        return None;
    }
    let fixated = PixelExtent::new(negotiated.width, negotiated.height).ok()?;
    (fixated != state.current.extent).then_some(fixated)
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
    stream: &StreamControl<'_>,
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
    stream
        .update_format(&restore)
        .map_err(|error| format!("failed to request prior PipeWire format: {error}"))
}

fn reject_pipewire_format(
    stream: &StreamControl<'_>,
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

fn fence_previous_publication(
    publication: &mut WaylandCapturePublication,
) -> Option<CapturedScreenSnapshot> {
    publication.clear_latest()
}

fn terminate_pipewire_loop(
    loop_exit: &Mutex<Option<PipeWireLoopExit>>,
    outcome: PipeWireLoopExit,
) -> CallbackAction {
    let mut exit = loop_exit
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if exit.is_none() {
        *exit = Some(outcome);
    }
    CallbackAction::Quit
}

fn commit_pending_pipewire_adoption(
    stream: &StreamControl<'_>,
    user_data: &mut WaylandCaptureUserData,
    format_state: &Mutex<PipeWireFormatState>,
    negotiated: NegotiatedVideoFormat,
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
    let frame = NegotiatedFormat::from_native(negotiated);
    let Some(required_capacity) = frame.byte_len() else {
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
        offer,
        callback_buffers,
        done,
        authority,
        ..
    } = pending;
    if !commit_claimed(&authority, true, || {
        user_data.install_prepared_format(frame, callback_buffers);
        let mut state = format_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.current = request;
        state.current_offer = offer;
        state.current_acknowledged = true;
    }) {
        return Err("PipeWire format install lost its claimed commit authority".to_owned());
    }
    info!(
        width = negotiated.width,
        height = negotiated.height,
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
    portal_remote: PortalRemote,
    command_rx: &mut Option<LoopReceiver<WorkerCommand>>,
    command_tx: LoopSender<WorkerCommand>,
    cancel: Arc<AtomicBool>,
    session_generation: u64,
    status_writer: Option<SourceSessionWriter>,
    native_extent_override: Option<PixelExtent>,
) -> anyhow::Result<PipeWireLoopExit> {
    let source =
        WaylandSourceMetadata::from_stream(portal_remote.descriptor(), session_generation)?;
    let exchange = Arc::new(AnalysisExchange::default());
    let callback_metrics = Arc::new(CaptureCallbackMetrics::default());
    let loop_exit = Arc::new(Mutex::new(None::<PipeWireLoopExit>));
    // A fixated native extent from a prior iteration outranks the portal's
    // logical size: scaled outputs stream physical pixels.
    let requested_extent = native_extent_override
        .or(source.signature.logical_extent)
        .map_or_else(
            || {
                demand
                    .requested_extent()
                    .context("active Wayland capture demand must carry an extent")
            },
            Ok,
        )?;
    let requested_output_extent = demand
        .requested_extent()
        .context("active Wayland capture demand must carry an extent")?;
    // Admission rejections here are typed capacity outcomes, not stream
    // faults: park the demand as unavailable instead of feeding the generic
    // reconnect loop, which would churn portal sessions every retry interval
    // against an extent that can never be admitted.
    let initial_request = match PipeWireFormatRequest::new_with_compute_policy(
        requested_extent,
        requested_output_extent,
        config,
        settings.compute_capacity_policy,
    ) {
        Ok(request) => request,
        Err(error) => {
            return Ok(unavailable_format_outcome(
                false,
                format!("capture admission rejected extent {requested_extent:?}: {error}"),
            ));
        }
    };
    let offer = FormatOffer::new(CaptureFormatRequest {
        width: requested_extent.width(),
        height: requested_extent.height(),
        target_fps: config.target_fps,
    })?;
    let callback_capacity = NegotiatedFormat {
        width: requested_extent.width(),
        height: requested_extent.height(),
        format: SpaVideoFormat::Rgba,
    }
    .byte_len()
    .ok_or(CaptureFrameError::StorageSizeOverflow)?;
    let callback_buffers = match DoubleBuffer::try_with_capacity_and_admission(
        callback_capacity,
        &settings.admission_coordinator,
    ) {
        Ok(buffers) => buffers,
        Err(error) => {
            return Ok(unavailable_format_outcome(
                false,
                format!("capture admission rejected extent {requested_extent:?}: {error}"),
            ));
        }
    };
    let decoding_enabled = Arc::new(AtomicBool::new(false));
    let format_state = Arc::new(Mutex::new(PipeWireFormatState {
        current: initial_request,
        current_offer: offer,
        current_acknowledged: false,
        pending: None,
        restoring: None,
    }));

    let handler = WaylandStreamEvents {
        callback: WaylandCaptureUserData::with_buffers(
            Arc::clone(&exchange),
            Arc::clone(&callback_metrics),
            callback_buffers,
            Arc::clone(&decoding_enabled),
            settings.admission_coordinator.clone(),
        ),
        format_state: Arc::clone(&format_state),
        loop_exit: Arc::clone(&loop_exit),
    };
    let receiver = command_rx
        .take()
        .context("PipeWire command receiver was not returned by the previous stream")?;
    let command_handler = {
        let loop_exit = Arc::clone(&loop_exit);
        let command_exchange = Arc::clone(&exchange);
        let command_format_state = Arc::clone(&format_state);
        let command_decoding_enabled = Arc::clone(&decoding_enabled);
        move |control: &StreamControl<'_>, command| {
            match command {
                WorkerCommand::SetDemand(demand) => {
                    let active = demand.is_active();
                    if let Err(error) = control.set_active(active) {
                        warn!(active, %error, "Failed to update PipeWire stream active state");
                    }
                }
                WorkerCommand::Reselect => {
                    return terminate_pipewire_loop(&loop_exit, PipeWireLoopExit::Reselect);
                }
                WorkerCommand::Exact(command) => {
                    if let Err(command) = command_exchange.send_exact(command) {
                        match *command {
                            CaptureExactCommand::Prepare { completion, .. } => {
                                let _ = completion.send(Err(anyhow!(
                                    "Wayland analysis worker rejected exact publication preparation"
                                )));
                            }
                            CaptureExactCommand::Reap { completion, .. } => {
                                let Some(completion) = completion else {
                                    return CallbackAction::Continue;
                                };
                                let _ = completion.send(Err(anyhow!(
                                    "Wayland analysis worker rejected exact publication retirement"
                                )));
                            }
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
                        return CallbackAction::Continue;
                    }
                    if !matches!(
                        decision.recv_timeout(WORKER_READY_TIMEOUT),
                        Ok(SettingsDecision::Commit)
                    ) {
                        authority.cancel();
                        return CallbackAction::Continue;
                    }
                    if authority.is_cancelled() {
                        return CallbackAction::Continue;
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
                        return CallbackAction::Continue;
                    }
                    if analysis_ready_rx
                        .recv_timeout(WORKER_READY_TIMEOUT)
                        .is_err()
                    {
                        authority.cancel();
                        let _ = done.send(Err(
                            "Wayland analysis worker exited before adoption".to_owned()
                        ));
                        return CallbackAction::Continue;
                    }

                    if let Some(PreparedPipeWireFormat {
                        callback_buffers,
                        offer,
                        request,
                    }) = pipewire_format
                    {
                        let cancellation_done = done.clone();
                        let pending = PendingPipeWireAdoption {
                            id: adoption_id,
                            request,
                            offer,
                            callback_buffers,
                            analysis_decision: analysis_decision_tx,
                            analysis_done: analysis_done_rx,
                            done,
                            authority: Arc::clone(&authority),
                        };
                        let update_offer = pending.offer;
                        {
                            let state = command_format_state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if !state.current_acknowledged {
                                pending.authority.cancel();
                                let _ = pending.done.send(Err(
                                    "PipeWire has not acknowledged the initial exact format"
                                        .to_owned(),
                                ));
                                return CallbackAction::Continue;
                            }
                            if !state.can_begin_adoption() {
                                pending.authority.cancel();
                                let _ = pending
                                    .done
                                    .send(Err("PipeWire already has an unsettled format adoption"
                                        .to_owned()));
                                return CallbackAction::Continue;
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
                                "Wayland format adoption was cancelled before negotiation"
                                    .to_owned(),
                            ));
                            return CallbackAction::Continue;
                        }
                        if let Err(error) = control.update_format(&update_offer) {
                            let pending = command_format_state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .cancel(adoption_id)
                                .expect("failed PipeWire update retains pending adoption");
                            command_decoding_enabled.store(false, Ordering::Release);
                            command_exchange.discard_latest_frame();
                            if let Err(restore_error) = request_pipewire_restoration(
                                control,
                                &command_format_state,
                                pending,
                                error.to_string(),
                            ) {
                                return terminate_pipewire_loop(
                                    &loop_exit,
                                    PipeWireLoopExit::Terminal(restore_error),
                                );
                            }
                        }
                        return CallbackAction::Continue;
                    }

                    if analysis_decision_tx.send(SettingsDecision::Commit).is_err() {
                        authority.cancel();
                        let _ = done.send(Err(
                            "Wayland analysis worker exited during adoption".to_owned()
                        ));
                        return CallbackAction::Continue;
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
                            control,
                            &command_format_state,
                            pending,
                            "PipeWire format adoption timed out".to_owned(),
                        ) {
                            return terminate_pipewire_loop(
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
                WorkerCommand::AnalysisExited => {
                    return terminate_pipewire_loop(
                        &loop_exit,
                        PipeWireLoopExit::Terminal("Wayland analysis worker panicked".to_owned()),
                    );
                }
                WorkerCommand::Stop => {
                    return terminate_pipewire_loop(&loop_exit, PipeWireLoopExit::Stopped);
                }
            }
            CallbackAction::Continue
        }
    };
    let mut session =
        match connect_stream(portal_remote, &offer, receiver, handler, command_handler) {
            Ok(session) => session,
            Err(error) => {
                let (error, receiver) = error.into_parts();
                *command_rx = Some(receiver);
                return Err(error.into());
            }
        };

    let analysis_exchange = Arc::clone(&exchange);
    let analysis_metrics = Arc::clone(&callback_metrics);
    let analysis_cancel = Arc::clone(&cancel);
    let analysis_config = config.clone();
    let analysis_demand = demand;
    let analysis_spawn = spawn_worker(
        thread::Builder::new().name("hypercolor-screen-analysis".to_owned()),
        move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_analysis_worker(
                    &analysis_exchange,
                    settings,
                    source,
                    analysis_config,
                    analysis_demand,
                    &analysis_cancel,
                    status_writer,
                    &analysis_metrics,
                );
            }));
            if result.is_err() {
                let _ = command_tx.send(WorkerCommand::AnalysisExited);
            }
        },
    );
    let analysis_handle = match analysis_spawn {
        Ok(handle) => handle,
        Err(error) => {
            exchange.stop();
            let (receiver, disconnect_result) = session.disconnect();
            *command_rx = Some(receiver);
            let spawn_error =
                anyhow::Error::new(error).context("failed to spawn Wayland screen analysis worker");
            return match disconnect_result {
                Ok(()) => Err(spawn_error),
                Err(disconnect_error) => Err(spawn_error.context(format!(
                    "PipeWire stream disconnect after analysis spawn failure also failed: \
                     {disconnect_error}"
                ))),
            };
        }
    };

    let run_result = session.run();
    exchange.stop();
    let (receiver, disconnect_result) = session.disconnect();
    *command_rx = Some(receiver);
    analysis_handle
        .join()
        .map_err(|panic| anyhow!("Wayland analysis worker join failed: {panic:?}"))?;
    run_result
        .and(disconnect_result)
        .context("PipeWire stream session failed")?;
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
