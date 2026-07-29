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

use std::collections::HashMap;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use hypercolor_windows_capture::{
    CaptureError, CaptureExtent as NativeCaptureExtent, CaptureLane, CapturePumpRequest,
    CaptureRegion, CpuDesktopFrame, DesktopDuplicator, DisplayRotation,
    Frame as NativeCaptureFrame, GpuAdapterLuid, GpuSurfaceAdmission, GpuSurfaceColorPipeline,
    GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy, GpuSurfaceDescriptor,
    GpuSurfaceDescriptorConfig, GpuSurfaceDescriptorId, GpuSurfaceFilter, GpuSurfaceFormat,
    GpuSurfacePlanGeneration, GpuSurfacePublicationDisposition, GpuSurfacePublishOutcome,
    GpuSurfaceSourceColorSpace, PreparedCpuDesktopReadback, PreparedGpuSurfacePlan,
    ReductionTelemetry,
};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::input::screen::{
    AnalyzedScreenSnapshot, BoundScreenNativeTargetPreparation, CaptureCadence,
    CaptureCadenceError, CaptureColorSpace, CaptureColorimetry, CaptureConfig, CaptureCursor,
    CaptureDamage, CaptureDynamicRange, CaptureEpoch, CaptureFrame, CaptureFrameMetadata,
    CaptureGeometry, CapturePacer, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, CpuReductionExecutor,
    PhysicalOrigin, PixelExtent, PlatformGpuApi, PlatformGpuSurface, PreparedCpuPublicationFanout,
    PreparedCpuPublicationFanoutCandidate, RawCaptureSurface, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenBackendResourceIdentity,
    ScreenBranchPayload, ScreenBranchPublisher, ScreenCaptureBackend, ScreenCaptureDemand,
    ScreenCaptureInput, ScreenColorTransformCapabilities, ScreenCursorCapabilities,
    ScreenCursorPolicy, ScreenExecutorColorCapabilities, ScreenGpuSurfacePayload,
    ScreenNativePreparationPayload, ScreenNativeTargetPreparation, ScreenPhysicalGpuDeviceIdentity,
    ScreenPreparedWorkerToken, ScreenPublicationColorimetry, ScreenPublicationExecutor,
    ScreenPublicationExecutorRequest, ScreenPublicationHealth, ScreenPublicationHub,
    ScreenPublicationKind, ScreenPublicationMetadata, ScreenReductionFilter, ScreenResourceApi,
    ScreenResourceKind, ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector,
    ScreenWorkerBinding, ScreenWorkerBindingState, ScreenWorkerExactLedgerBuilder,
    ScreenWorkerPreparation, ScreenWorkerPreparationTicket, ScreenWorkerRetirement, SourceScale,
    analyze_screen_frame,
};
use crate::input::status::{
    ScreenCaptureDiagnostics, ScreenCaptureReductionPath, SourceDiagnostics,
};
use crate::input::traits::{InputData, InputSource};
use crate::input::worker_retention::{retain_input_worker, spawn_input_worker};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceSessionWriter, SourceStatusHandle,
    SourceStatusReporter,
};
use crate::types::canvas::SurfaceResourceError;

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
    config: Mutex<VersionedCaptureConfig>,
    demand: Mutex<ScreenCaptureDemand>,
    generation: AtomicU64,
    session_generation: AtomicU64,
    activity_generation: AtomicU64,
}

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

#[derive(Default)]
struct ExactPublicationShared {
    source: Mutex<Option<WindowsPublicationSource>>,
    owned_sources: Mutex<Vec<CaptureSourceId>>,
    hub: Mutex<Option<Arc<ScreenPublicationHub>>>,
    cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
    resolution_revision: AtomicU64,
    next_descriptor_id: AtomicU64,
}

impl ExactPublicationShared {
    fn replace_source(&self, next: Option<WindowsPublicationSource>) {
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
            .expect("Windows screen publication resolution revision exhausted");
    }

    fn source(&self) -> Option<WindowsPublicationSource> {
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
                .contains(source_id)
    }

    fn replace_owned_sources(&self, sources: Vec<CaptureSourceId>) {
        *self
            .owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = sources;
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

struct CapturePublication<T> {
    source_generation: u64,
    activity_generation: u64,
    active: Option<ActiveCaptureEpoch>,
    latest: Option<T>,
}

impl<T> Default for CapturePublication<T> {
    fn default() -> Self {
        Self {
            source_generation: 0,
            activity_generation: 0,
            active: None,
            latest: None,
        }
    }
}

impl<T> CapturePublication<T> {
    fn activate(&mut self, active: ActiveCaptureEpoch) -> bool {
        if active.source_generation != self.source_generation
            || active.activity_generation != self.activity_generation
        {
            return false;
        }
        if self.active.as_ref() != Some(&active) {
            self.latest = None;
            self.active = Some(active);
        }
        true
    }

    fn fence_source(&mut self, source_generation: u64) {
        self.source_generation = source_generation;
        self.clear();
    }

    fn fence_activity(&mut self, activity_generation: u64) {
        self.activity_generation = activity_generation;
        self.clear();
    }

    fn clear(&mut self) {
        self.active = None;
        self.latest = None;
    }

    fn publish(&mut self, active: &ActiveCaptureEpoch, value: T) -> bool {
        if self.active.as_ref() != Some(active) {
            return false;
        }
        self.latest = Some(value);
        true
    }
}

impl SharedSettings {
    fn snapshot(&self) -> CaptureSettingsSnapshot {
        let config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        CaptureSettingsSnapshot {
            config: config.value.clone(),
            source_generation: config.source_generation,
            demand: *self
                .demand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
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
        {
            let mut config = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            config.value.clone_from(next_config);
            config.source_generation = source_generation;
        }
        *self
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = demand;
        self.generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
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
        let published = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .publish(&active, snapshot);
        if published && let Some(status) = self.status_session.load() {
            status.record_sample(captured_at, fresh_until, 1)?;
        }
        Ok(published)
    }
}

struct CaptureWorker {
    command_tx: mpsc::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    processed_activity_generation: Arc<AtomicU64>,
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
    routes: Vec<WindowsGpuRoute>,
}

struct PendingWindowsGpuRoute {
    id: GpuSurfaceDescriptorId,
    native: Arc<GpuSurfaceDescriptor>,
    descriptor: ResolvedScreenPublicationDescriptor,
    target: ScreenNativeTargetPreparation,
    capture_resource_name: Arc<str>,
    capture_allocation_byte_len: u64,
    requested_hz: NonZeroU32,
}

struct PendingWindowsGpuRuntime {
    plan: PreparedGpuSurfacePlan,
    routes: Vec<PendingWindowsGpuRoute>,
}

struct WindowsCpuRuntime {
    readback: PreparedCpuDesktopReadback,
    workspace_allocation_byte_len: u64,
    fanout_candidate: Option<PreparedCpuPublicationFanoutCandidate>,
    fanout: Option<PreparedCpuPublicationFanout>,
    latest_frame: Option<CaptureFrame<RawCaptureSurface>>,
}

struct WindowsExactRuntime {
    source: WindowsPublicationSource,
    binding: ScreenWorkerBinding,
    _lifetimes: Vec<ScreenResourceLifetime>,
    gpu: Option<WindowsGpuRuntime>,
    cpu: Option<WindowsCpuRuntime>,
}

impl WindowsExactRuntime {
    fn bind_if_active(&mut self, hub: &ScreenPublicationHub) -> anyhow::Result<()> {
        if self.binding.state() != ScreenWorkerBindingState::Active {
            return Ok(());
        }
        let authority = hub.committed_state();
        if authority.plan().generation() != self.binding.plan_generation()
            || authority.plan().demand_revision() != self.binding.demand_revision()
        {
            anyhow::bail!("Windows exact runtime authority does not match its worker binding");
        }
        if let Some(gpu) = &mut self.gpu {
            for route in &mut gpu.routes {
                if route.publisher.is_none() {
                    route.publisher = Some(authority.publisher(&route.descriptor, &self.binding)?);
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
        Ok(())
    }

    fn is_bound(&self) -> bool {
        self.binding.state() == ScreenWorkerBindingState::Active
            && self
                .gpu
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
        retain_input_worker(join_handle, "Windows screen capture worker");
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
    PrepareExact {
        ticket: ScreenWorkerPreparationTicket,
        cancelled: Arc<AtomicBool>,
        completion: oneshot::Sender<anyhow::Result<ScreenPreparedWorkerToken>>,
    },
    ReapExact {
        completion: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
    Stop,
}

enum SettingsDecision {
    Commit,
}

impl WindowsScreenCaptureInput {
    /// Create a new Windows screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            settings: Arc::new(SharedSettings {
                config: Mutex::new(VersionedCaptureConfig {
                    value: config,
                    source_generation: 0,
                }),
                demand: Mutex::new(ScreenCaptureDemand::Inactive),
                generation: AtomicU64::new(0),
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
        let state = Arc::new(WindowsScreenCaptureFixtureState {
            analyzer: Mutex::new(ScreenCaptureInput::new(config)),
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
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);

        let join_handle = spawn_input_worker(
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
        self.exact.replace_source(None);
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
        self.exact.replace_source(None);
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
        let analyzer = ScreenCaptureInput::with_requested_extent(config.clone(), requested_extent)?;
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
            self.publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fence_source(prepared.source_generation);
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
                let analyzer = ScreenCaptureInput::with_requested_extent(config, requested_extent)?;
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
        let previous_latest = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .clone();
        let activity_changed = previous.is_active() != demand.is_active();
        let activity_generation = if activity_changed {
            let generation = self
                .settings
                .activity_generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            self.publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fence_activity(generation);
            generation
        } else {
            self.settings.activity_generation.load(Ordering::Acquire)
        };
        *self
            .settings
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = demand;
        self.settings.generation.fetch_add(1, Ordering::Release);

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
            *self
                .settings
                .demand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = previous;
            self.settings.generation.fetch_add(1, Ordering::Release);
            if previous.is_active() {
                let rollback_generation = self
                    .settings
                    .activity_generation
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                self.publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fence_activity(rollback_generation);
                if let Err(rollback_error) = self.activate_backend(rollback_generation) {
                    return Err(error.context(format!(
                        "failed to restore previous Windows capture demand: {rollback_error}"
                    )));
                }
                self.publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .latest = previous_latest;
            } else {
                let mut publication = self
                    .publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                publication.active = None;
                publication.latest = previous_latest;
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
                self.exact.replace_source(None);
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
        *self
            .settings
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ScreenCaptureDemand::Inactive;
        self.settings.generation.fetch_add(1, Ordering::Release);
        self.shutdown_worker();

        #[cfg(feature = "windows-capture-fixtures")]
        if let Some(fixture) = self.fixture.as_ref() {
            *fixture
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }

        clear_capture_publication(&self.publication);
        self.exact.replace_source(None);
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
        let Some(active) = publication.active.as_ref() else {
            return Ok(InputData::None);
        };
        let Some(snapshot) = publication.latest.as_ref() else {
            return Ok(InputData::None);
        };
        if snapshot
            .geometry_frame()
            .validate_epoch(&active.epoch)
            .is_err()
        {
            return Ok(InputData::None);
        }
        Ok(InputData::Screen(snapshot.data().clone()))
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
        *self
            .exact
            .hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.exact.resolution_revision.load(Ordering::Acquire)
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
                anyhow!("Windows capture worker rejected exact publication preparation")
            })?;
        let abort_tx = worker.command_tx.clone();
        Ok(ScreenWorkerPreparation::with_abort(
            async move {
                completion_rx.await.map_err(|_| {
                    anyhow!("Windows capture worker exited during exact publication preparation")
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
                    "Windows capture worker rejected exact publication retirement"
                ))
            }));
        }
        Some(ScreenWorkerRetirement::new(async move {
            completion_rx.await.map_err(|_| {
                anyhow!("Windows capture worker exited during exact publication retirement")
            })?
        }))
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        self.reconfigure(config.clone())
    }
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
        source_rotation: display_rotation(source.rotation),
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

const fn display_rotation(rotation: CaptureRotation) -> DisplayRotation {
    match rotation {
        CaptureRotation::Identity => DisplayRotation::Identity,
        CaptureRotation::Clockwise90 => DisplayRotation::Clockwise90,
        CaptureRotation::Clockwise180 => DisplayRotation::Clockwise180,
        CaptureRotation::Clockwise270 => DisplayRotation::Clockwise270,
    }
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
    publication
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .activate(active)
}

fn clear_capture_publication(publication: &Mutex<CapturePublication<AnalyzedScreenSnapshot>>) {
    publication
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
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
) -> Result<ScreenCaptureInput, SurfaceResourceError> {
    let requested_extent = demand
        .requested_extent()
        .expect("an active Windows capture worker carries an extent");
    ScreenCaptureInput::with_requested_extent(config.clone(), requested_extent)
}

fn prepare_windows_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    duplicator: Option<&DesktopDuplicator>,
    source: Option<&WindowsPublicationSource>,
    exact: &ExactPublicationShared,
    hub: &ScreenPublicationHub,
) -> anyhow::Result<(ScreenPreparedWorkerToken, Option<WindowsExactRuntime>)> {
    let candidate = ticket.candidate_plan();
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
    let gpu = if gpu_branches.is_empty() {
        None
    } else {
        let preparation_gate = windows_gpu_preparation_gate(source.adapter_luid);
        let _preparation_guard = preparation_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let plan_generation = NonZeroU64::new(ticket.plan_generation().get())
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
        let plan = duplicator.prepare_gpu_surface_plan(plan_generation, &descriptors, admission)?;
        let mut routes = Vec::new();
        routes.try_reserve_exact(pending_routes.len())?;
        for (id, native, branch) in pending_routes {
            let ScreenPublicationExecutor::SourceNative(target) = branch.descriptor().executor()
            else {
                anyhow::bail!("Windows GPU route lost source-native target identity");
            };
            let manifest = Arc::new(plan.target_preparation(id)?);
            let capture_allocation_byte_len = manifest.allocation_byte_len();
            let platform = ScreenNativePreparationPayload::new(
                branch.descriptor(),
                ticket.plan_generation(),
                manifest,
            );
            let target = target.prepare(branch.descriptor(), &platform)?;
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

    let cpu_branch = source_branches.iter().copied().find(|branch| {
        matches!(
            branch.descriptor().executor(),
            ScreenPublicationExecutor::Cpu
        )
    });
    let cpu = if let Some(cpu_branch) = cpu_branch {
        let resolved_source = ResolvedScreenSource::new(
            ScreenSourceSelector::Exact(source.epoch.source_id.clone()),
            source.epoch.clone(),
            cpu_branch.descriptor().source().clone(),
        );
        let executor = exact.cpu_executor()?;
        let batch = executor.prepare_batch(&resolved_source, candidate)?;
        let workspace = batch.prepare_materialization_workspace(candidate)?;
        let workspace_allocation_byte_len = workspace.allocation_byte_len();
        let fanout_candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
            &executor, &batch, workspace, candidate,
        )?;
        let readback = duplicator.prepare_cpu_desktop_readback(slot_count)?;
        Some(WindowsCpuRuntime {
            readback,
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
        .map_or(0, |runtime| runtime.readback.allocation_byte_len());
    let workspace_bytes = cpu
        .as_ref()
        .map_or(0, |runtime| runtime.workspace_allocation_byte_len);
    let fanout_bytes = cpu.as_ref().map_or(0, |runtime| {
        runtime.fanout_candidate.as_ref().map_or(
            0,
            PreparedCpuPublicationFanoutCandidate::allocation_byte_len,
        )
    });
    let plane_minimum_bytes = ticket
        .required_minimums()
        .iter()
        .filter(|minimum| minimum.resource() == ScreenResourceKind::PhysicalPlane)
        .try_fold(0_u64, |total, minimum| {
            total
                .checked_add(minimum.minimum_bytes())
                .ok_or_else(|| anyhow!("Windows exact physical-plane accounting overflow"))
        })?;
    let reports = ticket
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
    let worker_metadata_bytes = workspace_bytes
        .saturating_sub(plane_minimum_bytes)
        .checked_add(u64::try_from(std::mem::size_of::<WindowsExactRuntime>()).unwrap_or(u64::MAX))
        .and_then(|bytes| {
            bytes.checked_add(
                u64::try_from(
                    gpu.as_ref()
                        .map_or(0, |runtime| runtime.routes.capacity())
                        .saturating_mul(std::mem::size_of::<WindowsGpuRoute>()),
                )
                .unwrap_or(u64::MAX),
            )
        })
        .ok_or_else(|| anyhow!("Windows exact worker allocation accounting overflow"))?;

    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
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
            let resource = route.target.exact_resource(
                format!("native-target-{}", route.id.get()),
                "worker-runtime-total",
            )?;
            ledger.report_native_target(resource)?;
        }
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
                routes,
            })
        })
        .transpose()?;
    Ok((
        token,
        Some(WindowsExactRuntime {
            source: source.clone(),
            binding,
            _lifetimes: lifetimes,
            gpu,
            cpu,
        }),
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

fn prepare_exact_command(
    ticket: ScreenWorkerPreparationTicket,
    cancelled: Arc<AtomicBool>,
    completion: oneshot::Sender<anyhow::Result<ScreenPreparedWorkerToken>>,
    duplicator: Option<&DesktopDuplicator>,
    exact: &ExactPublicationShared,
    runtimes: &mut Vec<WindowsExactRuntime>,
) {
    let result = exact
        .hub
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| anyhow!("Windows exact publication hub is unavailable"))
        .and_then(|hub| {
            let source = exact.source();
            prepare_windows_exact_runtime(ticket, duplicator, source.as_ref(), exact, hub.as_ref())
        });
    match result {
        Ok((token, runtime)) if !cancelled.load(Ordering::Acquire) => {
            if let Some(runtime) = runtime {
                runtimes.push(runtime);
                refresh_exact_runtime_ownership(runtimes, exact);
            }
            if completion.send(Ok(token)).is_err() {
                reap_exact_runtimes(runtimes, exact);
            }
        }
        Ok((_token, _runtime)) => {
            let _ = completion.send(Err(anyhow!(
                "Windows exact publication preparation was cancelled"
            )));
        }
        Err(error) => {
            let _ = completion.send(Err(error));
        }
    }
}

fn reap_exact_runtimes(runtimes: &mut Vec<WindowsExactRuntime>, exact: &ExactPublicationShared) {
    runtimes.retain(|runtime| runtime.binding.state() == ScreenWorkerBindingState::Active);
    refresh_exact_runtime_ownership(runtimes, exact);
}

fn refresh_exact_runtime_ownership(
    runtimes: &[WindowsExactRuntime],
    exact: &ExactPublicationShared,
) {
    let mut sources = runtimes
        .iter()
        .map(|runtime| runtime.source.epoch.source_id.clone())
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    sources.dedup();
    exact.replace_owned_sources(sources);
}

fn bind_current_exact_runtime<'a>(
    runtimes: &'a mut [WindowsExactRuntime],
    source: &WindowsPublicationSource,
    hub: &ScreenPublicationHub,
) -> anyhow::Result<Option<&'a mut WindowsExactRuntime>> {
    let mut selected = None;
    let mut selected_generation = None;
    for (index, runtime) in runtimes.iter_mut().enumerate() {
        if runtime.source != *source {
            continue;
        }
        runtime.bind_if_active(hub)?;
        let generation = runtime.binding.plan_generation();
        if runtime.is_bound() && selected_generation.is_none_or(|current| current < generation) {
            selected = Some(index);
            selected_generation = Some(generation);
        }
    }
    Ok(selected.map(|index| &mut runtimes[index]))
}

fn publish_windows_gpu_outcome(
    routes: &mut [WindowsGpuRoute],
    source: &WindowsPublicationSource,
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
        && provenance.plan_generation.get() == publisher.plan_generation().get()
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
    gpu_due
        .into_iter()
        .chain(cpu_due)
        .map(|deadline| deadline.saturating_duration_since(now))
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
    let mut cpu_needs_source = false;
    if let Some(cpu) = &mut runtime.cpu {
        let fanout = cpu
            .fanout
            .as_mut()
            .ok_or_else(|| anyhow!("Windows CPU fanout is not bound"))?;
        cpu_needs_source = fanout
            .publish_due(
                hub,
                cpu.latest_frame.as_ref(),
                now,
                ScreenPublicationHealth::Healthy,
            )?
            .needs_source()
            || cpu.readback.has_pending();
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
    if !gpu_requested && !cpu_needs_source {
        return Ok(exact_runtime_wait(runtime, now));
    }

    let source = runtime.source.clone();
    let mut gpu_error = None;
    let report = match (&mut runtime.gpu, &mut runtime.cpu) {
        (Some(gpu), Some(cpu)) if gpu_requested && cpu_needs_source => {
            let routes = &mut gpu.routes;
            session.pump_with_feedback(
                CapturePumpRequest::hybrid(&mut gpu.plan, &mut cpu.readback),
                FRAME_WAIT,
                |outcome| match publish_windows_gpu_outcome(routes, &source, hub, outcome) {
                    Ok(disposition) => disposition,
                    Err(error) => {
                        if gpu_error.is_none() {
                            gpu_error = Some(error);
                        }
                        GpuSurfacePublicationDisposition::Retry
                    }
                },
            )?
        }
        (Some(gpu), _) if gpu_requested => {
            let routes = &mut gpu.routes;
            session.pump_with_feedback(
                CapturePumpRequest::gpu(&mut gpu.plan),
                FRAME_WAIT,
                |outcome| match publish_windows_gpu_outcome(routes, &source, hub, outcome) {
                    Ok(disposition) => disposition,
                    Err(error) => {
                        if gpu_error.is_none() {
                            gpu_error = Some(error);
                        }
                        GpuSurfacePublicationDisposition::Retry
                    }
                },
            )?
        }
        (_, Some(cpu)) if cpu_needs_source => session.pump_with_feedback(
            CapturePumpRequest::cpu(&mut cpu.readback),
            FRAME_WAIT,
            |_| GpuSurfacePublicationDisposition::Accepted,
        )?,
        _ => return Ok(exact_runtime_wait(runtime, now)),
    };
    if let Some(error) = gpu_error {
        return Err(error);
    }
    if let CaptureLane::Failed(error) = report.gpu {
        return Err(error.into());
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
                .publish_due(
                    hub,
                    Some(&frame),
                    Instant::now(),
                    ScreenPublicationHealth::Healthy,
                )?;
            cpu.latest_frame = Some(frame);
        }
        CaptureLane::Busy => return Ok(READBACK_POLL_WAIT),
        CaptureLane::NotRequested | CaptureLane::Idle => {}
    }
    Ok(Duration::ZERO)
}

fn pump_current_windows_exact_runtime(
    session: &mut DesktopDuplicator,
    runtimes: &mut [WindowsExactRuntime],
    source: &WindowsPublicationSource,
    exact: &ExactPublicationShared,
) -> anyhow::Result<Option<Duration>> {
    let Some(hub) = exact.hub() else {
        return Ok(None);
    };
    let Some(runtime) = bind_current_exact_runtime(runtimes, source, &hub)? else {
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
    let mut generation = settings.generation.load(Ordering::Acquire);
    let mut analyzer = match build_worker_analyzer(&config, demand) {
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
    let mut failed_settings_generation = None;
    let mut settings_retry_at = Instant::now();
    let mut exact_runtimes = Vec::new();

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
            exact.replace_source(None);
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
                Ok(WorkerCommand::PrepareExact {
                    ticket,
                    cancelled,
                    completion,
                }) => prepare_exact_command(
                    ticket,
                    cancelled,
                    completion,
                    duplicator.as_ref(),
                    exact,
                    &mut exact_runtimes,
                ),
                Ok(WorkerCommand::ReapExact { completion }) => {
                    reap_exact_runtimes(&mut exact_runtimes, exact);
                    if let Some(completion) = completion {
                        let _ = completion.send(Ok(()));
                    }
                }
                Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            continue;
        }

        processed_activity_generation.store(activity_generation, Ordering::Release);

        let latest_generation = settings.generation.load(Ordering::Acquire);
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
            match build_worker_analyzer(&next_settings.config, next_settings.demand) {
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
                        exact.replace_source(None);
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
            match DesktopDuplicator::open(selector.clone(), native_capture_extent(requested_extent))
            {
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
                    exact.replace_source(None);
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
                        Ok(WorkerCommand::PrepareExact {
                            ticket,
                            cancelled,
                            completion,
                        }) => prepare_exact_command(
                            ticket,
                            cancelled,
                            completion,
                            duplicator.as_ref(),
                            exact,
                            &mut exact_runtimes,
                        ),
                        Ok(WorkerCommand::ReapExact { completion }) => {
                            reap_exact_runtimes(&mut exact_runtimes, exact);
                            if let Some(completion) = completion {
                                let _ = completion.send(Ok(()));
                            }
                        }
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
                exact.replace_source(None);
                duplicator = None;
                continue;
            }
        };
        exact.replace_source(Some(exact_source.clone()));

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
                    exact.replace_source(None);
                    duplicator = None;
                } else {
                    thread::sleep(FRAME_WAIT);
                }
                continue;
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
                exact.replace_source(None);
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
                    exact.replace_source(None);
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
                    Ok(publication
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .publish(&current_epoch, snapshot))
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
                exact.replace_source(None);
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
                    Ok(WorkerCommand::PrepareExact {
                        ticket,
                        cancelled,
                        completion,
                    }) => prepare_exact_command(
                        ticket,
                        cancelled,
                        completion,
                        duplicator.as_ref(),
                        exact,
                        &mut exact_runtimes,
                    ),
                    Ok(WorkerCommand::ReapExact { completion }) => {
                        reap_exact_runtimes(&mut exact_runtimes, exact);
                        if let Some(completion) = completion {
                            let _ = completion.send(Ok(()));
                        }
                    }
                    Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    clear_capture_publication(publication);
    exact.replace_source(None);
    exact.replace_owned_sources(Vec::new());
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
    exact_runtimes: &mut Vec<WindowsExactRuntime>,
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
            Ok(WorkerCommand::PrepareExact {
                ticket,
                cancelled,
                completion,
            }) => prepare_exact_command(
                ticket,
                cancelled,
                completion,
                duplicator.as_ref(),
                exact,
                exact_runtimes,
            ),
            Ok(WorkerCommand::ReapExact { completion }) => {
                reap_exact_runtimes(exact_runtimes, exact);
                if let Some(completion) = completion {
                    let _ = completion.send(Ok(()));
                }
            }
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
        publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fence_source(*source_generation);
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

fn screen_resource_issue(error: &SurfaceResourceError) -> SourceIssue {
    SourceIssue::new(
        "windows_capture_resource_exhausted",
        error.to_string(),
        true,
    )
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
