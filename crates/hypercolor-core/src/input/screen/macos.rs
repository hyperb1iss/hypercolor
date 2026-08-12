use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use hypercolor_macos_capture::{
    MacosCaptureColorimetry, MacosCaptureContentStyle, MacosCaptureFrame, MacosCapturePixelFormat,
    MacosCaptureSelection, MacosColorPrimaries, MacosDisplayClock, MacosFrameEvent,
    MacosFrameMailbox, MacosFrameStatus, MacosProtectedSourceState as NativeProtectedSourceState,
    MacosTransferFunction,
};
use tokio::sync::oneshot;

#[cfg(target_os = "macos")]
use hypercolor_macos_capture::{
    MacosCaptureCadence, MacosCaptureSelector, MacosScreenCaptureSession, MacosStreamRequest,
};

use super::{
    AdmittedScreenNativeTargetPreparation, BoundScreenNativeTargetPreparation, CaptureCadence,
    CaptureColorSpace, CaptureColorimetry, CaptureConfig, CaptureCursor, CaptureCursorContent,
    CaptureDamage, CaptureDynamicRange, CaptureEpoch, CaptureFrame, CaptureFrameMetadata,
    CapturePixelFormat, CapturePlanePool, CaptureRotation, CaptureSourceId, CaptureStorage,
    CaptureTransferFunction, CpuCaptureStorage, CpuExactReductionWorkPlan, CpuReductionExecutor,
    PixelExtent, PixelRect, PlatformGpuApi, PlatformGpuSurface, PreparedCpuPublicationFanout,
    PreparedCpuPublicationFanoutCandidate, RawCaptureSurface, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAnalysisComputeCapacity,
    ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan, ScreenBackendResourceIdentity,
    ScreenBranchPayload, ScreenBranchPublisher, ScreenByteAdmissionCoordinator,
    ScreenCaptureBackend, ScreenCaptureDemand, ScreenCaptureInput,
    ScreenColorTransformCapabilities, ScreenCursorCapabilities, ScreenExecutorColorCapabilities,
    ScreenGpuSurfacePayload, ScreenNativePreparationPayload, ScreenPhysicalGpuDeviceIdentity,
    ScreenPreparedWorkerToken, ScreenPublicationColorimetry, ScreenPublicationExecutor,
    ScreenPublicationExecutorRequest, ScreenPublicationHealth, ScreenPublicationHub,
    ScreenPublicationHubError, ScreenPublicationMetadata, ScreenRequiredResourceMinimum,
    ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime, ScreenSourceReflection,
    ScreenSourceSelector, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale, analyze_screen_frame,
};
#[cfg(target_os = "macos")]
use super::{ScreenByteAdmissionError, ScreenByteLease};
use crate::input::status::SourceSessionSlot;
use crate::input::traits::{
    InputData, InputSource, ProtectedSourceAuthorizationAction, ScreenSourcePickerAction,
};
use crate::input::{
    MacosAuthorizationState, MacosCapabilityOwner, MacosProtectedSourceState,
    MacosScreenPlatformStatus, MacosSelectionState, SourceKind, SourcePlatformStatus,
    SourceStatusHandle, SourceStatusReporter,
};

const WORKER_WAIT: Duration = Duration::from_millis(100);

#[cfg(target_os = "macos")]
struct MacosCapturePoolAdmission {
    lease: Arc<ScreenByteLease>,
    metadata_bytes: u64,
    observed: Vec<(u32, u64)>,
}

#[cfg(target_os = "macos")]
impl MacosCapturePoolAdmission {
    fn reserve(
        coordinator: &ScreenByteAdmissionCoordinator,
        conservative_surface_bytes: u64,
        native_metadata_bytes: u64,
    ) -> Result<Self, hypercolor_macos_capture::MacosCaptureError> {
        let tracking_bytes = u64::try_from(std::mem::size_of::<(u32, u64)>())
            .ok()
            .and_then(|bytes| {
                bytes.checked_mul(
                    u64::try_from(hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH).ok()?,
                )
            })
            .and_then(|bytes| bytes.checked_add(u64::try_from(std::mem::size_of::<Self>()).ok()?))
            .and_then(|bytes| {
                bytes.checked_add(u64::try_from(std::mem::size_of::<ScreenByteLease>()).ok()?)
            })
            .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        let metadata_bytes = native_metadata_bytes
            .checked_add(tracking_bytes)
            .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        let surface_bytes = conservative_surface_bytes
            .checked_mul(
                u64::try_from(hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH)
                    .map_err(|_| hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?,
            )
            .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        let total_bytes = surface_bytes
            .checked_add(metadata_bytes)
            .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        let reservation = coordinator
            .try_acquire(total_bytes)
            .map_err(map_macos_pool_admission_error)?;
        let mut observed = Vec::new();
        observed
            .try_reserve_exact(hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH)
            .map_err(
                |_| hypercolor_macos_capture::MacosCaptureError::ScreenResourceExhausted {
                    requested_bytes: tracking_bytes,
                    available_bytes: 0,
                },
            )?;
        Ok(Self {
            lease: Arc::new(reservation.freeze()),
            metadata_bytes,
            observed,
        })
    }

    fn observe(
        &mut self,
        iosurface_id: u32,
        allocation_bytes: u64,
    ) -> Result<Arc<ScreenByteLease>, hypercolor_macos_capture::MacosCaptureError> {
        if iosurface_id == 0 || allocation_bytes == 0 {
            return Err(hypercolor_macos_capture::MacosCaptureError::InvalidSurface);
        }
        let existing = self
            .observed
            .iter()
            .position(|(observed_id, _)| *observed_id == iosurface_id);
        if existing.is_none()
            && self.observed.len() == hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH
        {
            return Err(
                hypercolor_macos_capture::MacosCaptureError::ScreenResourceExhausted {
                    requested_bytes: allocation_bytes,
                    available_bytes: 0,
                },
            );
        }
        let observed_count = self.observed.len() + usize::from(existing.is_none());
        let mut observed_sum = 0_u64;
        let mut observed_max = allocation_bytes;
        for (index, (_, observed_bytes)) in self.observed.iter().enumerate() {
            let bytes = if Some(index) == existing {
                allocation_bytes
            } else {
                *observed_bytes
            };
            observed_sum = observed_sum
                .checked_add(bytes)
                .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
            observed_max = observed_max.max(bytes);
        }
        if existing.is_none() {
            observed_sum = observed_sum
                .checked_add(allocation_bytes)
                .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        }
        let unseen_count = hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH - observed_count;
        let projected_unseen = observed_max
            .checked_mul(
                u64::try_from(unseen_count)
                    .map_err(|_| hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?,
            )
            .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        let exact_bytes = self
            .metadata_bytes
            .checked_add(observed_sum)
            .and_then(|bytes| bytes.checked_add(projected_unseen))
            .ok_or(hypercolor_macos_capture::MacosCaptureError::ArithmeticOverflow)?;
        self.lease
            .try_reconcile_exact(exact_bytes)
            .map_err(map_macos_pool_admission_error)?;
        if let Some(index) = existing {
            self.observed[index].1 = allocation_bytes;
        } else {
            self.observed.push((iosurface_id, allocation_bytes));
        }
        Ok(Arc::clone(&self.lease))
    }

    #[cfg(test)]
    fn exact_observed_pool_bytes(&self) -> u64 {
        self.observed.iter().map(|(_, bytes)| bytes).sum()
    }

    #[cfg(test)]
    fn metadata_bytes(&self) -> u64 {
        self.metadata_bytes
    }

    #[cfg(test)]
    fn reservation_variance(&self) -> u64 {
        self.lease
            .bytes()
            .saturating_sub(self.metadata_bytes)
            .saturating_sub(self.exact_observed_pool_bytes())
    }
}

#[cfg(target_os = "macos")]
fn map_macos_pool_admission_error(
    error: ScreenByteAdmissionError,
) -> hypercolor_macos_capture::MacosCaptureError {
    let (requested_bytes, available_bytes) = match error {
        ScreenByteAdmissionError::CapacityExceeded {
            requested_bytes,
            available_bytes,
        } => (requested_bytes, available_bytes),
        ScreenByteAdmissionError::CapacityShrinkRejected {
            requested_capacity,
            reserved_bytes,
        } => (reserved_bytes, requested_capacity),
        ScreenByteAdmissionError::RevisionExhausted => (u64::MAX, 0),
    };
    hypercolor_macos_capture::MacosCaptureError::ScreenResourceExhausted {
        requested_bytes,
        available_bytes,
    }
}

/// Descriptor-keyed source data passed to the daemon-owned Metal target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacosNativeTargetManifest {
    capture_session_generation: u64,
    resource_generation: u64,
    metal_registry_id: u64,
}

impl MacosNativeTargetManifest {
    fn new(descriptor: &ResolvedScreenPublicationDescriptor) -> anyhow::Result<Self> {
        let resources = descriptor.physical().source().resources();
        let ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(metal_registry_id) = resources
            .physical_gpu_device()
            .ok_or_else(|| anyhow!("macOS native publication is missing Metal identity"))?
        else {
            return Err(anyhow!(
                "macOS native publication selected a non-Metal device"
            ));
        };
        if *metal_registry_id == 0
            || resources.device_generation() == 0
            || resources.resource_generation() == 0
        {
            return Err(anyhow!(
                "macOS native publication generations must be nonzero"
            ));
        }
        Ok(Self {
            capture_session_generation: resources.device_generation(),
            resource_generation: resources.resource_generation(),
            metal_registry_id: *metal_registry_id,
        })
    }

    /// Capture-session generation whose surfaces this target accepts.
    #[must_use]
    pub const fn capture_session_generation(&self) -> u64 {
        self.capture_session_generation
    }

    /// Storage-descriptor generation whose surfaces this target accepts.
    #[must_use]
    pub const fn resource_generation(&self) -> u64 {
        self.resource_generation
    }

    /// Physical Metal device registry identity.
    #[must_use]
    pub const fn metal_registry_id(&self) -> u64 {
        self.metal_registry_id
    }
}

trait MacosCaptureControl: Send + Sync {
    fn mailbox(&self) -> MacosFrameMailbox;
    fn set_active(&self, active: bool);
    fn present_picker(&self) -> anyhow::Result<()>;
    fn request_authorization(&self) -> NativeProtectedSourceState;
    fn status(&self) -> NativeProtectedSourceState;
    fn selection(&self) -> MacosCaptureSelection;
    fn authorization(&self) -> MacosAuthorizationState;
    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant>;
}

#[cfg(target_os = "macos")]
struct NativeCaptureControl {
    session: MacosScreenCaptureSession,
    clock: MacosDisplayClock,
}

#[cfg(target_os = "macos")]
impl MacosCaptureControl for NativeCaptureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.session.mailbox()
    }

    fn set_active(&self, active: bool) {
        self.session.set_capture_active(active);
    }

    fn present_picker(&self) -> anyhow::Result<()> {
        self.session.present_picker().map_err(anyhow::Error::from)
    }

    fn request_authorization(&self) -> NativeProtectedSourceState {
        self.session.request_authorization()
    }

    fn status(&self) -> NativeProtectedSourceState {
        self.session.status()
    }

    fn selection(&self) -> MacosCaptureSelection {
        self.session.selection()
    }

    fn authorization(&self) -> MacosAuthorizationState {
        if MacosScreenCaptureSession::screen_authorized() {
            MacosAuthorizationState::Authorized
        } else if self.session.status() == NativeProtectedSourceState::PermissionDenied {
            MacosAuthorizationState::Denied
        } else {
            MacosAuthorizationState::NotDetermined
        }
    }

    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant> {
        self.clock
            .timestamp(display_time)
            .map_err(anyhow::Error::from)
    }
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
    cursor_composed: bool,
}

impl MacosPublicationSource {
    fn from_frame(
        source_id: CaptureSourceId,
        topology_generation: u64,
        resource_generation: u64,
        frame: &MacosCaptureFrame,
    ) -> anyhow::Result<Self> {
        let storage_extent =
            PixelExtent::new(frame.storage_extent.width, frame.storage_extent.height)?;
        let content = frame.geometry.content_rect_pixels;
        let content_x = u32::try_from(content.x)?;
        let content_y = u32::try_from(content.y)?;
        let content_rect = PixelRect::new(content_x, content_y, content.width, content.height)?;
        let crop = (content_x != 0
            || content_y != 0
            || content.width != storage_extent.width()
            || content.height != storage_extent.height())
        .then_some(content_rect);
        Ok(Self {
            epoch: CaptureEpoch {
                source_id,
                topology_generation,
                session_generation: frame.epoch,
            },
            geometry: super::CaptureGeometry::new(
                capture_origin(frame)?,
                storage_extent,
                storage_extent,
                CaptureRotation::Identity,
                crop,
                SourceScale::ONE,
            )?,
            logical_extent: content_rect.extent(),
            colorimetry: capture_colorimetry(frame.color)?,
            pixel_format: frame.pixel_format,
            resource_generation,
            allocation_bytes: frame.surface.allocation_bytes,
            cursor_composed: frame.cursor_composed,
        })
    }

    fn matches_selector(&self, selector: &ScreenSourceSelector) -> bool {
        match selector {
            ScreenSourceSelector::Configured | ScreenSourceSelector::Primary => true,
            ScreenSourceSelector::Exact(source_id) => source_id == &self.epoch.source_id,
        }
    }

    fn cursor_capabilities(&self) -> ScreenCursorCapabilities {
        if self.cursor_composed {
            ScreenCursorCapabilities::composed_only()
        } else {
            ScreenCursorCapabilities::clean_only()
        }
    }

    fn cpu_source(&self, selector: ScreenSourceSelector) -> ResolvedScreenSource {
        ResolvedScreenSource::new(
            selector,
            self.epoch.clone(),
            ResolvedScreenSourceConfig::new_with_cursor_capabilities(
                self.geometry,
                self.logical_extent,
                ScreenSourceReflection::None,
                CapturePixelFormat::Rgba8,
                CaptureColorimetry::SRGB,
                self.cursor_capabilities(),
                ScreenBackendResourceIdentity::new(
                    ScreenCaptureBackend::MacosScreenCaptureKit,
                    ScreenResourceApi::Cpu,
                    self.epoch.session_generation,
                    self.resource_generation,
                ),
            ),
        )
    }

    fn gpu_source(
        &self,
        selector: ScreenSourceSelector,
        physical_gpu_device: ScreenPhysicalGpuDeviceIdentity,
    ) -> anyhow::Result<ResolvedScreenSource> {
        let ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(registry_id) = physical_gpu_device
        else {
            return Err(anyhow!("macOS capture requires a Metal execution target"));
        };
        if registry_id == 0 {
            return Err(anyhow!(
                "macOS capture received a zero Metal registry identity"
            ));
        }
        let pixel_format = match self.pixel_format {
            MacosCapturePixelFormat::Bgra8 => CapturePixelFormat::Bgra8,
            _ => {
                return Err(anyhow!(
                    "macOS native capture format is not implemented yet"
                ));
            }
        };
        Ok(ResolvedScreenSource::new(
            selector,
            self.epoch.clone(),
            ResolvedScreenSourceConfig::new_with_cursor_capabilities(
                self.geometry,
                self.logical_extent,
                ScreenSourceReflection::None,
                pixel_format,
                self.colorimetry,
                self.cursor_capabilities(),
                ScreenBackendResourceIdentity::new_with_physical_gpu_device(
                    ScreenCaptureBackend::MacosScreenCaptureKit,
                    ScreenResourceApi::PlatformGpu(PlatformGpuApi::Metal),
                    ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(registry_id),
                    self.epoch.session_generation,
                    self.resource_generation,
                ),
            ),
        ))
    }
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
    resolution_revision: AtomicU64,
}

impl MacosExactPublicationShared {
    fn replace_source(&self, next: Option<MacosPublicationSource>) {
        let mut source = lock(&self.source);
        if *source == next {
            return;
        }
        *source = next;
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("macOS screen publication resolution revision exhausted");
    }

    fn source(&self) -> Option<MacosPublicationSource> {
        lock(&self.source).clone()
    }

    fn hub(&self) -> Option<Arc<ScreenPublicationHub>> {
        lock(&self.hub).clone()
    }

    fn owns_source(&self, source_id: &CaptureSourceId) -> bool {
        self.source()
            .is_some_and(|source| &source.epoch.source_id == source_id)
            || lock(&self.owned_sources)
                .iter()
                .any(|source| &source.source_id == source_id)
    }

    fn register_owned_source(&self, source: MacosOwnedSource) {
        lock(&self.owned_sources).push(source);
    }

    fn reap_owned_sources(&self) {
        let authority = self.hub().map(|hub| hub.committed_state());
        lock(&self.owned_sources).retain(|source| {
            authority
                .as_ref()
                .is_some_and(|authority| authority.owns_runtime_binding(&source.binding))
        });
    }

    fn clear_owned_sources(&self) {
        lock(&self.owned_sources).clear();
    }

    fn cpu_executor(&self) -> anyhow::Result<Arc<CpuReductionExecutor>> {
        let mut executor = lock(&self.cpu_executor);
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

impl MacosExactRuntime {
    fn bind_if_current(&mut self, hub: &ScreenPublicationHub) -> anyhow::Result<()> {
        let authority = hub.committed_state();
        if !authority.owns_runtime_binding(&self.binding) {
            return Ok(());
        }
        match self.binding.state() {
            ScreenWorkerBindingState::Active | ScreenWorkerBindingState::Retired => {}
            ScreenWorkerBindingState::Prepared | ScreenWorkerBindingState::Armed => return Ok(()),
            ScreenWorkerBindingState::Aborted => {
                return Err(anyhow!("macOS exact runtime was aborted after commit"));
            }
        }
        for route in &mut self.native_routes {
            if route.publisher.is_none() {
                route.publisher =
                    Some(authority.publisher_for_runtime(&route.descriptor, &self.binding)?);
            }
        }
        if self.fanout.is_none()
            && let Some(candidate) = self.fanout_candidate.take()
        {
            self.fanout = Some(candidate.bind(&authority, &self.binding)?);
        }
        Ok(())
    }

    fn is_bound(&self) -> bool {
        self.native_routes
            .iter()
            .all(|route| route.publisher.is_some())
            && self.fanout_candidate.is_none()
    }
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

pub struct MacosScreenCaptureInput {
    config: CaptureConfig,
    control: Arc<dyn MacosCaptureControl>,
    admission: ScreenByteAdmissionCoordinator,
    publication: Arc<Mutex<MacosPublication>>,
    exact: Arc<MacosExactPublicationShared>,
    worker: Option<CaptureWorker>,
    worker_generation: u64,
    demand: ScreenCaptureDemand,
    running: bool,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    owner: MacosCapabilityOwner,
}

impl MacosScreenCaptureInput {
    #[cfg(target_os = "macos")]
    pub fn new(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> anyhow::Result<Self> {
        let request = MacosStreamRequest::new(
            MacosCaptureCadence::FramesPerSecond(config.target_fps),
            false,
        )?;
        let selector = MacosCaptureSelector::parse(&config.source)?;
        let pool_coordinator = admission.clone();
        let session = MacosScreenCaptureSession::new_with_pool_admission(
            request,
            selector,
            move |conservative_surface_bytes, native_metadata_bytes| {
                let pool = Arc::new(Mutex::new(MacosCapturePoolAdmission::reserve(
                    &pool_coordinator,
                    conservative_surface_bytes,
                    native_metadata_bytes,
                )?));
                Ok(move |iosurface_id, allocation_bytes| {
                    let lease = lock(&pool).observe(iosurface_id, allocation_bytes)?;
                    Ok(lease as Arc<dyn Send + Sync>)
                })
            },
        )?;
        let clock = MacosDisplayClock::system()?;
        Ok(Self::with_control(
            config,
            admission,
            Arc::new(NativeCaptureControl { session, clock }),
        ))
    }

    fn with_control(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        control: Arc<dyn MacosCaptureControl>,
    ) -> Self {
        let consented = control.authorization() == MacosAuthorizationState::Authorized;
        let mut source = Self {
            config,
            control,
            admission,
            publication: Arc::new(Mutex::new(MacosPublication::default())),
            exact: Arc::new(MacosExactPublicationShared::default()),
            worker: None,
            worker_generation: 0,
            demand: ScreenCaptureDemand::Inactive,
            running: false,
            status: SourceStatusReporter::new(
                "macos:session",
                SourceKind::Screen,
                "screen_capture_kit_cpu",
                true,
                consented,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            owner: MacosCapabilityOwner::Standalone,
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
        self.status
            .set_platform(Some(SourcePlatformStatus::MacosScreen(
                MacosScreenPlatformStatus {
                    state: map_protected_state(state),
                    tcc: self.control.authorization(),
                    owner: self.owner,
                    selection: map_selection(self.control.selection()),
                    tahoe_selection: None,
                    owner_conflict: None,
                },
            )))?;
        Ok(())
    }

    fn refresh_policy(&mut self) -> anyhow::Result<()> {
        self.refresh_policy_for(self.demand)
    }

    fn refresh_policy_for(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let consented = self.control.authorization() == MacosAuthorizationState::Authorized;
        self.status
            .set_policy(true, consented, demand.is_active())?;
        Ok(())
    }

    fn prepare_worker(&self, extent: PixelExtent) -> anyhow::Result<PreparedWorker> {
        let mut analyzer = ScreenCaptureInput::with_requested_extent_and_admission(
            self.config.clone(),
            extent,
            self.admission.clone(),
        )?;
        analyzer.start()?;
        Ok(PreparedWorker {
            analyzer,
            plane_pool: CapturePlanePool::with_admission_coordinator(self.admission.clone()),
            target_fps: self.config.target_fps,
        })
    }

    fn install_worker(&mut self, prepared: PreparedWorker) -> anyhow::Result<()> {
        let worker_generation = self
            .worker_generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("macOS capture worker generation exhausted"))?;
        let mailbox = self.control.mailbox();
        let worker_mailbox = mailbox.clone();
        let control = Arc::clone(&self.control);
        let publication = Arc::clone(&self.publication);
        let exact = Arc::clone(&self.exact);
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
                        worker_generation,
                        target_fps,
                        status_session,
                        worker_stop,
                        control,
                        command_rx,
                    )
                };
                let _ = exit_tx.send(result);
            })?;
        let previous_latest = lock(&self.publication).latest.clone();
        self.stop_worker();
        self.worker_generation = worker_generation;
        {
            let mut publication = lock(&self.publication);
            publication.worker_generation = worker_generation;
            publication.latest = previous_latest;
        }
        self.worker = Some(CaptureWorker {
            stop,
            mailbox: worker_mailbox,
            command_tx,
            exit_rx,
            join: Some(join),
        });
        start.store(true, Ordering::Release);
        self.worker
            .as_ref()
            .and_then(|worker| worker.join.as_ref())
            .expect("installed worker retains its thread handle")
            .thread()
            .unpark();
        Ok(())
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
            let prepared = self.prepare_worker(extent)?;
            let session = self.status.begin_session()?;
            if let Err(error) = self.install_worker(prepared) {
                self.status.stop();
                return Err(error);
            }
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

    fn is_screen_source(&self) -> bool {
        true
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
        None
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let prepared = demand
            .requested_extent()
            .map(|extent| self.prepare_worker(extent))
            .transpose()?;
        let was_active = self.demand.is_active();
        if !self.running {
            self.refresh_policy_for(demand)?;
            self.demand = demand;
            return Ok(());
        }
        if let Some(prepared) = prepared {
            let session = if was_active {
                None
            } else {
                self.refresh_policy_for(demand)?;
                self.status.begin_session()?
            };
            if let Err(error) = self.install_worker(prepared) {
                if !was_active {
                    self.refresh_policy_for(self.demand)?;
                }
                return Err(error);
            }
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        } else {
            self.control.set_active(false);
            self.status_session.clear();
            self.stop_worker();
            self.refresh_policy_for(demand)?;
        }
        self.demand = demand;
        self.refresh_platform_status()?;
        Ok(())
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
            return Ok(None);
        };
        resolve_macos_publication_branch(&source, demand)
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
        let prepared = self
            .demand
            .requested_extent()
            .map(|extent| {
                let mut analyzer = ScreenCaptureInput::with_requested_extent_and_admission(
                    config.clone(),
                    extent,
                    self.admission.clone(),
                )?;
                analyzer.start()?;
                Ok::<_, anyhow::Error>(PreparedWorker {
                    analyzer,
                    plane_pool: CapturePlanePool::with_admission_coordinator(
                        self.admission.clone(),
                    ),
                    target_fps: config.target_fps,
                })
            })
            .transpose()?;
        if self.running
            && let Some(prepared) = prepared
        {
            self.install_worker(prepared)?;
        }
        self.config.clone_from(config);
        Ok(())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.present_picker()
    }

    fn screen_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        let control = Arc::clone(&self.control);
        Some(Arc::new(move || {
            control.request_authorization();
            Ok(control.authorization() == MacosAuthorizationState::Authorized)
        }))
    }

    fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        let control = Arc::clone(&self.control);
        Some(Arc::new(move || control.present_picker()))
    }
}

fn resolve_macos_publication_branch(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let selector = demand.request().selector();
    if !source.matches_selector(selector) {
        return Ok(None);
    }
    let selector = selector.clone();
    let capabilities = ScreenColorTransformCapabilities::new(true, false, false, NonZeroU32::MIN);
    if matches!(
        demand.request().executor(),
        ScreenPublicationExecutorRequest::Cpu
    ) {
        return Ok(Some(demand.resolve_with_color_capabilities(
            &source.cpu_source(selector),
            capabilities,
        )?));
    }

    let ScreenPublicationExecutorRequest::SourceNative(target) = demand.request().executor() else {
        unreachable!("screen publication executor requests are exhaustive");
    };
    if target.accepted_api() == &PlatformGpuApi::Metal
        && let Ok(native_source) =
            source.gpu_source(selector.clone(), target.physical_gpu_device().clone())
        && let Ok(resolved) = demand.resolve_with_executor_capabilities(
            &native_source,
            ScreenExecutorColorCapabilities::new(capabilities, capabilities),
        )
        && matches!(
            resolved.descriptor().executor(),
            ScreenPublicationExecutor::SourceNative(_)
        )
        && macos_native_descriptor_is_identity(resolved.descriptor(), source)
        && MacosNativeTargetManifest::new(resolved.descriptor()).is_ok()
    {
        return Ok(Some(resolved));
    }

    Ok(Some(demand.resolve_with_color_capabilities(
        &source.cpu_source(selector),
        capabilities,
    )?))
}

fn macos_native_descriptor_is_identity(
    descriptor: &ResolvedScreenPublicationDescriptor,
    source: &MacosPublicationSource,
) -> bool {
    source.geometry.crop().is_none()
        && descriptor.geometry().output_extent() == source.geometry.storage_extent()
        && descriptor.physical().reduction_extent() == source.geometry.storage_extent()
        && descriptor.physical().target_pixel_format() == CapturePixelFormat::Bgra8
        && matches!(
            descriptor.physical().color_pipeline().transform(),
            ResolvedScreenColorTransform::PreserveEncodedSamples
        )
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

fn checked_macos_metadata_bytes<T>(count: usize, resource: &str) -> anyhow::Result<u64> {
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(std::mem::size_of::<T>())
                .ok()
                .and_then(|size| count.checked_mul(size))
        })
        .ok_or_else(|| anyhow!("macOS exact {resource} metadata accounting overflow"))
}

fn preflight_macos_scope_bytes(
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

fn prepare_macos_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    source: Option<&MacosPublicationSource>,
    exact: &MacosExactPublicationShared,
) -> anyhow::Result<(
    ScreenPreparedWorkerToken,
    Option<(MacosExactRuntime, MacosOwnedSource)>,
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
        .ok_or_else(|| anyhow!("macOS exact publication source changed before preparation"))?;
    let cpu_source = source.cpu_source(ScreenSourceSelector::Exact(source.epoch.source_id.clone()));
    let executor = exact.cpu_executor()?;
    let compute_plan =
        CpuExactReductionWorkPlan::try_for_source(&candidate, ticket.source_id(), |_| true)?;
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
                .ok_or_else(|| anyhow!("macOS exact physical-plane accounting overflow"))
        })?;
    let runtime_metadata_bytes = checked_macos_metadata_bytes::<MacosExactRuntime>(1, "runtime")?
        .checked_add(checked_macos_metadata_bytes::<MacosOwnedSource>(
            1,
            "owned source",
        )?)
        .and_then(|bytes| {
            bytes.checked_add(
                checked_macos_metadata_bytes::<MacosNativeRoute>(
                    source_branches.len(),
                    "native routes",
                )
                .ok()?,
            )
        })
        .ok_or_else(|| anyhow!("macOS exact runtime metadata accounting overflow"))?;
    preflight_macos_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        runtime_metadata_bytes,
    )?;

    let (fanout_candidate, fanout_bytes, workspace_bytes) = if compute_plan.cpu_reduction_count()
        == 0
    {
        (None, 0, 0)
    } else {
        let batch_quote = executor.batch_allocation_quote(&cpu_source, &candidate)?;
        preflight_macos_scope_bytes(&mut ledger, &mut processing_minimum_remaining, batch_quote)?;
        let batch = executor.prepare_batch(&cpu_source, &candidate)?;
        let workspace_quote = batch.materialization_workspace_allocation_quote(&candidate)?;
        let workspace_additional_bytes = workspace_quote
            .checked_sub(plane_minimum_bytes)
            .ok_or_else(|| anyhow!("macOS workspace quote understates physical-plane minima"))?;
        preflight_macos_scope_bytes(
            &mut ledger,
            &mut worker_minimum_remaining,
            workspace_additional_bytes,
        )?;
        let workspace = batch.prepare_materialization_workspace(&candidate)?;
        let workspace_bytes = workspace.allocation_byte_len();
        let fanout_quote = PreparedCpuPublicationFanout::candidate_allocation_quote(
            &batch, &workspace, &candidate,
        )?;
        let fanout_additional_bytes = fanout_quote
            .checked_sub(batch_quote)
            .ok_or_else(|| anyhow!("macOS fanout quote understates retained batch backing"))?;
        preflight_macos_scope_bytes(
            &mut ledger,
            &mut processing_minimum_remaining,
            fanout_additional_bytes,
        )?;
        let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
            &executor, &batch, workspace, &candidate,
        )?;
        let bytes = candidate.allocation_byte_len();
        (Some(candidate), bytes, workspace_bytes)
    };

    let mut pending_native = Vec::new();
    pending_native.try_reserve_exact(source_branches.len())?;
    for (index, branch) in source_branches.iter().enumerate() {
        let ScreenPublicationExecutor::SourceNative(target) = branch.descriptor().executor() else {
            continue;
        };
        let manifest = Arc::new(MacosNativeTargetManifest::new(branch.descriptor())?);
        let platform = ScreenNativePreparationPayload::new(
            branch.descriptor(),
            ledger.ticket().plan_generation(),
            manifest,
        );
        let resource_name: Arc<str> = Arc::from(format!("macos-native-target-{index}"));
        let capture_resource_name: Arc<str> = Arc::from(format!("macos-native-capture-{index}"));
        let prepared = ledger.prepare_native_target(
            target,
            branch.descriptor(),
            &platform,
            Arc::clone(&resource_name),
            "worker-runtime-total",
        )?;
        ledger.preflight_additional_bytes(source.allocation_bytes)?;
        ledger.report_scoped(
            &capture_resource_name,
            "worker-runtime-total",
            source.allocation_bytes,
        )?;
        pending_native.push(PendingMacosNativeRoute {
            resource_name,
            capture_resource_name,
            descriptor: branch.descriptor().clone(),
            target: prepared,
            requested_hz: branch.requested_hz(),
        });
    }

    let processing_scope = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::ProcessingProfileState)
        .map(|minimum| Arc::clone(minimum.name()));
    if fanout_bytes > 0 && processing_scope.is_none() {
        ledger.report_scoped("macos-cpu-fanout", "worker-runtime-total", fanout_bytes)?;
    }
    let expected_lifetime_count = ledger.prospective_resource_count()?;
    let lifetime_metadata_bytes = checked_macos_metadata_bytes::<ScreenResourceLifetime>(
        expected_lifetime_count,
        "runtime lifetimes",
    )?;
    preflight_macos_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        lifetime_metadata_bytes,
    )?;
    let worker_metadata_bytes = workspace_bytes
        .saturating_sub(plane_minimum_bytes)
        .checked_add(runtime_metadata_bytes)
        .and_then(|bytes| bytes.checked_add(lifetime_metadata_bytes))
        .ok_or_else(|| anyhow!("macOS exact worker accounting overflow"))?;
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| {
            (
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Vec<_>>();
    for (name, resource, minimum) in &reports {
        let actual = match resource {
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
    if exact_ledger.lifetimes().len() != expected_lifetime_count {
        return Err(anyhow!(
            "macOS exact lifetime metadata changed during preparation"
        ));
    }
    let binding = exact_ledger.token().binding().clone();
    let (token, lifetimes) = exact_ledger.into_parts();
    let mut native_routes = Vec::new();
    native_routes.try_reserve_exact(pending_native.len())?;
    for pending in pending_native {
        let lifetime = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name() == &pending.resource_name)
            .cloned()
            .ok_or_else(|| anyhow!("macOS native target lifetime is missing"))?;
        let capture_lifetime = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name() == &pending.capture_resource_name)
            .cloned()
            .ok_or_else(|| anyhow!("macOS native capture lifetime is missing"))?;
        native_routes.push(MacosNativeRoute {
            descriptor: pending.descriptor,
            target: pending.target.bind(lifetime)?,
            capture_lifetime,
            pacer: CaptureCadence::new(pending.requested_hz.get())?.pacer(),
            next_publish_at: Instant::now(),
            last_accepted_sequence: None,
            publisher: None,
        });
    }
    let runtime_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "worker-runtime-total")
        .cloned()
        .ok_or_else(|| anyhow!("macOS worker runtime lifetime is missing"))?;
    Ok((
        token,
        Some((
            MacosExactRuntime {
                source: source.clone(),
                binding: binding.clone(),
                _lifetimes: lifetimes,
                native_routes: native_routes.into_boxed_slice(),
                fanout_candidate,
                fanout: None,
            },
            MacosOwnedSource {
                source_id: source.epoch.source_id.clone(),
                binding,
                _runtime_lifetime: runtime_lifetime,
            },
        )),
    ))
}

fn reap_macos_exact_runtimes(
    runtimes: &mut Vec<MacosExactRuntime>,
    exact: &MacosExactPublicationShared,
) {
    exact.reap_owned_sources();
    let authority = exact.hub().map(|hub| hub.committed_state());
    runtimes.retain(|runtime| {
        authority
            .as_ref()
            .is_some_and(|authority| authority.owns_runtime_binding(&runtime.binding))
    });
}

fn bind_current_macos_exact_runtime<'a>(
    runtimes: &'a mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    hub: &ScreenPublicationHub,
) -> anyhow::Result<Option<&'a mut MacosExactRuntime>> {
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
    Ok(runtime.is_bound().then_some(runtime))
}

fn handle_exact_commands(
    command_rx: &mpsc::Receiver<WorkerCommand>,
    runtimes: &mut Vec<MacosExactRuntime>,
    exact: &MacosExactPublicationShared,
) {
    while let Ok(command) = command_rx.try_recv() {
        match command {
            WorkerCommand::PrepareExact {
                ticket,
                cancelled,
                completion,
            } => {
                if cancelled.load(Ordering::Acquire) {
                    let _ = completion.send(Err(anyhow!(
                        "macOS exact publication preparation was cancelled"
                    )));
                    continue;
                }
                let source = exact.source();
                match prepare_macos_exact_runtime(ticket, source.as_ref(), exact) {
                    Ok((token, runtime)) if !cancelled.load(Ordering::Acquire) => {
                        if let Some((runtime, owned_source)) = runtime {
                            exact.register_owned_source(owned_source);
                            runtimes.push(runtime);
                        }
                        if completion.send(Ok(token)).is_err() {
                            reap_macos_exact_runtimes(runtimes, exact);
                        }
                    }
                    Ok((_token, _runtime)) => {
                        let _ = completion.send(Err(anyhow!(
                            "macOS exact publication preparation was cancelled"
                        )));
                    }
                    Err(error) => {
                        let _ = completion.send(Err(error));
                    }
                }
            }
            WorkerCommand::ReapExact { completion } => {
                reap_macos_exact_runtimes(runtimes, exact);
                if let Some(completion) = completion {
                    let _ = completion.send(Ok(()));
                }
            }
        }
    }
}

fn run_worker(
    mut prepared: PreparedWorker,
    mailbox: MacosFrameMailbox,
    publication: Arc<Mutex<MacosPublication>>,
    exact: Arc<MacosExactPublicationShared>,
    worker_generation: u64,
    target_fps: u32,
    status_session: SourceSessionSlot,
    stop: Arc<AtomicBool>,
    control: Arc<dyn MacosCaptureControl>,
    command_rx: mpsc::Receiver<WorkerCommand>,
) -> anyhow::Result<()> {
    let mut topology = TopologyState::default();
    let mut resources = ResourceState::default();
    let mut exact_runtimes = Vec::new();
    while !stop.load(Ordering::Acquire) {
        handle_exact_commands(&command_rx, &mut exact_runtimes, &exact);
        let Some(delivery) =
            mailbox.wait_latest_while(WORKER_WAIT, || !stop.load(Ordering::Acquire))
        else {
            continue;
        };
        match delivery {
            Ok(MacosFrameEvent::Frame(frame)) => {
                publish_frame(
                    &mut prepared,
                    Arc::from(frame),
                    capture_source_id(control.selection())?,
                    &mut topology,
                    &mut resources,
                    &publication,
                    &exact,
                    &mut exact_runtimes,
                    worker_generation,
                    target_fps,
                    &status_session,
                    &control,
                )?;
            }
            Ok(MacosFrameEvent::Lifecycle(
                MacosFrameStatus::Suspended | MacosFrameStatus::Stopped,
            ))
            | Err(_) => lock(&publication).latest = None,
            Ok(MacosFrameEvent::Lifecycle(_)) => {}
            Ok(MacosFrameEvent::RecoverableError(_)) => {}
        }
    }
    exact.replace_source(None);
    exact.clear_owned_sources();
    exact_runtimes.clear();
    prepared.analyzer.stop();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn publish_frame(
    prepared: &mut PreparedWorker,
    frame: Arc<MacosCaptureFrame>,
    source_id: CaptureSourceId,
    topology: &mut TopologyState,
    resources: &mut ResourceState,
    publication: &Mutex<MacosPublication>,
    exact: &MacosExactPublicationShared,
    exact_runtimes: &mut [MacosExactRuntime],
    worker_generation: u64,
    target_fps: u32,
    status_session: &SourceSessionSlot,
    control: &Arc<dyn MacosCaptureControl>,
) -> anyhow::Result<()> {
    let extent = PixelExtent::new(frame.storage_extent.width, frame.storage_extent.height)?;
    let captured_at = control.captured_at(frame.display_time)?;
    let fresh_until = captured_at
        .checked_add(Duration::from_nanos(
            2_000_000_000_u64.div_ceil(u64::from(target_fps)),
        ))
        .ok_or_else(|| anyhow!("macOS capture freshness deadline overflow"))?;
    let topology_generation = topology.observe(&frame)?;
    let resource_generation = resources.observe(&frame)?;
    let source = MacosPublicationSource::from_frame(
        source_id.clone(),
        topology_generation,
        resource_generation,
        &frame,
    )?;
    exact.replace_source(Some(source.clone()));
    let exact_delivery = publish_macos_native_exact(
        &frame,
        captured_at,
        fresh_until,
        &source,
        exact,
        exact_runtimes,
    )?;
    if exact_delivery.native && !exact_delivery.cpu {
        if lock(publication).worker_generation == worker_generation {
            lock(publication).latest = None;
        }
        if let Some(status) = status_session.load() {
            status.record_sample(captured_at, fresh_until, 1)?;
        }
        return Ok(());
    }

    let row_stride = usize::try_from(extent.width())
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| anyhow!("macOS capture row stride overflow"))?;
    let byte_len = row_stride
        .checked_mul(usize::try_from(extent.height())?)
        .ok_or_else(|| anyhow!("macOS capture plane length overflow"))?;
    let mut plane = prepared.plane_pool.try_acquire(byte_len)?;
    plane.resize(byte_len, 0);
    frame.convert_bgra8_sdr_to_rgba8(&mut plane, row_stride)?;
    let cursor = CaptureCursor {
        visible: frame.cursor_composed,
        position: None,
        hotspot: None,
        shape_extent: None,
        shape_generation: None,
        content: if frame.cursor_composed {
            CaptureCursorContent::Composed
        } else {
            CaptureCursorContent::Hidden
        },
    };
    let damage = CaptureDamage::new(
        frame
            .damage
            .iter()
            .map(|rect| {
                Ok(PixelRect::new(
                    u32::try_from(rect.x)?,
                    u32::try_from(rect.y)?,
                    rect.width,
                    rect.height,
                )?)
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        Vec::new(),
    );
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let capture = CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry: source.geometry,
            colorimetry: CaptureColorimetry::SRGB,
            cursor,
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            plane.freeze(),
            CapturePixelFormat::Rgba8,
            i64::try_from(row_stride)?,
            0,
        )),
        damage,
    )?;
    publish_macos_cpu_exact(&capture, &source, exact, exact_runtimes)?;
    let snapshot = analyze_screen_frame(&mut prepared.analyzer, capture)?;
    if snapshot.geometry_frame().metadata().topology_generation != topology_generation {
        return Err(anyhow!("macOS analysis changed topology generation"));
    }
    let data = Arc::new(InputData::Screen(snapshot.data().clone()));
    if lock(publication).worker_generation != worker_generation {
        return Ok(());
    }
    if let Some(status) = status_session.load() {
        status.record_sample(captured_at, fresh_until, 1)?;
    }
    {
        let mut publication = lock(publication);
        if publication.worker_generation != worker_generation {
            return Ok(());
        }
        publication.latest = Some(data);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MacosExactDelivery {
    native: bool,
    cpu: bool,
}

fn publish_macos_native_exact(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
) -> anyhow::Result<MacosExactDelivery> {
    let Some(hub) = exact.hub() else {
        return Ok(MacosExactDelivery::default());
    };
    let Some(runtime) = bind_current_macos_exact_runtime(runtimes, source, &hub)? else {
        return Ok(MacosExactDelivery::default());
    };
    let delivery = MacosExactDelivery {
        native: !runtime.native_routes.is_empty(),
        cpu: runtime.fanout.is_some(),
    };
    let published_at = Instant::now();
    if published_at > fresh_until {
        return Ok(delivery);
    }
    let native_sequence = frame
        .sequence
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    for route in &mut runtime.native_routes {
        if published_at < route.next_publish_at
            || route
                .last_accepted_sequence
                .is_some_and(|accepted| frame.sequence <= accepted)
        {
            continue;
        }
        let publisher = route
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow!("macOS native route has no committed publisher"))?;
        let surface = PlatformGpuSurface::new(
            PlatformGpuApi::Metal,
            u64::from(frame.surface.iosurface_id),
            route.descriptor.geometry().output_extent(),
            route.descriptor.physical().target_pixel_format(),
            Arc::clone(frame),
        )?;
        let surface = route
            .target
            .retain_on_surface_with_capture_allocation(surface, route.capture_lifetime.clone())?;
        let metadata = ScreenPublicationMetadata::try_new(
            source.epoch.clone(),
            publisher.plan_generation(),
            native_sequence,
            captured_at,
            published_at,
            fresh_until,
            ScreenPublicationHealth::Healthy,
        )?;
        let payload = ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
            ScreenPublicationColorimetry::new(
                route.descriptor.physical().color_pipeline().output(),
            ),
            &surface,
        ));
        match hub.publish(publisher, payload, &metadata) {
            Ok(_) => {
                route.last_accepted_sequence = Some(frame.sequence);
                route.next_publish_at = route
                    .pacer
                    .advance_deadline(route.next_publish_at, published_at)?;
            }
            Err(ScreenPublicationHubError::PublicationPressure { .. }) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(delivery)
}

fn publish_macos_cpu_exact(
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) = bind_current_macos_exact_runtime(runtimes, source, &hub)? else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        fanout.publish_due(
            &hub,
            Some(frame),
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct TopologyState {
    descriptor: Option<TopologyDescriptor>,
    generation: u64,
}

impl TopologyState {
    fn observe(&mut self, frame: &MacosCaptureFrame) -> anyhow::Result<u64> {
        let descriptor = TopologyDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS topology generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

#[derive(Default)]
struct ResourceState {
    descriptor: Option<ResourceDescriptor>,
    generation: u64,
}

impl ResourceState {
    fn observe(&mut self, frame: &MacosCaptureFrame) -> anyhow::Result<u64> {
        let descriptor = ResourceDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS resource generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResourceDescriptor {
    width: u32,
    height: u32,
    pixel_format: MacosCapturePixelFormat,
    planes: Vec<(u32, u32, u32, usize, u64)>,
}

impl ResourceDescriptor {
    fn from_frame(frame: &MacosCaptureFrame) -> Self {
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            pixel_format: frame.pixel_format,
            planes: frame
                .planes
                .iter()
                .map(|plane| {
                    (
                        plane.index,
                        plane.extent.width,
                        plane.extent.height,
                        plane.bytes_per_row,
                        plane.length_bytes,
                    )
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopologyDescriptor {
    width: u32,
    height: u32,
    content: (i64, i64, u32, u32),
    scale_bits: u64,
    screen: Option<(u64, u64, u64, u64)>,
}

impl TopologyDescriptor {
    fn from_frame(frame: &MacosCaptureFrame) -> Self {
        let content = frame.geometry.content_rect_pixels;
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            content: (content.x, content.y, content.width, content.height),
            scale_bits: frame.geometry.display_scale_factor.get().to_bits(),
            screen: frame.geometry.screen_rect_points.map(|rect| {
                (
                    rect.x.to_bits(),
                    rect.y.to_bits(),
                    rect.width.to_bits(),
                    rect.height.to_bits(),
                )
            }),
        }
    }
}

fn capture_source_id(selection: MacosCaptureSelection) -> anyhow::Result<CaptureSourceId> {
    let source: Arc<str> = match selection {
        MacosCaptureSelection::Display { source_id } => source_id,
        MacosCaptureSelection::SessionScoped { content_style } => Arc::from(match content_style {
            MacosCaptureContentStyle::Window => "macos:session:window",
            MacosCaptureContentStyle::MultipleWindows => "macos:session:multiple-windows",
            MacosCaptureContentStyle::Application => "macos:session:application",
            MacosCaptureContentStyle::MultipleApplications => "macos:session:multiple-applications",
            MacosCaptureContentStyle::Mixed => "macos:session:mixed",
        }),
        MacosCaptureSelection::None => Arc::from("macos:session"),
    };
    Ok(CaptureSourceId::new(source)?)
}

fn capture_colorimetry(color: MacosCaptureColorimetry) -> anyhow::Result<CaptureColorimetry> {
    let color_space = match color.primaries {
        MacosColorPrimaries::Srgb => CaptureColorSpace::Srgb,
        MacosColorPrimaries::DisplayP3 => CaptureColorSpace::DisplayP3,
        MacosColorPrimaries::Rec2020 => CaptureColorSpace::Rec2020,
    };
    let (transfer_function, dynamic_range) = match color.transfer {
        MacosTransferFunction::Srgb => {
            (CaptureTransferFunction::Srgb, CaptureDynamicRange::Standard)
        }
        MacosTransferFunction::Linear => (
            CaptureTransferFunction::Linear,
            CaptureDynamicRange::Standard,
        ),
        MacosTransferFunction::Pq => (CaptureTransferFunction::Pq, CaptureDynamicRange::High),
        MacosTransferFunction::Hlg => (CaptureTransferFunction::Hlg, CaptureDynamicRange::High),
        MacosTransferFunction::Rec709 | MacosTransferFunction::Rec2020 => (
            CaptureTransferFunction::Unknown,
            CaptureDynamicRange::Standard,
        ),
    };
    Ok(CaptureColorimetry::new(
        color_space,
        transfer_function,
        Some(dynamic_range),
        None,
    )?)
}

fn capture_origin(frame: &MacosCaptureFrame) -> anyhow::Result<super::PhysicalOrigin> {
    let rect = frame
        .geometry
        .screen_rect_points
        .unwrap_or(frame.geometry.content_rect_points);
    let scale = frame.geometry.display_scale_factor.get();
    Ok(super::PhysicalOrigin {
        x: scaled_coordinate(rect.x, scale)?,
        y: scaled_coordinate(rect.y, scale)?,
    })
}

fn scaled_coordinate(value: f64, scale: f64) -> anyhow::Result<i32> {
    let value = (value * scale).floor();
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(anyhow!("macOS capture origin exceeds i32"));
    }
    Ok(value as i32)
}

const fn map_protected_state(state: NativeProtectedSourceState) -> MacosProtectedSourceState {
    match state {
        NativeProtectedSourceState::Disabled => MacosProtectedSourceState::Disabled,
        NativeProtectedSourceState::NeedsUserAction => MacosProtectedSourceState::NeedsUserAction,
        NativeProtectedSourceState::PermissionDenied => MacosProtectedSourceState::PermissionDenied,
        NativeProtectedSourceState::NeedsProcessRestart => {
            MacosProtectedSourceState::NeedsProcessRestart
        }
        NativeProtectedSourceState::NeedsSelection => MacosProtectedSourceState::NeedsSelection,
        NativeProtectedSourceState::ReadyIdle => MacosProtectedSourceState::ReadyIdle,
        NativeProtectedSourceState::Starting => MacosProtectedSourceState::Starting,
        NativeProtectedSourceState::Live => MacosProtectedSourceState::Live,
        NativeProtectedSourceState::Interrupted => MacosProtectedSourceState::Interrupted,
        NativeProtectedSourceState::Revoked => MacosProtectedSourceState::Revoked,
        NativeProtectedSourceState::Failed => MacosProtectedSourceState::Failed,
    }
}

fn map_selection(selection: MacosCaptureSelection) -> MacosSelectionState {
    match selection {
        MacosCaptureSelection::None => MacosSelectionState::None,
        MacosCaptureSelection::Display { source_id } => MacosSelectionState::Display { source_id },
        MacosCaptureSelection::SessionScoped { content_style } => {
            let content_style = match content_style {
                MacosCaptureContentStyle::Window => "window",
                MacosCaptureContentStyle::MultipleWindows => "multiple_windows",
                MacosCaptureContentStyle::Application => "application",
                MacosCaptureContentStyle::MultipleApplications => "multiple_applications",
                MacosCaptureContentStyle::Mixed => "mixed",
            };
            MacosSelectionState::SessionScoped {
                content_style: Arc::from(content_style),
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(feature = "macos-capture-fixtures")]
struct FixtureControl {
    mailbox: MacosFrameMailbox,
    active: AtomicBool,
    status: Mutex<NativeProtectedSourceState>,
    selection: Mutex<MacosCaptureSelection>,
    captured_at: Mutex<Option<Instant>>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl Default for FixtureControl {
    fn default() -> Self {
        Self {
            mailbox: MacosFrameMailbox::default(),
            active: AtomicBool::new(false),
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            selection: Mutex::new(MacosCaptureSelection::None),
            captured_at: Mutex::new(None),
        }
    }
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosCaptureControl for FixtureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.mailbox.clone()
    }

    fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
        *lock(&self.status) = if active {
            NativeProtectedSourceState::Starting
        } else {
            NativeProtectedSourceState::ReadyIdle
        };
    }

    fn present_picker(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn request_authorization(&self) -> NativeProtectedSourceState {
        *lock(&self.status) = NativeProtectedSourceState::NeedsSelection;
        NativeProtectedSourceState::NeedsSelection
    }

    fn status(&self) -> NativeProtectedSourceState {
        *lock(&self.status)
    }

    fn selection(&self) -> MacosCaptureSelection {
        lock(&self.selection).clone()
    }

    fn authorization(&self) -> MacosAuthorizationState {
        match self.status() {
            NativeProtectedSourceState::PermissionDenied | NativeProtectedSourceState::Revoked => {
                MacosAuthorizationState::Denied
            }
            NativeProtectedSourceState::NeedsUserAction => MacosAuthorizationState::NotDetermined,
            NativeProtectedSourceState::Disabled => MacosAuthorizationState::Unknown,
            _ => MacosAuthorizationState::Authorized,
        }
    }

    fn captured_at(&self, _display_time: u64) -> anyhow::Result<Instant> {
        Ok(lock(&self.captured_at).take().unwrap_or_else(Instant::now))
    }
}

#[cfg(feature = "macos-capture-fixtures")]
pub struct MacosScreenCaptureFixture {
    control: Arc<FixtureControl>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosScreenCaptureFixture {
    pub fn source(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> (MacosScreenCaptureInput, Self) {
        let control = Arc::new(FixtureControl {
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            ..FixtureControl::default()
        });
        let source = MacosScreenCaptureInput::with_control(config, admission, control.clone());
        (source, Self { control })
    }

    pub fn publish(&self, frame: MacosCaptureFrame) {
        *lock(&self.control.status) = NativeProtectedSourceState::Live;
        self.control
            .mailbox
            .publish(Ok(MacosFrameEvent::Frame(Box::new(frame))));
    }

    pub fn publish_at(&self, frame: MacosCaptureFrame, captured_at: Instant) {
        *lock(&self.control.captured_at) = Some(captured_at);
        self.publish(frame);
    }

    pub fn publish_recoverable_error(&self, error: hypercolor_macos_capture::MacosCaptureError) {
        self.control
            .mailbox
            .publish(Ok(MacosFrameEvent::RecoverableError(Box::new(error))));
    }

    pub fn is_active(&self) -> bool {
        self.control.active.load(Ordering::Acquire)
    }

    pub fn set_selection(&self, selection: MacosCaptureSelection) {
        *lock(&self.control.selection) = selection;
    }
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
mod tests {
    use super::*;
    use crate::input::screen::{
        InputPublicationDemandRevision, ScreenAdmissionCapacity, ScreenAspectPolicy,
        ScreenExtentRequest, ScreenInputGraphGeneration, ScreenNativeExecutionTarget,
        ScreenNativeExecutionTargetId, ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
        ScreenPlanBuilder, ScreenProcessingProfile, ScreenProcessingProfileConfig,
        ScreenPublicationKind, ScreenPublicationRequest,
    };
    use hypercolor_macos_capture::{
        MacosAttachment, MacosCaptureSurface, MacosColorRange, MacosFrameDecoder, MacosPixelExtent,
        MacosPointRect, MacosRawCapturePlane, MacosRawCaptureSample, MacosRawCompleteFrame,
        MacosRawFrameAttachments,
    };

    const BGRA8: u32 = 0x4247_5241;

    #[cfg(target_os = "macos")]
    #[test]
    fn capture_pool_rebases_before_exposing_an_observed_surface() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let mut pool = MacosCapturePoolAdmission::reserve(&coordinator, 100, 32)
            .expect("conservative queue quote should fit");
        let initial = pool.lease.bytes();
        assert!(initial >= 8 * 100 + 32);

        let first = pool
            .observe(1, 120)
            .expect("first exact pool observation should fit");
        assert_eq!(first.bytes(), pool.metadata_bytes() + 8 * 120);
        assert_eq!(coordinator.snapshot().reserved_bytes(), first.bytes());
        assert_eq!(pool.exact_observed_pool_bytes(), 120);
        assert_eq!(pool.reservation_variance(), 7 * 120);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn capture_pool_collapses_to_exact_sum_after_all_slots_are_observed() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let mut pool = MacosCapturePoolAdmission::reserve(&coordinator, 128, 64)
            .expect("conservative queue quote should fit");
        let allocations = [112_u64, 128, 144, 160, 176, 192, 208, 224];
        for (index, allocation) in allocations.into_iter().enumerate() {
            pool.observe(
                u32::try_from(index + 1).expect("fixture id fits"),
                allocation,
            )
            .expect("exact slot observation should fit");
        }
        let exact_sum: u64 = allocations.into_iter().sum();
        assert_eq!(pool.exact_observed_pool_bytes(), exact_sum);
        assert_eq!(pool.reservation_variance(), 0);
        assert_eq!(pool.lease.bytes(), pool.metadata_bytes() + exact_sum);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn capture_pool_rejects_larger_surface_without_recording_or_rebasing() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_200, 1_200));
        let mut pool = MacosCapturePoolAdmission::reserve(&coordinator, 100, 32)
            .expect("conservative queue quote should fit");
        let reserved_before = coordinator.snapshot().reserved_bytes();

        assert!(matches!(
            pool.observe(1, 200),
            Err(hypercolor_macos_capture::MacosCaptureError::ScreenResourceExhausted { .. })
        ));
        assert_eq!(pool.exact_observed_pool_bytes(), 0);
        assert_eq!(pool.lease.bytes(), reserved_before);
        assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retained_surface_lifetime_keeps_the_pool_admitted_after_stream_drop() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
        let mut pool = MacosCapturePoolAdmission::reserve(&coordinator, 100, 32)
            .expect("conservative queue quote should fit");
        let retained = pool
            .observe(1, 120)
            .expect("first exact pool observation should fit");
        let admitted_bytes = retained.bytes();

        drop(pool);
        assert_eq!(coordinator.snapshot().reserved_bytes(), admitted_bytes);
        drop(retained);
        assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn candidate_pool_reserves_alongside_a_pinned_old_generation() {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(2_000, 2_000));
        let mut old = MacosCapturePoolAdmission::reserve(&coordinator, 100, 32)
            .expect("old stream quote should fit");
        let pinned = old
            .observe(1, 120)
            .expect("old stream observation should fit");
        drop(old);

        assert!(matches!(
            MacosCapturePoolAdmission::reserve(&coordinator, 100, 32),
            Err(hypercolor_macos_capture::MacosCaptureError::ScreenResourceExhausted { .. })
        ));
        assert_eq!(coordinator.snapshot().reserved_bytes(), pinned.bytes());
    }

    #[derive(Debug)]
    struct TestPreparedTarget;

    struct TestTargetPreparer;

    impl ScreenNativeTargetPreparer for TestTargetPreparer {
        fn quote_retained_bytes(
            &self,
            _descriptor: &ResolvedScreenPublicationDescriptor,
            platform: &ScreenNativePreparationPayload,
        ) -> anyhow::Result<u64> {
            MacosNativeTargetManifest::new(platform.descriptor())?;
            Ok(0)
        }

        fn prepare(
            &self,
            descriptor: &ResolvedScreenPublicationDescriptor,
            platform: &ScreenNativePreparationPayload,
        ) -> anyhow::Result<ScreenNativeTargetPreparation> {
            MacosNativeTargetManifest::new(platform.descriptor())?;
            Ok(ScreenNativeTargetPreparation::new(
                ScreenNativePreparationPayload::new(
                    descriptor,
                    platform.plan_generation(),
                    Arc::new(TestPreparedTarget),
                ),
                0,
            ))
        }
    }

    fn frame() -> Arc<MacosCaptureFrame> {
        let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
        let surface = MacosCaptureSurface::new_cpu_fixture(
            7,
            32,
            1,
            vec![Arc::<[u8]>::from([0_u8, 0, 255, 255].repeat(8))],
        )
        .expect("fixture surface is valid");
        let sample = MacosRawCaptureSample {
            frame: Some(MacosRawCompleteFrame {
                storage_extent: extent,
                planes: vec![MacosRawCapturePlane {
                    index: 0,
                    extent,
                    bytes_per_row: 16,
                    length_bytes: 32,
                }],
                pixel_format_fourcc: BGRA8,
                color: MacosCaptureColorimetry {
                    primaries: MacosColorPrimaries::Srgb,
                    transfer: MacosTransferFunction::Srgb,
                    matrix: None,
                    range: MacosColorRange::Full,
                    chroma_location: None,
                },
                cursor_composed: false,
                surface,
            }),
            attachments: MacosRawFrameAttachments {
                status: MacosAttachment::Value(0),
                display_time: MacosAttachment::Value(1_000),
                display_scale_factor: MacosAttachment::Value(1.0),
                content_scale: MacosAttachment::Value(1.0),
                content_rect: MacosAttachment::Value(
                    MacosPointRect::new(0.0, 0.0, 4.0, 2.0).expect("fixture content rect is valid"),
                ),
                dirty_rects: MacosAttachment::Missing,
                screen_rect: MacosAttachment::Missing,
                bounding_rect: MacosAttachment::Missing,
            },
        };
        let mut decoder = MacosFrameDecoder::new(7);
        let MacosFrameEvent::Frame(frame) = decoder.decode(sample).expect("fixture frame decodes")
        else {
            panic!("complete fixture sample produces a frame");
        };
        Arc::from(frame)
    }

    fn source(frame: &MacosCaptureFrame) -> MacosPublicationSource {
        MacosPublicationSource::from_frame(
            CaptureSourceId::new("display:test").expect("fixture source id is valid"),
            3,
            5,
            frame,
        )
        .expect("fixture source resolves")
    }

    fn target() -> ScreenNativeExecutionTarget {
        ScreenNativeExecutionTarget::new(
            ScreenNativeExecutionTargetId::new(NonZeroU64::new(11).expect("nonzero target")),
            PlatformGpuApi::Metal,
            ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(91),
            NonZeroU32::new(16_384).expect("nonzero texture limit"),
            Arc::new(TestTargetPreparer),
        )
    }

    fn native_demand(target: &ScreenNativeExecutionTarget) -> RegisteredScreenBranchDemand {
        RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenPublicationExecutorRequest::SourceNative(target.clone()),
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Contain,
                Arc::new(ScreenProcessingProfile::new(
                    ScreenProcessingProfileConfig::exact_encoded_identity(
                        CapturePixelFormat::Bgra8,
                    ),
                )),
            ),
            NonZeroU32::new(60).expect("nonzero cadence"),
        )
    }

    #[test]
    fn native_publication_commits_owner_backed_metal_surface() {
        let frame = frame();
        let source = source(&frame);
        let demand = native_demand(&target());
        let resolved = resolve_macos_publication_branch(&source, &demand)
            .expect("native demand resolves")
            .expect("configured macOS source owns native demand");
        assert!(matches!(
            resolved.descriptor().executor(),
            ScreenPublicationExecutor::SourceNative(_)
        ));

        let exact = MacosExactPublicationShared::default();
        exact.replace_source(Some(source.clone()));
        let mut builder = ScreenPlanBuilder::new();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let revision = InputPublicationDemandRevision::new(1);
        let graph = ScreenInputGraphGeneration::new(1);
        let mut preparing = builder
            .prepare(
                [resolved],
                None,
                revision,
                graph,
                ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
            )
            .expect("native candidate plan prepares");
        let ticket = preparing
            .worker_ticket(&source.epoch.source_id)
            .expect("macOS source owns its worker ticket");
        let (token, runtime) = prepare_macos_exact_runtime(ticket, Some(&source), &exact)
            .expect("native runtime prepares");
        let (runtime, owned_source) = runtime.expect("native branch owns a runtime");
        exact.register_owned_source(owned_source);
        let mut runtimes = vec![runtime];
        preparing
            .acknowledge(token)
            .expect("native worker token matches candidate");
        let armed = preparing
            .arm(builder.current().generation(), revision, graph)
            .unwrap_or_else(|failure| panic!("native plan arms: {}", failure.error()));
        let committed = builder
            .commit(armed, revision, graph)
            .unwrap_or_else(|failure| panic!("native plan commits: {}", failure.error()));
        let (_, retirement) = committed.into_parts();
        retirement
            .try_reclaim()
            .expect("initial plan has no retired readers");

        let now = Instant::now();
        publish_macos_native_exact(
            &frame,
            now,
            now + Duration::from_secs(1),
            &source,
            &exact,
            &mut runtimes,
        )
        .expect("native frame publishes");
        let hub = exact.hub().expect("test hub remains installed");
        let (_, lease) = hub.observe_matching_lease(|_| true);
        let publication = lease
            .expect("committed native branch has a lease")
            .read()
            .expect("native branch has a publication");
        assert_eq!(publication.native_sequence(), NonZeroU64::MIN);
        let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
            panic!("macOS native branch publishes a GPU surface");
        };
        let surface = payload.surface();
        assert_eq!(surface.api(), &PlatformGpuApi::Metal);
        assert_eq!(surface.handle_id(), 7);
        assert_eq!(surface.format(), CapturePixelFormat::Bgra8);
        assert!(surface.owner::<MacosCaptureFrame>().is_some());
        assert!(surface.retained_owner::<TestPreparedTarget>().is_some());
        assert!(surface.resource_lifetime().is_some());
        assert!(surface.capture_resource_lifetime().is_some());
    }

    #[test]
    fn reduced_rgba_demand_falls_back_until_native_reducer_exists() {
        let frame = frame();
        let source = source(&frame);
        let demand = RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenPublicationExecutorRequest::SourceNative(target()),
                ScreenExtentRequest::bounded(
                    NonZeroU32::new(2),
                    NonZeroU32::new(1),
                    super::super::ScreenUpscalePolicy::Never,
                ),
                ScreenAspectPolicy::Contain,
                Arc::new(ScreenProcessingProfile::default()),
            ),
            NonZeroU32::new(60).expect("nonzero cadence"),
        );
        let resolved = resolve_macos_publication_branch(&source, &demand)
            .expect("reduced demand resolves")
            .expect("configured macOS source owns reduced demand");
        assert!(matches!(
            resolved.descriptor().executor(),
            ScreenPublicationExecutor::Cpu
        ));
    }
}
