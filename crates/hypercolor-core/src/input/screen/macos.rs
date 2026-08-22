#[cfg(feature = "macos-capture-fixtures")]
use std::num::NonZeroUsize;
use std::num::{NonZeroU32, NonZeroU64};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use hypercolor_macos_capture::{
    MacosCaptureCadence, MacosCaptureCallbackDiagnostics,
    MacosCaptureCapabilities as NativeCaptureCapabilities, MacosCaptureContentStyle,
    MacosCaptureDynamicRange, MacosCaptureFrame, MacosCapturePixelFormat, MacosCaptureSelection,
    MacosColorPrimaries, MacosFrameDropReason, MacosFrameEvent, MacosFrameMailbox,
    MacosFrameStatus, MacosHostArchitecture,
    MacosProtectedSourceState as NativeProtectedSourceState, MacosScreenAuthorizationState,
    MacosScreenTahoeSelectionStatus, MacosScreenTahoeStatus, MacosSourceTimingStatus,
    MacosStreamRequest, MacosTahoeSelectionCapabilities as NativeTahoeSelectionCapabilities,
    MacosTransferFunction,
};
#[cfg(feature = "macos-capture-fixtures")]
use hypercolor_macos_capture::{
    MacosCpuSourceView, MacosRuntimeCapability, MacosTahoeRuntimeProbes,
};
#[cfg(target_os = "macos")]
use hypercolor_macos_capture::{
    MacosDisplayClock, MacosScreenCaptureSession, MacosScreenshotReferenceCapture,
};
use hypercolor_macos_input::{MacosCapabilityOwner, MacosDaemonOwnerConflict};

#[cfg(not(feature = "macos-capture-fixtures"))]
use super::ScreenColorTransformCapabilities;
use super::{
    AdmittedScreenNativeTargetPreparation, BoundScreenNativeTargetPreparation, CaptureCadence,
    CaptureColorSpace, CaptureColorimetry, CaptureConfig, CaptureDynamicRange, CaptureEpoch,
    CaptureLuminanceContext, CapturePixelFormat, CapturePositiveScalar, CaptureRotation,
    CaptureSourceId, CaptureTransferFunction, LedToneMapCalibration, PixelExtent, PixelRect,
    PlatformGpuApi, PlatformGpuSurface, PlatformGpuSurfaceTimingSink, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan,
    ScreenAnalysisWorkPlan, ScreenBackendResourceIdentity, ScreenBranchPayload,
    ScreenBranchPublisher, ScreenByteAdmissionCoordinator, ScreenCaptureBackend,
    ScreenCaptureCadence, ScreenCaptureDemand, ScreenCommittedState, ScreenComputeCapacityPolicy,
    ScreenCursorCapabilities, ScreenCursorPolicy, ScreenExecutorColorCapabilities,
    ScreenGpuSurfacePayload, ScreenNativeExecutionTargetId, ScreenNativeExecutionUnavailableReason,
    ScreenNativePreparationPayload, ScreenNativeWorkPayload, ScreenPhysicalGpuDeviceIdentity,
    ScreenPreparedWorkerToken, ScreenPublicationColorimetry, ScreenPublicationError,
    ScreenPublicationExecutor, ScreenPublicationExecutorFallbackReason,
    ScreenPublicationExecutorRequest, ScreenPublicationHealth, ScreenPublicationHub,
    ScreenPublicationHubError, ScreenPublicationMetadata, ScreenPublicationRequest,
    ScreenRendererExecutionState, ScreenRequiredResourceMinimum, ScreenResourceApi,
    ScreenResourceKind, ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector,
    ScreenWorkerBinding, ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation,
    ScreenWorkerPreparationTicket, ScreenWorkerRetirement, SourceScale,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::{
    CaptureCursor, CaptureCursorContent, CaptureDamage, CaptureFrame, CaptureFrameMetadata,
    CapturePlanePool, CaptureStorage, CpuCaptureStorage, CpuExactReductionWorkPlan,
    CpuPublicationFanoutError, CpuReductionExecutor, CpuSamplingError, CpuScalarSource,
    KnownCaptureColorimetry, PreparedCpuPublicationFanout, PreparedCpuPublicationFanoutCandidate,
    PreparedLedToneMap, RawCaptureSurface, ScreenCaptureInput, analyze_screen_frame,
};
use crate::input::status::SourceSessionSlot;
use crate::input::traits::{
    CapabilityActionDisposition, CapabilityActionIdentity, InputData, InputSource,
    ProtectedSourceAuthorizationAction, ScreenSource, ScreenSourcePickerAction, ScreenSourceRole,
    SourceCapabilityContext, SourceDiagnosticArtifactAction, SourceRoleBinding,
};
use crate::input::{SourceIssue, SourceStatusHandle, SourceStatusReporter};

use super::adapter::{
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureExactRuntimeOwner, CaptureOwnedSource,
    CapturePublicationSource, begin_capture_exact_preparation, begin_capture_exact_retirement,
    execute_capture_exact_command,
};

#[cfg(target_os = "macos")]
mod surface_pool;

#[cfg(target_os = "macos")]
use surface_pool::MacosSurfacePool;

mod admission;
mod control;
mod input_source;
mod publication;
mod screen_source;
mod session;
mod status;
mod worker;

#[cfg(feature = "macos-capture-fixtures")]
mod fixtures;
#[cfg(feature = "macos-capture-fixtures")]
pub use fixtures::MacosScreenCaptureFixture;

#[cfg(all(test, feature = "macos-capture-fixtures"))]
mod tests;

use control::production_stream_request;
use publication::resolve_macos_publication_branch_with_telemetry;
use status::{
    color_space_name, dynamic_range_name, executable_architecture, frame_drop_counters,
    map_tahoe_capabilities, map_tahoe_selection_capabilities, nonzero_telemetry, pixel_format_name,
    protected_action_identity, timing_status, transfer_function_name,
};
use worker::run_worker;

const WORKER_WAIT: Duration = Duration::from_millis(100);

// BT.2408 diffuse white; identical to the target LED calibration default so
// unsignalled HDR content maps through the tone map at unity.
const DEFAULT_HDR_SOURCE_REFERENCE_WHITE_NITS: f32 = 203.0;
// One stop of assumed highlight headroom for HDR frames whose surfaces carry
// no IOSurfaceContentHeadroom, mirroring the stop the tone map reserves on
// the output side.
const DEFAULT_HDR_SOURCE_CONTENT_HEADROOM: f32 = 2.0;

const PUBLICATION_PATH_UNKNOWN: u8 = 0;
#[cfg(feature = "macos-capture-fixtures")]
const PUBLICATION_PATH_CPU: u8 = 1;
const PUBLICATION_PATH_NATIVE: u8 = 2;
#[cfg(feature = "macos-capture-fixtures")]
const PUBLICATION_PATH_CPU_FALLBACK: u8 = 3;
const PUBLICATION_PATH_NATIVE_UNAVAILABLE: u8 = 4;
const TIMING_BUCKET_WIDTH_NS: u64 = 100_000;
const TIMING_BUCKET_COUNT: usize = 4096;

#[derive(Debug, Default)]
struct MacosScreenRuntimeTelemetry {
    publication_path: AtomicU8,
    renderer_authoritative: bool,
    renderer_target: Mutex<Option<ScreenNativeExecutionTargetId>>,
    fallback_reason: Mutex<Option<Arc<str>>>,
    publication_plan_generation: AtomicU64,
    stale_frames: AtomicU64,
    cpu_reduction_timing: AtomicTimingHistogram,
    native_import_timing: AtomicTimingHistogram,
    native_reduction_submit_timing: AtomicTimingHistogram,
    capture_to_native_publication_timing: AtomicTimingHistogram,
    capture_to_converted_publication_timing: AtomicTimingHistogram,
    admitted_native_bytes: AtomicU64,
    pinned_generations: AtomicUsize,
}

#[derive(Debug)]
struct AtomicTimingHistogram {
    buckets: Box<[AtomicU64]>,
    generation: AtomicU64,
    sample_count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl Default for AtomicTimingHistogram {
    fn default() -> Self {
        Self {
            buckets: (0..=TIMING_BUCKET_COUNT)
                .map(|_| AtomicU64::new(0))
                .collect(),
            generation: AtomicU64::new(0),
            sample_count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
        }
    }
}

/// Descriptor-keyed source data passed to the daemon-owned Metal target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacosNativeTargetManifest {
    capture_session_generation: u64,
    resource_generation: u64,
    metal_registry_id: u64,
}

trait MacosCaptureControl: Send + Sync {
    fn mailbox(&self) -> MacosFrameMailbox;
    fn set_active(&self, active: bool);
    fn present_picker(&self) -> anyhow::Result<()>;
    fn request_authorization(&self) -> NativeProtectedSourceState;
    fn status(&self) -> NativeProtectedSourceState;
    fn selection(&self) -> MacosCaptureSelection;
    fn selection_revision(&self) -> u64;
    fn begin_stream_request(&self, request: MacosStreamRequest) -> anyhow::Result<StreamRequest>;
    fn tahoe_selection_capabilities(&self) -> Option<NativeTahoeSelectionCapabilities>;
    fn host_capabilities(&self) -> NativeCaptureCapabilities;
    fn authorization(&self) -> MacosScreenAuthorizationState;
    fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics;
    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant>;

    #[cfg(target_os = "macos")]
    fn capture_screenshot_reference(
        &self,
    ) -> anyhow::Result<
        mpsc::Receiver<
            Result<MacosScreenshotReferenceCapture, hypercolor_macos_capture::MacosCaptureError>,
        >,
    > {
        anyhow::bail!("macOS screenshot references are unavailable for this capture control")
    }
}

struct StreamRequest {
    generation: u64,
    completion: Box<dyn FnOnce() -> anyhow::Result<()> + Send>,
}

#[cfg(target_os = "macos")]
struct NativeCaptureControl {
    session: MacosScreenCaptureSession,
    clock: MacosDisplayClock,
    host_capabilities: NativeCaptureCapabilities,
}

#[derive(Default)]
struct MacosPublication {
    worker_generation: u64,
    #[cfg(feature = "macos-capture-fixtures")]
    latest: Option<Arc<InputData>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MacosPublicationSource {
    epoch: CaptureEpoch,
    geometry: super::CaptureGeometry,
    logical_extent: PixelExtent,
    colorimetry: CaptureColorimetry,
    pixel_format: MacosCapturePixelFormat,
    resource_generation: u64,
    allocation_bytes: u64,
    display_scale_bits: u64,
    cursor_composed: bool,
}

impl CapturePublicationSource for MacosPublicationSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.epoch.source_id
    }
}

struct MacosOwnedSource {
    source_id: CaptureSourceId,
    binding: ScreenWorkerBinding,
    _runtime_lifetime: ScreenResourceLifetime,
}

impl CaptureOwnedSource for MacosOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    fn belongs_to_authority(&self, authority: &ScreenCommittedState) -> bool {
        authority.owns_runtime_binding(&self.binding)
    }
}

#[derive(Default)]
struct MacosExactPublicationShared {
    common: CaptureExactPublicationShared<MacosPublicationSource, MacosOwnedSource>,
    #[cfg(feature = "macos-capture-fixtures")]
    cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
    #[cfg(feature = "macos-capture-fixtures")]
    compute_capacity_policy: ScreenComputeCapacityPolicy,
}

impl Deref for MacosExactPublicationShared {
    type Target = CaptureExactPublicationShared<MacosPublicationSource, MacosOwnedSource>;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

struct MacosNativeRoute {
    descriptor: ResolvedScreenPublicationDescriptor,
    target: BoundScreenNativeTargetPreparation,
    capture_lifetime: ScreenResourceLifetime,
    pacer: super::CapturePacer,
    next_publish_at: Instant,
    last_accepted_sequence: Option<u64>,
    publisher: Option<ScreenBranchPublisher>,
}

struct MacosExactRuntime {
    source: MacosPublicationSource,
    binding: ScreenWorkerBinding,
    _lifetimes: Box<[ScreenResourceLifetime]>,
    native_routes: Box<[MacosNativeRoute]>,
    #[cfg(feature = "macos-capture-fixtures")]
    fanout_candidate: Option<PreparedCpuPublicationFanoutCandidate>,
    #[cfg(feature = "macos-capture-fixtures")]
    fanout: Option<PreparedCpuPublicationFanout>,
}

impl CaptureExactRuntimeOwner for MacosExactRuntime {
    type Source = MacosPublicationSource;

    const BACKEND_NAME: &'static str = "macOS";
    const ABORTED_BINDING_ERROR: &'static str = "macOS exact runtime was aborted after commit";

    fn source(&self) -> &Self::Source {
        &self.source
    }

    fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }

    fn bind_routes(&mut self, authority: &ScreenCommittedState) -> anyhow::Result<bool> {
        MacosExactRuntime::bind_routes(self, authority)
    }

    fn is_bound(&self) -> bool {
        MacosExactRuntime::is_bound(self)
    }
}

enum WorkerCommand {
    Exact(CaptureExactCommand),
    #[cfg(feature = "macos-capture-fixtures")]
    ReconfigureProcessing {
        calibration: LedToneMapCalibration,
        completion: mpsc::SyncSender<anyhow::Result<()>>,
    },
}

struct PreparedWorker {
    #[cfg(feature = "macos-capture-fixtures")]
    analyzer: ScreenCaptureInput,
    #[cfg(feature = "macos-capture-fixtures")]
    plane_pool: CapturePlanePool,
    target_fps: u32,
}

struct CaptureWorker {
    stop: Arc<AtomicBool>,
    mailbox: MacosFrameMailbox,
    command_tx: mpsc::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<anyhow::Result<()>>,
    join: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
struct MacosExactCommandEndpoint {
    command_tx: mpsc::Sender<WorkerCommand>,
    mailbox: MacosFrameMailbox,
}

impl CaptureExactCommandEndpoint for MacosExactCommandEndpoint {
    const SOURCE_NAME: &'static str = "macOS capture";

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected> {
        self.command_tx
            .send(WorkerCommand::Exact(command))
            .map_err(|_| CaptureExactCommandRejected)
    }

    fn wake(&self) {
        self.mailbox.wake();
    }
}

impl CaptureWorker {
    fn exact_command_endpoint(&self) -> MacosExactCommandEndpoint {
        MacosExactCommandEndpoint {
            command_tx: self.command_tx.clone(),
            mailbox: self.mailbox.clone(),
        }
    }
}

struct StagedCaptureWorker {
    generation: u64,
    worker: Option<CaptureWorker>,
    start: Arc<AtomicBool>,
}

pub struct MacosScreenCaptureInput {
    config: CaptureConfig,
    control: Arc<dyn MacosCaptureControl>,
    #[cfg(feature = "macos-capture-fixtures")]
    admission: ScreenByteAdmissionCoordinator,
    #[cfg(feature = "macos-capture-fixtures")]
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    publication: Arc<Mutex<MacosPublication>>,
    exact: Arc<MacosExactPublicationShared>,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
    worker: Option<CaptureWorker>,
    worker_generation: u64,
    demand: ScreenCaptureDemand,
    running: bool,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    owner: MacosCapabilityOwner,
    owner_conflict: Option<Arc<MacosDaemonOwnerConflict>>,
    owner_designated_requirement_hash: Option<Arc<str>>,
    authorization: MacosScreenAuthorizationState,
    authorization_last_transition_at: Option<Instant>,
    metal4: bool,
}

struct PendingMacosNativeRoute {
    resource_name: Arc<str>,
    capture_resource_name: Arc<str>,
    descriptor: ResolvedScreenPublicationDescriptor,
    target: AdmittedScreenNativeTargetPreparation,
    requested_hz: NonZeroU32,
}

/// Ceiling on the surface handed to the legacy analyzer. The analyzer reduces
/// whatever it receives into the coarse ScreenData grid, so tone-mapping every
/// pixel of a native 4K HDR frame is wasted work that blows the publication
/// freshness budget and starves every HTML screen effect.
#[cfg(feature = "macos-capture-fixtures")]
const LEGACY_ANALYSIS_MAX_WIDTH: u32 = 640;
#[cfg(feature = "macos-capture-fixtures")]
const LEGACY_ANALYSIS_MAX_HEIGHT: u32 = 480;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MacosExactDelivery {
    native: bool,
    #[cfg(feature = "macos-capture-fixtures")]
    cpu: bool,
    stale: bool,
}

#[derive(Default)]
struct TopologyState {
    descriptor: Option<TopologyDescriptor>,
    generation: u64,
}

#[derive(Default)]
struct ResourceState {
    descriptor: Option<ResourceDescriptor>,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResourceDescriptor {
    width: u32,
    height: u32,
    pixel_format: MacosCapturePixelFormat,
    planes: Vec<(u32, u32, u32, usize, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopologyDescriptor {
    width: u32,
    height: u32,
    content: (i64, i64, u32, u32),
    scale_bits: u64,
    screen: Option<(u64, u64, u64, u64)>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
