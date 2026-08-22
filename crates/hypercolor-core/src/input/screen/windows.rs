//! Windows screen capture source backed by DXGI Desktop Duplication.
//!
//! Shaped like [`super::wayland::WaylandScreenCaptureInput`]: a worker thread
//! owns the capture session and the analysis pipeline, and the render loop
//! only clones the latest processed [`ScreenData`]. It is markedly simpler
//! than the Wayland source because Windows has nothing to negotiate — no
//! portal handshake, no source picker, no restore token, and no permission
//! grant of any kind.
//!
//! The duplication interface is opened lazily when capture goes active and
//! dropped the moment it goes idle. Windows allows one duplication per output
//! per process, and other ambient-lighting tools want the same interface, so
//! holding it while no effect needs it would be antisocial.

use std::alloc::Layout;
use std::collections::HashMap;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use hypercolor_windows_capture::{
    CaptureError, CaptureExtent as NativeCaptureExtent, CaptureLane, CapturePumpRequest,
    CaptureRegion, CaptureResourceAdmission, CaptureResourceKind, CaptureResourceLease,
    CaptureResourceReservation, CpuDesktopFrame, CpuDesktopReadbackResourceQuote,
    DesktopDuplicator, DisplayRotation, Frame as NativeCaptureFrame, GpuAdapterLuid,
    GpuReductionAdmission, GpuReductionPublicationDisposition, GpuReductionPublishOutcome,
    GpuSurfaceAdmission, GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace,
    GpuSurfaceCursorPolicy, GpuSurfaceDescriptor, GpuSurfaceDescriptorConfig,
    GpuSurfaceDescriptorId, GpuSurfaceFilter, GpuSurfaceFormat, GpuSurfacePlanGeneration,
    GpuSurfacePublicationDisposition, GpuSurfacePublishOutcome, GpuSurfaceSourceColorSpace,
    GpuSurfaceTargetPreparation, GpuSurfaceTargetPreparationResourceQuote,
    PreparedCpuDesktopReadback, PreparedGpuReductionPlan, PreparedGpuSurfacePlan,
    ReductionTelemetry,
};
use tracing::{debug, info, warn};

use crate::input::screen::{
    AdmittedScreenNativeTargetPreparation, AnalyzedScreenSnapshot,
    BoundScreenNativeTargetPreparation, CaptureCadence, CaptureCadenceError, CaptureColorSpace,
    CaptureColorimetry, CaptureConfig, CaptureCursor, CaptureDamage, CaptureDynamicRange,
    CaptureEpoch, CaptureFrame, CaptureFrameMetadata, CaptureGeometry, CapturePacer,
    CapturePixelFormat, CaptureRotation, CaptureSourceId, CaptureStorage, CaptureTransferFunction,
    CpuCaptureStorage, CpuExactReductionWorkPlan, CpuReductionExecutor, ExactBoxList, ExactBoxNode,
    PhysicalOrigin, PixelExtent, PlatformGpuApi, PlatformGpuSurface, PreparedCpuPublicationFanout,
    PreparedCpuPublicationFanoutCandidate, RawCaptureSurface, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity,
    ScreenAnalysisAdmissionError, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan,
    ScreenAnalysisWorkPlan, ScreenBackendResourceIdentity, ScreenBranchPayload,
    ScreenBranchPublisher, ScreenByteAdmissionCoordinator, ScreenByteLease, ScreenByteReservation,
    ScreenCaptureBackend, ScreenCaptureDemand, ScreenCaptureInput,
    ScreenColorTransformCapabilities, ScreenCommittedState, ScreenComputeCapacityPolicy,
    ScreenCursorCapabilities, ScreenCursorPolicy, ScreenExecutorColorCapabilities,
    ScreenGpuSurfacePayload, ScreenNativePreparationPayload, ScreenPhysicalGpuDeviceIdentity,
    ScreenPhysicalReductionDescriptor, ScreenPreparedWorkerToken, ScreenPublicationColorimetry,
    ScreenPublicationExecutor, ScreenPublicationExecutorRequest, ScreenPublicationHealth,
    ScreenPublicationHub, ScreenPublicationKind, ScreenPublicationMetadata, ScreenReductionFilter,
    ScreenRequiredResourceMinimum, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerBinding,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale, analyze_screen_frame,
};
use crate::input::status::{
    ScreenCaptureDiagnostics, ScreenCaptureReductionPath, SourceDiagnostics,
};
use crate::input::traits::{
    InputData, InputSource, ScreenSource, ScreenSourceRole, SourceRoleBinding,
};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceSessionWriter, SourceStatusHandle,
    SourceStatusReporter,
};
use hypercolor_worker_retention::{retain_worker, spawn_worker};

use super::adapter::{
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureExactRuntimeOwner, CaptureOwnedSource,
    CapturePublication as AdapterCapturePublication, CapturePublicationFence,
    CapturePublicationSource, CaptureSessionAuthority, VersionedCaptureSettings,
    begin_capture_exact_preparation, begin_capture_exact_retirement,
    bind_current_capture_exact_runtime, execute_capture_exact_command,
};

/// How long a worker waits on DXGI before checking its command channel.
///
/// Bounded well under a second so a stop or deactivate lands promptly even
/// while the desktop is perfectly static and producing no frames at all.
const FRAME_WAIT: Duration = Duration::from_millis(100);
const READBACK_POLL_WAIT: Duration = Duration::from_millis(1);
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);

static WINDOWS_GPU_PREPARATION_GATES: OnceLock<Mutex<HashMap<GpuAdapterLuid, Arc<Mutex<()>>>>> =
    OnceLock::new();

/// Backoff after a failed attempt to open the duplication interface.
///
/// The common cause is another application holding it, which resolves when
/// that application exits, so retrying quietly beats surfacing an error the
/// user cannot act on.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);

/// Persists a legacy monitor selector after its stable output id is known.
pub type CaptureSourceSink = Arc<dyn Fn(ResolvedCaptureSource) + Send + Sync>;

/// A successfully opened legacy source and the stable value it resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCaptureSource {
    /// Exact configured value used to open the capture session.
    pub configured_source: String,
    /// Stable source value suitable for persistence.
    pub stable_source: String,
}

/// Settings shared between the input source handle and the capture worker.
struct SharedSettings {
    values: VersionedCaptureSettings<VersionedCaptureConfig>,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    admission_coordinator: ScreenByteAdmissionCoordinator,
    session_generation: AtomicU64,
    activity_generation: AtomicU64,
}

#[derive(Debug)]
struct WindowsCaptureResourceAdmission {
    coordinator: ScreenByteAdmissionCoordinator,
}

#[derive(Debug)]
struct WindowsCaptureResourceReservation {
    kind: CaptureResourceKind,
    reservation: ScreenByteReservation,
}

#[derive(Debug)]
struct WindowsCaptureResourceLease {
    kind: CaptureResourceKind,
    lease: ScreenByteLease,
}

impl CaptureResourceAdmission for WindowsCaptureResourceAdmission {
    fn try_reserve(
        &self,
        kind: CaptureResourceKind,
        peak_bytes: u64,
    ) -> Result<Box<dyn CaptureResourceReservation>, CaptureError> {
        let reservation = self.coordinator.try_acquire(peak_bytes).map_err(|_| {
            CaptureError::ResourceExhausted {
                operation: capture_resource_operation(kind),
                requested_bytes: usize::try_from(peak_bytes).unwrap_or(usize::MAX),
            }
        })?;
        Ok(Box::new(WindowsCaptureResourceReservation {
            kind,
            reservation,
        }))
    }
}

impl CaptureResourceReservation for WindowsCaptureResourceReservation {
    fn kind(&self) -> CaptureResourceKind {
        self.kind
    }

    fn bytes(&self) -> u64 {
        self.reservation.bytes()
    }

    fn commit(
        mut self: Box<Self>,
        retained_bytes: u64,
    ) -> Result<Arc<dyn CaptureResourceLease>, CaptureError> {
        let quoted_bytes = self.reservation.bytes();
        self.reservation
            .reconcile_down(retained_bytes)
            .map_err(|_| CaptureError::ResourceAdmissionMismatch {
                operation: "reconcile Windows capture resource reservation",
                expected_kind: self.kind,
                expected_bytes: quoted_bytes,
                actual_kind: self.kind,
                actual_bytes: retained_bytes,
            })?;
        Ok(Arc::new(WindowsCaptureResourceLease {
            kind: self.kind,
            lease: self.reservation.freeze(),
        }))
    }
}

impl CaptureResourceLease for WindowsCaptureResourceLease {
    fn kind(&self) -> CaptureResourceKind {
        self.kind
    }

    fn bytes(&self) -> u64 {
        self.lease.bytes()
    }
}

const fn capture_resource_operation(kind: CaptureResourceKind) -> &'static str {
    match kind {
        CaptureResourceKind::PointerShape => "reserve Windows pointer shape",
        CaptureResourceKind::CanonicalDesktop => "reserve Windows canonical desktop",
        CaptureResourceKind::PointerTexture => "reserve Windows pointer texture",
        CaptureResourceKind::CompatibilityReductionConstantBuffer => {
            "reserve Windows compatibility reduction constant buffer"
        }
        CaptureResourceKind::CompatibilityReductionTextures => {
            "reserve Windows compatibility reduction textures"
        }
        CaptureResourceKind::CompatibilityCpuStagingTexture => {
            "reserve Windows compatibility CPU staging texture"
        }
        CaptureResourceKind::CompatibilityFramePlane => "reserve Windows compatibility frame plane",
    }
}

#[derive(Clone)]
struct VersionedCaptureConfig {
    value: CaptureConfig,
    source_generation: u64,
}

struct CaptureSettingsSnapshot {
    config: CaptureConfig,
    source_generation: u64,
    demand: ScreenCaptureDemand,
}

struct PreparedWorkerSettings {
    config: CaptureConfig,
    cadence: CaptureCadence,
    source_generation: u64,
    demand: ScreenCaptureDemand,
    analyzer: ScreenCaptureInput,
}

struct WorkerCaptureSchedule {
    cadence: CaptureCadence,
    pacer: CapturePacer,
    next_analysis_at: Instant,
}

impl WorkerCaptureSchedule {
    fn new(cadence: CaptureCadence, now: Instant) -> Self {
        Self {
            cadence,
            pacer: cadence.pacer(),
            next_analysis_at: now,
        }
    }

    fn replace(&mut self, cadence: CaptureCadence, now: Instant) {
        *self = Self::new(cadence, now);
    }

    fn wait_duration(&self, now: Instant) -> Option<Duration> {
        let wait = self.next_analysis_at.saturating_duration_since(now);
        (!wait.is_zero()).then_some(wait)
    }

    fn record_frame(
        &mut self,
        captured_at: Instant,
        now: Instant,
    ) -> Result<Instant, CaptureCadenceError> {
        let freshness_deadline = self.cadence.freshness_deadline(captured_at)?;
        self.next_analysis_at = self.pacer.advance_deadline(self.next_analysis_at, now)?;
        Ok(freshness_deadline)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveCaptureEpoch {
    epoch: CaptureEpoch,
    source_generation: u64,
    activity_generation: u64,
    duplication_generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CaptureAuthorityFence {
    source_generation: u64,
    activity_generation: u64,
}

type CapturePublication<T> =
    AdapterCapturePublication<CaptureAuthorityFence, ActiveCaptureEpoch, T>;

impl CapturePublicationFence<ActiveCaptureEpoch> for CaptureAuthorityFence {
    fn admits(&self, epoch: &ActiveCaptureEpoch) -> bool {
        epoch.source_generation == self.source_generation
            && epoch.activity_generation == self.activity_generation
    }
}

impl<T> CapturePublication<T> {
    fn fence_source(&mut self, source_generation: u64) -> Option<T> {
        self.replace_fence(CaptureAuthorityFence {
            source_generation,
            ..*self.fence()
        })
        .latest
    }

    fn fence_activity(&mut self, activity_generation: u64) -> Option<T> {
        self.replace_fence(CaptureAuthorityFence {
            activity_generation,
            ..*self.fence()
        })
        .latest
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowsPublicationSource {
    epoch: CaptureEpoch,
    native_extent: PixelExtent,
    logical_extent: PixelExtent,
    origin: PhysicalOrigin,
    rotation: CaptureRotation,
    colorimetry: CaptureColorimetry,
    source_color_space: GpuSurfaceSourceColorSpace,
    adapter_luid: GpuAdapterLuid,
    duplication_generation: u64,
    is_primary: bool,
}

impl WindowsPublicationSource {
    fn from_session(session: &DesktopDuplicator, session_generation: u64) -> anyhow::Result<Self> {
        let (native_width, native_height) = session.native_extent();
        let (logical_width, logical_height) = session.logical_extent();
        let (origin_x, origin_y) = session.origin();
        Ok(Self {
            epoch: CaptureEpoch {
                source_id: capture_source_id(session.source_id())?,
                topology_generation: session.topology_generation(),
                session_generation,
            },
            native_extent: PixelExtent::new(native_width, native_height)?,
            logical_extent: PixelExtent::new(logical_width, logical_height)?,
            origin: PhysicalOrigin {
                x: origin_x,
                y: origin_y,
            },
            rotation: capture_rotation(session.rotation()),
            colorimetry: capture_colorimetry(session.source_color_space())?,
            source_color_space: session.source_color_space(),
            adapter_luid: session.adapter_luid(),
            duplication_generation: session.duplication_generation(),
            is_primary: session.is_primary(),
        })
    }

    fn matches_selector(&self, selector: &ScreenSourceSelector) -> bool {
        match selector {
            ScreenSourceSelector::Configured => true,
            ScreenSourceSelector::Primary => self.is_primary,
            ScreenSourceSelector::Exact(source_id) => source_id == &self.epoch.source_id,
        }
    }

    fn cpu_source(&self, selector: ScreenSourceSelector) -> anyhow::Result<ResolvedScreenSource> {
        let geometry = CaptureGeometry::new(
            self.origin,
            self.native_extent,
            self.native_extent,
            self.rotation,
            None,
            SourceScale::ONE,
        )?;
        Ok(ResolvedScreenSource::new(
            selector,
            self.epoch.clone(),
            ResolvedScreenSourceConfig::new_with_cursor_capabilities(
                geometry,
                self.logical_extent,
                ScreenSourceReflection::None,
                CapturePixelFormat::Bgra8,
                self.colorimetry,
                ScreenCursorCapabilities::clean_only(),
                ScreenBackendResourceIdentity::new(
                    ScreenCaptureBackend::WindowsDesktopDuplication,
                    ScreenResourceApi::Cpu,
                    self.epoch.session_generation,
                    self.duplication_generation,
                ),
            ),
        ))
    }

    fn gpu_source(&self, selector: ScreenSourceSelector) -> anyhow::Result<ResolvedScreenSource> {
        let geometry = CaptureGeometry::new(
            self.origin,
            self.logical_extent,
            self.logical_extent,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        )?;
        Ok(ResolvedScreenSource::new(
            selector,
            self.epoch.clone(),
            ResolvedScreenSourceConfig::new_with_cursor_capabilities(
                geometry,
                self.logical_extent,
                ScreenSourceReflection::None,
                CapturePixelFormat::Rgba8,
                self.colorimetry,
                ScreenCursorCapabilities::clean_with_separate_cursor(),
                ScreenBackendResourceIdentity::new_with_physical_gpu_device(
                    ScreenCaptureBackend::WindowsDesktopDuplication,
                    ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
                    screen_gpu_identity(self.adapter_luid),
                    self.epoch.session_generation,
                    self.duplication_generation,
                ),
            ),
        ))
    }
}

impl CapturePublicationSource for WindowsPublicationSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.epoch.source_id
    }
}

struct WindowsOwnedSource {
    source_id: CaptureSourceId,
    binding: ScreenWorkerBinding,
    _runtime_lifetime: ScreenResourceLifetime,
}

impl CaptureOwnedSource for WindowsOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    fn belongs_to_authority(&self, authority: &ScreenCommittedState) -> bool {
        authority.owns_runtime_binding(&self.binding)
    }
}

#[derive(Default)]
struct ExactPublicationShared {
    common: CaptureExactPublicationShared<WindowsPublicationSource, WindowsOwnedSource>,
    cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
    next_descriptor_id: AtomicU64,
}

impl Deref for ExactPublicationShared {
    type Target = CaptureExactPublicationShared<WindowsPublicationSource, WindowsOwnedSource>;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl ExactPublicationShared {
    #[cfg(test)]
    fn install_test_source(&self, next: Option<WindowsPublicationSource>) -> bool {
        let authority = CaptureSessionAuthority::new(1);
        drop(self.activate_authority(authority));
        self.replace_source_if_current(authority, next)
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

    fn next_gpu_descriptor_id(&self) -> anyhow::Result<GpuSurfaceDescriptorId> {
        let previous = self
            .next_descriptor_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| anyhow!("Windows GPU descriptor identity exhausted"))?;
        let id = previous
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or_else(|| anyhow!("Windows GPU descriptor identity exhausted"))?;
        Ok(GpuSurfaceDescriptorId::new(id))
    }
}

impl SharedSettings {
    fn snapshot(&self) -> CaptureSettingsSnapshot {
        let snapshot = self.values.snapshot();
        CaptureSettingsSnapshot {
            config: snapshot.config.value,
            source_generation: snapshot.config.source_generation,
            demand: snapshot.demand,
        }
    }

    fn commit(&self, prepared: &PreparedWorkerSettings) -> u64 {
        self.commit_values(
            &prepared.config,
            prepared.source_generation,
            prepared.demand,
        )
    }

    fn commit_values(
        &self,
        next_config: &CaptureConfig,
        source_generation: u64,
        demand: ScreenCaptureDemand,
    ) -> u64 {
        let mut values = self.values.lock();
        values.config_mut().value.clone_from(next_config);
        values.config_mut().source_generation = source_generation;
        *values.demand_mut() = demand;
        values.commit()
    }
}

/// Windows-only live screen capture input source.
pub struct WindowsScreenCaptureInput {
    settings: Arc<SharedSettings>,
    running: bool,
    capture_demand: ScreenCaptureDemand,
    publication: Arc<Mutex<CapturePublication<AnalyzedScreenSnapshot>>>,
    exact: Arc<ExactPublicationShared>,
    worker: Option<CaptureWorker>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    source_sink: Option<CaptureSourceSink>,
    #[cfg(feature = "windows-capture-fixtures")]
    fixture: Option<Arc<WindowsScreenCaptureFixtureState>>,
}

#[cfg(feature = "windows-capture-fixtures")]
struct WindowsScreenCaptureFixtureState {
    analyzer: Mutex<ScreenCaptureInput>,
    epoch: CaptureEpoch,
    active: Mutex<Option<ActiveCaptureEpoch>>,
}

/// Deterministic adapter-boundary publisher for Windows capture integration tests.
#[cfg(feature = "windows-capture-fixtures")]
#[doc(hidden)]
pub struct WindowsScreenCaptureFixture {
    state: Arc<WindowsScreenCaptureFixtureState>,
    publication: Arc<Mutex<CapturePublication<AnalyzedScreenSnapshot>>>,
    status_session: SourceSessionSlot,
}

#[cfg(feature = "windows-capture-fixtures")]
impl WindowsScreenCaptureFixture {
    /// Whether daemon demand has activated the deterministic source.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Extent currently owned by the deterministic analysis worker.
    #[must_use]
    pub fn requested_extent(&self) -> PixelExtent {
        self.state
            .analyzer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .requested_extent()
    }

    /// Apply production frame-failure publication semantics without hardware.
    pub fn inject_frame_failure(&self, error: &CaptureError) {
        if capture_frame_failure_invalidates_session(error) {
            clear_capture_publication(&self.publication);
        }
    }

    /// Analyze and publish one raw frame as if Desktop Duplication produced it.
    ///
    /// # Errors
    ///
    /// Returns an error while the source is inactive or when frame validation
    /// or analysis rejects the injected adapter output.
    pub fn publish(&self, frame: CaptureFrame<RawCaptureSurface>) -> anyhow::Result<bool> {
        let captured_at = frame.metadata().captured_at;
        let fresh_until = frame.metadata().fresh_until;
        let active = self
            .state
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| anyhow!("deterministic Windows capture source is inactive"))?;
        let snapshot = {
            let mut analyzer = self
                .state
                .analyzer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            analyze_capture_frame(&mut analyzer, &active, frame)?
        };
        let publication = {
            self.publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .publish(&active, snapshot)
        };
        let published = publication.is_ok();
        if published && let Some(status) = self.status_session.load() {
            status.record_sample(captured_at, fresh_until, 1)?;
        }
        Ok(published)
    }
}

struct CaptureWorker {
    authority: CaptureSessionAuthority,
    command_tx: mpsc::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    processed_activity_generation: Arc<AtomicU64>,
}

#[derive(Clone)]
struct WindowsExactCommandEndpoint {
    authority: CaptureSessionAuthority,
    command_tx: mpsc::Sender<WorkerCommand>,
}

impl CaptureExactCommandEndpoint for WindowsExactCommandEndpoint {
    const SOURCE_NAME: &'static str = "Windows capture";

    fn authority(&self) -> CaptureSessionAuthority {
        self.authority
    }

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected> {
        self.command_tx
            .send(WorkerCommand::Exact(command))
            .map_err(|_| CaptureExactCommandRejected)
    }
}

impl CaptureWorker {
    fn exact_command_endpoint(&self) -> WindowsExactCommandEndpoint {
        WindowsExactCommandEndpoint {
            authority: self.authority,
            command_tx: self.command_tx.clone(),
        }
    }
}

struct WindowsGpuRoute {
    id: GpuSurfaceDescriptorId,
    native: Arc<GpuSurfaceDescriptor>,
    descriptor: ResolvedScreenPublicationDescriptor,
    target: BoundScreenNativeTargetPreparation,
    capture_lifetime: ScreenResourceLifetime,
    pacer: CapturePacer,
    next_publish_at: Instant,
    retry_not_before: Option<Instant>,
    last_accepted_sequence: Option<u64>,
    publisher: Option<ScreenBranchPublisher>,
}

struct WindowsGpuRuntime {
    plan: PreparedGpuSurfacePlan,
    routes: Box<[WindowsGpuRoute]>,
}

struct PendingWindowsGpuRoute {
    id: GpuSurfaceDescriptorId,
    native: Arc<GpuSurfaceDescriptor>,
    descriptor: ResolvedScreenPublicationDescriptor,
    target: AdmittedScreenNativeTargetPreparation,
    capture_resource_name: Arc<str>,
    capture_allocation_byte_len: u64,
    requested_hz: NonZeroU32,
}

struct PendingWindowsGpuRuntime {
    plan: PreparedGpuSurfacePlan,
    routes: Vec<PendingWindowsGpuRoute>,
}

struct WindowsCpuRuntime {
    readback: Option<PreparedCpuDesktopReadback>,
    native_physical_mask: Box<[bool]>,
    reduction: Option<WindowsGpuReductionRuntime>,
    workspace_allocation_byte_len: u64,
    fanout_candidate: Option<PreparedCpuPublicationFanoutCandidate>,
    fanout: Option<PreparedCpuPublicationFanout>,
    latest_frame: Option<CaptureFrame<RawCaptureSurface>>,
}

struct WindowsGpuReductionRoute {
    id: GpuSurfaceDescriptorId,
    native: Arc<GpuSurfaceDescriptor>,
    physical_index: usize,
}

struct WindowsGpuReductionRuntime {
    plan: PreparedGpuReductionPlan,
    routes: Box<[WindowsGpuReductionRoute]>,
}

struct WindowsExactRuntime {
    source: WindowsPublicationSource,
    binding: ScreenWorkerBinding,
    gpu: Option<WindowsGpuRuntime>,
    cpu: Option<WindowsCpuRuntime>,
    _lifetimes: Box<[ScreenResourceLifetime]>,
}

type WindowsExactRuntimes = ExactBoxList<WindowsExactRuntime>;

impl CaptureExactRuntimeOwner for WindowsExactRuntime {
    type Source = WindowsPublicationSource;

    const BACKEND_NAME: &'static str = "Windows";
    const ABORTED_BINDING_ERROR: &'static str =
        "Windows exact runtime binding was aborted after commit";

    fn source(&self) -> &Self::Source {
        &self.source
    }

    fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }

    fn bind_routes(&mut self, authority: &ScreenCommittedState) -> anyhow::Result<bool> {
        let was_bound = self.is_bound();
        if let Some(gpu) = &mut self.gpu {
            for route in &mut gpu.routes {
                if route.publisher.is_none() {
                    route.publisher =
                        Some(authority.publisher_for_runtime(&route.descriptor, &self.binding)?);
                }
            }
        }
        if let Some(cpu) = &mut self.cpu
            && cpu.fanout.is_none()
        {
            let candidate = cpu
                .fanout_candidate
                .take()
                .ok_or_else(|| anyhow!("Windows CPU fanout candidate was already consumed"))?;
            cpu.fanout = Some(candidate.bind(authority, &self.binding)?);
        }
        Ok(!was_bound && self.is_bound())
    }

    fn is_bound(&self) -> bool {
        self.gpu
            .as_ref()
            .is_none_or(|gpu| gpu.routes.iter().all(|route| route.publisher.is_some()))
            && self.cpu.as_ref().is_none_or(|cpu| cpu.fanout.is_some())
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        let Some(join_handle) = self.join_handle.take() else {
            return;
        };
        self.cancel.store(true, Ordering::Release);
        let _ = self.command_tx.send(WorkerCommand::Stop);
        let _ = self.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
        if join_handle.is_finished() {
            let _ = join_handle.join();
            return;
        }
        retain_worker(join_handle, "Windows screen capture worker");
    }
}

enum WorkerCommand {
    SetActive {
        active: bool,
        activity_generation: u64,
    },
    AdoptSettings {
        prepared: PreparedWorkerSettings,
        ready: mpsc::SyncSender<()>,
        decision: mpsc::Receiver<SettingsDecision>,
        done: mpsc::SyncSender<()>,
    },
    Exact(CaptureExactCommand),
    Stop,
}

enum SettingsDecision {
    Commit,
}

impl WindowsScreenCaptureInput {
    /// Create a new Windows screen capture source.
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

    /// Create a Windows source inside an existing process-wide screen byte fence.
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
                values: VersionedCaptureSettings::new(
                    VersionedCaptureConfig {
                        value: config,
                        source_generation: 0,
                    },
                    ScreenCaptureDemand::Inactive,
                ),
                compute_capacity_policy,
                admission_coordinator,
                session_generation: AtomicU64::new(0),
                activity_generation: AtomicU64::new(0),
            }),
            running: false,
            capture_demand: ScreenCaptureDemand::Inactive,
            publication: Arc::new(Mutex::new(CapturePublication::default())),
            exact: Arc::new(ExactPublicationShared::default()),
            worker: None,
            status: SourceStatusReporter::new(
                "windows_screen_capture",
                SourceKind::Screen,
                "dxgi_desktop_duplication",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            source_sink: None,
            #[cfg(feature = "windows-capture-fixtures")]
            fixture: None,
        }
    }

    /// Create a source whose post-adapter frames are supplied deterministically.
    ///
    /// The returned source retains the production epoch validation, screen
    /// analysis, lifecycle status, and latest-value publication path. It never
    /// opens Desktop Duplication.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied epoch contains a zero generation.
    #[cfg(feature = "windows-capture-fixtures")]
    #[doc(hidden)]
    pub fn new_deterministic_fixture(
        config: CaptureConfig,
        epoch: CaptureEpoch,
    ) -> anyhow::Result<(Self, WindowsScreenCaptureFixture)> {
        if epoch.topology_generation == 0 {
            anyhow::bail!("deterministic Windows capture topology generation must be nonzero");
        }
        if epoch.session_generation == 0 {
            anyhow::bail!("deterministic Windows capture session generation must be nonzero");
        }
        let mut source = Self::new(config.clone());
        let admission_coordinator = source.settings.admission_coordinator.clone();
        let state = Arc::new(WindowsScreenCaptureFixtureState {
            analyzer: Mutex::new(ScreenCaptureInput::with_requested_extent_and_admission(
                config,
                PixelExtent::new(super::DEFAULT_CANVAS_WIDTH, super::DEFAULT_CANVAS_HEIGHT)
                    .expect("default canvas extent is non-empty"),
                admission_coordinator,
            )?),
            epoch,
            active: Mutex::new(None),
        });
        source.fixture = Some(Arc::clone(&state));
        let fixture = WindowsScreenCaptureFixture {
            state,
            publication: Arc::clone(&source.publication),
            status_session: source.status_session.clone(),
        };
        Ok((source, fixture))
    }

    /// Attach a callback that persists resolved legacy monitor selections.
    #[must_use]
    pub fn with_capture_source_sink(mut self, sink: CaptureSourceSink) -> Self {
        self.source_sink = Some(sink);
        self
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        self.observe_worker_exit(false);
        if self.worker.is_some() {
            anyhow::bail!("previous Windows screen capture worker is still stopping");
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let settings = Arc::clone(&self.settings);
        let publication = Arc::clone(&self.publication);
        let exact = Arc::clone(&self.exact);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_processed_activity_generation = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let processed_activity_generation = Arc::clone(&worker_processed_activity_generation);
        let status_session = self.status_session.clone();
        let source_sink = self.source_sink.clone();
        let session_generation = self
            .settings
            .session_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| anyhow!("Windows capture session generation exhausted"))?
            + 1;
        let authority = CaptureSessionAuthority::new(session_generation);
        self.exact.activate_authority(authority);

        let join_handle = spawn_worker(
            thread::Builder::new().name("hypercolor-screen-capture".to_owned()),
            move || {
                run_worker(
                    &settings,
                    &publication,
                    &exact,
                    &command_rx,
                    &worker_cancel,
                    &worker_processed_activity_generation,
                    status_session,
                    session_generation,
                    source_sink,
                    ready_tx,
                );
                let _ = exit_tx.send(());
            },
        )
        .map_err(|error| anyhow!("failed to spawn screen capture worker: {error}"))?;

        self.worker = Some(CaptureWorker {
            authority,
            command_tx,
            exit_rx,
            join_handle: Some(join_handle),
            cancel,
            #[cfg(test)]
            processed_activity_generation,
        });
        match ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.shutdown_worker();
                anyhow::bail!("Windows screen capture worker initialization failed: {error}");
            }
            Err(error) => {
                self.shutdown_worker();
                anyhow::bail!("Windows screen capture worker readiness timed out: {error}");
            }
        }
        if self.observe_worker_exit(true) {
            anyhow::bail!("Windows screen capture worker exited during startup");
        }
        Ok(())
    }

    fn shutdown_worker(&mut self) {
        let Some(worker) = self.worker.as_mut() else {
            return;
        };

        worker.cancel.store(true, Ordering::Release);
        let _ = worker.command_tx.send(WorkerCommand::Stop);
        let exit_observed = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT).is_ok();
        let Some(join_handle) = worker.join_handle.as_ref() else {
            self.worker = None;
            return;
        };
        if !exit_observed && !join_handle.is_finished() {
            warn!(
                "screen capture worker did not stop before the deadline; retaining its join handle"
            );
            return;
        }
        let mut worker = self.worker.take().expect("finished worker remains owned");
        if worker
            .join_handle
            .take()
            .expect("finished screen worker retains its join handle")
            .join()
            .is_err()
        {
            warn!("screen capture worker panicked during shutdown");
        }
    }

    fn observe_worker_exit(&mut self, publish_failure: bool) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        if !worker
            .join_handle
            .as_ref()
            .is_some_and(thread::JoinHandle::is_finished)
        {
            return false;
        }
        let mut worker = self.worker.take().expect("finished worker remains owned");
        let failure = worker
            .join_handle
            .take()
            .expect("finished screen worker retains its join handle")
            .join()
            .err();
        if publish_failure && let Some(status) = self.status.session() {
            let reason = failure.map_or_else(
                || "Windows screen capture worker exited unexpectedly".to_owned(),
                |panic| format!("Windows screen capture worker panicked: {panic:?}"),
            );
            status.failed(SourceIssue::new(
                "windows_screen_worker_exited",
                reason,
                true,
            ));
        }
        clear_capture_publication(&self.publication);
        self.exact.replace_source_if_current(worker.authority, None);
        true
    }

    fn send_activity_command(&self, active: bool, activity_generation: u64) -> bool {
        self.worker.as_ref().is_some_and(|worker| {
            worker
                .command_tx
                .send(WorkerCommand::SetActive {
                    active,
                    activity_generation,
                })
                .is_ok()
        })
    }

    fn activate_worker(&mut self, activity_generation: u64) -> anyhow::Result<()> {
        self.observe_worker_exit(false);
        if self.worker.is_none() {
            self.spawn_worker()?;
        }
        if self.send_activity_command(true, activity_generation) {
            return Ok(());
        }

        self.shutdown_worker();
        if self.worker.is_some() {
            anyhow::bail!("disconnected Windows screen capture worker could not be reaped");
        }
        self.spawn_worker()?;
        if self.send_activity_command(true, activity_generation) {
            return Ok(());
        }

        self.shutdown_worker();
        anyhow::bail!("replacement Windows screen capture worker rejected activation")
    }

    fn activate_backend(&mut self, activity_generation: u64) -> anyhow::Result<()> {
        #[cfg(feature = "windows-capture-fixtures")]
        if let Some(fixture) = self.fixture.as_ref() {
            let source_generation = self.settings.snapshot().source_generation;
            let active = ActiveCaptureEpoch {
                epoch: fixture.epoch.clone(),
                source_generation,
                activity_generation,
                duplication_generation: 1,
            };
            let activated = activate_capture_epoch(&self.publication, active.clone());
            if !activated {
                anyhow::bail!("deterministic Windows capture epoch was fenced before activation");
            }
            *fixture
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(active);
            return Ok(());
        }
        self.activate_worker(activity_generation)
    }

    fn deactivate_backend(&mut self, activity_generation: u64) {
        if let Some(worker) = self.worker.as_ref() {
            self.exact.replace_source_if_current(worker.authority, None);
        }
        #[cfg(feature = "windows-capture-fixtures")]
        if let Some(fixture) = self.fixture.as_ref() {
            *fixture
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            return;
        }
        if !self.send_activity_command(false, activity_generation) {
            self.shutdown_worker();
        }
    }

    fn prepare_active_settings(
        &self,
        config: CaptureConfig,
        source_generation: u64,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<PreparedWorkerSettings> {
        let requested_extent = demand
            .requested_extent()
            .expect("active Windows capture settings carry an extent");
        let cadence = CaptureCadence::new(config.target_fps)?;
        let source_is_known = source_generation == self.settings.snapshot().source_generation
            && self.exact.source().is_some();
        if source_is_known && let Some(capacity) = self.settings.compute_capacity_policy.analysis()
        {
            ScreenAnalysisWorkPlan::try_new(requested_extent, requested_extent, &config)?
                .admit(capacity)?;
        }
        let mut analyzer = build_analyzer_for_extent(
            config.clone(),
            requested_extent,
            self.settings.admission_coordinator.clone(),
            self.settings.compute_capacity_policy,
        )?;
        if source_is_known {
            analyzer.admit_frame_extent(requested_extent)?;
        }
        Ok(PreparedWorkerSettings {
            config,
            cadence,
            source_generation,
            demand,
            analyzer,
        })
    }

    fn adopt_worker_settings(&self, prepared: PreparedWorkerSettings) -> anyhow::Result<()> {
        let worker = self
            .worker
            .as_ref()
            .ok_or_else(|| anyhow!("Windows capture worker is unavailable for live adoption"))?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (decision_tx, decision_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        worker
            .command_tx
            .send(WorkerCommand::AdoptSettings {
                prepared,
                ready: ready_tx,
                decision: decision_rx,
                done: done_tx,
            })
            .map_err(|_| anyhow!("Windows capture worker rejected prepared settings"))?;
        ready_rx
            .recv_timeout(WORKER_READY_TIMEOUT)
            .map_err(|error| anyhow!("Windows capture worker adoption timed out: {error}"))?;
        decision_tx
            .send(SettingsDecision::Commit)
            .map_err(|_| anyhow!("Windows capture worker exited before settings commit"))?;
        done_rx
            .recv()
            .map_err(|_| anyhow!("Windows capture worker exited during settings commit"))
    }

    #[cfg(feature = "windows-capture-fixtures")]
    fn adopt_fixture_settings(&self, prepared: PreparedWorkerSettings) {
        let snapshot = self.settings.snapshot();
        self.settings.commit_values(
            &prepared.config,
            prepared.source_generation,
            prepared.demand,
        );
        *self
            .fixture
            .as_ref()
            .expect("fixture adoption requires a deterministic source")
            .analyzer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = prepared.analyzer;
        if snapshot.source_generation != prepared.source_generation {
            let displaced = {
                self.publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fence_source(prepared.source_generation)
            };
            drop(displaced);
        }
    }

    fn set_capture_demand_state(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        if self.capture_demand == demand {
            return Ok(());
        }
        let previous = self.capture_demand;
        if previous.is_active() && demand.is_active() && self.running {
            let snapshot = self.settings.snapshot();
            let prepared =
                self.prepare_active_settings(snapshot.config, snapshot.source_generation, demand)?;
            #[cfg(feature = "windows-capture-fixtures")]
            if self.fixture.is_some() {
                self.adopt_fixture_settings(prepared);
                self.capture_demand = demand;
                return Ok(());
            }
            self.adopt_worker_settings(prepared)?;
            self.capture_demand = demand;
            return Ok(());
        }
        let admission = demand
            .requested_extent()
            .map(|requested_extent| -> anyhow::Result<_> {
                let config = self.settings.snapshot().config;
                let cadence = CaptureCadence::new(config.target_fps)?;
                let analyzer = build_analyzer_for_extent(
                    config,
                    requested_extent,
                    self.settings.admission_coordinator.clone(),
                    self.settings.compute_capacity_policy,
                )?;
                Ok((analyzer, cadence))
            })
            .transpose()?;
        #[cfg(feature = "windows-capture-fixtures")]
        let prepared_fixture_analyzer = if self.fixture.is_some() {
            admission.map(|(analyzer, _)| analyzer)
        } else {
            None
        };
        #[cfg(not(feature = "windows-capture-fixtures"))]
        drop(admission);
        let previous_publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .checkpoint();
        let activity_changed = previous.is_active() != demand.is_active();
        let activity_generation = if activity_changed {
            let generation = self
                .settings
                .activity_generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            let displaced = {
                self.publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fence_activity(generation)
            };
            drop(displaced);
            generation
        } else {
            self.settings.activity_generation.load(Ordering::Acquire)
        };
        *self.settings.values.lock_demand() = demand;
        self.settings.values.bump_revision();

        if !self.running {
            self.capture_demand = demand;
            return Ok(());
        }

        let transition = if !activity_changed {
            Ok(())
        } else if demand.is_active() {
            self.activate_backend(activity_generation)
        } else {
            self.deactivate_backend(activity_generation);
            Ok(())
        };
        if let Err(error) = transition {
            *self.settings.values.lock_demand() = previous;
            self.settings.values.bump_revision();
            if previous.is_active() {
                let rollback_generation = self
                    .settings
                    .activity_generation
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                let displaced = {
                    self.publication
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .fence_activity(rollback_generation)
                };
                drop(displaced);
                if let Err(rollback_error) = self.activate_backend(rollback_generation) {
                    return Err(error.context(format!(
                        "failed to restore previous Windows capture demand: {rollback_error}"
                    )));
                }
                let displaced = {
                    let mut publication = self
                        .publication
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let active = publication
                        .active()
                        .cloned()
                        .expect("reactivated Windows capture installs an active epoch");
                    let Ok(displaced) =
                        publication.restore_checkpoint(Some(&active), previous_publication)
                    else {
                        unreachable!("reactivated Windows capture retains its publication epoch");
                    };
                    displaced
                };
                drop(displaced);
            } else {
                let (cleared, displaced) = {
                    let mut publication = self
                        .publication
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let cleared = publication.clear();
                    let Ok(displaced) = publication.restore_checkpoint(None, previous_publication)
                    else {
                        unreachable!("inactive Windows capture has no active publication epoch");
                    };
                    (cleared, displaced)
                };
                drop((cleared, displaced));
            }
            return Err(error);
        }

        #[cfg(feature = "windows-capture-fixtures")]
        if let Some(analyzer) = prepared_fixture_analyzer
            && let Some(fixture) = self.fixture.as_ref()
        {
            *fixture
                .analyzer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = analyzer;
        }
        self.capture_demand = demand;
        Ok(())
    }

    /// Publish new settings and bump the generation the worker polls.
    fn reconfigure(&mut self, config: CaptureConfig) -> anyhow::Result<()> {
        CaptureCadence::new(config.target_fps)?;
        let snapshot = self.settings.snapshot();
        if snapshot.config == config {
            return Ok(());
        }
        let source_generation = if snapshot.config.source == config.source {
            snapshot.source_generation
        } else {
            snapshot.source_generation.wrapping_add(1).max(1)
        };
        if self.capture_demand.is_active() {
            let prepared =
                self.prepare_active_settings(config, source_generation, self.capture_demand)?;
            #[cfg(feature = "windows-capture-fixtures")]
            if self.fixture.is_some() {
                self.adopt_fixture_settings(prepared);
                return Ok(());
            }
            if self.running {
                self.adopt_worker_settings(prepared)?;
            } else {
                self.settings.commit(&prepared);
            }
            if snapshot.source_generation != source_generation {
                if let Some(worker) = self.worker.as_ref() {
                    self.exact.replace_source_if_current(worker.authority, None);
                }
            }
            return Ok(());
        }
        self.settings
            .commit_values(&config, source_generation, self.capture_demand);
        Ok(())
    }
}

impl InputSource for WindowsScreenCaptureInput {
    fn name(&self) -> &'static str {
        "windows_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        if self.capture_demand.is_active() {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            let activity_generation = self.settings.activity_generation.load(Ordering::Acquire);
            if let Err(error) = self.activate_backend(activity_generation) {
                self.status_session.clear();
                self.status.stop();
                self.shutdown_worker();
                return Err(error);
            }
        } else {
            debug!(
                "Windows screen capture armed but idle until a screen-reactive effect requests capture"
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
        *self.settings.values.lock_demand() = ScreenCaptureDemand::Inactive;
        self.settings.values.bump_revision();
        self.shutdown_worker();

        #[cfg(feature = "windows-capture-fixtures")]
        if let Some(fixture) = self.fixture.as_ref() {
            *fixture
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }

        clear_capture_publication(&self.publication);
        if let Some(worker) = self.worker.as_ref() {
            self.exact.replace_source_if_current(worker.authority, None);
        }
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.observe_worker_exit(self.running && self.capture_demand.is_active());
        if !self.running || !self.capture_demand.is_active() {
            return Ok(InputData::None);
        }

        let publication = self
            .publication
            .lock()
            .map_err(|_| anyhow!("windows screen capture publication mutex poisoned"))?;
        let Some(snapshot) = publication.snapshot() else {
            return Ok(InputData::None);
        };
        if snapshot
            .value
            .geometry_frame()
            .validate_epoch(&snapshot.epoch.epoch)
            .is_err()
        {
            return Ok(InputData::None);
        }
        Ok(InputData::Screen(snapshot.value.data().clone()))
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

impl ScreenSource for WindowsScreenCaptureInput {
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
        let config = self.settings.snapshot().config;
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
        let config = self.settings.snapshot().config;
        Ok(Some(ScreenAnalysisWorkPlan::try_new(
            requested_extent,
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
        let was_active = previous.is_active();
        self.status.set_policy(true, true, active)?;
        if was_active != active {
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
            self.status.set_policy(true, true, was_active)?;
            if was_active
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
        self.exact.install_hub(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.exact.resolution_revision()
    }

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        let Some(source) = self.exact.source() else {
            return Ok(None);
        };
        resolve_windows_publication_branch(&source, demand)
    }

    fn owns_screen_publication_source(&self, source_id: &CaptureSourceId) -> bool {
        self.exact.owns_source(source_id)
    }

    fn begin_screen_publication_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        let worker = self.worker.as_ref().ok_or_else(|| {
            anyhow!("Windows capture worker is unavailable for exact publication preparation")
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
}

impl SourceRoleBinding for WindowsScreenCaptureInput {
    type Role = ScreenSourceRole;
}

fn resolve_windows_publication_branch(
    source: &WindowsPublicationSource,
    demand: &RegisteredScreenBranchDemand,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let selector = demand.request().selector();
    if !source.matches_selector(selector) {
        return Ok(None);
    }
    let selector = selector.clone();
    if matches!(
        demand.request().executor(),
        ScreenPublicationExecutorRequest::Cpu
    ) {
        return Ok(Some(demand.resolve_with_color_capabilities(
            &source.cpu_source(selector)?,
            windows_cpu_color_capabilities(),
        )?));
    }

    let gpu_source = source.gpu_source(selector.clone())?;
    let gpu_resolution = demand.resolve_with_executor_capabilities(
        &gpu_source,
        ScreenExecutorColorCapabilities::new(
            windows_cpu_color_capabilities(),
            ScreenColorTransformCapabilities::NONE,
        ),
    );
    if let Ok(resolved) = gpu_resolution
        && matches!(
            resolved.descriptor().executor(),
            ScreenPublicationExecutor::SourceNative(_)
        )
        && capture_gpu_descriptor(
            resolved.descriptor(),
            source,
            GpuSurfaceDescriptorId::new(NonZeroU64::MIN),
            capture_freshness(demand.requested_hz()),
        )
        .is_ok()
    {
        return Ok(Some(resolved));
    }

    Ok(Some(demand.resolve_with_color_capabilities(
        &source.cpu_source(selector)?,
        windows_cpu_color_capabilities(),
    )?))
}

const fn windows_cpu_color_capabilities() -> ScreenColorTransformCapabilities {
    ScreenColorTransformCapabilities::new(true, false, false, NonZeroU32::MIN)
}

fn capture_gpu_descriptor(
    descriptor: &ResolvedScreenPublicationDescriptor,
    source: &WindowsPublicationSource,
    id: GpuSurfaceDescriptorId,
    freshness: Duration,
) -> anyhow::Result<GpuSurfaceDescriptor> {
    if !matches!(descriptor.kind(), ScreenPublicationKind::Surface) {
        anyhow::bail!("Windows native capture only publishes Surface branches");
    }
    if !matches!(
        descriptor.executor(),
        ScreenPublicationExecutor::SourceNative(_)
    ) {
        anyhow::bail!("Windows native descriptor requires source-native execution");
    }
    let physical = descriptor.physical();
    let source_region = physical.source_region();
    let region = CaptureRegion::new(
        capture_integer_coordinate(source_region.x())?,
        capture_integer_coordinate(source_region.y())?,
        capture_integer_coordinate(source_region.width())?,
        capture_integer_coordinate(source_region.height())?,
    )
    .ok_or_else(|| anyhow!("Windows native source region must be non-empty"))?;
    let filter = match physical.reduction_filter() {
        ScreenReductionFilter::Nearest => GpuSurfaceFilter::Nearest,
        other => anyhow::bail!("Windows native capture does not implement {other:?} filtering"),
    };
    if physical.target_pixel_format() != CapturePixelFormat::Rgba8 {
        anyhow::bail!("Windows native capture requires RGBA8 output");
    }
    if physical.color_pipeline().transform() != ResolvedScreenColorTransform::PreserveEncodedSamples
    {
        anyhow::bail!("Windows native capture requires encoded-sample preservation");
    }
    let cursor = match physical.cursor() {
        ScreenCursorPolicy::Exclude => GpuSurfaceCursorPolicy::Exclude,
        ScreenCursorPolicy::Include => GpuSurfaceCursorPolicy::Include,
    };
    let output = physical.reduction_extent();
    let descriptor = GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
        id,
        source_region: region,
        coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
        source_rotation: display_rotation(source.rotation)?,
        source_color_space: source.source_color_space,
        output_extent: NativeCaptureExtent::try_new(output.width(), output.height())?,
        filter,
        format: GpuSurfaceFormat::Rgba8Unorm,
        color_pipeline: GpuSurfaceColorPipeline::PreserveEncoded,
        cursor,
        algorithm_revision: physical.algorithm_revision(),
        freshness,
    });
    descriptor.validate_exact_gpu()?;
    Ok(descriptor)
}

fn capture_gpu_reduction_descriptor(
    physical: &ScreenPhysicalReductionDescriptor,
    source: &WindowsPublicationSource,
    id: GpuSurfaceDescriptorId,
    freshness: Duration,
) -> anyhow::Result<GpuSurfaceDescriptor> {
    if !matches!(physical.executor(), ScreenPublicationExecutor::Cpu) {
        anyhow::bail!("Windows reduced readback requires a CPU physical descriptor");
    }
    let config = physical.source();
    let geometry = config.geometry();
    let scale = geometry.source_scale();
    if geometry.native_extent() != source.native_extent
        || geometry.storage_extent() != source.native_extent
        || geometry.rotation() != source.rotation
        || geometry.crop().is_some()
        || scale.numerator() != scale.denominator()
        || config.logical_extent() != source.logical_extent
        || config.reflection() != ScreenSourceReflection::None
        || config.pixel_format() != CapturePixelFormat::Bgra8
    {
        anyhow::bail!("Windows GPU readback does not implement this source transform");
    }
    let source_region = physical.source_region();
    let region = CaptureRegion::new(
        capture_integer_coordinate(source_region.x())?,
        capture_integer_coordinate(source_region.y())?,
        capture_integer_coordinate(source_region.width())?,
        capture_integer_coordinate(source_region.height())?,
    )
    .ok_or_else(|| anyhow!("Windows GPU readback source region must be non-empty"))?;
    let filter = match physical.reduction_filter() {
        ScreenReductionFilter::Nearest => GpuSurfaceFilter::Nearest,
        ScreenReductionFilter::Bilinear => GpuSurfaceFilter::Bilinear,
        ScreenReductionFilter::Area => GpuSurfaceFilter::Area,
    };
    if physical.target_pixel_format() != CapturePixelFormat::Rgba8 {
        anyhow::bail!("Windows GPU readback requires RGBA8 output");
    }
    let color_pipeline = match physical.color_pipeline().transform() {
        ResolvedScreenColorTransform::PreserveEncodedSamples => {
            GpuSurfaceColorPipeline::PreserveEncoded
        }
        ResolvedScreenColorTransform::LinearLightSdr => GpuSurfaceColorPipeline::LinearSdr,
        transform => anyhow::bail!("Windows GPU readback does not implement {transform:?}"),
    };
    let cursor = match physical.cursor() {
        ScreenCursorPolicy::Exclude => GpuSurfaceCursorPolicy::Exclude,
        ScreenCursorPolicy::Include => GpuSurfaceCursorPolicy::Include,
    };
    let output = physical.reduction_extent();
    let descriptor = GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
        id,
        source_region: region,
        coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
        source_rotation: display_rotation(source.rotation)?,
        source_color_space: source.source_color_space,
        output_extent: NativeCaptureExtent::try_new(output.width(), output.height())?,
        filter,
        format: GpuSurfaceFormat::Rgba8Unorm,
        color_pipeline,
        cursor,
        algorithm_revision: physical.algorithm_revision(),
        freshness,
    });
    descriptor.validate_exact_gpu_readback()?;
    Ok(descriptor)
}

fn capture_integer_coordinate(value: super::ScreenRational) -> anyhow::Result<u32> {
    if value.denominator().get() != 1 {
        anyhow::bail!("Windows native capture requires integer source coordinates");
    }
    u32::try_from(value.numerator())
        .map_err(|_| anyhow!("Windows native source coordinate exceeds u32"))
}

fn capture_freshness(requested_hz: NonZeroU32) -> Duration {
    Duration::from_nanos(2_000_000_000_u64.div_ceil(u64::from(requested_hz.get())))
}

fn physical_capture_freshness(
    plan: &crate::input::screen::ScreenCapturePlan,
    descriptor: &ScreenPhysicalReductionDescriptor,
) -> anyhow::Result<Duration> {
    let physical = plan
        .physical_reductions()
        .binary_search_by(|candidate| candidate.descriptor().cmp(descriptor))
        .ok()
        .and_then(|index| plan.physical_reductions().get(index))
        .ok_or_else(|| anyhow!("Windows physical reduction is absent from its candidate plan"))?;
    physical
        .branch_indices()
        .iter()
        .filter_map(|index| plan.branches().get(*index))
        .map(|branch| capture_freshness(branch.requested_hz()))
        .min()
        .ok_or_else(|| anyhow!("Windows physical reduction has no logical consumers"))
}

fn windows_physical_reduction_executes_on_cpu(
    plan: &crate::input::screen::ScreenCapturePlan,
    descriptor: &ScreenPhysicalReductionDescriptor,
    source: &WindowsPublicationSource,
) -> bool {
    let Ok(freshness) = physical_capture_freshness(plan, descriptor) else {
        return true;
    };
    matches!(
        classify_windows_physical_reduction(descriptor, source, freshness),
        WindowsPhysicalReductionRoute::Cpu
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsPhysicalReductionRoute {
    Cpu,
    GuaranteedGpuPreReduced,
}

fn classify_windows_physical_reduction(
    descriptor: &ScreenPhysicalReductionDescriptor,
    source: &WindowsPublicationSource,
    freshness: Duration,
) -> WindowsPhysicalReductionRoute {
    let probe_id = GpuSurfaceDescriptorId::new(NonZeroU64::MIN);
    if capture_gpu_reduction_descriptor(descriptor, source, probe_id, freshness).is_ok() {
        WindowsPhysicalReductionRoute::GuaranteedGpuPreReduced
    } else {
        WindowsPhysicalReductionRoute::Cpu
    }
}

const fn screen_gpu_identity(adapter: GpuAdapterLuid) -> ScreenPhysicalGpuDeviceIdentity {
    ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
        low_part: adapter.low_part(),
        high_part: adapter.high_part(),
    }
}

fn capture_colorimetry(source: GpuSurfaceSourceColorSpace) -> anyhow::Result<CaptureColorimetry> {
    match source {
        GpuSurfaceSourceColorSpace::RgbFullG22P709 => Ok(CaptureColorimetry::SRGB),
        GpuSurfaceSourceColorSpace::RgbFullLinearP709 => Ok(CaptureColorimetry::new(
            CaptureColorSpace::Srgb,
            CaptureTransferFunction::Linear,
            Some(CaptureDynamicRange::Standard),
            None,
        )?),
        GpuSurfaceSourceColorSpace::RgbFullPqP2020 => Ok(CaptureColorimetry::new(
            CaptureColorSpace::Rec2020,
            CaptureTransferFunction::Pq,
            Some(CaptureDynamicRange::High),
            None,
        )?),
        GpuSurfaceSourceColorSpace::Unknown => Ok(CaptureColorimetry::unknown()),
    }
}

fn windows_gpu_preparation_gate(adapter_luid: GpuAdapterLuid) -> Arc<Mutex<()>> {
    Arc::clone(
        WINDOWS_GPU_PREPARATION_GATES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(adapter_luid)
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

fn display_rotation(rotation: CaptureRotation) -> anyhow::Result<DisplayRotation> {
    Ok(match rotation {
        CaptureRotation::Identity => DisplayRotation::Identity,
        CaptureRotation::Clockwise90 => DisplayRotation::Clockwise90,
        CaptureRotation::Clockwise180 => DisplayRotation::Clockwise180,
        CaptureRotation::Clockwise270 => DisplayRotation::Clockwise270,
        CaptureRotation::Flipped
        | CaptureRotation::Flipped90
        | CaptureRotation::Flipped180
        | CaptureRotation::Flipped270 => {
            anyhow::bail!("reflected capture transforms are not DXGI display rotations")
        }
    })
}

impl Drop for WindowsScreenCaptureInput {
    fn drop(&mut self) {
        self.shutdown_worker();
    }
}

fn active_capture_epoch(
    session: &DesktopDuplicator,
    session_generation: u64,
    source_generation: u64,
    activity_generation: u64,
) -> anyhow::Result<ActiveCaptureEpoch> {
    Ok(ActiveCaptureEpoch {
        epoch: CaptureEpoch {
            source_id: capture_source_id(session.source_id())?,
            topology_generation: session.topology_generation(),
            session_generation,
        },
        source_generation,
        activity_generation,
        duplication_generation: session.duplication_generation(),
    })
}

fn activate_capture_epoch(
    publication: &Mutex<CapturePublication<AnalyzedScreenSnapshot>>,
    active: ActiveCaptureEpoch,
) -> bool {
    let activation = {
        publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .activate(active)
    };
    activation.is_ok()
}

fn clear_capture_publication(publication: &Mutex<CapturePublication<AnalyzedScreenSnapshot>>) {
    let displaced = {
        publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear()
    };
    drop(displaced);
}

fn settle_inactive_capture<T>(
    resource: &mut Option<T>,
    processed_activity_generation: &AtomicU64,
    activity_generation: u64,
) {
    *resource = None;
    processed_activity_generation.store(activity_generation, Ordering::Release);
}

fn build_worker_analyzer(
    config: &CaptureConfig,
    demand: ScreenCaptureDemand,
    admission_coordinator: ScreenByteAdmissionCoordinator,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
) -> Result<ScreenCaptureInput, ScreenAnalysisAdmissionError> {
    let requested_extent = demand
        .requested_extent()
        .expect("an active Windows capture worker carries an extent");
    build_analyzer_for_extent(
        config.clone(),
        requested_extent,
        admission_coordinator,
        compute_capacity_policy,
    )
}

fn build_analyzer_for_extent(
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

fn checked_retained_metadata_bytes<T>(count: usize, resource: &str) -> anyhow::Result<u64> {
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(std::mem::size_of::<T>())
                .ok()
                .and_then(|size| count.checked_mul(size))
        })
        .ok_or_else(|| anyhow!("Windows exact {resource} metadata accounting overflow"))
}

fn checked_retained_arc_bytes<T>(count: usize, resource: &str) -> anyhow::Result<u64> {
    let (layout, _) = Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .map_err(|_| anyhow!("Windows exact {resource} Arc accounting overflow"))?;
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(layout.pad_to_align().size())
                .ok()
                .and_then(|size| count.checked_mul(size))
        })
        .ok_or_else(|| anyhow!("Windows exact {resource} Arc accounting overflow"))
}

fn preflight_required_scope_bytes(
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

fn prepare_windows_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    duplicator: Option<&DesktopDuplicator>,
    source: Option<&WindowsPublicationSource>,
    exact: &ExactPublicationShared,
    hub: &ScreenPublicationHub,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
) -> anyhow::Result<(
    ScreenPreparedWorkerToken,
    Option<(WindowsExactRuntime, WindowsOwnedSource)>,
)> {
    let candidate = ticket.candidate_plan().clone();
    let source_branches = candidate
        .branches()
        .iter()
        .filter(|branch| branch.descriptor().source_epoch().source_id == *ticket.source_id())
        .collect::<Vec<_>>();
    if source_branches.is_empty() {
        let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
        let reports = ledger
            .ticket()
            .required_minimums()
            .iter()
            .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
            .collect::<Vec<_>>();
        for (name, bytes) in reports {
            ledger.report(&name, bytes)?;
        }
        let (token, _) = ledger.finish()?.into_parts();
        return Ok((token, None));
    }

    let source = source
        .filter(|source| &source.epoch.source_id == ticket.source_id())
        .ok_or_else(|| anyhow!("Windows exact publication source changed before preparation"))?;
    let duplicator = duplicator
        .ok_or_else(|| anyhow!("Windows duplication session is unavailable for preparation"))?;
    let slot_count = NonZeroU32::new(hub.committed_state().slot_policy().total_slots())
        .ok_or_else(|| anyhow!("Windows exact publication slot count must be nonzero"))?;
    let ticket_source_id = ticket.source_id().clone();
    let ticket_plan_generation = ticket.plan_generation();
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
    let (mut cpu_api_minimum_remaining, mut processing_minimum_remaining) = {
        let required_minimum = |resource, native| {
            ledger
                .ticket()
                .required_minimums()
                .iter()
                .find(|minimum| {
                    minimum.resource() == resource
                        && matches!(
                            minimum.descriptor().executor(),
                            ScreenPublicationExecutor::SourceNative(_)
                        ) == native
                })
                .map_or(0, ScreenRequiredResourceMinimum::minimum_bytes)
        };
        (
            required_minimum(ScreenResourceKind::ApiAllocation, false),
            required_minimum(ScreenResourceKind::ProcessingProfileState, false),
        )
    };
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
                .ok_or_else(|| anyhow!("Windows exact physical-plane accounting overflow"))
        })?;
    let runtime_node_bytes =
        checked_retained_metadata_bytes::<ExactBoxNode<WindowsExactRuntime>>(1, "runtime node")?
            .checked_add(checked_retained_metadata_bytes::<
                ExactBoxNode<WindowsOwnedSource>,
            >(1, "owned source node")?)
            .ok_or_else(|| anyhow!("Windows exact runtime node accounting overflow"))?;
    preflight_required_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        runtime_node_bytes,
    )?;

    let cpu_source = source_branches
        .iter()
        .copied()
        .find(|branch| {
            matches!(
                branch.descriptor().executor(),
                ScreenPublicationExecutor::Cpu
            )
        })
        .map(|cpu_branch| -> anyhow::Result<_> {
            let resolved_source = ResolvedScreenSource::new(
                ScreenSourceSelector::Exact(source.epoch.source_id.clone()),
                source.epoch.clone(),
                cpu_branch.descriptor().source().clone(),
            );
            let worker_count = exact.cpu_worker_count();
            let compute_plan = CpuExactReductionWorkPlan::try_for_source(
                &candidate,
                &ticket_source_id,
                |descriptor| {
                    windows_physical_reduction_executes_on_cpu(&candidate, descriptor, source)
                },
            )?;
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
                "planned Windows exact CPU reduction compute"
            );
            Ok(resolved_source)
        })
        .transpose()?;

    let gpu_branches = source_branches
        .iter()
        .copied()
        .filter(|branch| {
            matches!(
                branch.descriptor().executor(),
                ScreenPublicationExecutor::SourceNative(_)
            )
        })
        .collect::<Vec<_>>();
    preflight_required_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        checked_retained_metadata_bytes::<WindowsGpuRoute>(gpu_branches.len(), "GPU route")?
            .checked_add(checked_retained_arc_bytes::<GpuSurfaceDescriptor>(
                gpu_branches.len(),
                "GPU descriptor",
            )?)
            .ok_or_else(|| anyhow!("Windows GPU route metadata accounting overflow"))?,
    )?;
    let gpu = if gpu_branches.is_empty() {
        None
    } else {
        let preparation_gate = windows_gpu_preparation_gate(source.adapter_luid);
        let _preparation_guard = preparation_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let plan_generation = NonZeroU64::new(ticket_plan_generation.get())
            .map(GpuSurfacePlanGeneration::new)
            .ok_or_else(|| {
                anyhow!("Windows native publication requires a nonzero plan generation")
            })?;
        let mut descriptors = Vec::new();
        let mut pending_routes = Vec::new();
        descriptors.try_reserve_exact(gpu_branches.len())?;
        pending_routes.try_reserve_exact(gpu_branches.len())?;
        for branch in &gpu_branches {
            let id = exact.next_gpu_descriptor_id()?;
            let native = Arc::new(capture_gpu_descriptor(
                branch.descriptor(),
                source,
                id,
                capture_freshness(branch.requested_hz()),
            )?);
            descriptors.push(native.as_ref().clone());
            pending_routes.push((id, native, *branch));
        }
        let available_gpu_bytes = duplicator.available_gpu_memory_bytes()?;
        let admission = windows_gpu_candidate_admission(
            native_capture_extent(source.logical_extent),
            &descriptors,
            slot_count,
            available_gpu_bytes,
        )?;
        let capture_quote =
            admission.quote(native_capture_extent(source.logical_extent), &descriptors)?;
        ledger.preflight_additional_bytes(capture_quote.retained_byte_len())?;
        let plan = duplicator.prepare_gpu_surface_plan(plan_generation, &descriptors, admission)?;
        let mut routes = Vec::new();
        routes.try_reserve_exact(pending_routes.len())?;
        for (id, native, branch) in pending_routes {
            let ScreenPublicationExecutor::SourceNative(target) = branch.descriptor().executor()
            else {
                anyhow::bail!("Windows GPU route lost source-native target identity");
            };
            let manifest_quote = GpuSurfaceTargetPreparationResourceQuote::try_new(slot_count)?;
            let manifest_metadata_bytes =
                checked_retained_arc_bytes::<GpuSurfaceTargetPreparation>(
                    1,
                    "GPU target manifest",
                )?
                .checked_add(checked_retained_arc_bytes::<
                    ResolvedScreenPublicationDescriptor,
                >(1, "GPU target manifest descriptor")?)
                .ok_or_else(|| anyhow!("Windows GPU target manifest accounting overflow"))?;
            ledger.preflight_additional_bytes(
                manifest_quote
                    .retained_byte_len()
                    .checked_add(manifest_metadata_bytes)
                    .ok_or_else(|| anyhow!("Windows GPU target manifest accounting overflow"))?,
            )?;
            let manifest = Arc::new(plan.target_preparation(id)?);
            let capture_allocation_byte_len = manifest.allocation_byte_len();
            let platform = ScreenNativePreparationPayload::new(
                branch.descriptor(),
                ticket_plan_generation,
                manifest,
            );
            let target = ledger.prepare_native_target(
                target,
                branch.descriptor(),
                &platform,
                format!("native-target-{}", id.get()),
                "worker-runtime-total",
            )?;
            routes.push(PendingWindowsGpuRoute {
                id,
                native,
                descriptor: branch.descriptor().clone(),
                target,
                capture_resource_name: Arc::from(format!("windows-gpu-route-{}-slots", id.get())),
                capture_allocation_byte_len,
                requested_hz: branch.requested_hz(),
            });
        }
        Some(PendingWindowsGpuRuntime { plan, routes })
    };

    let cpu = if let Some(resolved_source) = cpu_source {
        let executor = exact.cpu_executor()?;
        let batch_quote = executor.batch_allocation_quote(&resolved_source, &candidate)?;
        preflight_required_scope_bytes(
            &mut ledger,
            &mut processing_minimum_remaining,
            batch_quote,
        )?;
        let batch = executor.prepare_batch(&resolved_source, &candidate)?;
        let workspace_quote = batch.materialization_workspace_allocation_quote(&candidate)?;
        let workspace_additional_bytes = workspace_quote
            .checked_sub(plane_minimum_bytes)
            .ok_or_else(|| anyhow!("Windows workspace quote understates physical-plane minima"))?;
        preflight_required_scope_bytes(
            &mut ledger,
            &mut worker_minimum_remaining,
            workspace_additional_bytes,
        )?;
        let workspace = batch.prepare_materialization_workspace(&candidate)?;
        let workspace_allocation_byte_len = workspace.allocation_byte_len();
        let fanout_quote = PreparedCpuPublicationFanout::candidate_allocation_quote(
            &batch, &workspace, &candidate,
        )?;
        let fanout_additional_bytes = fanout_quote
            .checked_sub(batch_quote)
            .ok_or_else(|| anyhow!("Windows fanout quote understates retained batch backing"))?;
        preflight_required_scope_bytes(
            &mut ledger,
            &mut processing_minimum_remaining,
            fanout_additional_bytes,
        )?;
        let fanout_candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
            &executor, &batch, workspace, &candidate,
        )?;
        let mut classifications = Vec::new();
        classifications.try_reserve_exact(batch.len())?;
        for physical_index in 0..batch.len() {
            let physical = batch
                .descriptor(physical_index)
                .expect("prepared CPU batch index is valid");
            let id = exact.next_gpu_descriptor_id()?;
            let freshness = physical_capture_freshness(&candidate, physical)?;
            match capture_gpu_reduction_descriptor(physical, source, id, freshness) {
                Ok(descriptor) => {
                    classifications.push(Some((id, descriptor, physical_index)));
                }
                Err(error) => {
                    debug!(
                        physical_index,
                        reason = %error,
                        "routing unsupported Windows physical reduction through native readback"
                    );
                    classifications.push(None);
                }
            }
        }
        let reduction_count = classifications.iter().flatten().count();
        let mask_metadata_bytes =
            checked_retained_metadata_bytes::<bool>(classifications.len(), "CPU route mask")?;
        let reduction_route_metadata_bytes = checked_retained_metadata_bytes::<
            WindowsGpuReductionRoute,
        >(reduction_count, "GPU reduction route")?
        .checked_add(checked_retained_arc_bytes::<GpuSurfaceDescriptor>(
            reduction_count,
            "GPU reduction descriptor",
        )?)
        .ok_or_else(|| anyhow!("Windows GPU reduction route metadata accounting overflow"))?;
        preflight_required_scope_bytes(
            &mut ledger,
            &mut worker_minimum_remaining,
            mask_metadata_bytes
                .checked_add(reduction_route_metadata_bytes)
                .ok_or_else(|| anyhow!("Windows CPU route metadata accounting overflow"))?,
        )?;
        let native_physical_mask = classifications
            .iter()
            .map(Option::is_none)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let mut reduction_descriptors = Vec::new();
        let mut reduction_routes = Vec::new();
        reduction_descriptors.try_reserve_exact(reduction_count)?;
        reduction_routes.try_reserve_exact(reduction_count)?;
        for (id, descriptor, physical_index) in classifications.into_iter().flatten() {
            let native = Arc::new(descriptor.clone());
            reduction_descriptors.push(descriptor);
            reduction_routes.push(WindowsGpuReductionRoute {
                id,
                native,
                physical_index,
            });
        }
        let reduction_preparation = if reduction_descriptors.is_empty() {
            None
        } else {
            let generation = GpuSurfacePlanGeneration::new(
                NonZeroU64::new(ticket_plan_generation.get())
                    .ok_or_else(|| anyhow!("Windows reduction plan generation must be nonzero"))?,
            );
            let available_gpu_bytes = duplicator.available_gpu_memory_bytes()?;
            let admission = GpuReductionAdmission::new(available_gpu_bytes, slot_count);
            let quote = admission.quote(
                native_capture_extent(source.logical_extent),
                &reduction_descriptors,
            )?;
            Some((generation, admission, quote))
        };
        let readback_quote = native_physical_mask
            .iter()
            .any(|native| *native)
            .then(|| {
                CpuDesktopReadbackResourceQuote::try_new(
                    native_capture_extent(source.native_extent),
                    slot_count,
                )
            })
            .transpose()?;
        let cpu_api_quote = reduction_preparation
            .as_ref()
            .map_or(0, |(_, _, quote)| quote.retained_byte_len())
            .checked_add(
                readback_quote.map_or(0, CpuDesktopReadbackResourceQuote::retained_byte_len),
            )
            .ok_or_else(|| anyhow!("Windows CPU API resource quote overflow"))?;
        preflight_required_scope_bytes(&mut ledger, &mut cpu_api_minimum_remaining, cpu_api_quote)?;
        let reduction = if let Some((generation, admission, _quote)) = reduction_preparation {
            let preparation_gate = windows_gpu_preparation_gate(source.adapter_luid);
            let _preparation_guard = preparation_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let plan = duplicator.prepare_gpu_reduction_plan(
                generation,
                &reduction_descriptors,
                admission,
            )?;
            Some(WindowsGpuReductionRuntime {
                plan,
                routes: reduction_routes.into_boxed_slice(),
            })
        } else {
            None
        };
        let readback = readback_quote
            .is_some()
            .then(|| duplicator.prepare_cpu_desktop_readback(slot_count))
            .transpose()?;
        Some(WindowsCpuRuntime {
            readback,
            native_physical_mask,
            reduction,
            workspace_allocation_byte_len,
            fanout_candidate: Some(fanout_candidate),
            fanout: None,
            latest_frame: None,
        })
    } else {
        None
    };

    let cpu_api_bytes = cpu
        .as_ref()
        .map(|runtime| {
            let native = runtime
                .readback
                .as_ref()
                .map_or(0, PreparedCpuDesktopReadback::retained_byte_len);
            let reduced = runtime
                .reduction
                .as_ref()
                .map_or(0, |reduction| reduction.plan.retained_byte_len());
            native
                .checked_add(reduced)
                .ok_or_else(|| anyhow!("Windows reduction allocation accounting overflow"))
        })
        .transpose()?
        .unwrap_or(0);
    let workspace_bytes = cpu
        .as_ref()
        .map_or(0, |runtime| runtime.workspace_allocation_byte_len);
    let fanout_bytes = cpu.as_ref().map_or(0, |runtime| {
        runtime.fanout_candidate.as_ref().map_or(
            0,
            PreparedCpuPublicationFanoutCandidate::allocation_byte_len,
        )
    });
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| {
            (
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
                matches!(
                    minimum.descriptor().executor(),
                    ScreenPublicationExecutor::SourceNative(_)
                ),
                Arc::clone(minimum.descriptor()),
            )
        })
        .collect::<Vec<_>>();
    let cpu_api_scope = reports
        .iter()
        .find(|(_, resource, _, native, _)| {
            *resource == ScreenResourceKind::ApiAllocation && !*native
        })
        .map(|(name, _, _, _, _)| Arc::clone(name));
    let processing_scope = reports
        .iter()
        .find(|(_, resource, _, _, _)| *resource == ScreenResourceKind::ProcessingProfileState)
        .map(|(name, _, _, _, _)| Arc::clone(name));
    let gpu_route_count = gpu.as_ref().map_or(0, |runtime| runtime.routes.len());
    let reduction_route_count = cpu
        .as_ref()
        .and_then(|runtime| runtime.reduction.as_ref())
        .map_or(0, |reduction| reduction.routes.len());
    let gpu_route_metadata_bytes =
        checked_retained_metadata_bytes::<WindowsGpuRoute>(gpu_route_count, "GPU route")?
            .checked_add(checked_retained_arc_bytes::<GpuSurfaceDescriptor>(
                gpu_route_count,
                "GPU descriptor",
            )?)
            .ok_or_else(|| anyhow!("Windows GPU route metadata accounting overflow"))?;
    let reduction_route_metadata_bytes =
        checked_retained_metadata_bytes::<WindowsGpuReductionRoute>(
            reduction_route_count,
            "GPU reduction route",
        )?
        .checked_add(checked_retained_arc_bytes::<GpuSurfaceDescriptor>(
            reduction_route_count,
            "GPU reduction descriptor",
        )?)
        .ok_or_else(|| anyhow!("Windows GPU reduction route metadata accounting overflow"))?;
    let mask_metadata_bytes = cpu.as_ref().map_or(Ok(0), |runtime| {
        checked_retained_metadata_bytes::<bool>(
            runtime.native_physical_mask.len(),
            "CPU route mask",
        )
    })?;
    let worker_metadata_bytes = workspace_bytes
        .checked_sub(plane_minimum_bytes)
        .ok_or_else(|| anyhow!("Windows workspace accounting understates physical-plane minima"))?
        .checked_add(runtime_node_bytes)
        .and_then(|bytes| bytes.checked_add(gpu_route_metadata_bytes))
        .and_then(|bytes| bytes.checked_add(reduction_route_metadata_bytes))
        .and_then(|bytes| bytes.checked_add(mask_metadata_bytes))
        .ok_or_else(|| anyhow!("Windows exact worker allocation accounting overflow"))?;

    if cpu_api_bytes > 0 && cpu_api_scope.is_none() {
        ledger.report_scoped(
            "windows-cpu-readback",
            "worker-runtime-total",
            cpu_api_bytes,
        )?;
    }
    if fanout_bytes > 0 && processing_scope.is_none() {
        ledger.report_scoped("windows-cpu-fanout", "worker-runtime-total", fanout_bytes)?;
    }
    if let Some(gpu) = &gpu {
        let native_api_scope = reports
            .iter()
            .find(|(_, resource, _, native, _)| {
                *resource == ScreenResourceKind::ApiAllocation && *native
            })
            .map_or("worker-runtime-total", |(name, _, _, _, _)| name.as_ref());
        let plan_resource_bytes = gpu
            .plan
            .metadata_byte_len()
            .checked_add(gpu.plan.constant_buffer_byte_len())
            .ok_or_else(|| anyhow!("Windows GPU plan resource accounting overflow"))?;
        if plan_resource_bytes > 0 {
            ledger.report_scoped(
                "windows-gpu-plan-resources",
                native_api_scope,
                plan_resource_bytes,
            )?;
        }
        for route in &gpu.routes {
            let allocation_scope = reports
                .iter()
                .find(|(_, resource, _, native, descriptor)| {
                    *resource == ScreenResourceKind::ApiAllocation
                        && *native
                        && descriptor.as_ref() == &route.descriptor
                })
                .map_or("worker-runtime-total", |(name, _, _, _, _)| name.as_ref());
            ledger.report_scoped(
                &route.capture_resource_name,
                allocation_scope,
                route.capture_allocation_byte_len,
            )?;
        }
    }
    let lifetime_metadata_bytes = checked_retained_metadata_bytes::<ScreenResourceLifetime>(
        ledger.prospective_resource_count()?,
        "runtime lifetimes",
    )?;
    preflight_required_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        lifetime_metadata_bytes,
    )?;
    let worker_metadata_bytes = worker_metadata_bytes
        .checked_add(lifetime_metadata_bytes)
        .ok_or_else(|| anyhow!("Windows exact lifetime accounting overflow"))?;
    for (name, resource, minimum, _native, _descriptor) in &reports {
        let actual = match resource {
            ScreenResourceKind::ApiAllocation if cpu_api_scope.as_ref() == Some(name) => {
                cpu_api_bytes.max(*minimum)
            }
            ScreenResourceKind::ProcessingProfileState
                if processing_scope.as_ref() == Some(name) =>
            {
                fanout_bytes.max(*minimum)
            }
            ScreenResourceKind::WorkerAdditional => worker_metadata_bytes.max(*minimum),
            _ => *minimum,
        };
        ledger.report(name, actual)?;
    }
    let exact_ledger = ledger.finish()?;
    let binding = exact_ledger.token().binding().clone();
    let (token, lifetimes) = exact_ledger.into_parts();
    let gpu = gpu
        .map(|pending| {
            let mut routes = Vec::new();
            routes.try_reserve_exact(pending.routes.len())?;
            for route in pending.routes {
                let resource_name = format!("native-target-{}", route.id.get());
                let lifetime = lifetimes
                    .iter()
                    .find(|lifetime| lifetime.resource().name().as_ref() == resource_name)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!("Windows native target lifetime is missing from exact ledger")
                    })?;
                let capture_lifetime = lifetimes
                    .iter()
                    .find(|lifetime| lifetime.resource().name() == &route.capture_resource_name)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!("Windows GPU route lifetime is missing from exact ledger")
                    })?;
                let cadence = CaptureCadence::new(route.requested_hz.get())?;
                routes.push(WindowsGpuRoute {
                    id: route.id,
                    native: route.native,
                    descriptor: route.descriptor,
                    target: route.target.bind(lifetime)?,
                    capture_lifetime,
                    pacer: cadence.pacer(),
                    next_publish_at: Instant::now(),
                    retry_not_before: None,
                    last_accepted_sequence: None,
                    publisher: None,
                });
            }
            Ok::<_, anyhow::Error>(WindowsGpuRuntime {
                plan: pending.plan,
                routes: routes.into_boxed_slice(),
            })
        })
        .transpose()?;
    let runtime_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "worker-runtime-total")
        .cloned()
        .ok_or_else(|| anyhow!("Windows worker runtime lifetime is missing from exact ledger"))?;
    Ok((
        token,
        Some((
            WindowsExactRuntime {
                source: source.clone(),
                binding: binding.clone(),
                _lifetimes: lifetimes,
                gpu,
                cpu,
            },
            WindowsOwnedSource {
                source_id: source.epoch.source_id.clone(),
                binding,
                _runtime_lifetime: runtime_lifetime,
            },
        )),
    ))
}

fn windows_gpu_candidate_admission(
    source_extent: NativeCaptureExtent,
    descriptors: &[GpuSurfaceDescriptor],
    slot_count: NonZeroU32,
    available_gpu_bytes: u64,
) -> Result<GpuSurfaceAdmission, CaptureError> {
    let capture_admission = GpuSurfaceAdmission::new(u64::MAX, slot_count);
    let capture_slot_bytes = capture_admission.admit(source_extent, descriptors)?;
    let renderer_target_bytes = descriptors.iter().try_fold(0_u64, |total, descriptor| {
        let extent = descriptor.output_extent();
        let bytes = u64::from(extent.width())
            .checked_mul(u64::from(extent.height()))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account Windows native renderer target",
                width: extent.width(),
                height: extent.height(),
            })?;
        total
            .checked_add(bytes)
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account Windows native renderer targets",
                width: extent.width(),
                height: extent.height(),
            })
    })?;
    let required_gpu_bytes = capture_slot_bytes
        .checked_add(renderer_target_bytes)
        .ok_or(CaptureError::GeometryOverflow {
            operation: "account Windows exact GPU candidate",
            width: source_extent.width(),
            height: source_extent.height(),
        })?;
    if required_gpu_bytes > available_gpu_bytes {
        return Err(CaptureError::GpuSurfaceBudgetExceeded {
            requested_bytes: required_gpu_bytes,
            budget_bytes: available_gpu_bytes,
        });
    }
    Ok(GpuSurfaceAdmission::new(
        available_gpu_bytes - renderer_target_bytes,
        slot_count,
    ))
}

fn execute_windows_exact_command(
    command: CaptureExactCommand,
    duplicator: Option<&DesktopDuplicator>,
    exact: &ExactPublicationShared,
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    runtimes: &mut WindowsExactRuntimes,
) {
    execute_capture_exact_command(command, exact, runtimes, |ticket, source| {
        exact
            .hub()
            .ok_or_else(|| anyhow!("Windows exact publication hub is unavailable"))
            .and_then(|hub| {
                prepare_windows_exact_runtime(
                    ticket,
                    duplicator,
                    source,
                    exact,
                    hub.as_ref(),
                    compute_capacity_policy,
                )
            })
    });
}

fn publish_windows_gpu_outcome(
    routes: &mut [WindowsGpuRoute],
    source: &WindowsPublicationSource,
    runtime_plan_generation: super::ScreenPlanGeneration,
    hub: &ScreenPublicationHub,
    outcome: GpuSurfacePublishOutcome,
) -> anyhow::Result<GpuSurfacePublicationDisposition> {
    let publication = match outcome {
        GpuSurfacePublishOutcome::Published(publication) => publication,
        GpuSurfacePublishOutcome::Busy(descriptor_id) => {
            let route = routes
                .iter_mut()
                .find(|route| route.id == descriptor_id)
                .ok_or_else(|| anyhow!("Windows GPU pressure named an unknown exact route"))?;
            defer_windows_gpu_route_retry(route, Instant::now())?;
            return Ok(GpuSurfacePublicationDisposition::Retry);
        }
    };
    let provenance = publication.provenance();
    let route = routes
        .iter_mut()
        .find(|route| route.id == provenance.descriptor.id())
        .ok_or_else(|| anyhow!("Windows GPU publication named an unknown exact route"))?;
    let publisher = route
        .publisher
        .as_ref()
        .ok_or_else(|| anyhow!("Windows GPU route has no committed publisher"))?;
    let source_id = capture_source_id(&provenance.source_id)?;
    let output_extent = PixelExtent::new(
        provenance.output_extent.width(),
        provenance.output_extent.height(),
    )?;
    let valid = provenance.descriptor.as_ref() == route.native.as_ref()
        && provenance.plan_generation.get() == runtime_plan_generation.get()
        && source_id == source.epoch.source_id
        && provenance.topology_generation == source.epoch.topology_generation
        && provenance.duplication_generation == source.duplication_generation
        && provenance.adapter_luid == source.adapter_luid
        && provenance.native_source_extent.width() == source.native_extent.width()
        && provenance.native_source_extent.height() == source.native_extent.height()
        && provenance.logical_source_extent.width() == source.logical_extent.width()
        && provenance.logical_source_extent.height() == source.logical_extent.height()
        && output_extent == route.descriptor.geometry().output_extent()
        && provenance.coordinate_space == GpuSurfaceCoordinateSpace::LogicalDisplay
        && provenance.source_color_space == source.source_color_space
        && provenance.output_format == GpuSurfaceFormat::Rgba8Unorm
        && provenance.color_pipeline == GpuSurfaceColorPipeline::PreserveEncoded
        && provenance.pending_rotation == DisplayRotation::Identity;
    if !valid {
        anyhow::bail!("Windows GPU publication provenance violated its exact route contract");
    }
    let native_sequence = NonZeroU64::new(provenance.source_sequence)
        .ok_or_else(|| anyhow!("Windows GPU publication sequence must be nonzero"))?;
    if route
        .last_accepted_sequence
        .is_some_and(|accepted| provenance.source_sequence <= accepted)
    {
        return Ok(GpuSurfacePublicationDisposition::Accepted);
    }
    let surface = PlatformGpuSurface::new(
        PlatformGpuApi::Direct3d11,
        publication.opaque_handle_id().get(),
        output_extent,
        CapturePixelFormat::Rgba8,
        Arc::clone(&publication),
    )?;
    let surface = route
        .target
        .retain_on_surface_with_capture_allocation(surface, route.capture_lifetime.clone())?;
    let published_at = Instant::now();
    let metadata = ScreenPublicationMetadata::try_new(
        source.epoch.clone(),
        publisher.plan_generation(),
        native_sequence,
        provenance.captured_at,
        published_at,
        provenance.freshness_deadline,
        ScreenPublicationHealth::Healthy,
    )?;
    let payload = ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
        ScreenPublicationColorimetry::new(route.descriptor.physical().color_pipeline().output()),
        &surface,
    ));
    match hub.publish(publisher, payload, &metadata) {
        Ok(_) => {
            route.last_accepted_sequence = Some(provenance.source_sequence);
            route.retry_not_before = None;
            route.next_publish_at = route
                .pacer
                .advance_deadline(route.next_publish_at, published_at)?;
            Ok(GpuSurfacePublicationDisposition::Accepted)
        }
        Err(super::ScreenPublicationHubError::PublicationPressure { .. }) => {
            defer_windows_gpu_route_retry(route, published_at)?;
            Ok(GpuSurfacePublicationDisposition::Retry)
        }
        Err(error) => Err(error.into()),
    }
}

fn publish_windows_reduction_outcome(
    routes: &[WindowsGpuReductionRoute],
    source: &WindowsPublicationSource,
    hub: &ScreenPublicationHub,
    fanout: &mut PreparedCpuPublicationFanout,
    outcome: GpuReductionPublishOutcome<'_>,
) -> anyhow::Result<GpuReductionPublicationDisposition> {
    let provenance = outcome.provenance();
    let route = routes
        .iter()
        .find(|route| route.id == provenance.descriptor.id())
        .ok_or_else(|| anyhow!("Windows GPU reduction named an unknown physical route"))?;
    let physical = fanout
        .physical_descriptor(route.physical_index)
        .ok_or_else(|| anyhow!("Windows GPU reduction named an absent physical route"))?;
    let source_id = capture_source_id(&provenance.source_id)?;
    let output = physical.reduction_extent();
    let valid = provenance.descriptor.as_ref() == route.native.as_ref()
        && provenance.plan_generation.get() == fanout.plan_generation().get()
        && source_id == source.epoch.source_id
        && provenance.topology_generation == source.epoch.topology_generation
        && provenance.duplication_generation == source.duplication_generation
        && provenance.adapter_luid == source.adapter_luid
        && provenance.native_source_extent.width() == source.native_extent.width()
        && provenance.native_source_extent.height() == source.native_extent.height()
        && provenance.logical_source_extent.width() == source.logical_extent.width()
        && provenance.logical_source_extent.height() == source.logical_extent.height()
        && provenance.source_color_space == source.source_color_space
        && provenance.source_rotation == display_rotation(source.rotation)?
        && provenance.descriptor.output_extent().width() == output.width()
        && provenance.descriptor.output_extent().height() == output.height()
        && (!provenance.cursor_composed || physical.cursor() == ScreenCursorPolicy::Include);
    if !valid
        || provenance.completed_at < provenance.captured_at
        || provenance.freshness_deadline < provenance.captured_at
    {
        anyhow::bail!("Windows GPU reduction provenance violated its physical route contract");
    }
    fanout.publish_prereduced_physical(
        hub,
        route.physical_index,
        outcome.pixels(),
        provenance.source_sequence,
        provenance.captured_at,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )?;
    Ok(GpuReductionPublicationDisposition::Accepted)
}

fn defer_windows_gpu_route_retry(
    route: &mut WindowsGpuRoute,
    now: Instant,
) -> Result<(), CaptureCadenceError> {
    route.retry_not_before = Some(windows_gpu_retry_at(route.pacer, now)?);
    Ok(())
}

fn windows_gpu_route_attempt_at(route: &WindowsGpuRoute) -> Instant {
    windows_gpu_attempt_at(route.next_publish_at, route.retry_not_before)
}

fn windows_gpu_retry_at(pacer: CapturePacer, now: Instant) -> Result<Instant, CaptureCadenceError> {
    let mut retry_pacer = pacer;
    retry_pacer.advance_deadline(now, now)
}

fn windows_gpu_attempt_at(next_publish_at: Instant, retry_not_before: Option<Instant>) -> Instant {
    retry_not_before.map_or(next_publish_at, |retry| retry.max(next_publish_at))
}

fn build_exact_cpu_frame(
    frame: CpuDesktopFrame,
    source: &WindowsPublicationSource,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let frame_source_id = capture_source_id(frame.source_id())?;
    if frame_source_id != source.epoch.source_id
        || frame.topology_generation() != source.epoch.topology_generation
        || frame.duplication_generation() != source.duplication_generation
        || frame.source_color_space() != source.source_color_space
    {
        anyhow::bail!("Windows CPU readback provenance violated its exact source contract");
    }
    let native_extent = PixelExtent::new(frame.width(), frame.height())?;
    let geometry = capture_geometry(
        native_extent,
        native_extent,
        PhysicalOrigin {
            x: frame.origin_x(),
            y: frame.origin_y(),
        },
        frame.rotation(),
    )?;
    let cursor = frame.cursor();
    let cursor = CaptureCursor {
        visible: cursor.visible,
        position: cursor.visible.then_some(PhysicalOrigin {
            x: cursor.position_x,
            y: cursor.position_y,
        }),
        hotspot: cursor.visible.then_some(PhysicalOrigin {
            x: cursor.hotspot_x,
            y: cursor.hotspot_y,
        }),
        shape_extent: (cursor.width > 0 && cursor.height > 0)
            .then(|| PixelExtent::new(cursor.width, cursor.height))
            .transpose()?,
        shape_generation: (cursor.shape_generation > 0).then_some(cursor.shape_generation),
        content: if cursor.visible {
            super::CaptureCursorContent::Absent
        } else {
            super::CaptureCursorContent::Hidden
        },
    };
    let captured_at = frame.captured_at();
    let sequence = frame.sequence();
    let row_stride = i64::try_from(frame.row_stride_bytes())?;
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch.source_id.clone(),
            topology_generation: source.epoch.topology_generation,
            session_generation: source.epoch.session_generation,
            sequence,
            captured_at,
            fresh_until: captured_at,
            geometry,
            colorimetry: source.colorimetry,
            cursor,
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            frame,
            CapturePixelFormat::Bgra8,
            row_stride,
            0,
        )),
        CaptureDamage::default(),
    )
    .map_err(anyhow::Error::from)
}

fn exact_runtime_wait(runtime: &WindowsExactRuntime, now: Instant) -> Duration {
    let gpu_due = runtime
        .gpu
        .as_ref()
        .and_then(|gpu| gpu.routes.iter().map(windows_gpu_route_attempt_at).min());
    let cpu_due = runtime
        .cpu
        .as_ref()
        .and_then(|cpu| cpu.fanout.as_ref())
        .and_then(PreparedCpuPublicationFanout::next_due_at);
    let reduction_poll = runtime
        .cpu
        .as_ref()
        .and_then(|cpu| cpu.reduction.as_ref())
        .is_some_and(|reduction| reduction.plan.has_pending_routes())
        .then_some(READBACK_POLL_WAIT);
    gpu_due
        .into_iter()
        .chain(cpu_due)
        .map(|deadline| deadline.saturating_duration_since(now))
        .chain(reduction_poll)
        .min()
        .unwrap_or(FRAME_WAIT)
        .min(FRAME_WAIT)
}

fn pump_windows_exact_runtime(
    session: &mut DesktopDuplicator,
    runtime: &mut WindowsExactRuntime,
    hub: &ScreenPublicationHub,
) -> anyhow::Result<Duration> {
    let now = Instant::now();
    let mut native_needs_source = false;
    let mut reduction_requested = false;
    if let Some(cpu) = &mut runtime.cpu {
        let fanout = cpu
            .fanout
            .as_mut()
            .ok_or_else(|| anyhow!("Windows CPU fanout is not bound"))?;
        native_needs_source = fanout
            .publish_due_masked(
                hub,
                cpu.latest_frame.as_ref(),
                now,
                ScreenPublicationHealth::Healthy,
                &cpu.native_physical_mask,
            )?
            .needs_source()
            || cpu
                .readback
                .as_ref()
                .is_some_and(PreparedCpuDesktopReadback::has_pending);
        if let Some(reduction) = &mut cpu.reduction {
            reduction
                .plan
                .select_routes_for_next_acquisition(|descriptor| {
                    reduction
                        .routes
                        .iter()
                        .find(|route| route.id == descriptor.id())
                        .is_some_and(|route| fanout.physical_pending(route.physical_index))
                });
            reduction_requested =
                reduction.plan.has_selected_routes() || reduction.plan.has_pending_routes();
        }
    }

    let mut gpu_requested = false;
    if let Some(gpu) = &mut runtime.gpu {
        for route in &gpu.routes {
            if let Some(publisher) = &route.publisher {
                publisher.reap_releasable_gpu_payloads();
            }
        }
        gpu.plan.select_routes_for_next_acquisition(|descriptor| {
            gpu.routes
                .iter()
                .find(|route| route.id == descriptor.id())
                .is_some_and(|route| now >= windows_gpu_route_attempt_at(route))
        });
        gpu_requested = gpu.plan.has_selected_routes();
    }
    if !gpu_requested && !reduction_requested && !native_needs_source {
        return Ok(exact_runtime_wait(runtime, now));
    }

    let source = runtime.source.clone();
    let runtime_plan_generation = runtime.binding.plan_generation();
    let mut gpu_error = None;
    let mut reduction_error = None;
    let (gpu_plan, mut gpu_routes) = if gpu_requested {
        runtime.gpu.as_mut().map_or((None, None), |gpu| {
            (Some(&mut gpu.plan), Some(gpu.routes.as_mut()))
        })
    } else {
        (None, None)
    };
    let (reduction_plan, reduction_routes, native_readback, mut reduction_fanout) =
        if let Some(cpu) = &mut runtime.cpu {
            let fanout = cpu
                .fanout
                .as_mut()
                .ok_or_else(|| anyhow!("Windows CPU fanout is not bound"))?;
            let native_readback = if native_needs_source {
                Some(cpu.readback.as_mut().ok_or_else(|| {
                    anyhow!("Windows native fallback demand has no prepared readback ring")
                })?)
            } else {
                None
            };
            let (plan, routes) = if reduction_requested {
                cpu.reduction.as_mut().map_or((None, None), |reduction| {
                    (Some(&mut reduction.plan), Some(reduction.routes.as_ref()))
                })
            } else {
                (None, None)
            };
            (plan, routes, native_readback, Some(fanout))
        } else {
            (None, None, None, None)
        };
    let request = CapturePumpRequest::with_reduction(gpu_plan, reduction_plan, native_readback);
    let report = session.pump_with_reduction_feedback(
        request,
        FRAME_WAIT,
        |outcome| {
            let result = gpu_routes
                .as_deref_mut()
                .ok_or_else(|| anyhow!("Windows GPU callback has no requested route set"))
                .and_then(|routes| {
                    publish_windows_gpu_outcome(
                        routes,
                        &source,
                        runtime_plan_generation,
                        hub,
                        outcome,
                    )
                });
            match result {
                Ok(disposition) => disposition,
                Err(error) => {
                    if gpu_error.is_none() {
                        gpu_error = Some(error);
                    }
                    GpuSurfacePublicationDisposition::Retry
                }
            }
        },
        |outcome| {
            let result = reduction_routes
                .ok_or_else(|| anyhow!("Windows reduction callback has no requested route set"))
                .and_then(|routes| {
                    reduction_fanout
                        .as_deref_mut()
                        .ok_or_else(|| anyhow!("Windows reduction callback has no bound fanout"))
                        .and_then(|fanout| {
                            publish_windows_reduction_outcome(routes, &source, hub, fanout, outcome)
                        })
                });
            match result {
                Ok(disposition) => disposition,
                Err(error) => {
                    if reduction_error.is_none() {
                        reduction_error = Some(error);
                    }
                    GpuReductionPublicationDisposition::Retry
                }
            }
        },
    )?;
    if let Some(error) = gpu_error {
        return Err(error);
    }
    if let Some(error) = reduction_error {
        return Err(error);
    }
    if let CaptureLane::Failed(error) = report.gpu {
        return Err(error.into());
    }
    let mut poll_again = false;
    match report.reduction {
        CaptureLane::Failed(error) => return Err(error.into()),
        CaptureLane::Ready(info) => {
            debug!(
                submitted = info.submitted(),
                completed = info.completed(),
                busy = info.busy(),
                readback_bytes = info.readback_bytes(),
                "advanced Windows descriptor-keyed GPU reductions"
            );
            poll_again = runtime
                .cpu
                .as_ref()
                .and_then(|cpu| cpu.reduction.as_ref())
                .is_some_and(|reduction| reduction.plan.has_pending_routes());
        }
        CaptureLane::Busy => poll_again = true,
        CaptureLane::NotRequested | CaptureLane::Idle => {}
    }
    match report.cpu {
        CaptureLane::Failed(error) => return Err(error.into()),
        CaptureLane::Ready(frame) => {
            let frame = build_exact_cpu_frame(frame, &source)?;
            let cpu = runtime
                .cpu
                .as_mut()
                .ok_or_else(|| anyhow!("Windows CPU readback completed without a runtime"))?;
            cpu.fanout
                .as_mut()
                .ok_or_else(|| anyhow!("Windows CPU fanout is not bound"))?
                .publish_due_masked(
                    hub,
                    Some(&frame),
                    Instant::now(),
                    ScreenPublicationHealth::Healthy,
                    &cpu.native_physical_mask,
                )?;
            cpu.latest_frame = Some(frame);
        }
        CaptureLane::Busy => poll_again = true,
        CaptureLane::NotRequested | CaptureLane::Idle => {}
    }
    Ok(if poll_again {
        READBACK_POLL_WAIT
    } else {
        Duration::ZERO
    })
}

fn pump_current_windows_exact_runtime(
    session: &mut DesktopDuplicator,
    runtimes: &mut WindowsExactRuntimes,
    source: &WindowsPublicationSource,
    exact: &ExactPublicationShared,
) -> anyhow::Result<Option<Duration>> {
    let Some(hub) = exact.hub() else {
        return Ok(None);
    };
    let Some(runtime) = bind_current_capture_exact_runtime(runtimes, source, &hub, |_, _| Ok(()))?
    else {
        return Ok(None);
    };
    pump_windows_exact_runtime(session, runtime, &hub).map(Some)
}

/// Worker loop: own the duplication session, analyze frames, publish results.
fn run_worker(
    settings: &Arc<SharedSettings>,
    publication: &Arc<Mutex<CapturePublication<AnalyzedScreenSnapshot>>>,
    exact: &Arc<ExactPublicationShared>,
    command_rx: &mpsc::Receiver<WorkerCommand>,
    cancel: &Arc<AtomicBool>,
    processed_activity_generation: &AtomicU64,
    status_session: SourceSessionSlot,
    session_generation: u64,
    source_sink: Option<CaptureSourceSink>,
    ready: mpsc::SyncSender<std::result::Result<(), String>>,
) {
    let authority = CaptureSessionAuthority::new(session_generation);
    let initial_settings = settings.snapshot();
    let mut config = initial_settings.config;
    let mut schedule = match CaptureCadence::new(config.target_fps) {
        Ok(cadence) => WorkerCaptureSchedule::new(cadence, Instant::now()),
        Err(error) => {
            if let Some(status) = status_session.load() {
                status.unavailable(SourceIssue::new(
                    "windows_capture_cadence_unrepresentable",
                    error.to_string(),
                    false,
                ));
            }
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    let mut source_generation = initial_settings.source_generation;
    let mut demand = initial_settings.demand;
    let mut generation = settings.values.revision();
    let mut analyzer = match build_worker_analyzer(
        &config,
        demand,
        settings.admission_coordinator.clone(),
        settings.compute_capacity_policy,
    ) {
        Ok(analyzer) => analyzer,
        Err(error) => {
            if let Some(status) = status_session.load() {
                status.unavailable(screen_resource_issue(&error));
            }
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    let mut duplicator: Option<DesktopDuplicator> = None;
    let mut active = false;
    let mut activity_generation = 0_u64;
    let mut open_failure_logged = false;
    let mut resource_failure_logged = false;
    let mut analysis_failure_latched = false;
    let mut rejected_analysis_work = None;
    let mut failed_settings_generation = None;
    let mut settings_retry_at = Instant::now();
    let mut exact_runtimes = WindowsExactRuntimes::default();

    loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }

        match drain_commands(
            command_rx,
            &mut active,
            &mut activity_generation,
            settings,
            publication,
            &mut config,
            &mut schedule,
            &mut source_generation,
            &mut demand,
            &mut generation,
            &mut analyzer,
            &mut duplicator,
            exact,
            &mut exact_runtimes,
        ) {
            ControlFlow::Stop => break,
            ControlFlow::Continue => {}
        }

        if !active {
            // Release the duplication interface so other applications can use
            // it while no screen-reactive effect is running.
            settle_inactive_capture(
                &mut duplicator,
                processed_activity_generation,
                activity_generation,
            );
            clear_capture_publication(publication);
            exact.replace_source_if_current(authority, None);
            open_failure_logged = false;
            match command_rx.recv_timeout(FRAME_WAIT) {
                Ok(WorkerCommand::SetActive {
                    active: next,
                    activity_generation: next_generation,
                }) => {
                    active = next;
                    activity_generation = next_generation;
                }
                Ok(WorkerCommand::AdoptSettings {
                    prepared,
                    ready,
                    decision,
                    done,
                }) => adopt_prepared_worker_settings(
                    settings,
                    publication,
                    &mut config,
                    &mut schedule,
                    &mut source_generation,
                    &mut demand,
                    &mut generation,
                    &mut analyzer,
                    &mut duplicator,
                    prepared,
                    ready,
                    decision,
                    done,
                ),
                Ok(WorkerCommand::Exact(command)) => execute_windows_exact_command(
                    command,
                    duplicator.as_ref(),
                    exact,
                    settings.compute_capacity_policy,
                    &mut exact_runtimes,
                ),
                Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            continue;
        }

        processed_activity_generation.store(activity_generation, Ordering::Release);

        let latest_generation = settings.values.revision();
        if latest_generation != generation
            && (failed_settings_generation != Some(latest_generation)
                || Instant::now() >= settings_retry_at)
        {
            let next_settings = settings.snapshot();
            let next_cadence = match CaptureCadence::new(next_settings.config.target_fps) {
                Ok(cadence) => cadence,
                Err(error) => {
                    failed_settings_generation = Some(latest_generation);
                    settings_retry_at = Instant::now()
                        .checked_add(REOPEN_BACKOFF)
                        .unwrap_or_else(Instant::now);
                    warn!(%error, generation = latest_generation, "Retaining prior Windows capture cadence after admission failure");
                    if let Some(status) = status_session.load() {
                        status.degraded(SourceIssue::new(
                            "windows_capture_cadence_unrepresentable",
                            error.to_string(),
                            false,
                        ));
                    }
                    continue;
                }
            };
            match build_worker_analyzer(
                &next_settings.config,
                next_settings.demand,
                settings.admission_coordinator.clone(),
                settings.compute_capacity_policy,
            ) {
                Ok(next_analyzer) => {
                    let previous_source = config.source.clone();
                    config = next_settings.config;
                    schedule.replace(next_cadence, Instant::now());
                    source_generation = next_settings.source_generation;
                    demand = next_settings.demand;
                    analyzer = next_analyzer;
                    generation = latest_generation;
                    failed_settings_generation = None;
                    if previous_source != config.source {
                        duplicator = None;
                        clear_capture_publication(publication);
                        exact.replace_source_if_current(authority, None);
                    } else if let Some(duplicator) = duplicator.as_mut() {
                        let requested_extent = demand
                            .requested_extent()
                            .expect("active Windows capture demand carries an extent");
                        duplicator.set_requested_extent(native_capture_extent(requested_extent));
                    }
                }
                Err(error) => {
                    failed_settings_generation = Some(latest_generation);
                    settings_retry_at = Instant::now()
                        .checked_add(REOPEN_BACKOFF)
                        .unwrap_or_else(Instant::now);
                    warn!(%error, generation = latest_generation, "Retaining prior Windows capture settings after resource admission failure");
                    if let Some(status) = status_session.load() {
                        status.degraded(screen_resource_issue(&error));
                    }
                }
            }
        }

        let session = if let Some(session) = duplicator.as_mut() {
            session
        } else {
            let configured_source = config.source.clone();
            let selector = super::monitor_selector_from_source(&configured_source);
            let requested_extent = demand
                .requested_extent()
                .expect("active Windows capture demand carries an extent");
            let resource_admission = Arc::new(WindowsCaptureResourceAdmission {
                coordinator: settings.admission_coordinator.clone(),
            });
            match DesktopDuplicator::open_with_resource_admission(
                selector.clone(),
                native_capture_extent(requested_extent),
                resource_admission,
            ) {
                Ok(session) => {
                    if let Some(source) = selector.canonical_source(session.source_id()) {
                        if let Some(sink) = source_sink.as_ref() {
                            sink(ResolvedCaptureSource {
                                configured_source,
                                stable_source: source.clone(),
                            });
                        }
                        config.source = source;
                    }
                    let (width, height) = session.native_extent();
                    info!(
                        source = session.source_id(),
                        width, height, "Windows screen capture online"
                    );
                    open_failure_logged = false;
                    duplicator.insert(session)
                }
                Err(error) => {
                    clear_capture_publication(publication);
                    exact.replace_source_if_current(authority, None);
                    if !open_failure_logged {
                        log_open_failure(&error);
                        open_failure_logged = true;
                    }
                    if let Some(status) = status_session.load() {
                        status.unavailable(capture_issue(&error));
                    }
                    match command_rx.recv_timeout(REOPEN_BACKOFF) {
                        Ok(WorkerCommand::SetActive {
                            active: next,
                            activity_generation: next_generation,
                        }) => {
                            active = next;
                            activity_generation = next_generation;
                        }
                        Ok(WorkerCommand::AdoptSettings {
                            prepared,
                            ready,
                            decision,
                            done,
                        }) => adopt_prepared_worker_settings(
                            settings,
                            publication,
                            &mut config,
                            &mut schedule,
                            &mut source_generation,
                            &mut demand,
                            &mut generation,
                            &mut analyzer,
                            &mut duplicator,
                            prepared,
                            ready,
                            decision,
                            done,
                        ),
                        Ok(WorkerCommand::Exact(command)) => execute_windows_exact_command(
                            command,
                            duplicator.as_ref(),
                            exact,
                            settings.compute_capacity_policy,
                            &mut exact_runtimes,
                        ),
                        Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    continue;
                }
            }
        };

        let exact_source = match WindowsPublicationSource::from_session(session, session_generation)
        {
            Ok(source) => source,
            Err(error) => {
                warn!(%error, "Windows screen capture source metadata is invalid; reopening session");
                clear_capture_publication(publication);
                exact.replace_source_if_current(authority, None);
                duplicator = None;
                continue;
            }
        };
        exact.replace_source_if_current(authority, Some(exact_source.clone()));

        match pump_current_windows_exact_runtime(session, &mut exact_runtimes, &exact_source, exact)
        {
            Ok(Some(wait)) => {
                resource_failure_logged = false;
                if !wait.is_zero() {
                    thread::sleep(wait);
                }
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                let capture_error = error.downcast_ref::<CaptureError>();
                if !resource_failure_logged {
                    warn!(%error, "Windows exact publication failed; retaining its last good publications");
                    if let Some(status) = status_session.load() {
                        let issue = capture_error.map_or_else(
                            || {
                                SourceIssue::new(
                                    "windows_exact_publication_failed",
                                    error.to_string(),
                                    true,
                                )
                            },
                            capture_issue,
                        );
                        status.degraded(issue);
                    }
                    resource_failure_logged = true;
                }
                if capture_error.is_some_and(capture_frame_failure_invalidates_session) {
                    clear_capture_publication(publication);
                    exact.replace_source_if_current(authority, None);
                    duplicator = None;
                } else {
                    thread::sleep(FRAME_WAIT);
                }
                continue;
            }
        }

        let analysis_extent = demand
            .requested_extent()
            .expect("active Windows capture demand carries an extent");
        let analysis_revision = (analysis_extent, generation);
        if rejected_analysis_work == Some(analysis_revision) {
            clear_capture_publication(publication);
            thread::sleep(FRAME_WAIT);
            continue;
        }
        if analyzer
            .analysis_work_plan()
            .is_none_or(|plan| plan.input_extent() != analysis_extent)
        {
            match analyzer.admit_frame_extent(analysis_extent) {
                Ok(_) => rejected_analysis_work = None,
                Err(error) => {
                    clear_capture_publication(publication);
                    warn!(%error, "Windows compatibility screen analysis exceeds admitted CPU compute");
                    if let Some(status) = status_session.load() {
                        status.unavailable(screen_analysis_admission_issue(&error));
                    }
                    rejected_analysis_work = Some(analysis_revision);
                    thread::sleep(FRAME_WAIT);
                    continue;
                }
            }
        }

        let active_epoch = match active_capture_epoch(
            session,
            session_generation,
            source_generation,
            activity_generation,
        ) {
            Ok(active_epoch) => active_epoch,
            Err(error) => {
                warn!(%error, "Windows screen capture identity is invalid; reopening session");
                clear_capture_publication(publication);
                exact.replace_source_if_current(authority, None);
                duplicator = None;
                continue;
            }
        };
        if !activate_capture_epoch(publication, active_epoch) {
            continue;
        }

        if let Some(wait) = schedule.wait_duration(Instant::now()) {
            thread::sleep(wait.min(FRAME_WAIT));
            continue;
        }

        let frame_result = session.next_frame(FRAME_WAIT);
        let reduction_telemetry = session.reduction_telemetry();
        let current_epoch = if frame_result.is_ok() {
            match active_capture_epoch(
                session,
                session_generation,
                source_generation,
                activity_generation,
            ) {
                Ok(current_epoch) => {
                    if !activate_capture_epoch(publication, current_epoch.clone()) {
                        continue;
                    }
                    Some(current_epoch)
                }
                Err(error) => {
                    warn!(%error, "Windows screen capture identity became invalid");
                    clear_capture_publication(publication);
                    exact.replace_source_if_current(authority, None);
                    duplicator = None;
                    continue;
                }
            }
        } else {
            None
        };

        match frame_result {
            Ok(Some(frame)) => {
                resource_failure_logged = false;
                let Some(current_epoch) = current_epoch else {
                    duplicator = None;
                    continue;
                };
                let captured_at = frame.captured_at;
                let freshness_deadline = match schedule.record_frame(captured_at, Instant::now()) {
                    Ok(deadline) => deadline,
                    Err(error) => {
                        warn!(%error, "Windows screen capture cadence deadline is unrepresentable");
                        if let Some(status) = status_session.load() {
                            status.unavailable(SourceIssue::new(
                                "windows_capture_cadence_deadline_unrepresentable",
                                error.to_string(),
                                false,
                            ));
                        }
                        break;
                    }
                };
                let raw_frame = build_capture_frame(frame, session_generation, freshness_deadline);
                let published = raw_frame.and_then(|frame| {
                    let snapshot = analyze_capture_frame(&mut analyzer, &current_epoch, frame)?;
                    let publication = {
                        publication
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .publish(&current_epoch, snapshot)
                    };
                    Ok(publication.is_ok())
                });
                let published = match published {
                    Ok(published) => published,
                    Err(error) => {
                        if !analysis_failure_latched {
                            warn!(%error, "Windows screen analysis rejected a frame; retaining last good publication");
                            if let Some(status) = status_session.load() {
                                status.degraded(SourceIssue::new(
                                    "windows_screen_analysis_rejected",
                                    error.to_string(),
                                    true,
                                ));
                            }
                            analysis_failure_latched = true;
                        }
                        continue;
                    }
                };
                if published && let Some(status) = status_session.load() {
                    analysis_failure_latched = false;
                    record_capture_health(
                        &status,
                        captured_at,
                        freshness_deadline,
                        &reduction_telemetry,
                    );
                }
            }
            // Static desktop or pointer-only update: nothing new to analyze.
            Ok(None) => {}
            Err(error) => {
                if !capture_frame_failure_invalidates_session(&error) {
                    if !resource_failure_logged {
                        warn!(%error, "Windows screen capture frame failed; retaining last good publication and live session");
                        if let Some(status) = status_session.load() {
                            status.degraded(capture_issue(&error));
                        }
                        resource_failure_logged = true;
                    }
                    thread::sleep(FRAME_WAIT);
                    continue;
                }
                resource_failure_logged = false;
                if let Some(status) = status_session.load() {
                    status.degraded(capture_issue(&error));
                }
                clear_capture_publication(publication);
                exact.replace_source_if_current(authority, None);
                warn!(%error, "Windows screen capture frame failed; reopening session");
                duplicator = None;
                match command_rx.recv_timeout(REOPEN_BACKOFF) {
                    Ok(WorkerCommand::SetActive {
                        active: next,
                        activity_generation: next_generation,
                    }) => {
                        active = next;
                        activity_generation = next_generation;
                    }
                    Ok(WorkerCommand::AdoptSettings {
                        prepared,
                        ready,
                        decision,
                        done,
                    }) => adopt_prepared_worker_settings(
                        settings,
                        publication,
                        &mut config,
                        &mut schedule,
                        &mut source_generation,
                        &mut demand,
                        &mut generation,
                        &mut analyzer,
                        &mut duplicator,
                        prepared,
                        ready,
                        decision,
                        done,
                    ),
                    Ok(WorkerCommand::Exact(command)) => execute_windows_exact_command(
                        command,
                        duplicator.as_ref(),
                        exact,
                        settings.compute_capacity_policy,
                        &mut exact_runtimes,
                    ),
                    Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    clear_capture_publication(publication);
    exact.replace_source_if_current(authority, None);
    exact.clear_owned_sources_if_current(authority);
    exact_runtimes.clear();
    debug!("Windows screen capture worker stopped");
}

const fn capture_frame_failure_invalidates_session(error: &CaptureError) -> bool {
    !matches!(
        error,
        CaptureError::ResourceExhausted { .. }
            | CaptureError::GeometryOverflow { .. }
            | CaptureError::InvalidBufferGeometry { .. }
    )
}

fn native_capture_extent(extent: PixelExtent) -> NativeCaptureExtent {
    NativeCaptureExtent::try_new(extent.width(), extent.height())
        .expect("core pixel extents are non-empty")
}

fn analyze_capture_frame(
    analyzer: &mut ScreenCaptureInput,
    active: &ActiveCaptureEpoch,
    frame: CaptureFrame<RawCaptureSurface>,
) -> anyhow::Result<AnalyzedScreenSnapshot> {
    frame.validate_epoch(&active.epoch)?;
    analyze_screen_frame(analyzer, frame)
}

fn capture_source_id(source_id: &str) -> anyhow::Result<CaptureSourceId> {
    CaptureSourceId::new(Arc::<str>::from(format!("windows:{source_id}")))
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
fn capture_epoch(
    source_id: &str,
    topology_generation: u64,
    session_generation: u64,
) -> anyhow::Result<CaptureEpoch> {
    Ok(CaptureEpoch {
        source_id: capture_source_id(source_id)?,
        topology_generation,
        session_generation,
    })
}

fn build_capture_frame(
    frame: NativeCaptureFrame,
    session_generation: u64,
    freshness_deadline: Instant,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let source_id = capture_source_id(&frame.source_id)?;
    let topology_generation = frame.topology_generation;
    let cursor_content = if frame.cursor.visible {
        if frame.cursor.composed {
            super::CaptureCursorContent::Composed
        } else {
            super::CaptureCursorContent::Absent
        }
    } else {
        super::CaptureCursorContent::Hidden
    };
    let cursor = CaptureCursor {
        visible: frame.cursor.visible,
        position: (frame.cursor.width > 0 && frame.cursor.height > 0).then_some(PhysicalOrigin {
            x: frame.cursor.position_x,
            y: frame.cursor.position_y,
        }),
        hotspot: (frame.cursor.width > 0 && frame.cursor.height > 0).then_some(PhysicalOrigin {
            x: frame.cursor.hotspot_x,
            y: frame.cursor.hotspot_y,
        }),
        shape_extent: (frame.cursor.width > 0 && frame.cursor.height > 0)
            .then(|| PixelExtent::new(frame.cursor.width, frame.cursor.height))
            .transpose()?,
        shape_generation: (frame.cursor.shape_generation > 0)
            .then_some(frame.cursor.shape_generation),
        content: cursor_content,
    };
    let storage_extent = PixelExtent::new(frame.width, frame.height)?;
    let native_extent = PixelExtent::new(frame.native_width, frame.native_height)?;
    let row_stride = i64::from(frame.width)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("Windows capture row stride overflow"))?;
    let geometry = capture_geometry(
        native_extent,
        storage_extent,
        PhysicalOrigin {
            x: frame.origin_x,
            y: frame.origin_y,
        },
        frame.rotation,
    )?;
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation,
            session_generation,
            sequence: frame.sequence,
            captured_at: frame.captured_at,
            fresh_until: freshness_deadline,
            geometry,
            colorimetry: CaptureColorimetry::unknown(),
            cursor,
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            frame,
            CapturePixelFormat::Rgba8,
            row_stride,
            0,
        )),
        CaptureDamage::default(),
    )
    .map_err(anyhow::Error::from)
}

fn capture_geometry(
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    origin: PhysicalOrigin,
    rotation: DisplayRotation,
) -> Result<CaptureGeometry, crate::input::screen::CaptureFrameError> {
    CaptureGeometry::new(
        origin,
        native_extent,
        storage_extent,
        capture_rotation(rotation),
        None,
        SourceScale::ONE,
    )
}

const fn capture_rotation(rotation: DisplayRotation) -> CaptureRotation {
    match rotation {
        DisplayRotation::Identity => CaptureRotation::Identity,
        DisplayRotation::Clockwise90 => CaptureRotation::Clockwise90,
        DisplayRotation::Clockwise180 => CaptureRotation::Clockwise180,
        DisplayRotation::Clockwise270 => CaptureRotation::Clockwise270,
    }
}

enum ControlFlow {
    Continue,
    Stop,
}

/// Apply every queued command without blocking.
fn drain_commands(
    command_rx: &mpsc::Receiver<WorkerCommand>,
    active: &mut bool,
    activity_generation: &mut u64,
    settings: &SharedSettings,
    publication: &Mutex<CapturePublication<AnalyzedScreenSnapshot>>,
    config: &mut CaptureConfig,
    schedule: &mut WorkerCaptureSchedule,
    source_generation: &mut u64,
    demand: &mut ScreenCaptureDemand,
    generation: &mut u64,
    analyzer: &mut ScreenCaptureInput,
    duplicator: &mut Option<DesktopDuplicator>,
    exact: &ExactPublicationShared,
    exact_runtimes: &mut WindowsExactRuntimes,
) -> ControlFlow {
    loop {
        match command_rx.try_recv() {
            Ok(WorkerCommand::SetActive {
                active: next,
                activity_generation: next_generation,
            }) => {
                *active = next;
                *activity_generation = next_generation;
            }
            Ok(WorkerCommand::AdoptSettings {
                prepared,
                ready,
                decision,
                done,
            }) => adopt_prepared_worker_settings(
                settings,
                publication,
                config,
                schedule,
                source_generation,
                demand,
                generation,
                analyzer,
                duplicator,
                prepared,
                ready,
                decision,
                done,
            ),
            Ok(WorkerCommand::Exact(command)) => execute_windows_exact_command(
                command,
                duplicator.as_ref(),
                exact,
                settings.compute_capacity_policy,
                exact_runtimes,
            ),
            Ok(WorkerCommand::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                return ControlFlow::Stop;
            }
            Err(mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn adopt_prepared_worker_settings(
    settings: &SharedSettings,
    publication: &Mutex<CapturePublication<AnalyzedScreenSnapshot>>,
    config: &mut CaptureConfig,
    schedule: &mut WorkerCaptureSchedule,
    source_generation: &mut u64,
    demand: &mut ScreenCaptureDemand,
    generation: &mut u64,
    analyzer: &mut ScreenCaptureInput,
    duplicator: &mut Option<DesktopDuplicator>,
    prepared: PreparedWorkerSettings,
    ready: mpsc::SyncSender<()>,
    decision: mpsc::Receiver<SettingsDecision>,
    done: mpsc::SyncSender<()>,
) {
    if ready.send(()).is_err() || !matches!(decision.recv(), Ok(SettingsDecision::Commit)) {
        return;
    }
    let source_changed = *source_generation != prepared.source_generation;
    if source_changed {
        *duplicator = None;
    } else if let Some(duplicator) = duplicator.as_mut() {
        let requested_extent = prepared
            .demand
            .requested_extent()
            .expect("prepared active Windows settings carry an extent");
        duplicator.set_requested_extent(native_capture_extent(requested_extent));
    }
    config.clone_from(&prepared.config);
    schedule.replace(prepared.cadence, Instant::now());
    *source_generation = prepared.source_generation;
    *demand = prepared.demand;
    *generation = settings.commit_values(
        &prepared.config,
        prepared.source_generation,
        prepared.demand,
    );
    *analyzer = prepared.analyzer;
    if source_changed {
        let displaced = {
            publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fence_source(*source_generation)
        };
        drop(displaced);
    }
    let _ = done.send(());
}

/// Log an open failure at a level that matches how actionable it is.
fn log_open_failure(error: &CaptureError) {
    match error {
        CaptureError::AlreadyDuplicating => {
            info!("desktop duplication has no free client slot; retrying in the background");
        }
        CaptureError::AccessDenied
        | CaptureError::SessionUnavailable
        | CaptureError::AccessLost
        | CaptureError::Timeout => {
            debug!(%error, "Windows desktop temporarily unavailable; retrying");
        }
        other => warn!(%other, "failed to open Windows screen capture"),
    }
}

fn capture_issue(error: &CaptureError) -> SourceIssue {
    match error {
        CaptureError::AlreadyDuplicating => {
            SourceIssue::new("windows_desktop_duplication_limit", error.to_string(), true)
                .with_remediation("close an application that is capturing this desktop")
        }
        CaptureError::AccessDenied => {
            SourceIssue::new("windows_desktop_access_denied", error.to_string(), true)
                .with_remediation("dismiss the secure desktop prompt or unlock the session")
        }
        CaptureError::SessionUnavailable => {
            SourceIssue::new("windows_session_unavailable", error.to_string(), true)
                .with_remediation("return to the interactive Windows session")
        }
        CaptureError::DeviceLost => {
            SourceIssue::new("windows_capture_device_lost", error.to_string(), true)
                .with_remediation("wait for the display driver to recover")
        }
        CaptureError::AccessLost => {
            SourceIssue::new("windows_desktop_access_lost", error.to_string(), true)
                .with_remediation("wait for the desktop transition to finish")
        }
        CaptureError::Timeout => {
            SourceIssue::new("windows_capture_timeout", error.to_string(), true)
        }
        CaptureError::MonitorNotFound { .. } | CaptureError::SourceNotFound { .. } => {
            SourceIssue::new("windows_capture_source_missing", error.to_string(), true)
                .with_remediation("select an attached display")
        }
        CaptureError::InvalidExtent { .. } => {
            SourceIssue::new("windows_capture_extent_invalid", error.to_string(), false)
        }
        CaptureError::ResourceExhausted { .. } | CaptureError::SessionResourceExhausted { .. } => {
            SourceIssue::new(
                "windows_capture_resource_exhausted",
                error.to_string(),
                true,
            )
        }
        CaptureError::ResourceAdmissionMismatch { .. } => SourceIssue::new(
            "windows_capture_resource_contract_invalid",
            error.to_string(),
            false,
        ),
        CaptureError::GeometryOverflow { .. } => SourceIssue::new(
            "windows_capture_geometry_overflow",
            error.to_string(),
            false,
        ),
        CaptureError::UnsupportedGpuSurface { .. }
        | CaptureError::DuplicateGpuSurfaceDescriptor { .. }
        | CaptureError::GpuSurfaceDescriptorNotPrepared { .. }
        | CaptureError::GpuSurfaceRegionOutOfBounds { .. }
        | CaptureError::GpuSurfaceRotationMismatch { .. }
        | CaptureError::GpuSurfaceInFlightDepthTooSmall { .. } => SourceIssue::new(
            "windows_capture_gpu_surface_contract_invalid",
            error.to_string(),
            false,
        ),
        CaptureError::GpuSurfaceUseUnavailable { .. }
        | CaptureError::GpuSurfaceCursorShapeUnavailable { .. }
        | CaptureError::GpuSurfacePlanInvalidated => SourceIssue::new(
            "windows_capture_gpu_surface_transient",
            error.to_string(),
            true,
        ),
        CaptureError::GpuSurfacePlanPoisoned { .. }
        | CaptureError::GpuSurfaceSynchronizationExhausted => SourceIssue::new(
            "windows_capture_gpu_surface_lifecycle_failed",
            error.to_string(),
            true,
        ),
        CaptureError::GpuSurfaceBudgetExceeded { .. } => SourceIssue::new(
            "windows_capture_gpu_surface_budget_exceeded",
            error.to_string(),
            true,
        ),
        CaptureError::GpuSurfaceFreshnessOverflow => SourceIssue::new(
            "windows_capture_gpu_surface_freshness_unrepresentable",
            error.to_string(),
            false,
        ),
        CaptureError::InvalidBufferGeometry { .. } => SourceIssue::new(
            "windows_capture_buffer_geometry_invalid",
            error.to_string(),
            true,
        ),
        CaptureError::UnsupportedPlatform | CaptureError::Windows { .. } => SourceIssue::new(
            "windows_desktop_duplication_unavailable",
            error.to_string(),
            true,
        ),
    }
}

fn screen_resource_issue(error: &ScreenAnalysisAdmissionError) -> SourceIssue {
    screen_analysis_admission_issue(error)
}

fn screen_analysis_admission_issue(error: &ScreenAnalysisAdmissionError) -> SourceIssue {
    let code = if matches!(
        error,
        ScreenAnalysisAdmissionError::ComputeCapacityExceeded { .. }
    ) {
        "windows_screen_analysis_compute_capacity_exceeded"
    } else {
        "windows_capture_resource_exhausted"
    };
    SourceIssue::new(code, error.to_string(), true)
}

fn reduction_issue(telemetry: &ReductionTelemetry) -> Option<SourceIssue> {
    telemetry.issue.as_ref().map(|issue| {
        SourceIssue::new(
            "windows_capture_gpu_reduction_degraded",
            format!("GPU reduction fell back to CPU: {issue}"),
            true,
        )
        .with_remediation("update the display driver or restart the capture session")
    })
}

fn record_capture_health(
    status: &SourceSessionWriter,
    captured_at: std::time::Instant,
    freshness_deadline: std::time::Instant,
    telemetry: &ReductionTelemetry,
) {
    status.publish_diagnostics(reduction_diagnostics(telemetry));
    if let Some(issue) = reduction_issue(telemetry) {
        let _ = status.record_degraded_sample(captured_at, freshness_deadline, 1, issue);
    } else {
        let _ = status.record_sample(captured_at, freshness_deadline, 1);
    }
}

fn reduction_diagnostics(telemetry: &ReductionTelemetry) -> SourceDiagnostics {
    let reduction_path = match telemetry.path {
        hypercolor_windows_capture::ReductionPath::Gpu => ScreenCaptureReductionPath::Gpu,
        hypercolor_windows_capture::ReductionPath::CpuFallback => {
            ScreenCaptureReductionPath::CpuFallback
        }
    };
    SourceDiagnostics::ScreenCapture(ScreenCaptureDiagnostics {
        reduction_path,
        gpu_submitted: telemetry.gpu_submitted,
        gpu_completed: telemetry.gpu_completed,
        cpu_completed: telemetry.cpu_completed,
        ring_busy: telemetry.ring_busy,
        readback_bytes: telemetry.readback_bytes,
        gpu_failures: telemetry.gpu_failures,
    })
}

#[cfg(test)]
mod tests;
