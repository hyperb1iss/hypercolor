//! Platform-neutral surface: errors, frame view, and the subsample math.

use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Screen capture result type.
pub type CaptureResult<T> = Result<T, CaptureError>;

/// Source-owned logical backing classes that can outlive one prepared output plan.
///
/// Byte accounting covers texture payloads, constant buffers, host planes, and
/// explicitly quoted Rust metadata. Opaque driver objects, resource views,
/// queries, heap alignment, and allocator bookkeeping remain backend baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureResourceKind {
    /// CPU pointer-shape payload retained across metadata generations.
    PointerShape,
    /// Canonical clean desktop retained between duplication acquisitions.
    CanonicalDesktop,
    /// Cursor texture shared by exact GPU publication lanes.
    PointerTexture,
    /// Constant buffer retained by the compatibility reduction path.
    CompatibilityReductionConstantBuffer,
    /// Output and staging-ring textures retained by compatibility reduction.
    CompatibilityReductionTextures,
    /// CPU-readable full-desktop staging texture used by compatibility capture.
    CompatibilityCpuStagingTexture,
    /// Packed host frame plane returned by compatibility capture.
    CompatibilityFramePlane,
}

/// Immutable ownership of one admitted source allocation.
pub trait CaptureResourceLease: std::fmt::Debug + Send + Sync {
    /// Allocation class owned by this lease.
    fn kind(&self) -> CaptureResourceKind;

    /// Exact retained logical backing bytes within the documented boundary.
    fn bytes(&self) -> u64;
}

/// Mutable peak quote held across fallible source allocation.
pub trait CaptureResourceReservation: std::fmt::Debug + Send {
    /// Allocation class reserved by this quote.
    fn kind(&self) -> CaptureResourceKind;

    /// Peak bytes currently reserved.
    fn bytes(&self) -> u64;

    /// Reconcile temporary quote slack and freeze the retained ownership.
    ///
    /// # Errors
    ///
    /// Rejects a retained size larger than the original peak quote.
    fn commit(self: Box<Self>, retained_bytes: u64)
    -> CaptureResult<Arc<dyn CaptureResourceLease>>;
}

/// Admission authority for allocations owned by a live capture source.
pub trait CaptureResourceAdmission: std::fmt::Debug + Send + Sync {
    /// Reserve a peak quote before any corresponding allocation occurs.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::ResourceExhausted`] when the shared fence cannot
    /// admit the quote.
    fn try_reserve(
        &self,
        kind: CaptureResourceKind,
        peak_bytes: u64,
    ) -> CaptureResult<Box<dyn CaptureResourceReservation>>;
}

#[derive(Debug)]
struct UnboundedCaptureResourceAdmission;

#[derive(Debug)]
struct UnboundedCaptureResourceReservation {
    kind: CaptureResourceKind,
    bytes: u64,
}

#[derive(Debug)]
struct UnboundedCaptureResourceLease {
    kind: CaptureResourceKind,
    bytes: u64,
}

impl CaptureResourceAdmission for UnboundedCaptureResourceAdmission {
    fn try_reserve(
        &self,
        kind: CaptureResourceKind,
        peak_bytes: u64,
    ) -> CaptureResult<Box<dyn CaptureResourceReservation>> {
        Ok(Box::new(UnboundedCaptureResourceReservation {
            kind,
            bytes: peak_bytes,
        }))
    }
}

impl CaptureResourceReservation for UnboundedCaptureResourceReservation {
    fn kind(&self) -> CaptureResourceKind {
        self.kind
    }

    fn bytes(&self) -> u64 {
        self.bytes
    }

    fn commit(
        self: Box<Self>,
        retained_bytes: u64,
    ) -> CaptureResult<Arc<dyn CaptureResourceLease>> {
        if retained_bytes > self.bytes {
            return Err(CaptureError::ResourceAdmissionMismatch {
                operation: "commit default capture resource reservation",
                expected_kind: self.kind,
                expected_bytes: self.bytes,
                actual_kind: self.kind,
                actual_bytes: retained_bytes,
            });
        }
        Ok(Arc::new(UnboundedCaptureResourceLease {
            kind: self.kind,
            bytes: retained_bytes,
        }))
    }
}

impl CaptureResourceLease for UnboundedCaptureResourceLease {
    fn kind(&self) -> CaptureResourceKind {
        self.kind
    }

    fn bytes(&self) -> u64 {
        self.bytes
    }
}

pub(crate) fn default_capture_resource_admission() -> Arc<dyn CaptureResourceAdmission> {
    Arc::new(UnboundedCaptureResourceAdmission)
}

pub(crate) fn reserve_capture_resource(
    admission: &dyn CaptureResourceAdmission,
    kind: CaptureResourceKind,
    peak_bytes: u64,
    operation: &'static str,
) -> CaptureResult<Box<dyn CaptureResourceReservation>> {
    let reservation = admission.try_reserve(kind, peak_bytes)?;
    if reservation.kind() != kind || reservation.bytes() != peak_bytes {
        return Err(CaptureError::ResourceAdmissionMismatch {
            operation,
            expected_kind: kind,
            expected_bytes: peak_bytes,
            actual_kind: reservation.kind(),
            actual_bytes: reservation.bytes(),
        });
    }
    Ok(reservation)
}

pub(crate) fn commit_capture_resource(
    reservation: Box<dyn CaptureResourceReservation>,
    retained_bytes: u64,
    operation: &'static str,
) -> CaptureResult<Arc<dyn CaptureResourceLease>> {
    let kind = reservation.kind();
    let lease = reservation.commit(retained_bytes)?;
    if lease.kind() != kind || lease.bytes() != retained_bytes {
        return Err(CaptureError::ResourceAdmissionMismatch {
            operation,
            expected_kind: kind,
            expected_bytes: retained_bytes,
            actual_kind: lease.kind(),
            actual_bytes: lease.bytes(),
        });
    }
    Ok(lease)
}

/// Independent outcome for one requested lane in a hybrid capture pump.
#[derive(Debug)]
pub enum CaptureLane<T> {
    /// The caller did not request this lane.
    NotRequested,
    /// The lane is healthy but produced no result in this pump cycle.
    Idle,
    /// Every bounded slot or output allocation is still owned in flight.
    Busy,
    /// The lane produced one result.
    Ready(T),
    /// This lane failed without suppressing work in the sibling lane.
    Failed(CaptureError),
}

/// Exact algorithm revision implemented by the D3D11 Surface shader.
pub const GPU_SURFACE_ALGORITHM_REVISION: NonZeroU32 = NonZeroU32::MIN;

/// Active Windows capture reduction implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReductionPath {
    /// D3D11 compute reduction with pipelined reduced-surface readback.
    Gpu,
    /// Full-quality CPU box reduction used when the GPU path is unavailable.
    #[default]
    CpuFallback,
}

/// Snapshot of capture reduction health and throughput counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReductionTelemetry {
    /// Currently active implementation.
    pub path: ReductionPath,
    /// GPU reductions submitted to the immediate context.
    pub gpu_submitted: u64,
    /// GPU reductions whose reduced surface reached the CPU.
    pub gpu_completed: u64,
    /// Frames reduced by the full-quality CPU fallback.
    pub cpu_completed: u64,
    /// Submissions coalesced because every staging slot was still busy.
    pub ring_busy: u64,
    /// Bytes copied from reduced staging surfaces to CPU memory.
    pub readback_bytes: u64,
    /// GPU initialization or execution failures that selected fallback.
    pub gpu_failures: u64,
    /// Degraded-path reason, absent while GPU reduction is healthy.
    pub issue: Option<Arc<str>>,
}

/// Stable caller-owned identity for one exact GPU Surface descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GpuSurfaceDescriptorId(NonZeroU64);

impl GpuSurfaceDescriptorId {
    /// Construct a non-zero descriptor identity.
    #[must_use]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// Numeric identity used by the publication coordinator.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Committed screen-plan generation fenced into GPU Surface results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GpuSurfacePlanGeneration(NonZeroU64);

impl GpuSurfacePlanGeneration {
    /// Construct a non-zero plan generation.
    #[must_use]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// Monotonic committed generation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Stable identity of the DXGI adapter that owns a native GPU Surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GpuAdapterLuid {
    low_part: u32,
    high_part: i32,
}

impl GpuAdapterLuid {
    /// Construct an adapter identity from the two Windows LUID components.
    #[must_use]
    pub const fn new(low_part: u32, high_part: i32) -> Self {
        Self {
            low_part,
            high_part,
        }
    }

    /// Unsigned low 32 bits reported by DXGI.
    #[must_use]
    pub const fn low_part(self) -> u32 {
        self.low_part
    }

    /// Signed high 32 bits reported by DXGI.
    #[must_use]
    pub const fn high_part(self) -> i32 {
        self.high_part
    }
}

/// Stable native texture slot identity within one committed Surface plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GpuSurfaceSlotId(NonZeroU64);

impl GpuSurfaceSlotId {
    /// Construct a non-zero native slot identity.
    #[must_use]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// Numeric identity paired with plan generation and adapter identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Exact raster filter requested from the Windows GPU executor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GpuSurfaceFilter {
    /// Center-mapped nearest-neighbor sampling.
    #[default]
    Nearest,
    /// Four-tap center-mapped interpolation.
    Bilinear,
    /// Area-weighted source coverage.
    Area,
}

/// Pixel storage format participating in exact GPU Surface identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GpuSurfaceFormat {
    /// DXGI `B8G8R8A8_UNORM`, the Desktop Duplication source format.
    Bgra8Unorm,
    /// DXGI `R8G8B8A8_UNORM`, the shareable Surface output format.
    #[default]
    Rgba8Unorm,
    /// Linear half-float RGBA storage.
    Rgba16Float,
}

/// Exact color operation requested from the Windows GPU executor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GpuSurfaceColorPipeline {
    /// Preserve encoded channel values while changing BGRA storage to RGBA.
    #[default]
    PreserveEncoded,
    /// Decode and process SDR samples in linear light before re-encoding.
    LinearSdr,
    /// Tone-map HDR samples into an SDR output contract.
    ToneMapHdrToSdr,
}

/// Coordinate system used by an exact GPU Surface descriptor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GpuSurfaceCoordinateSpace {
    /// Upright display coordinates after applying the DXGI output rotation.
    #[default]
    LogicalDisplay,
    /// Raw coordinates in the duplicated scanout texture.
    NativeScanout,
}

/// DXGI output color space participating in exact source identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GpuSurfaceSourceColorSpace {
    /// Full-range SDR RGB using the DXGI G22 transfer and Rec. 709 primaries.
    RgbFullG22P709,
    /// Full-range linear RGB using Rec. 709 primaries.
    RgbFullLinearP709,
    /// Full-range PQ RGB using Rec. 2020 primaries.
    RgbFullPqP2020,
    /// A DXGI color-space value outside this executor's declared vocabulary.
    #[default]
    Unknown,
}

/// Cursor treatment participating in exact GPU Surface identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GpuSurfaceCursorPolicy {
    /// Publish the clean desktop without the separately reported pointer.
    #[default]
    Exclude,
    /// Composite the current pointer before publication.
    Include,
}

/// Typed reason an exact GPU Surface descriptor cannot execute on this backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuSurfaceUnsupportedReason {
    /// The requested raster filter has no byte-exact shader implementation.
    Filter(GpuSurfaceFilter),
    /// The requested output storage has no shareable typed-UAV implementation.
    OutputFormat(GpuSurfaceFormat),
    /// The requested color operation has no byte-exact shader implementation.
    ColorPipeline(GpuSurfaceColorPipeline),
    /// The requested source coordinates are not normalized logical pixels.
    CoordinateSpace(GpuSurfaceCoordinateSpace),
    /// The duplicated source color space is not byte-exact for revision 1.
    SourceColorSpace(GpuSurfaceSourceColorSpace),
    /// The descriptor names another reduction algorithm revision.
    AlgorithmRevision(NonZeroU32),
    /// The duplicated source format is outside the exact shader contract.
    SourceFormat,
    /// The D3D11 device cannot expose shareable fence synchronization.
    SharedFenceUnavailable,
}

impl std::fmt::Display for GpuSurfaceUnsupportedReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter(filter) => write!(formatter, "unsupported exact filter {filter:?}"),
            Self::OutputFormat(format) => {
                write!(formatter, "unsupported exact output format {format:?}")
            }
            Self::ColorPipeline(pipeline) => {
                write!(formatter, "unsupported exact color pipeline {pipeline:?}")
            }
            Self::CoordinateSpace(coordinates) => {
                write!(
                    formatter,
                    "unsupported exact coordinate space {coordinates:?}"
                )
            }
            Self::SourceColorSpace(color_space) => {
                write!(
                    formatter,
                    "unsupported exact source color space {color_space:?}"
                )
            }
            Self::AlgorithmRevision(revision) => {
                write!(formatter, "unsupported exact algorithm revision {revision}")
            }
            Self::SourceFormat => formatter.write_str("unsupported exact source format"),
            Self::SharedFenceUnavailable => formatter.write_str("shared D3D11 fences unavailable"),
        }
    }
}

/// Screen capture failures.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// A requested capture extent was empty.
    #[error("capture extent must be non-zero, got {width}x{height}")]
    InvalidExtent {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },

    /// A capture resource could not reserve the requested storage.
    #[error("{operation} could not reserve {requested_bytes} bytes")]
    ResourceExhausted {
        /// Resource operation that failed.
        operation: &'static str,
        /// Number of bytes requested by the operation.
        requested_bytes: usize,
    },

    /// An admission implementation returned ownership for another quote.
    #[error(
        "{operation} admission mismatch: expected {expected_kind:?}/{expected_bytes} bytes, got {actual_kind:?}/{actual_bytes} bytes"
    )]
    ResourceAdmissionMismatch {
        /// Resource operation whose ownership proof was invalid.
        operation: &'static str,
        /// Allocation class requested by the capture source.
        expected_kind: CaptureResourceKind,
        /// Exact bytes requested or committed by the capture source.
        expected_bytes: u64,
        /// Allocation class returned by the admission implementation.
        actual_kind: CaptureResourceKind,
        /// Bytes returned by the admission implementation.
        actual_bytes: u64,
    },

    /// A replacement capture session could not reserve its resources.
    #[error("capture session rebuild: {operation} could not reserve {requested_bytes} bytes")]
    SessionResourceExhausted {
        /// Resource operation that failed.
        operation: &'static str,
        /// Number of bytes requested by the operation.
        requested_bytes: usize,
    },

    /// Resolution-derived byte geometry could not be represented safely.
    #[error("{operation} byte geometry overflows for {width}x{height}")]
    GeometryOverflow {
        /// Resource operation that failed.
        operation: &'static str,
        /// Width whose byte geometry overflowed.
        width: u32,
        /// Height whose byte geometry overflowed.
        height: u32,
    },

    /// A descriptor requests semantics this exact GPU executor cannot reproduce.
    #[error("GPU Surface descriptor {descriptor_id:?}: {reason}")]
    UnsupportedGpuSurface {
        /// Exact descriptor rejected before publication.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Unsupported operation or backend capability.
        reason: GpuSurfaceUnsupportedReason,
    },

    /// One candidate plan repeats a descriptor identity.
    #[error("GPU Surface descriptor identity {descriptor_id:?} is duplicated")]
    DuplicateGpuSurfaceDescriptor {
        /// Repeated descriptor identity.
        descriptor_id: GpuSurfaceDescriptorId,
    },

    /// A prepared plan does not contain the requested descriptor identity.
    #[error("GPU Surface descriptor identity {descriptor_id:?} is not prepared")]
    GpuSurfaceDescriptorNotPrepared {
        /// Descriptor absent from the prepared native plan.
        descriptor_id: GpuSurfaceDescriptorId,
    },

    /// A descriptor source rectangle escapes its declared coordinate space.
    #[error(
        "GPU Surface descriptor {descriptor_id:?} region is outside source {source_width}x{source_height}"
    )]
    GpuSurfaceRegionOutOfBounds {
        /// Exact descriptor with invalid source geometry.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Prepared source width in the descriptor coordinate space.
        source_width: u32,
        /// Prepared source height in the descriptor coordinate space.
        source_height: u32,
    },

    /// A descriptor was prepared for another physical output orientation.
    #[error(
        "GPU Surface descriptor {descriptor_id:?} expects {descriptor_rotation:?} but source is {source_rotation:?}"
    )]
    GpuSurfaceRotationMismatch {
        /// Exact descriptor with stale source orientation.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Rotation encoded into the descriptor identity.
        descriptor_rotation: DisplayRotation,
        /// Rotation reported by the current duplication source.
        source_rotation: DisplayRotation,
    },

    /// A native Surface publication was already claimed, expired, or superseded.
    #[error(
        "GPU Surface descriptor {descriptor_id:?} sequence {source_sequence} is no longer claimable"
    )]
    GpuSurfaceUseUnavailable {
        /// Descriptor whose single native hand-off was requested.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Source acquisition sequence bound to that hand-off.
        source_sequence: u64,
    },

    /// Exact cursor composition is waiting for the matching pointer shape.
    #[error(
        "GPU Surface descriptor {descriptor_id:?} sequence {source_sequence} has no visible cursor shape"
    )]
    GpuSurfaceCursorShapeUnavailable {
        /// Cursor-including descriptor that cannot yet publish exactly.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Source acquisition whose visible cursor lacks shape pixels.
        source_sequence: u64,
    },

    /// A claimed native hand-off disappeared before queuing its release.
    #[error(
        "GPU Surface descriptor {descriptor_id:?} use {use_id} lost its claim guard before release"
    )]
    GpuSurfacePlanPoisoned {
        /// Descriptor whose ownership can no longer be proven.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Native slot use generation that violated the hand-off.
        use_id: u64,
    },

    /// Exact GPU resources exceed the caller-supplied byte ledger.
    #[error(
        "GPU Surface plan requires {requested_bytes} bytes but its admission budget is {budget_bytes} bytes"
    )]
    GpuSurfaceBudgetExceeded {
        /// Checked bytes required by clean and output textures.
        requested_bytes: u64,
        /// Explicit candidate-plan budget.
        budget_bytes: u64,
    },

    /// Exact native publication needs at least two independently owned slots.
    #[error(
        "GPU Surface plan requested {requested} in-flight slot but requires at least {minimum}"
    )]
    GpuSurfaceInFlightDepthTooSmall {
        /// Requested reusable slots per descriptor.
        requested: u32,
        /// Minimum required to avoid one claimant stalling acquisition.
        minimum: u32,
    },

    /// A prepared GPU plan belongs to another capture source or device epoch.
    #[error("GPU Surface plan no longer matches the active capture source epoch")]
    GpuSurfacePlanInvalidated,

    /// Publication timestamps cannot represent the requested freshness window.
    #[error("GPU Surface freshness timestamp overflowed")]
    GpuSurfaceFreshnessOverflow,

    /// A per-slot shared-fence sequence cannot advance without aliasing old work.
    #[error("GPU Surface synchronization sequence exhausted")]
    GpuSurfaceSynchronizationExhausted,

    /// A mapped capture surface reported unusable row geometry.
    #[error("{operation} returned invalid {width}x{height} row geometry with pitch {row_pitch}")]
    InvalidBufferGeometry {
        /// Resource operation that failed.
        operation: &'static str,
        /// Mapped surface width.
        width: u32,
        /// Mapped surface height.
        height: u32,
        /// Reported row pitch in bytes.
        row_pitch: usize,
    },

    /// Desktop Duplication is a Windows-only API.
    #[error("desktop screen capture is only available on Windows")]
    UnsupportedPlatform,

    /// No display output matched the requested monitor index.
    #[error("monitor {requested} not found ({available} attached)")]
    MonitorNotFound {
        /// Zero-based monitor index that was requested.
        requested: usize,
        /// How many outputs enumeration actually found.
        available: usize,
    },

    /// No attached output has the requested stable source id.
    #[error("display source {requested:?} is no longer attached")]
    SourceNotFound {
        /// Stable source id that was requested.
        requested: String,
    },

    /// Windows has no free Desktop Duplication client slot for this session.
    ///
    /// The operating system caps concurrent duplication clients per session.
    #[error("desktop duplication concurrency limit reached")]
    AlreadyDuplicating,

    /// The active desktop cannot currently be accessed, such as during UAC.
    #[error("the active Windows desktop is not accessible")]
    AccessDenied,

    /// The interactive Windows session disconnected or switched away.
    #[error("the interactive Windows session is disconnected")]
    SessionUnavailable,

    /// The graphics device was removed or reset.
    #[error("the capture graphics device was removed or reset")]
    DeviceLost,

    /// The duplicated desktop changed and the capture session must reopen it.
    #[error("desktop duplication access was lost during a display transition")]
    AccessLost,

    /// A Windows capture operation exceeded its wait budget.
    #[error("the Windows capture operation timed out")]
    Timeout,

    /// A Windows API call failed.
    ///
    /// Carries a rendered message rather than the `windows` error type: with
    /// `default-features = false` that type does not implement
    /// `std::error::Error`, and the HRESULT text is the part worth keeping.
    #[error("{context}: {message}")]
    Windows {
        /// What we were attempting.
        context: &'static str,
        /// Rendered HRESULT description.
        message: String,
    },
}

/// Validated non-empty reduction extent requested by a capture consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureExtent {
    width: u32,
    height: u32,
}

impl CaptureExtent {
    /// Construct a non-empty requested capture extent.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidExtent`] when either dimension is zero.
    pub const fn try_new(width: u32, height: u32) -> CaptureResult<Self> {
        if width == 0 || height == 0 {
            return Err(CaptureError::InvalidExtent { width, height });
        }
        Ok(Self { width, height })
    }

    /// Requested width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Requested height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

// Only the cfg(windows) duplication module builds this variant, so the
// constructor must be gated with it: on Linux it would have no callers and
// the workspace's -D warnings turns dead code into a build failure.
#[cfg(target_os = "windows")]
impl CaptureError {
    /// Build a [`CaptureError::Windows`] from anything printable.
    pub(crate) fn windows(context: &'static str, message: impl std::fmt::Display) -> Self {
        Self::Windows {
            context,
            message: message.to_string(),
        }
    }
}

/// Pending display rotation reported by Desktop Duplication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayRotation {
    /// Pixels already share the logical display orientation.
    #[default]
    Identity,
    /// Rotate 90 degrees clockwise.
    Clockwise90,
    /// Rotate 180 degrees.
    Clockwise180,
    /// Rotate 270 degrees clockwise.
    Clockwise270,
}

/// Non-empty pixel rectangle selected for capture or publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRegion {
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
}

/// Complete byte-changing identity for one exact shareable GPU Surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuSurfaceDescriptor {
    id: GpuSurfaceDescriptorId,
    source_region: CaptureRegion,
    coordinate_space: GpuSurfaceCoordinateSpace,
    source_rotation: DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    output_extent: CaptureExtent,
    filter: GpuSurfaceFilter,
    format: GpuSurfaceFormat,
    color_pipeline: GpuSurfaceColorPipeline,
    cursor: GpuSurfaceCursorPolicy,
    algorithm_revision: NonZeroU32,
    freshness: Duration,
}

/// Inputs for constructing one exact GPU Surface descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuSurfaceDescriptorConfig {
    /// Stable identity supplied by the screen publication plan.
    pub id: GpuSurfaceDescriptorId,
    /// Rectangle sampled in the declared source coordinate space.
    pub source_region: CaptureRegion,
    /// Coordinate system in which `source_region` is expressed.
    pub coordinate_space: GpuSurfaceCoordinateSpace,
    /// DXGI rotation normalized by this exact descriptor.
    pub source_rotation: DisplayRotation,
    /// DXGI color space whose encoded channel values are consumed.
    pub source_color_space: GpuSurfaceSourceColorSpace,
    /// Exact output raster extent.
    pub output_extent: CaptureExtent,
    /// Exact raster filter.
    pub filter: GpuSurfaceFilter,
    /// Exact output storage.
    pub format: GpuSurfaceFormat,
    /// Exact color operation.
    pub color_pipeline: GpuSurfaceColorPipeline,
    /// Exact cursor treatment.
    pub cursor: GpuSurfaceCursorPolicy,
    /// Revision of the complete reduction algorithm.
    pub algorithm_revision: NonZeroU32,
    /// Maximum age at which this publication remains deliverable.
    pub freshness: Duration,
}

impl GpuSurfaceDescriptor {
    /// Construct a complete exact descriptor from already validated scalar types.
    #[must_use]
    pub const fn new(config: GpuSurfaceDescriptorConfig) -> Self {
        Self {
            id: config.id,
            source_region: config.source_region,
            coordinate_space: config.coordinate_space,
            source_rotation: config.source_rotation,
            source_color_space: config.source_color_space,
            output_extent: config.output_extent,
            filter: config.filter,
            format: config.format,
            color_pipeline: config.color_pipeline,
            cursor: config.cursor,
            algorithm_revision: config.algorithm_revision,
            freshness: config.freshness,
        }
    }

    /// Stable publication-plan identity.
    #[must_use]
    pub const fn id(&self) -> GpuSurfaceDescriptorId {
        self.id
    }

    /// Rectangle sampled by the shader in the declared coordinate space.
    #[must_use]
    pub const fn source_region(&self) -> CaptureRegion {
        self.source_region
    }

    /// Coordinate system of the exact source rectangle.
    #[must_use]
    pub const fn coordinate_space(&self) -> GpuSurfaceCoordinateSpace {
        self.coordinate_space
    }

    /// Physical DXGI output rotation normalized by the shader.
    #[must_use]
    pub const fn source_rotation(&self) -> DisplayRotation {
        self.source_rotation
    }

    /// Exact DXGI source color space represented by encoded input channels.
    #[must_use]
    pub const fn source_color_space(&self) -> GpuSurfaceSourceColorSpace {
        self.source_color_space
    }

    /// Exact output raster extent.
    #[must_use]
    pub const fn output_extent(&self) -> CaptureExtent {
        self.output_extent
    }

    /// Exact raster filter.
    #[must_use]
    pub const fn filter(&self) -> GpuSurfaceFilter {
        self.filter
    }

    /// Exact output storage.
    #[must_use]
    pub const fn format(&self) -> GpuSurfaceFormat {
        self.format
    }

    /// Exact color operation.
    #[must_use]
    pub const fn color_pipeline(&self) -> GpuSurfaceColorPipeline {
        self.color_pipeline
    }

    /// Exact cursor treatment.
    #[must_use]
    pub const fn cursor(&self) -> GpuSurfaceCursorPolicy {
        self.cursor
    }

    /// Reduction algorithm revision.
    #[must_use]
    pub const fn algorithm_revision(&self) -> NonZeroU32 {
        self.algorithm_revision
    }

    /// Maximum deliverable age for one result.
    #[must_use]
    pub const fn freshness(&self) -> Duration {
        self.freshness
    }

    /// Validate the exact operations implemented by the current D3D11 shader.
    ///
    /// # Errors
    ///
    /// Returns a typed unsupported reason instead of silently substituting an
    /// approximate filter, storage format, or color operation.
    pub const fn validate_exact_gpu(&self) -> CaptureResult<()> {
        if !matches!(
            self.coordinate_space,
            GpuSurfaceCoordinateSpace::LogicalDisplay
        ) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::CoordinateSpace(self.coordinate_space),
            });
        }
        if !matches!(self.filter, GpuSurfaceFilter::Nearest) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::Filter(self.filter),
            });
        }
        if !matches!(self.format, GpuSurfaceFormat::Rgba8Unorm) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::OutputFormat(self.format),
            });
        }
        if !matches!(
            self.color_pipeline,
            GpuSurfaceColorPipeline::PreserveEncoded
        ) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::ColorPipeline(self.color_pipeline),
            });
        }
        if !matches!(
            self.source_color_space,
            GpuSurfaceSourceColorSpace::RgbFullG22P709
        ) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::SourceColorSpace(self.source_color_space),
            });
        }
        if self.algorithm_revision.get() != GPU_SURFACE_ALGORITHM_REVISION.get() {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::AlgorithmRevision(self.algorithm_revision),
            });
        }
        Ok(())
    }

    /// Validate the exact GPU reduction/readback operations implemented by
    /// the current D3D11 shader.
    ///
    /// Unlike shareable renderer surfaces, reduced readback supports every
    /// raster filter and same-encoding SDR processing. The output remains
    /// tightly packed RGBA8 and source coordinates remain logical pixels.
    ///
    /// # Errors
    ///
    /// Returns a typed unsupported reason without changing the requested
    /// descriptor or silently falling back to approximate GPU work.
    pub const fn validate_exact_gpu_readback(&self) -> CaptureResult<()> {
        if !matches!(
            self.coordinate_space,
            GpuSurfaceCoordinateSpace::LogicalDisplay
        ) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::CoordinateSpace(self.coordinate_space),
            });
        }
        if !matches!(self.format, GpuSurfaceFormat::Rgba8Unorm) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::OutputFormat(self.format),
            });
        }
        if !matches!(
            self.color_pipeline,
            GpuSurfaceColorPipeline::PreserveEncoded | GpuSurfaceColorPipeline::LinearSdr
        ) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::ColorPipeline(self.color_pipeline),
            });
        }
        if !matches!(
            self.source_color_space,
            GpuSurfaceSourceColorSpace::RgbFullG22P709
        ) {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::SourceColorSpace(self.source_color_space),
            });
        }
        if self.algorithm_revision.get() != GPU_SURFACE_ALGORITHM_REVISION.get() {
            return Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id: self.id,
                reason: GpuSurfaceUnsupportedReason::AlgorithmRevision(self.algorithm_revision),
            });
        }
        Ok(())
    }
}

/// Checked pre-allocation resources for one descriptor-keyed GPU readback plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuReductionResourceQuote {
    allocation_byte_len: u64,
    constant_buffer_byte_len: u64,
    readback_byte_len: u64,
    publication_buffer_byte_len: usize,
    metadata_byte_len: u64,
    retained_byte_len: u64,
}

impl GpuReductionResourceQuote {
    /// Output UAV and staging-ring texture bytes.
    #[must_use]
    pub const fn allocation_byte_len(self) -> u64 {
        self.allocation_byte_len
    }

    /// Constant-buffer bytes retained by descriptor-local reducers.
    #[must_use]
    pub const fn constant_buffer_byte_len(self) -> u64 {
        self.constant_buffer_byte_len
    }

    /// Staging-ring texture bytes within the full GPU allocation.
    #[must_use]
    pub const fn readback_byte_len(self) -> u64 {
        self.readback_byte_len
    }

    /// Tightly packed host buffers retained for publication callbacks.
    #[must_use]
    pub const fn publication_buffer_byte_len(self) -> usize {
        self.publication_buffer_byte_len
    }

    /// Rust-owned route, descriptor, and staging-slot payloads.
    ///
    /// This excludes allocator bookkeeping and opaque COM/driver internals.
    #[must_use]
    pub const fn metadata_byte_len(self) -> u64 {
        self.metadata_byte_len
    }

    /// GPU textures and constants, host publication buffers, and metadata.
    #[must_use]
    pub const fn retained_byte_len(self) -> u64 {
        self.retained_byte_len
    }
}

/// Caller-supplied resource ledger for descriptor-keyed GPU readback rings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuReductionAdmission {
    max_texture_bytes: u64,
    slots_per_descriptor: NonZeroU32,
}

impl GpuReductionAdmission {
    /// Define the byte budget and reusable asynchronous readback depth.
    #[must_use]
    pub const fn new(max_texture_bytes: u64, slots_per_descriptor: NonZeroU32) -> Self {
        Self {
            max_texture_bytes,
            slots_per_descriptor,
        }
    }

    /// Maximum admitted bytes across output UAVs and staging rings.
    #[must_use]
    pub const fn max_texture_bytes(self) -> u64 {
        self.max_texture_bytes
    }

    /// Fixed asynchronous staging depth for every descriptor.
    #[must_use]
    pub const fn slots_per_descriptor(self) -> NonZeroU32 {
        self.slots_per_descriptor
    }

    /// Validate and quote a complete immutable plan before backing allocation.
    ///
    /// # Errors
    ///
    /// Rejects unsupported semantics, duplicate identities, regions outside
    /// the source, checked byte overflow, and plans over the supplied budget.
    pub fn quote(
        self,
        source_extent: CaptureExtent,
        descriptors: &[GpuSurfaceDescriptor],
    ) -> CaptureResult<GpuReductionResourceQuote> {
        let mut allocation_byte_len = 0_u64;
        let mut readback_byte_len = 0_u64;
        let mut publication_buffer_byte_len = 0_usize;
        for (index, descriptor) in descriptors.iter().enumerate() {
            descriptor.validate_exact_gpu_readback()?;
            if !descriptor
                .source_region()
                .fits_within(source_extent.width(), source_extent.height())
            {
                return Err(CaptureError::GpuSurfaceRegionOutOfBounds {
                    descriptor_id: descriptor.id(),
                    source_width: source_extent.width(),
                    source_height: source_extent.height(),
                });
            }
            if descriptors[..index]
                .iter()
                .any(|candidate| candidate.id() == descriptor.id())
            {
                return Err(CaptureError::DuplicateGpuSurfaceDescriptor {
                    descriptor_id: descriptor.id(),
                });
            }
            let output_byte_len = checked_gpu_surface_bytes(descriptor.output_extent())?;
            let output_byte_len_usize =
                usize::try_from(output_byte_len).map_err(|_| CaptureError::GeometryOverflow {
                    operation: "allocate GPU reduction output",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            let route_readback_byte_len = output_byte_len
                .checked_mul(u64::from(self.slots_per_descriptor.get()))
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction staging slots",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            let route_allocation_byte_len = output_byte_len
                .checked_add(route_readback_byte_len)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction readback ring",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            allocation_byte_len = allocation_byte_len
                .checked_add(route_allocation_byte_len)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction plan",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            readback_byte_len = readback_byte_len
                .checked_add(route_readback_byte_len)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction staging plan",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            publication_buffer_byte_len = publication_buffer_byte_len
                .checked_add(output_byte_len_usize)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction publication buffers",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
        }
        if allocation_byte_len > self.max_texture_bytes {
            return Err(CaptureError::GpuSurfaceBudgetExceeded {
                requested_bytes: allocation_byte_len,
                budget_bytes: self.max_texture_bytes,
            });
        }
        #[cfg(target_os = "windows")]
        let metadata_byte_len = crate::duplication::gpu_reduction_metadata_byte_len(
            descriptors.len(),
            self.slots_per_descriptor,
        )?;
        #[cfg(target_os = "windows")]
        let constant_buffer_byte_len =
            crate::duplication::gpu_reduction_constant_buffer_byte_len(descriptors.len())?;
        #[cfg(not(target_os = "windows"))]
        let metadata_byte_len = 0;
        #[cfg(not(target_os = "windows"))]
        let constant_buffer_byte_len = 0;
        let retained_byte_len = allocation_byte_len
            .checked_add(constant_buffer_byte_len)
            .and_then(|bytes| bytes.checked_add(u64::try_from(publication_buffer_byte_len).ok()?))
            .and_then(|bytes| bytes.checked_add(metadata_byte_len))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account GPU reduction retained resources",
                width: source_extent.width(),
                height: source_extent.height(),
            })?;
        Ok(GpuReductionResourceQuote {
            allocation_byte_len,
            constant_buffer_byte_len,
            readback_byte_len,
            publication_buffer_byte_len,
            metadata_byte_len,
            retained_byte_len,
        })
    }

    /// Validate a complete immutable plan and return its checked GPU bytes.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::quote`].
    pub fn admit(
        self,
        source_extent: CaptureExtent,
        descriptors: &[GpuSurfaceDescriptor],
    ) -> CaptureResult<u64> {
        Ok(self
            .quote(source_extent, descriptors)?
            .allocation_byte_len())
    }
}

/// Immutable provenance for one descriptor-keyed GPU reduction readback.
#[derive(Clone, Debug)]
pub struct GpuReductionProvenance {
    /// Complete physical descriptor represented by the returned bytes.
    pub descriptor: Arc<GpuSurfaceDescriptor>,
    /// Committed plan generation that authorized the reduction.
    pub plan_generation: GpuSurfacePlanGeneration,
    /// Physical adapter that executed the reduction.
    pub adapter_luid: GpuAdapterLuid,
    /// Stable display source id.
    pub source_id: Arc<str>,
    /// Attached-output topology generation.
    pub topology_generation: u64,
    /// Desktop Duplication session generation.
    pub duplication_generation: u64,
    /// Native acquisition sequence reduced by the shader.
    pub source_sequence: u64,
    /// Native acquisition time.
    pub captured_at: Instant,
    /// Time the staging result became CPU-readable.
    pub completed_at: Instant,
    /// Last instant at which this result may be delivered.
    pub freshness_deadline: Instant,
    /// Native scanout extent feeding the reduction.
    pub native_source_extent: CaptureExtent,
    /// Upright logical display extent represented by descriptor coordinates.
    pub logical_source_extent: CaptureExtent,
    /// Source color space consumed by the shader.
    pub source_color_space: GpuSurfaceSourceColorSpace,
    /// Pending scanout transform normalized by the shader.
    pub source_rotation: DisplayRotation,
    /// Whether the shader composed the separately reported cursor.
    pub cursor_composed: bool,
}

/// Checked pre-allocation resources for one exact GPU Surface plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuSurfaceResourceQuote {
    allocation_byte_len: u64,
    constant_buffer_byte_len: u64,
    metadata_byte_len: u64,
    retained_byte_len: u64,
}

impl GpuSurfaceResourceQuote {
    /// Shareable output texture bytes across every descriptor slot.
    #[must_use]
    pub const fn allocation_byte_len(self) -> u64 {
        self.allocation_byte_len
    }

    /// Constant-buffer bytes retained by the plan-wide surface shader.
    #[must_use]
    pub const fn constant_buffer_byte_len(self) -> u64 {
        self.constant_buffer_byte_len
    }

    /// Rust-owned route, descriptor, slot, and publication payloads.
    ///
    /// This excludes allocator bookkeeping and opaque COM/driver internals.
    #[must_use]
    pub const fn metadata_byte_len(self) -> u64 {
        self.metadata_byte_len
    }

    /// Logical GPU texture and constant bytes plus Rust-owned metadata.
    #[must_use]
    pub const fn retained_byte_len(self) -> u64 {
        self.retained_byte_len
    }
}

/// Checked Rust metadata retained by one GPU Surface target manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuSurfaceTargetPreparationResourceQuote {
    metadata_byte_len: u64,
}

impl GpuSurfaceTargetPreparationResourceQuote {
    /// Quote the owned slot manifest before allocation.
    ///
    /// # Errors
    ///
    /// Rejects a slot count that cannot be represented by the current process.
    pub fn try_new(slot_count: NonZeroU32) -> CaptureResult<Self> {
        #[cfg(target_os = "windows")]
        let metadata_byte_len =
            crate::duplication::gpu_surface_target_preparation_metadata_byte_len(slot_count)?;
        #[cfg(not(target_os = "windows"))]
        let metadata_byte_len = 0;
        Ok(Self { metadata_byte_len })
    }

    /// Rust-owned slot-array payload retained by the manifest.
    ///
    /// This excludes allocator bookkeeping and opaque COM/driver internals.
    #[must_use]
    pub const fn metadata_byte_len(self) -> u64 {
        self.metadata_byte_len
    }

    /// Total resources allocated uniquely for the manifest.
    #[must_use]
    pub const fn retained_byte_len(self) -> u64 {
        self.metadata_byte_len
    }
}

/// Caller-supplied resource ledger for one prepared GPU Surface plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuSurfaceAdmission {
    max_texture_bytes: u64,
    slots_per_descriptor: NonZeroU32,
}

impl GpuSurfaceAdmission {
    /// Define the byte budget and reusable in-flight depth explicitly.
    #[must_use]
    pub const fn new(max_texture_bytes: u64, slots_per_descriptor: NonZeroU32) -> Self {
        Self {
            max_texture_bytes,
            slots_per_descriptor,
        }
    }

    /// Maximum admitted bytes across candidate output slots.
    #[must_use]
    pub const fn max_texture_bytes(self) -> u64 {
        self.max_texture_bytes
    }

    /// Number of independently synchronized reusable slots per descriptor.
    #[must_use]
    pub const fn slots_per_descriptor(self) -> NonZeroU32 {
        self.slots_per_descriptor
    }

    /// Validate and quote exact descriptors before backing allocation.
    ///
    /// The ledger includes every descriptor's configured in-flight output
    /// slots. The source-owned clean desktop and pointer texture are already
    /// represented by current adapter usage. This imposes no axis or
    /// resolution cap; actual D3D allocation remains the final admission.
    ///
    /// # Errors
    ///
    /// Rejects unsupported exact operations, duplicate identities, regions
    /// outside the source, checked byte overflow, and plans over this budget.
    pub fn quote(
        self,
        source_extent: CaptureExtent,
        descriptors: &[GpuSurfaceDescriptor],
    ) -> CaptureResult<GpuSurfaceResourceQuote> {
        if self.slots_per_descriptor.get() < 2 {
            return Err(CaptureError::GpuSurfaceInFlightDepthTooSmall {
                requested: self.slots_per_descriptor.get(),
                minimum: 2,
            });
        }
        let mut bytes = 0_u64;
        for (index, descriptor) in descriptors.iter().enumerate() {
            descriptor.validate_exact_gpu()?;
            if !descriptor
                .source_region()
                .fits_within(source_extent.width(), source_extent.height())
            {
                return Err(CaptureError::GpuSurfaceRegionOutOfBounds {
                    descriptor_id: descriptor.id(),
                    source_width: source_extent.width(),
                    source_height: source_extent.height(),
                });
            }
            if descriptors[..index]
                .iter()
                .any(|candidate| candidate.id() == descriptor.id())
            {
                return Err(CaptureError::DuplicateGpuSurfaceDescriptor {
                    descriptor_id: descriptor.id(),
                });
            }
            let route = checked_gpu_surface_bytes(descriptor.output_extent())?
                .checked_mul(u64::from(self.slots_per_descriptor.get()))
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU Surface slots",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            bytes = bytes
                .checked_add(route)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU Surface plan",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
        }
        if bytes > self.max_texture_bytes {
            return Err(CaptureError::GpuSurfaceBudgetExceeded {
                requested_bytes: bytes,
                budget_bytes: self.max_texture_bytes,
            });
        }
        #[cfg(target_os = "windows")]
        let metadata_byte_len = crate::duplication::gpu_surface_metadata_byte_len(
            descriptors.len(),
            self.slots_per_descriptor,
        )?;
        #[cfg(target_os = "windows")]
        let constant_buffer_byte_len = u64::try_from(
            crate::duplication::gpu_surface_constant_buffer_byte_len(),
        )
        .map_err(|_| CaptureError::GeometryOverflow {
            operation: "account GPU Surface constant buffer",
            width: source_extent.width(),
            height: source_extent.height(),
        })?;
        #[cfg(not(target_os = "windows"))]
        let metadata_byte_len = 0;
        #[cfg(not(target_os = "windows"))]
        let constant_buffer_byte_len = 0;
        let retained_byte_len = bytes
            .checked_add(constant_buffer_byte_len)
            .and_then(|bytes| bytes.checked_add(metadata_byte_len))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account GPU Surface retained resources",
                width: source_extent.width(),
                height: source_extent.height(),
            })?;
        Ok(GpuSurfaceResourceQuote {
            allocation_byte_len: bytes,
            constant_buffer_byte_len,
            metadata_byte_len,
            retained_byte_len,
        })
    }

    /// Validate a complete immutable plan and return its checked GPU bytes.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::quote`].
    pub fn admit(
        self,
        source_extent: CaptureExtent,
        descriptors: &[GpuSurfaceDescriptor],
    ) -> CaptureResult<u64> {
        Ok(self
            .quote(source_extent, descriptors)?
            .allocation_byte_len())
    }
}

/// Checked pre-allocation resources for one native CPU readback lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuDesktopReadbackResourceQuote {
    frame_byte_len: usize,
    allocation_byte_len: u64,
    metadata_byte_len: u64,
    retained_byte_len: u64,
}

impl CpuDesktopReadbackResourceQuote {
    /// Quote fixed staging textures and pooled output planes.
    ///
    /// # Errors
    ///
    /// Rejects byte geometry that cannot be represented by the current process.
    pub fn try_new(source_extent: CaptureExtent, slot_count: NonZeroU32) -> CaptureResult<Self> {
        let frame_byte_len = usize::try_from(source_extent.width())
            .ok()
            .and_then(|width| {
                usize::try_from(source_extent.height())
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account native CPU readback",
                width: source_extent.width(),
                height: source_extent.height(),
            })?;
        let allocation_byte_len = u64::try_from(frame_byte_len)
            .ok()
            .and_then(|bytes| bytes.checked_mul(u64::from(slot_count.get())))
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account native CPU readback",
                width: source_extent.width(),
                height: source_extent.height(),
            })?;
        #[cfg(target_os = "windows")]
        let metadata_byte_len = crate::duplication::cpu_readback_metadata_byte_len(slot_count)?;
        #[cfg(not(target_os = "windows"))]
        let metadata_byte_len = 0;
        let retained_byte_len = allocation_byte_len.checked_add(metadata_byte_len).ok_or(
            CaptureError::GeometryOverflow {
                operation: "account native CPU readback retained resources",
                width: source_extent.width(),
                height: source_extent.height(),
            },
        )?;
        Ok(Self {
            frame_byte_len,
            allocation_byte_len,
            metadata_byte_len,
            retained_byte_len,
        })
    }

    /// Bytes in one tightly packed native BGRA plane.
    #[must_use]
    pub const fn frame_byte_len(self) -> usize {
        self.frame_byte_len
    }

    /// Staging textures plus pooled output planes.
    #[must_use]
    pub const fn allocation_byte_len(self) -> u64 {
        self.allocation_byte_len
    }

    /// Rust-owned slot array, frame-pool array, and pool payload.
    ///
    /// This excludes allocator bookkeeping and opaque COM/driver internals.
    #[must_use]
    pub const fn metadata_byte_len(self) -> u64 {
        self.metadata_byte_len
    }

    /// Logical staging/output bytes plus Rust-owned metadata.
    #[must_use]
    pub const fn retained_byte_len(self) -> u64 {
        self.retained_byte_len
    }
}

pub(crate) fn checked_gpu_surface_bytes(extent: CaptureExtent) -> CaptureResult<u64> {
    u64::from(extent.width())
        .checked_mul(u64::from(extent.height()))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::GeometryOverflow {
            operation: "account GPU Surface texture",
            width: extent.width(),
            height: extent.height(),
        })
}

/// Borrowed Windows handle value kept alive by its native resource owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuSharedHandle<'lease> {
    raw: isize,
    lease: PhantomData<&'lease ()>,
}

impl GpuSharedHandle<'_> {
    #[cfg(target_os = "windows")]
    pub(crate) const fn from_raw(raw: isize) -> Self {
        Self {
            raw,
            lease: PhantomData,
        }
    }

    /// Raw NT handle value. The caller must not close this borrowed handle.
    #[must_use]
    pub const fn as_raw(self) -> isize {
        self.raw
    }
}

/// Explicit cross-API ownership transfer for one reusable Surface slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuSurfaceSynchronization {
    /// Key the producer acquires before overwriting the reusable texture.
    pub producer_acquire_key: u64,
    /// Key the producer releases after queuing the exact Surface write.
    pub producer_release_key: u64,
    /// Key the consumer acquires before importing or copying the texture.
    pub consumer_acquire_key: u64,
    /// Key the consumer releases after its final texture access is queued.
    pub consumer_release_key: u64,
    /// Fence value the consumer waits for before reading the texture.
    pub producer_ready_value: u64,
    /// Fence value the consumer signals after its final GPU read completes.
    pub consumer_release_value: u64,
}

/// Immutable provenance captured when one exact GPU Surface is produced.
#[derive(Clone, Debug)]
pub struct GpuSurfaceProvenance {
    /// Complete exact descriptor represented by the bytes.
    pub descriptor: Arc<GpuSurfaceDescriptor>,
    /// Committed plan generation that authorized the result.
    pub plan_generation: GpuSurfacePlanGeneration,
    /// DXGI adapter that owns the shared texture and fence handles.
    pub adapter_luid: GpuAdapterLuid,
    /// Stable physical texture slot within this plan generation.
    pub slot_id: GpuSurfaceSlotId,
    /// Monotonic content generation for this physical slot.
    pub use_id: u64,
    /// Stable display source id.
    pub source_id: Arc<str>,
    /// Attached-output topology generation.
    pub topology_generation: u64,
    /// Desktop Duplication session generation.
    pub duplication_generation: u64,
    /// Native acquisition sequence.
    pub source_sequence: u64,
    /// Native acquisition time.
    pub captured_at: Instant,
    /// Time the GPU publication was enqueued and fenced.
    pub published_at: Instant,
    /// Last instant at which this result may be delivered.
    pub freshness_deadline: Instant,
    /// Native scanout extent feeding the publication.
    pub native_source_extent: CaptureExtent,
    /// Upright logical display extent represented by descriptor coordinates.
    pub logical_source_extent: CaptureExtent,
    /// Coordinate system represented by the published pixels.
    pub coordinate_space: GpuSurfaceCoordinateSpace,
    /// Exact output raster extent.
    pub output_extent: CaptureExtent,
    /// Duplicated source storage format.
    pub source_format: GpuSurfaceFormat,
    /// DXGI color space represented by duplicated source channels.
    pub source_color_space: GpuSurfaceSourceColorSpace,
    /// Published output storage format.
    pub output_format: GpuSurfaceFormat,
    /// Exact color operation represented by the output.
    pub color_pipeline: GpuSurfaceColorPipeline,
    /// Pending transform still represented by the published pixels.
    pub pending_rotation: DisplayRotation,
    /// Whether the publication shader included cursor composition.
    pub cursor_composed: bool,
}

impl CaptureRegion {
    /// Construct a non-empty pixel rectangle.
    #[must_use]
    pub const fn new(origin_x: u32, origin_y: u32, width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            origin_x,
            origin_y,
            width,
            height,
        })
    }

    #[cfg(target_os = "windows")]
    pub(crate) const fn full(width: u32, height: u32) -> Self {
        Self {
            origin_x: 0,
            origin_y: 0,
            width,
            height,
        }
    }

    /// Horizontal origin in the rectangle's declared coordinate space.
    #[must_use]
    pub const fn origin_x(self) -> u32 {
        self.origin_x
    }

    /// Vertical origin in the rectangle's declared coordinate space.
    #[must_use]
    pub const fn origin_y(self) -> u32 {
        self.origin_y
    }

    /// Selected width in the rectangle's declared coordinate space.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Selected height in the rectangle's declared coordinate space.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    pub(crate) fn fits_within(self, width: u32, height: u32) -> bool {
        self.origin_x
            .checked_add(self.width)
            .is_some_and(|right| right <= width)
            && self
                .origin_y
                .checked_add(self.height)
                .is_some_and(|bottom| bottom <= height)
    }
}

/// Cursor metadata associated with an already-composited frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorInfo {
    /// Whether the backend reported a separately visible pointer.
    pub visible: bool,
    /// Pointer shape origin in native scanout coordinates.
    pub position_x: i32,
    /// Pointer shape origin in native scanout coordinates.
    pub position_y: i32,
    /// Hotspot offset transformed into the native scanout bounding box.
    pub hotspot_x: i32,
    /// Hotspot offset transformed into the native scanout bounding box.
    pub hotspot_y: i32,
    /// Visible pointer-shape width.
    pub width: u32,
    /// Visible pointer-shape height.
    pub height: u32,
    /// Monotonic shape generation within this duplication session.
    pub shape_generation: u64,
    /// Whether the returned RGBA plane already contains the pointer pixels.
    pub composed: bool,
}

type FramePool = Arc<Mutex<Vec<Vec<u8>>>>;

#[derive(Debug)]
pub(crate) struct LegacyFramePlane {
    pub(crate) rgba: Vec<u8>,
    pub(crate) resource_lease: Arc<dyn CaptureResourceLease>,
}

pub(crate) type LegacyFramePool = Arc<Mutex<Vec<LegacyFramePlane>>>;
const LEGACY_FRAME_POOL_WARM_LEN: usize = 3;

pub(crate) fn recycle_legacy_frame_plane(pool: &LegacyFramePool, mut plane: LegacyFramePlane) {
    plane.rgba.clear();
    let mut pool = pool
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pool.len() < LEGACY_FRAME_POOL_WARM_LEN {
        pool.push(plane);
        return;
    }
    let Some((smallest_index, smallest_capacity)) = pool
        .iter()
        .enumerate()
        .map(|(index, candidate)| (index, candidate.rgba.capacity()))
        .min_by_key(|(_, capacity)| *capacity)
    else {
        return;
    };
    if plane.rgba.capacity() > smallest_capacity {
        pool[smallest_index] = plane;
    }
}

/// Owned, tightly packed native BGRA desktop produced by async readback.
///
/// The frame excludes the separately reported cursor. Its allocation returns
/// to the prepared readback's fixed pool when the frame drops.
#[derive(Debug)]
pub struct CpuDesktopFrame {
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
    sequence: u64,
    captured_at: Instant,
    cursor: CursorInfo,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    rotation: DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    bgra: Vec<u8>,
    pool: FramePool,
}

#[cfg(target_os = "windows")]
impl CpuDesktopFrame {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        source_id: Arc<str>,
        topology_generation: u64,
        duplication_generation: u64,
        sequence: u64,
        captured_at: Instant,
        cursor: CursorInfo,
        width: u32,
        height: u32,
        origin_x: i32,
        origin_y: i32,
        rotation: DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
        bgra: Vec<u8>,
        pool: FramePool,
    ) -> Self {
        Self {
            source_id,
            topology_generation,
            duplication_generation,
            sequence,
            captured_at,
            cursor,
            width,
            height,
            origin_x,
            origin_y,
            rotation,
            source_color_space,
            bgra,
            pool,
        }
    }
}

impl CpuDesktopFrame {
    /// Stable display id that produced this frame.
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Attached-output topology generation at acquisition.
    #[must_use]
    pub const fn topology_generation(&self) -> u64 {
        self.topology_generation
    }

    /// Desktop Duplication session generation at acquisition.
    #[must_use]
    pub const fn duplication_generation(&self) -> u64 {
        self.duplication_generation
    }

    /// Monotonic source sequence assigned at acquisition.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Acquisition timestamp before asynchronous readback.
    #[must_use]
    pub const fn captured_at(&self) -> Instant {
        self.captured_at
    }

    /// Separately reported cursor state. The returned pixels exclude it.
    #[must_use]
    pub const fn cursor(&self) -> CursorInfo {
        self.cursor
    }

    /// Native scanout width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Native scanout height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Tight row stride in bytes.
    #[must_use]
    pub const fn row_stride_bytes(&self) -> usize {
        self.width as usize * 4
    }

    /// Native BGRA storage format.
    #[must_use]
    pub const fn format(&self) -> GpuSurfaceFormat {
        GpuSurfaceFormat::Bgra8Unorm
    }

    /// Horizontal origin in virtual-desktop coordinates.
    #[must_use]
    pub const fn origin_x(&self) -> i32 {
        self.origin_x
    }

    /// Vertical origin in virtual-desktop coordinates.
    #[must_use]
    pub const fn origin_y(&self) -> i32 {
        self.origin_y
    }

    /// Display transform still pending on the native pixels.
    #[must_use]
    pub const fn rotation(&self) -> DisplayRotation {
        self.rotation
    }

    /// DXGI color space attached to the native samples.
    #[must_use]
    pub const fn source_color_space(&self) -> GpuSurfaceSourceColorSpace {
        self.source_color_space
    }

    /// Tightly packed native BGRA bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bgra
    }
}

impl AsRef<[u8]> for CpuDesktopFrame {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Drop for CpuDesktopFrame {
    fn drop(&mut self) {
        let mut bgra = std::mem::take(&mut self.bgra);
        bgra.clear();
        self.pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(bgra);
    }
}

/// Owned RGBA frame produced by the capture backend.
///
/// The pixel allocation returns to the duplicator's pool when the final frame
/// owner drops it, so downstream adapters can retain the plane without copying.
#[derive(Debug)]
pub struct Frame {
    /// Stable id of the display that produced this frame.
    pub source_id: Arc<str>,
    /// Attached-output topology generation at acquisition.
    pub topology_generation: u64,
    /// Monotonic capture sequence assigned when Desktop Duplication acquires the state.
    pub sequence: u64,
    /// Time Desktop Duplication acquired the state, before asynchronous reduction.
    pub captured_at: Instant,
    /// Cursor state represented by this frame.
    pub cursor: CursorInfo,
    /// Frame width in pixels, after subsampling.
    pub width: u32,
    /// Frame height in pixels, after subsampling.
    pub height: u32,
    /// Native scanout width before subsampling.
    pub native_width: u32,
    /// Native scanout height before subsampling.
    pub native_height: u32,
    /// Horizontal origin in virtual-desktop coordinates.
    pub origin_x: i32,
    /// Vertical origin in virtual-desktop coordinates.
    pub origin_y: i32,
    /// Display transform still pending on the stored pixels.
    pub rotation: DisplayRotation,
    /// Tightly packed RGBA8 pixels, `width * height * 4` bytes.
    rgba: Vec<u8>,
    resource_lease: Option<Arc<dyn CaptureResourceLease>>,
    pool: LegacyFramePool,
}

#[cfg(target_os = "windows")]
impl Frame {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        source_id: Arc<str>,
        topology_generation: u64,
        sequence: u64,
        captured_at: Instant,
        cursor: CursorInfo,
        width: u32,
        height: u32,
        native_width: u32,
        native_height: u32,
        origin_x: i32,
        origin_y: i32,
        rotation: DisplayRotation,
        rgba: Vec<u8>,
        resource_lease: Arc<dyn CaptureResourceLease>,
        pool: LegacyFramePool,
    ) -> Self {
        Self {
            source_id,
            topology_generation,
            sequence,
            captured_at,
            cursor,
            width,
            height,
            native_width,
            native_height,
            origin_x,
            origin_y,
            rotation,
            rgba,
            resource_lease: Some(resource_lease),
            pool,
        }
    }
}

impl AsRef<[u8]> for Frame {
    fn as_ref(&self) -> &[u8] {
        &self.rgba
    }
}

impl Frame {
    /// Tightly packed immutable RGBA8 pixels.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let rgba = std::mem::take(&mut self.rgba);
        let Some(resource_lease) = self.resource_lease.take() else {
            return;
        };
        recycle_legacy_frame_plane(
            &self.pool,
            LegacyFramePlane {
                rgba,
                resource_lease,
            },
        );
    }
}

/// Number of attached display outputs, or zero when capture is unavailable.
#[must_use]
pub fn monitor_count() -> usize {
    #[cfg(target_os = "windows")]
    {
        crate::duplication::output_count().unwrap_or(0)
    }
    #[cfg(not(target_os = "windows"))]
    {
        0
    }
}

/// One attached display output, in capture index order.
///
/// New callers persist `id`; `index` remains for ordering and legacy configs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Legacy zero-based enumeration index for display ordering.
    pub index: usize,
    /// Stable source id used for persisted selection and capture epochs.
    pub id: String,
    /// OS device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Desktop width in pixels.
    pub width: u32,
    /// Desktop height in pixels.
    pub height: u32,
    /// Horizontal origin in virtual-desktop coordinates.
    pub origin_x: i32,
    /// Vertical origin in virtual-desktop coordinates.
    pub origin_y: i32,
    /// Whether this output hosts the origin of the virtual desktop.
    pub primary: bool,
    /// Transform still pending on duplicated scanout pixels.
    pub rotation: DisplayRotation,
    /// Monotonic generation of the attached-output topology.
    pub topology_generation: u64,
}

/// A persisted display selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorSelector {
    /// Follow whichever attached output Windows marks primary.
    Auto,
    /// Follow one output across enumeration reorder by its stable id.
    StableId(String),
    /// Legacy adapter/output enumeration index.
    Index(usize),
}

impl MonitorSelector {
    /// Parse a configured capture source.
    #[must_use]
    pub fn parse(source: &str) -> Self {
        let source = source.trim();
        if source.is_empty() || source.eq_ignore_ascii_case("auto") {
            return Self::Auto;
        }

        if let Some(value) = source.strip_prefix("monitor:") {
            let value = value.trim();
            return value
                .parse::<usize>()
                .map_or_else(|_| Self::StableId(value.to_owned()), Self::Index);
        }
        if let Some(value) = source.strip_prefix("display:")
            && let Ok(index) = value.trim().parse::<usize>()
        {
            return Self::Index(index);
        }
        source
            .parse::<usize>()
            .map_or_else(|_| Self::StableId(source.to_owned()), Self::Index)
    }

    /// Convert a resolved legacy index into its stable persisted form.
    #[must_use]
    pub fn canonical_source(&self, resolved_source_id: &str) -> Option<String> {
        matches!(self, Self::Index(_)).then(|| format!("monitor:{resolved_source_id}"))
    }

    /// Resolve this selection against one topology snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::MonitorNotFound`] for a legacy index outside
    /// the snapshot, or [`CaptureError::SourceNotFound`] for an absent stable
    /// id. `Auto` resolves the primary output even when it is not index zero.
    pub fn resolve<'a>(&self, monitors: &'a [MonitorInfo]) -> CaptureResult<&'a MonitorInfo> {
        match self {
            Self::Auto => monitors
                .iter()
                .find(|monitor| monitor.primary)
                .or_else(|| monitors.first())
                .ok_or(CaptureError::MonitorNotFound {
                    requested: 0,
                    available: 0,
                }),
            Self::StableId(requested) => monitors
                .iter()
                .find(|monitor| monitor.id == *requested)
                .ok_or_else(|| CaptureError::SourceNotFound {
                    requested: requested.clone(),
                }),
            Self::Index(requested) => {
                monitors
                    .get(*requested)
                    .ok_or(CaptureError::MonitorNotFound {
                        requested: *requested,
                        available: monitors.len(),
                    })
            }
        }
    }
}

/// Describe every attached display output.
///
/// Empty when capture is unavailable (non-Windows, headless, RDP), so
/// callers can use emptiness itself as "this platform has no monitor
/// picker" rather than needing a separate capability probe.
#[must_use]
pub fn list_monitors() -> Vec<MonitorInfo> {
    #[cfg(target_os = "windows")]
    {
        crate::duplication::describe_outputs().unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Integer subsample stride that brings `source` at or under `target`.
///
/// Capture reduction averages each `stride` square into one output pixel. The
/// stride keeps the intermediate surface bounded without aliasing thin desktop
/// content before the later sector-grid reduction.
#[must_use]
pub fn subsample_stride(source: u32, target: u32) -> u32 {
    if target == 0 || source <= target {
        return 1;
    }
    source.div_ceil(target).max(1)
}

/// Integer stride that fits both source axes within independent bounds.
#[must_use]
pub fn subsample_stride_within(
    source_width: u32,
    source_height: u32,
    requested_extent: CaptureExtent,
) -> u32 {
    subsample_stride(source_width, requested_extent.width())
        .max(subsample_stride(source_height, requested_extent.height()))
}

/// Width target that makes a width-driven reducer honor two-axis bounds.
#[must_use]
pub fn width_target_within(
    source_width: u32,
    source_height: u32,
    requested_extent: CaptureExtent,
) -> u32 {
    source_width.div_ceil(subsample_stride_within(
        source_width,
        source_height,
        requested_extent,
    ))
}

/// Dimension after applying `stride` to `source`.
#[must_use]
pub const fn subsampled_extent(source: u32, stride: u32) -> u32 {
    if stride <= 1 {
        return source;
    }
    source.div_ceil(stride)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn native_frame_owns_acquisition_sequence_and_time() {
        let captured_at = Instant::now();
        let pool = Arc::new(Mutex::new(Vec::new()));
        let resource_lease = commit_capture_resource(
            reserve_capture_resource(
                default_capture_resource_admission().as_ref(),
                CaptureResourceKind::CompatibilityFramePlane,
                4,
                "reserve test compatibility frame",
            )
            .expect("test reservation succeeds"),
            4,
            "commit test compatibility frame",
        )
        .expect("test lease succeeds");
        let frame = Frame::new(
            Arc::from("display:test"),
            3,
            41,
            captured_at,
            CursorInfo::default(),
            1,
            1,
            1,
            1,
            0,
            0,
            DisplayRotation::Identity,
            vec![1, 2, 3, 0xFF],
            resource_lease,
            pool,
        );

        assert_eq!(frame.sequence, 41);
        assert_eq!(frame.captured_at, captured_at);
    }

    #[test]
    fn native_frame_without_a_lease_drops_without_recycling() {
        let pool = Arc::new(Mutex::new(Vec::new()));
        let resource_lease = commit_capture_resource(
            reserve_capture_resource(
                default_capture_resource_admission().as_ref(),
                CaptureResourceKind::CompatibilityFramePlane,
                4,
                "reserve test compatibility frame",
            )
            .expect("test reservation succeeds"),
            4,
            "commit test compatibility frame",
        )
        .expect("test lease succeeds");
        let mut frame = Frame::new(
            Arc::from("display:test"),
            1,
            1,
            Instant::now(),
            CursorInfo::default(),
            1,
            1,
            1,
            1,
            0,
            0,
            DisplayRotation::Identity,
            vec![0; 4],
            resource_lease,
            Arc::clone(&pool),
        );

        drop(frame.resource_lease.take());
        drop(frame);

        assert!(
            pool.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }
}
