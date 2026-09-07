use std::cell::Cell;
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
    ResolvedScreenSourceConfig, ScreenBackendResourceIdentity, ScreenBranchPayload,
    ScreenBranchPublisher, ScreenCaptureBackend, ScreenCaptureCadence, ScreenCaptureDemand,
    ScreenCommittedState, ScreenCursorCapabilities, ScreenCursorPolicy,
    ScreenExecutorColorCapabilities, ScreenGpuSurfacePayload, ScreenNativeExecutionTargetId,
    ScreenNativeExecutionUnavailableReason, ScreenNativePreparationPayload,
    ScreenNativeWorkPayload, ScreenPhysicalGpuDeviceIdentity, ScreenPreparedWorkerToken,
    ScreenPublicationColorimetry, ScreenPublicationError, ScreenPublicationExecutor,
    ScreenPublicationExecutorFallbackReason, ScreenPublicationExecutorRequest,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenRendererExecutionState,
    ScreenRequiredResourceMinimum, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerBinding,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale,
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
use crate::input::traits::SourceDiagnosticArtifactAction;
use crate::input::traits::{
    CapabilityActionDisposition, CapabilityActionIdentity, InputData, InputSource,
    ProtectedSourceAuthorizationAction, ScreenSource, ScreenSourcePickerAction, ScreenSourceRole,
    SourceCapabilityContext, SourceRoleBinding,
};
use crate::input::{SourceIssue, SourceStatusHandle, SourceStatusReporter};

#[cfg(feature = "macos-capture-fixtures")]
use super::adapter::CpuExecutorSlot;
use super::adapter::{
    CaptureActivityHandle, CaptureBackend, CaptureBackendHandles, CaptureCommandEndpoint,
    CaptureExactPublicationShared, CaptureExactRuntimeOwner, CaptureExactState,
    CaptureOwnedSourceRecord, CapturePublicationFence, CapturePublicationSource,
    CaptureRetirementCause, CaptureSessionAuthority, CaptureSessionTransaction, CaptureSourceShell,
    CaptureWorkerCommand, ReservedCaptureSessionAuthority, VersionedCaptureSettings,
    execute_capture_exact_command,
};
use super::{ScreenByteAdmissionCoordinator, ScreenComputeCapacityPolicy};

mod surface_pool;

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

struct NativeCaptureControl {
    session: MacosScreenCaptureSession,
    clock: MacosDisplayClock,
    host_capabilities: NativeCaptureCapabilities,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MacosPublicationFence(u64);

impl CapturePublicationFence<u64> for MacosPublicationFence {
    fn admits(&self, epoch: &u64) -> bool {
        self.0 == *epoch
    }
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

type MacosOwnedSource = CaptureOwnedSourceRecord;

#[derive(Default)]
struct MacosExactPublicationShared {
    common: CaptureExactPublicationShared<MacosPublicationSource, MacosOwnedSource>,
    #[cfg(feature = "macos-capture-fixtures")]
    cpu_executor: CpuExecutorSlot,
    #[cfg(feature = "macos-capture-fixtures")]
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    /// Latest reference `ScreenData` analyzed by the fixture CPU lane, keyed
    /// by the worker generation that produced it.
    #[cfg(feature = "macos-capture-fixtures")]
    fixture_reference: Mutex<Option<(u64, Arc<InputData>)>>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosExactPublicationShared {
    fn publish_fixture_reference(&self, worker_generation: u64, value: Arc<InputData>) {
        *lock(&self.fixture_reference) = Some((worker_generation, value));
    }

    fn clear_fixture_reference(&self) {
        drop(lock(&self.fixture_reference).take());
    }

    fn fixture_reference(&self, worker_generation: u64) -> Option<Arc<InputData>> {
        lock(&self.fixture_reference)
            .as_ref()
            .filter(|(generation, _)| *generation == worker_generation)
            .map(|(_, value)| Arc::clone(value))
    }

    fn latest_fixture_reference(&self) -> Option<Arc<InputData>> {
        lock(&self.fixture_reference)
            .as_ref()
            .map(|(_, value)| Arc::clone(value))
    }
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

enum BackendWorkerCommand {
    #[cfg(feature = "macos-capture-fixtures")]
    ReconfigureProcessing {
        calibration: LedToneMapCalibration,
        completion: mpsc::SyncSender<anyhow::Result<()>>,
    },
}

type WorkerCommand = CaptureWorkerCommand<BackendWorkerCommand>;
type MacosExactCommandEndpoint = CaptureCommandEndpoint<
    BackendWorkerCommand,
    mpsc::Sender<WorkerCommand>,
    CaptureSessionAuthority,
>;

struct PreparedWorker {
    #[cfg(feature = "macos-capture-fixtures")]
    analyzer: ScreenCaptureInput,
    #[cfg(feature = "macos-capture-fixtures")]
    plane_pool: CapturePlanePool,
    target_fps: u32,
}

struct CaptureWorker {
    authority: CaptureSessionAuthority,
    stop: Arc<AtomicBool>,
    start: Arc<AtomicBool>,
    mailbox: MacosFrameMailbox,
    command_tx: mpsc::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<anyhow::Result<()>>,
    join: Option<thread::JoinHandle<()>>,
}

struct MacosCaptureBackend {
    control: Arc<dyn MacosCaptureControl>,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
    status_session: SourceSessionSlot,
    worker_generation: Cell<u64>,
}

struct MacosWorkerSpawn {
    prepared: PreparedWorker,
}

struct MacosAuthorityCommitCheckpoint<'a> {
    handles: CaptureBackendHandles<'a, MacosCaptureBackend>,
    worker_generation: &'a Cell<u64>,
    generation: u64,
    /// Last-good fixture reference carried across the worker handoff.
    #[cfg(feature = "macos-capture-fixtures")]
    fixture_reference: Option<Arc<InputData>>,
}

impl CaptureBackend for MacosCaptureBackend {
    type Worker = CaptureWorker;
    type Readiness = ();
    type SpawnRequest = MacosWorkerSpawn;
    type SettingsConfig = CaptureConfig;
    type ExactState = MacosExactPublicationShared;
    type ActivityFence = MacosPublicationFence;
    type ActivityEpoch = u64;
    type AuthorityCommitCheckpoint<'a> = MacosAuthorityCommitCheckpoint<'a>;

    const NAME: &'static str = "macOS capture";
    const READINESS_TIMEOUT: Duration = Duration::ZERO;

    fn resolve_publication_branch(
        &self,
        settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        source: &<Self::ExactState as CaptureExactState>::Source,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        let config = settings.snapshot().config;
        let calibration = LedToneMapCalibration::try_new(
            config.target_led_white_x,
            config.target_led_white_y,
            config.target_led_reference_white_nits,
            config.target_led_peak_nits,
            config.exposure_ev,
        )?;
        let request = demand.request();
        let processing_profile = request
            .processing_profile()
            .as_ref()
            .clone()
            .with_led_tone_map(calibration);
        let calibrated = RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                request.selector().clone(),
                request.kind(),
                request.executor().clone(),
                request.extent(),
                request.aspect(),
                Arc::new(processing_profile),
            ),
            demand.requested_hz(),
        );
        resolve_macos_publication_branch_with_telemetry(source, &calibrated, &self.telemetry)
    }

    fn spawn_worker(
        &self,
        request: Self::SpawnRequest,
        handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        let MacosWorkerSpawn { prepared } = request;
        let mailbox = self.control.mailbox();
        let control = Arc::clone(&self.control);
        let activity = handles.activity_handle_ref();
        let exact = handles.exact_state_handle();
        let telemetry = Arc::clone(&self.telemetry);
        let status_session = self.status_session.clone();
        let authority = reservation.authority();
        let worker_generation = authority.generation();
        let worker_mailbox = mailbox.clone();
        let target_fps = prepared.target_fps;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let start = Arc::new(AtomicBool::new(false));
        let worker_start = Arc::clone(&start);
        let (exit_tx, exit_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("hypercolor-macos-screen-capture".to_owned())
            .spawn(move || {
                while !worker_start.load(Ordering::Acquire) && !worker_stop.load(Ordering::Acquire)
                {
                    thread::park();
                }
                let result = if worker_stop.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    run_worker(
                        prepared,
                        mailbox,
                        activity,
                        exact,
                        telemetry,
                        worker_generation,
                        target_fps,
                        status_session,
                        worker_stop,
                        control,
                        command_rx,
                    )
                };
                // The exit channel is only drained by the source's sampling
                // tick; the exact-publication path never observes it, so an
                // unlogged error here is a silently dead capture pump.
                if let Err(error) = &result {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "macOS screen capture worker exited with error"
                    );
                } else {
                    tracing::debug!("macOS screen capture worker exited cleanly");
                }
                let _ = exit_tx.send(result);
            })?;
        Ok(CaptureSessionTransaction::new(
            CaptureWorker {
                authority,
                stop,
                start,
                mailbox: worker_mailbox,
                command_tx,
                exit_rx,
                join: Some(join),
            },
            (),
            reservation,
        ))
    }

    fn prepare_authority_commit<'a>(
        &'a self,
        handles: CaptureBackendHandles<'a, Self>,
        reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>> {
        let generation = reservation.authority().generation();
        #[cfg(feature = "macos-capture-fixtures")]
        let fixture_reference = handles.exact_state().latest_fixture_reference();
        Some(MacosAuthorityCommitCheckpoint {
            handles,
            worker_generation: &self.worker_generation,
            generation,
            #[cfg(feature = "macos-capture-fixtures")]
            fixture_reference,
        })
    }

    fn commit_authority(
        reservation: ReservedCaptureSessionAuthority,
        checkpoint: Self::AuthorityCommitCheckpoint<'_>,
    ) {
        assert_eq!(reservation.authority().generation(), checkpoint.generation);
        checkpoint.worker_generation.set(checkpoint.generation);
        let displaced_exact = checkpoint
            .handles
            .exact_state()
            .activate_reserved_authority(reservation)
            .expect("reserved macOS capture authority remains current");
        let displaced_epoch = lock(checkpoint.handles.activity())
            .replace_fence_and_activate(
                MacosPublicationFence(checkpoint.generation),
                checkpoint.generation,
            )
            .expect("the macOS worker generation matches its activity fence");
        #[cfg(feature = "macos-capture-fixtures")]
        if let Some(reference) = checkpoint.fixture_reference {
            checkpoint
                .handles
                .exact_state()
                .publish_fixture_reference(checkpoint.generation, reference);
        }
        drop((displaced_exact, displaced_epoch));
    }

    fn retire_authority(
        &self,
        handles: CaptureBackendHandles<'_, Self>,
        authority: CaptureSessionAuthority,
        _cause: CaptureRetirementCause,
    ) {
        let Some(retirement) = handles
            .exact_state()
            .retire_authority_if_current(authority)
            .expect("macOS capture worker generation exhausted during retirement")
        else {
            return;
        };
        let retirement_generation = retirement.replacement().generation();
        self.worker_generation.set(retirement_generation);
        let displaced_exact = retirement.into_displaced();
        let displaced_epoch = lock(handles.activity())
            .replace_fence_and_activate(
                MacosPublicationFence(retirement_generation),
                retirement_generation,
            )
            .expect("macOS retirement generation matches its activity fence");
        #[cfg(feature = "macos-capture-fixtures")]
        handles.exact_state().clear_fixture_reference();
        drop((displaced_exact, displaced_epoch));
    }
}

pub struct MacosScreenCaptureInput {
    control: Arc<dyn MacosCaptureControl>,
    #[cfg(feature = "macos-capture-fixtures")]
    admission: ScreenByteAdmissionCoordinator,
    #[cfg(feature = "macos-capture-fixtures")]
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
    shell: CaptureSourceShell<MacosCaptureBackend>,
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

/// Extent the fixture-only reference analyzer reduces into.
#[cfg(feature = "macos-capture-fixtures")]
fn fixture_analysis_extent() -> PixelExtent {
    PixelExtent::new(LEGACY_ANALYSIS_MAX_WIDTH, LEGACY_ANALYSIS_MAX_HEIGHT)
        .expect("fixture analysis extent is non-empty")
}

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
