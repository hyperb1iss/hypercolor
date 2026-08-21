use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use hypercolor_macos_capture::{
    MacosCaptureCadence, MacosCaptureCallbackDiagnostics,
    MacosCaptureCapabilities as NativeCaptureCapabilities, MacosCaptureContentStyle,
    MacosCaptureDynamicRange, MacosCaptureFrame, MacosCapturePixelFormat, MacosCaptureSelection,
    MacosColorPrimaries, MacosCpuSourceView, MacosFrameDropReason, MacosFrameEvent,
    MacosFrameMailbox, MacosFrameStatus, MacosHostArchitecture,
    MacosProtectedSourceState as NativeProtectedSourceState, MacosScreenAuthorizationState,
    MacosScreenOwnerConflict, MacosScreenSelectionSnapshot, MacosScreenStatusSnapshot,
    MacosScreenTahoeSelectionStatus, MacosScreenTahoeStatus, MacosScreenTimingStatus,
    MacosSourceTimingStatus, MacosStreamRequest,
    MacosTahoeSelectionCapabilities as NativeTahoeSelectionCapabilities, MacosTransferFunction,
};
#[cfg(feature = "macos-capture-fixtures")]
use hypercolor_macos_capture::{MacosRuntimeCapability, MacosTahoeRuntimeProbes};
use hypercolor_macos_input::{MacosCapabilityOwner, MacosDaemonOwnerConflict};
use tokio::sync::oneshot;

#[cfg(target_os = "macos")]
use hypercolor_macos_capture::{
    MacosCaptureSelector, MacosDisplayClock, MacosScreenCaptureSession,
    MacosScreenshotReferenceCapture,
};

use super::{
    AdmittedScreenNativeTargetPreparation, BoundScreenNativeTargetPreparation, CaptureCadence,
    CaptureColorSpace, CaptureColorimetry, CaptureConfig, CaptureCursor, CaptureCursorContent,
    CaptureDamage, CaptureDynamicRange, CaptureEpoch, CaptureFrame, CaptureFrameMetadata,
    CaptureLuminanceContext, CapturePixelFormat, CapturePlanePool, CapturePositiveScalar,
    CaptureRotation, CaptureSourceId, CaptureStorage, CaptureTransferFunction, CpuCaptureStorage,
    CpuExactReductionWorkPlan, CpuPublicationFanoutError, CpuReductionExecutor, CpuSamplingError,
    CpuScalarSource, KnownCaptureColorimetry, LedToneMapCalibration, PixelExtent, PixelRect,
    PlatformGpuApi, PlatformGpuSurface, PlatformGpuSurfaceTimingSink, PreparedCpuPublicationFanout,
    PreparedCpuPublicationFanoutCandidate, PreparedLedToneMap, RawCaptureSurface,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAnalysisComputeCapacity,
    ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan, ScreenBackendResourceIdentity,
    ScreenBranchPayload, ScreenBranchPublisher, ScreenByteAdmissionCoordinator,
    ScreenCaptureBackend, ScreenCaptureCadence, ScreenCaptureDemand, ScreenCaptureInput,
    ScreenComputeCapacityPolicy, ScreenCursorCapabilities, ScreenCursorPolicy,
    ScreenExecutorColorCapabilities, ScreenGpuSurfacePayload, ScreenNativeExecutionTargetId,
    ScreenNativeExecutionUnavailableReason, ScreenNativePreparationPayload,
    ScreenNativeWorkPayload, ScreenPhysicalGpuDeviceIdentity, ScreenPreparedWorkerToken,
    ScreenPublicationColorimetry, ScreenPublicationError, ScreenPublicationExecutor,
    ScreenPublicationExecutorFallbackReason, ScreenPublicationExecutorRequest,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenRendererExecutionState,
    ScreenRequiredResourceMinimum, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale, analyze_screen_frame,
};
#[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
use crate::input::SourceKind;
use crate::input::status::SourceSessionSlot;
use crate::input::traits::{
    CapabilityActionDisposition, CapabilityActionIdentity, InputData, InputSource,
    ProtectedSourceAuthorizationAction, ScreenSource, ScreenSourcePickerAction, ScreenSourceRole,
    SourceCapabilityContext, SourceDiagnosticArtifactAction, SourceRoleBinding,
};
use crate::input::{SourceIssue, SourceStatusHandle, SourceStatusReporter};

#[cfg(target_os = "macos")]
mod surface_pool;

#[cfg(target_os = "macos")]
use surface_pool::MacosSurfacePool;

mod admission;
mod control;
mod publication;
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
    protected_action_identity, protected_screen_action_issue, timing_status,
    transfer_function_name,
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
const PUBLICATION_PATH_CPU: u8 = 1;
const PUBLICATION_PATH_NATIVE: u8 = 2;
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

struct MacosOwnedSource {
    source_id: CaptureSourceId,
    binding: ScreenWorkerBinding,
    _runtime_lifetime: ScreenResourceLifetime,
}

#[derive(Default)]
struct MacosExactPublicationShared {
    source: Mutex<Option<MacosPublicationSource>>,
    owned_sources: Mutex<Vec<MacosOwnedSource>>,
    hub: Mutex<Option<Arc<ScreenPublicationHub>>>,
    cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    resolution_revision: AtomicU64,
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
    fanout_candidate: Option<PreparedCpuPublicationFanoutCandidate>,
    fanout: Option<PreparedCpuPublicationFanout>,
}

enum WorkerCommand {
    PrepareExact {
        ticket: ScreenWorkerPreparationTicket,
        cancelled: Arc<AtomicBool>,
        completion: oneshot::Sender<anyhow::Result<ScreenPreparedWorkerToken>>,
    },
    ReapExact {
        completion: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
    ReconfigureProcessing {
        calibration: LedToneMapCalibration,
        completion: mpsc::SyncSender<anyhow::Result<()>>,
    },
}

struct PreparedWorker {
    analyzer: ScreenCaptureInput,
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

struct StagedCaptureWorker {
    generation: u64,
    worker: Option<CaptureWorker>,
    start: Arc<AtomicBool>,
}

pub struct MacosScreenCaptureInput {
    config: CaptureConfig,
    control: Arc<dyn MacosCaptureControl>,
    admission: ScreenByteAdmissionCoordinator,
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

impl MacosScreenCaptureInput {
    #[cfg(target_os = "macos")]
    pub fn new(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> anyhow::Result<Self> {
        Self::with_admission_and_compute_capacity(
            config,
            admission,
            ScreenComputeCapacityPolicy::UNBOUNDED,
        )
    }

    #[cfg(target_os = "macos")]
    fn with_admission_and_compute_capacity(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> anyhow::Result<Self> {
        let selector = MacosCaptureSelector::parse(&config.source)?;
        let host_capabilities = MacosScreenCaptureSession::capabilities()?;
        let request =
            production_stream_request(&config, ScreenCaptureDemand::Inactive, host_capabilities)?;
        let pool_coordinator = admission.clone();
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::renderer_authoritative());
        let pool_telemetry = Arc::clone(&telemetry);
        let session = MacosScreenCaptureSession::new_with_pool_admission(
            request,
            selector,
            move |conservative_surface_bytes, native_metadata_bytes| {
                let pool = MacosSurfacePool::reserve(
                    &pool_coordinator,
                    Arc::clone(&pool_telemetry),
                    conservative_surface_bytes,
                    native_metadata_bytes,
                )?;
                Ok(move |iosurface_id, allocation_bytes| {
                    let token = pool.observe(iosurface_id, allocation_bytes)?;
                    Ok(token as Arc<dyn Send + Sync>)
                })
            },
        )?;
        let clock = MacosDisplayClock::system()?;
        Ok(Self::with_control_and_telemetry(
            config,
            admission,
            compute_capacity_policy,
            Arc::new(NativeCaptureControl {
                session,
                clock,
                host_capabilities,
            }),
            telemetry,
            "screen_capture_kit_native",
        ))
    }

    #[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
    fn with_control_and_telemetry(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
        control: Arc<dyn MacosCaptureControl>,
        telemetry: Arc<MacosScreenRuntimeTelemetry>,
        backend: &'static str,
    ) -> Self {
        let consented = control.authorization() == MacosScreenAuthorizationState::Authorized;
        let authorization = control.authorization();
        let mut source = Self {
            config,
            control,
            admission,
            compute_capacity_policy,
            publication: Arc::new(Mutex::new(MacosPublication::default())),
            exact: Arc::new(MacosExactPublicationShared::with_compute_capacity_policy(
                compute_capacity_policy,
            )),
            telemetry,
            worker: None,
            worker_generation: 0,
            demand: ScreenCaptureDemand::Inactive,
            running: false,
            status: SourceStatusReporter::new(
                "macos:session",
                SourceKind::Screen,
                backend,
                true,
                consented,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            owner: MacosCapabilityOwner::Standalone,
            owner_conflict: None,
            owner_designated_requirement_hash: None,
            authorization,
            authorization_last_transition_at: None,
            metal4: false,
        };
        source
            .refresh_platform_status()
            .expect("new macOS screen status is not retired");
        source
    }

    pub fn authorize(&mut self) -> anyhow::Result<NativeProtectedSourceState> {
        let state = self.control.request_authorization();
        self.refresh_policy()?;
        self.refresh_platform_status()?;
        Ok(state)
    }

    pub fn present_picker(&mut self) -> anyhow::Result<()> {
        let result = self.control.present_picker();
        self.refresh_platform_status()?;
        result
    }

    pub fn protected_state(&self) -> NativeProtectedSourceState {
        self.control.status()
    }

    pub fn set_capability_owner(&mut self, owner: MacosCapabilityOwner) -> anyhow::Result<()> {
        self.owner = owner;
        self.refresh_platform_status()
    }

    fn refresh_platform_status(&mut self) -> anyhow::Result<()> {
        let state = self.control.status();
        let authorization = self.control.authorization();
        if authorization != self.authorization {
            self.authorization = authorization;
            self.authorization_last_transition_at = Some(Instant::now());
        }
        let diagnostics = self.control.diagnostics();
        let source = self.exact.source();
        let timing = MacosScreenTimingStatus {
            callback: timing_status(
                diagnostics.callback_sample_count,
                diagnostics.callback_total_ns,
                diagnostics.callback_max_ns,
                diagnostics.callback_p95_ns,
                diagnostics.callback_p99_ns,
            ),
            retain: timing_status(
                diagnostics.retain_sample_count,
                diagnostics.retain_total_ns,
                diagnostics.retain_max_ns,
                diagnostics.retain_p95_ns,
                diagnostics.retain_p99_ns,
            ),
            enqueue: timing_status(
                diagnostics.enqueue_sample_count,
                diagnostics.enqueue_total_ns,
                diagnostics.enqueue_max_ns,
                diagnostics.enqueue_p95_ns,
                diagnostics.enqueue_p99_ns,
            ),
            conversion: timing_status(
                diagnostics.conversion_sample_count,
                diagnostics.conversion_total_ns,
                diagnostics.conversion_max_ns,
                diagnostics.conversion_p95_ns,
                diagnostics.conversion_p99_ns,
            ),
            cpu_reduction: self.telemetry.cpu_reduction_timing.snapshot(),
            native_import: self.telemetry.native_import_timing.snapshot(),
            native_reduction_submit: self.telemetry.native_reduction_submit_timing.snapshot(),
            publication: timing_status(
                diagnostics.publication_sample_count,
                diagnostics.publication_total_ns,
                diagnostics.publication_max_ns,
                diagnostics.publication_p95_ns,
                diagnostics.publication_p99_ns,
            ),
            capture_to_native_publication: self
                .telemetry
                .capture_to_native_publication_timing
                .snapshot(),
            capture_to_converted_publication: self
                .telemetry
                .capture_to_converted_publication_timing
                .snapshot(),
        };
        let selection = self.control.selection();
        let host_capabilities = self.control.host_capabilities();
        let tahoe = map_tahoe_capabilities(host_capabilities, self.metal4);
        let tahoe_selection = self
            .control
            .tahoe_selection_capabilities()
            .map(map_tahoe_selection_capabilities);
        self.status
            .set_action_issue(protected_screen_action_issue(state))?;
        let status = MacosScreenStatusSnapshot {
            state,
            authorization,
            owner: Arc::from(self.owner.as_str()),
            selection: MacosScreenSelectionSnapshot {
                revision: self.control.selection_revision(),
                selection,
            },
            tahoe,
            tahoe_selection,
            owner_conflict: self
                .owner_conflict
                .as_ref()
                .map(|conflict| MacosScreenOwnerConflict {
                    active: Arc::from(conflict.active.as_str()),
                    contender: Arc::from(conflict.contender.as_str()),
                    observed_at_ms: conflict.observed_at_ms,
                }),
            authorization_last_transition_age_ms: self.authorization_last_transition_at.map(
                |transition| u64::try_from(transition.elapsed().as_millis()).unwrap_or(u64::MAX),
            ),
            owner_designated_requirement_hash: self.owner_designated_requirement_hash.clone(),
            executable_architecture: executable_architecture(),
            capture_session_generation: source
                .as_ref()
                .map(|source| source.epoch.session_generation),
            topology_generation: source
                .as_ref()
                .map(|source| source.epoch.topology_generation),
            resource_generation: source.as_ref().map(|source| source.resource_generation),
            publication_plan_generation: nonzero_telemetry(
                self.telemetry
                    .publication_plan_generation
                    .load(Ordering::Acquire),
            ),
            pixel_format: source
                .as_ref()
                .map(|source| Arc::from(pixel_format_name(source.pixel_format))),
            dynamic_range: source.as_ref().and_then(|source| {
                source
                    .colorimetry
                    .dynamic_range()
                    .map(|range| Arc::from(dynamic_range_name(range)))
            }),
            color_space: source
                .as_ref()
                .map(|source| Arc::from(color_space_name(source.colorimetry.color_space()))),
            transfer_function: source.as_ref().map(|source| {
                Arc::from(transfer_function_name(
                    source.colorimetry.transfer_function(),
                ))
            }),
            display_scale: source
                .as_ref()
                .map(|source| f64::from_bits(source.display_scale_bits)),
            native_width: source
                .as_ref()
                .map(|source| source.geometry.native_extent().width()),
            native_height: source
                .as_ref()
                .map(|source| source.geometry.native_extent().height()),
            queue_depth: hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH,
            admitted_native_bytes: self.telemetry.admitted_native_bytes.load(Ordering::Acquire),
            pinned_generations: self.telemetry.pinned_generations.load(Ordering::Acquire),
            frames_received: diagnostics.frames_received,
            frames_published: diagnostics.frames_published,
            frames_superseded: diagnostics.superseded_deliveries,
            frames_malformed: diagnostics.malformed_frames,
            frames_dropped: frame_drop_counters(&diagnostics).to_vec(),
            frames_stale: self.telemetry.stale_frames.load(Ordering::Acquire),
            publication_path: self.telemetry.publication_path(),
            fallback_reason: lock(&self.telemetry.fallback_reason).clone(),
            timing,
        };
        let diagnostics = hypercolor_macos_capture::screen_diagnostics_envelope(&status)
            .inspect_err(
                |error| tracing::warn!(%error, "dropping invalid macOS screen diagnostics"),
            )
            .ok();
        self.status.set_diagnostics(diagnostics)?;
        Ok(())
    }

    fn refresh_policy(&mut self) -> anyhow::Result<()> {
        self.refresh_policy_for(self.demand)
    }

    fn refresh_policy_for(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let consented = self.control.authorization() == MacosScreenAuthorizationState::Authorized;
        self.status
            .set_policy(true, consented, demand.is_active())?;
        Ok(())
    }

    fn prepare_worker(&self, extent: PixelExtent) -> anyhow::Result<PreparedWorker> {
        let mut analyzer = match self.compute_capacity_policy.analysis() {
            Some(capacity) => {
                ScreenCaptureInput::with_requested_extent_admission_and_compute_capacity(
                    self.config.clone(),
                    extent,
                    self.admission.clone(),
                    capacity,
                )?
            }
            None => ScreenCaptureInput::with_requested_extent_and_admission(
                self.config.clone(),
                extent,
                self.admission.clone(),
            )?,
        };
        analyzer.start()?;
        Ok(PreparedWorker {
            analyzer,
            plane_pool: CapturePlanePool::with_admission_coordinator(self.admission.clone()),
            target_fps: self.config.target_fps,
        })
    }

    fn stage_worker(&self, prepared: PreparedWorker) -> anyhow::Result<StagedCaptureWorker> {
        let worker_generation = self
            .worker_generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("macOS capture worker generation exhausted"))?;
        let mailbox = self.control.mailbox();
        let worker_mailbox = mailbox.clone();
        let control = Arc::clone(&self.control);
        let publication = Arc::clone(&self.publication);
        let exact = Arc::clone(&self.exact);
        let telemetry = Arc::clone(&self.telemetry);
        let status_session = self.status_session.clone();
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
                while !worker_start.load(Ordering::Acquire) {
                    thread::park();
                }
                let result = if worker_stop.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    run_worker(
                        prepared,
                        mailbox,
                        publication,
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
                // The exit channel is only drained by the legacy sampling
                // path; the exact-publication path never observes it, so an
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
        Ok(StagedCaptureWorker {
            generation: worker_generation,
            worker: Some(CaptureWorker {
                stop,
                mailbox: worker_mailbox,
                command_tx,
                exit_rx,
                join: Some(join),
            }),
            start,
        })
    }

    fn install_worker(&mut self, staged: StagedCaptureWorker) {
        let generation = staged.generation;
        let start = Arc::clone(&staged.start);
        let worker = staged.commit();
        let previous_latest = lock(&self.publication).latest.clone();
        self.stop_worker();
        self.worker_generation = generation;
        {
            let mut publication = lock(&self.publication);
            publication.worker_generation = generation;
            publication.latest = previous_latest;
        }
        self.worker = Some(worker);
        start.store(true, Ordering::Release);
        self.worker
            .as_ref()
            .and_then(|worker| worker.join.as_ref())
            .expect("installed worker retains its thread handle")
            .thread()
            .unpark();
    }

    fn stop_worker(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        worker.mailbox.wake();
        if let Some(join) = worker.join.take() {
            let _ = join.join();
        }
        lock(&self.publication).latest = None;
        self.exact.replace_source(None);
    }

    fn observe_worker_exit(&mut self) -> anyhow::Result<()> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        match worker.exit_rx.try_recv() {
            Ok(Ok(())) => {
                self.stop_worker();
                if self.running && self.demand.is_active() {
                    return Err(anyhow!("macOS capture worker exited while active"));
                }
            }
            Ok(Err(error)) => {
                self.stop_worker();
                return Err(error);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.stop_worker();
                return Err(anyhow!("macOS capture worker disconnected"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        Ok(())
    }
}

impl InputSource for MacosScreenCaptureInput {
    fn name(&self) -> &'static str {
        "macos_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        self.refresh_policy()?;
        if let Some(extent) = self.demand.requested_extent() {
            let prepared = self.stage_worker(self.prepare_worker(extent)?)?;
            let session = self.status.begin_session()?;
            self.install_worker(prepared);
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        }
        self.refresh_platform_status()?;
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.control.set_active(false);
        self.refresh_platform_status()
            .expect("live macOS screen status is not retired");
        self.status_session.clear();
        self.stop_worker();
        self.status.stop();
        self.demand = ScreenCaptureDemand::Inactive;
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.running || !self.demand.is_active() {
            return Ok(InputData::None);
        }
        let publication = lock(&self.publication);
        if publication.worker_generation != self.worker_generation {
            return Ok(InputData::None);
        }
        Ok(publication
            .latest
            .as_deref()
            .cloned()
            .unwrap_or(InputData::None))
    }

    fn sample_shared_and_drain_into(
        &mut self,
        _delta_secs: f32,
        _events: &mut Vec<crate::types::event::TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.running || !self.demand.is_active() {
            return Ok(None);
        }
        let publication = lock(&self.publication);
        Ok((publication.worker_generation == self.worker_generation)
            .then(|| publication.latest.clone())
            .flatten())
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

impl ScreenSource for MacosScreenCaptureInput {
    fn set_capability_context(&mut self, context: &SourceCapabilityContext) -> anyhow::Result<()> {
        let Some(owner) = MacosCapabilityOwner::from_id(&context.owner) else {
            return Ok(());
        };
        let conflict = context.conflict.as_ref().and_then(|conflict| {
            Some(MacosDaemonOwnerConflict {
                active: MacosCapabilityOwner::from_id(&conflict.active)?,
                contender: MacosCapabilityOwner::from_id(&conflict.contender)?,
                observed_at_ms: conflict.observed_at_ms,
            })
        });
        self.owner = owner;
        self.owner_conflict = conflict.map(Arc::new);
        self.owner_designated_requirement_hash
            .clone_from(&context.identity_hash);
        self.metal4 = context.features.get("metal4").copied().unwrap_or(false);
        self.refresh_platform_status()
    }
    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.demand
    }

    fn screen_analysis_resource_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisResourcePlan>> {
        let Some(extent) = demand.requested_extent() else {
            return Ok(None);
        };
        Ok(Some(ScreenAnalysisResourcePlan::try_new_for_extent(
            self.config.grid_cols,
            self.config.grid_rows,
            self.config.target_fps,
            extent,
            u64::MAX,
        )?))
    }

    fn screen_analysis_work_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisWorkPlan>> {
        let Some(extent) = demand.requested_extent() else {
            return Ok(None);
        };
        Ok(Some(ScreenAnalysisWorkPlan::try_new(
            extent,
            extent,
            &self.config,
        )?))
    }

    fn screen_analysis_compute_capacity(&self) -> Option<ScreenAnalysisComputeCapacity> {
        self.compute_capacity_policy.analysis()
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let was_active = self.demand.is_active();
        if !demand.is_active() {
            self.refresh_policy_for(demand)?;
            if self.running {
                self.control.set_active(false);
                self.status_session.clear();
                self.stop_worker();
            }
            self.demand = demand;
            self.refresh_platform_status()?;
            return Ok(());
        }
        let request =
            production_stream_request(&self.config, demand, self.control.host_capabilities())?;
        let prepared = demand
            .requested_extent()
            .map(|extent| self.prepare_worker(extent))
            .transpose()?
            .map(|prepared| self.stage_worker(prepared))
            .transpose()?;
        if !self.running {
            self.control.begin_stream_request(request)?.wait()?;
            self.refresh_policy_for(demand)?;
            self.demand = demand;
            return Ok(());
        }
        let request = self.control.begin_stream_request(request)?;
        if let Some(prepared) = prepared {
            let session = if was_active {
                None
            } else {
                self.refresh_policy_for(demand)?;
                self.status.begin_session()?
            };
            if let Err(error) = request.wait() {
                if !was_active {
                    self.refresh_policy_for(self.demand)?;
                }
                return Err(error);
            }
            self.install_worker(prepared);
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        } else {
            request.wait()?;
        }
        self.demand = demand;
        self.refresh_platform_status()?;
        Ok(())
    }

    fn set_screen_renderer_execution_state(&mut self, state: ScreenRendererExecutionState) {
        self.telemetry.set_renderer_execution_state(state);
        let _ = self.refresh_platform_status();
    }

    fn set_screen_publication_hub(&mut self, hub: Arc<ScreenPublicationHub>) {
        *lock(&self.exact.hub) = Some(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.exact.resolution_revision.load(Ordering::Acquire)
    }

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        let Some(source) = self.exact.source() else {
            tracing::debug!(
                shared = ?std::ptr::from_ref(self.exact.as_ref()),
                "exact branch unresolvable: no publication source installed"
            );
            return Ok(None);
        };
        let calibration = LedToneMapCalibration::try_new(
            self.config.target_led_white_x,
            self.config.target_led_white_y,
            self.config.target_led_reference_white_nits,
            self.config.target_led_peak_nits,
            self.config.exposure_ev,
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
        resolve_macos_publication_branch_with_telemetry(&source, &calibrated, &self.telemetry)
    }

    fn owns_screen_publication_source(&self, source_id: &CaptureSourceId) -> bool {
        self.exact.owns_source(source_id)
    }

    fn begin_screen_publication_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        let worker = self.worker.as_ref().ok_or_else(|| {
            anyhow!("macOS capture worker is unavailable for exact publication preparation")
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
            .map_err(|_| anyhow!("macOS capture worker rejected exact publication preparation"))?;
        worker.mailbox.wake();
        let abort_tx = worker.command_tx.clone();
        let abort_mailbox = worker.mailbox.clone();
        Ok(ScreenWorkerPreparation::with_abort(
            async move {
                completion_rx.await.map_err(|_| {
                    anyhow!("macOS capture worker exited during exact publication preparation")
                })?
            },
            move || {
                cancelled.store(true, Ordering::Release);
                let _ = abort_tx.send(WorkerCommand::ReapExact { completion: None });
                abort_mailbox.wake();
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
                    "macOS capture worker rejected exact publication retirement"
                ))
            }));
        }
        worker.mailbox.wake();
        Some(ScreenWorkerRetirement::new(async move {
            completion_rx.await.map_err(|_| {
                anyhow!("macOS capture worker exited during exact publication retirement")
            })?
        }))
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        if !self.demand.is_active() {
            self.config.clone_from(config);
            return Ok(());
        }
        let request =
            production_stream_request(config, self.demand, self.control.host_capabilities())?;
        let prepared = self
            .demand
            .requested_extent()
            .map(|extent| {
                let mut analyzer = match self.compute_capacity_policy.analysis() {
                    Some(capacity) => {
                        ScreenCaptureInput::with_requested_extent_admission_and_compute_capacity(
                            config.clone(),
                            extent,
                            self.admission.clone(),
                            capacity,
                        )?
                    }
                    None => ScreenCaptureInput::with_requested_extent_and_admission(
                        config.clone(),
                        extent,
                        self.admission.clone(),
                    )?,
                };
                analyzer.start()?;
                Ok::<_, anyhow::Error>(PreparedWorker {
                    analyzer,
                    plane_pool: CapturePlanePool::with_admission_coordinator(
                        self.admission.clone(),
                    ),
                    target_fps: config.target_fps,
                })
            })
            .transpose()?
            .map(|prepared| self.stage_worker(prepared))
            .transpose()?;
        let request = self.control.begin_stream_request(request)?;
        request.wait()?;
        if self.running
            && let Some(prepared) = prepared
        {
            self.install_worker(prepared);
        }
        self.config.clone_from(config);
        Ok(())
    }

    fn reconfigure_screen_processing(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        let next = LedToneMapCalibration::try_new(
            config.target_led_white_x,
            config.target_led_white_y,
            config.target_led_reference_white_nits,
            config.target_led_peak_nits,
            config.exposure_ev,
        )?;
        let current = LedToneMapCalibration::try_new(
            self.config.target_led_white_x,
            self.config.target_led_white_y,
            self.config.target_led_reference_white_nits,
            self.config.target_led_peak_nits,
            self.config.exposure_ev,
        )?;
        if current == next {
            return Ok(());
        }
        if let Some(worker) = self.worker.as_ref() {
            let (completion_tx, completion_rx) = mpsc::sync_channel(1);
            worker
                .command_tx
                .send(WorkerCommand::ReconfigureProcessing {
                    calibration: next,
                    completion: completion_tx,
                })
                .map_err(|_| anyhow!("macOS capture worker rejected processing reconfiguration"))?;
            worker.mailbox.wake();
            completion_rx.recv().map_err(|_| {
                anyhow!("macOS capture worker exited during processing reconfiguration")
            })??;
        }
        self.config.target_led_white_x = next.target_white_x();
        self.config.target_led_white_y = next.target_white_y();
        self.config.target_led_reference_white_nits = next.target_reference_white_nits();
        self.config.target_led_peak_nits = next.target_peak_nits();
        self.config.exposure_ev = next.exposure_ev();
        self.exact.advance_resolution_revision();
        Ok(())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.present_picker()
    }

    fn screen_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        let control = Arc::clone(&self.control);
        Some(ProtectedSourceAuthorizationAction::new(
            Arc::new(move || {
                control.request_authorization();
                Ok(control.authorization() == MacosScreenAuthorizationState::Authorized)
            }),
            protected_action_identity(self.owner, false),
        ))
    }

    fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        let control = Arc::clone(&self.control);
        Some(ScreenSourcePickerAction::new(
            Arc::new(move || control.present_picker()),
            protected_action_identity(self.owner, true),
        ))
    }

    #[cfg(target_os = "macos")]
    fn diagnostic_artifact_action(&self) -> Option<SourceDiagnosticArtifactAction> {
        let control = Arc::clone(&self.control);
        Some(Arc::new(move || {
            control
                .capture_screenshot_reference()
                .map(|receiver| Box::new(receiver) as crate::input::SourceDiagnosticArtifact)
        }))
    }
}

impl SourceRoleBinding for MacosScreenCaptureInput {
    type Role = ScreenSourceRole;
}

impl Drop for MacosScreenCaptureInput {
    fn drop(&mut self) {
        self.control.set_active(false);
        self.stop_worker();
    }
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
const LEGACY_ANALYSIS_MAX_WIDTH: u32 = 640;
const LEGACY_ANALYSIS_MAX_HEIGHT: u32 = 480;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MacosExactDelivery {
    native: bool,
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
