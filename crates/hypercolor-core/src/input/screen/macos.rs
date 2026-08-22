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
    MacosFrameMailbox, MacosFrameStatus, MacosHostArchitecture as NativeHostArchitecture,
    MacosProtectedSourceState as NativeProtectedSourceState, MacosStreamRequest,
    MacosTahoeSelectionCapabilities as NativeTahoeSelectionCapabilities, MacosTransferFunction,
};
#[cfg(feature = "macos-capture-fixtures")]
use hypercolor_macos_capture::{MacosRuntimeCapability, MacosTahoeRuntimeProbes};
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
    ScreenExecutorColorCapabilities, ScreenGpuSurfacePayload, ScreenNativePreparationPayload,
    ScreenNativeWorkPayload, ScreenPhysicalGpuDeviceIdentity, ScreenPreparedWorkerToken,
    ScreenPublicationColorimetry, ScreenPublicationExecutor, ScreenPublicationExecutorRequest,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenRequiredResourceMinimum,
    ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime, ScreenSourceReflection,
    ScreenSourceSelector, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale, analyze_screen_frame,
};
#[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
use crate::input::SourceKind;
use crate::input::status::SourceSessionSlot;
#[cfg(target_os = "macos")]
use crate::input::traits::MacosScreenshotReferenceAction;
use crate::input::traits::{
    InputData, InputSource, ProtectedSourceAuthorizationAction, ScreenSourcePickerAction,
};
use crate::input::{
    MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner, MacosProtectedSourceState,
    MacosScreenPlatformStatus, MacosScreenTimingStatus, MacosSelectionState,
    MacosTahoeCapabilities, MacosTahoeSelectionCapabilities, MacosTimingStatus,
    SourcePlatformStatus, SourceStatusHandle, SourceStatusReporter,
};

#[cfg(target_os = "macos")]
mod surface_pool;

#[cfg(target_os = "macos")]
use surface_pool::MacosSurfacePool;

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
const TIMING_BUCKET_WIDTH_NS: u64 = 100_000;
const TIMING_BUCKET_COUNT: usize = 4096;

#[derive(Debug, Default)]
struct MacosScreenRuntimeTelemetry {
    publication_path: AtomicU8,
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

impl AtomicTimingHistogram {
    fn record(&self, elapsed: Duration) {
        self.record_with_hook(elapsed, || {});
    }

    fn record_with_hook(&self, elapsed: Duration, before_complete: impl FnOnce()) {
        let generation = self.begin_write();
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let bucket = usize::try_from(nanos / TIMING_BUCKET_WIDTH_NS)
            .unwrap_or(usize::MAX)
            .min(TIMING_BUCKET_COUNT);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        let _ = self
            .total_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(total.saturating_add(nanos))
            });
        self.max_ns.fetch_max(nanos, Ordering::Relaxed);
        before_complete();
        self.sample_count.fetch_add(1, Ordering::Relaxed);
        self.generation
            .store(generation.wrapping_add(1), Ordering::Release);
    }

    fn begin_write(&self) -> u64 {
        let mut generation = self.generation.load(Ordering::Relaxed);
        loop {
            if generation & 1 == 1 {
                std::hint::spin_loop();
                generation = self.generation.load(Ordering::Acquire);
                continue;
            }
            let started = generation.wrapping_add(1);
            match self.generation.compare_exchange_weak(
                generation,
                started,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return started,
                Err(observed) => generation = observed,
            }
        }
    }

    fn percentile_upper_bound_ns(&self, percentile: u64, sample_count: u64, maximum: u64) -> u64 {
        if sample_count == 0 {
            return 0;
        }
        let rank = sample_count.saturating_mul(percentile).saturating_add(99) / 100;
        let mut observed = 0_u64;
        for (index, count) in self.buckets.iter().enumerate() {
            observed = observed.saturating_add(count.load(Ordering::Relaxed));
            if observed >= rank {
                if index == TIMING_BUCKET_COUNT {
                    return maximum;
                }
                return u64::try_from(index.saturating_add(1))
                    .unwrap_or(u64::MAX)
                    .saturating_mul(TIMING_BUCKET_WIDTH_NS)
                    .min(maximum);
            }
        }
        maximum
    }

    fn snapshot(&self) -> MacosTimingStatus {
        self.snapshot_with_hooks(|| {}, || {})
    }

    fn snapshot_with_hooks(
        &self,
        mut retrying: impl FnMut(),
        mut after_p95: impl FnMut(),
    ) -> MacosTimingStatus {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            if generation & 1 == 1 {
                retrying();
                std::hint::spin_loop();
                continue;
            }
            let sample_count = self.sample_count.load(Ordering::Relaxed);
            let total_ns = self.total_ns.load(Ordering::Relaxed);
            let max_ns = self.max_ns.load(Ordering::Relaxed);
            let p95_ns = self.percentile_upper_bound_ns(95, sample_count, max_ns);
            after_p95();
            let p99_ns = self.percentile_upper_bound_ns(99, sample_count, max_ns);
            std::sync::atomic::fence(Ordering::Acquire);
            if self.generation.load(Ordering::Relaxed) == generation {
                return MacosTimingStatus {
                    sample_count,
                    total_ns,
                    max_ns,
                    p95_ns,
                    p99_ns,
                };
            }
            retrying();
        }
    }
}

impl PlatformGpuSurfaceTimingSink for MacosScreenRuntimeTelemetry {
    fn record_import(&self, elapsed: Duration) {
        self.native_import_timing.record(elapsed);
    }

    fn record_native_reduction_submission(&self, elapsed: Duration) {
        self.native_reduction_submit_timing.record(elapsed);
    }
}

impl MacosScreenRuntimeTelemetry {
    fn set_cpu(&self) {
        self.publication_path
            .store(PUBLICATION_PATH_CPU, Ordering::Release);
        *lock(&self.fallback_reason) = None;
    }

    fn set_native(&self) {
        self.publication_path
            .store(PUBLICATION_PATH_NATIVE, Ordering::Release);
        *lock(&self.fallback_reason) = None;
    }

    fn set_cpu_fallback(&self, reason: &'static str) {
        self.publication_path
            .store(PUBLICATION_PATH_CPU_FALLBACK, Ordering::Release);
        *lock(&self.fallback_reason) = Some(Arc::from(reason));
    }

    fn publication_path(&self) -> Option<Arc<str>> {
        match self.publication_path.load(Ordering::Acquire) {
            PUBLICATION_PATH_CPU => Some(Arc::from("cpu")),
            PUBLICATION_PATH_NATIVE => Some(Arc::from("native")),
            PUBLICATION_PATH_CPU_FALLBACK => Some(Arc::from("cpu_fallback")),
            PUBLICATION_PATH_UNKNOWN => None,
            _ => None,
        }
    }

    fn record_cpu_reduction(&self, elapsed: Duration) {
        self.cpu_reduction_timing.record(elapsed);
    }

    fn record_native_publication(&self, captured_at: Instant) {
        self.capture_to_native_publication_timing
            .record(Instant::now().saturating_duration_since(captured_at));
    }

    fn record_converted_publication(&self, captured_at: Instant) {
        self.capture_to_converted_publication_timing
            .record(Instant::now().saturating_duration_since(captured_at));
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
    fn selection_revision(&self) -> u64;
    fn begin_stream_request(&self, request: MacosStreamRequest) -> anyhow::Result<StreamRequest>;
    fn tahoe_selection_capabilities(&self) -> Option<NativeTahoeSelectionCapabilities>;
    fn host_capabilities(&self) -> NativeCaptureCapabilities;
    fn authorization(&self) -> MacosAuthorizationState;
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

impl StreamRequest {
    #[cfg(feature = "macos-capture-fixtures")]
    fn completed(generation: u64, result: anyhow::Result<()>) -> Self {
        Self {
            generation,
            completion: Box::new(|| result),
        }
    }

    fn wait(self) -> anyhow::Result<()> {
        (self.completion)().with_context(|| {
            format!(
                "macOS stream request generation {} did not commit",
                self.generation
            )
        })
    }
}

#[cfg(target_os = "macos")]
struct NativeCaptureControl {
    session: MacosScreenCaptureSession,
    clock: MacosDisplayClock,
    host_capabilities: NativeCaptureCapabilities,
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

    fn selection_revision(&self) -> u64 {
        self.session.selection_revision()
    }

    fn begin_stream_request(&self, request: MacosStreamRequest) -> anyhow::Result<StreamRequest> {
        let transaction = self.session.begin_stream_request(request)?;
        let generation = transaction.generation();
        Ok(StreamRequest {
            generation,
            completion: Box::new(move || transaction.wait().map_err(anyhow::Error::from)),
        })
    }

    fn tahoe_selection_capabilities(&self) -> Option<NativeTahoeSelectionCapabilities> {
        self.session.tahoe_selection_capabilities()
    }

    fn host_capabilities(&self) -> NativeCaptureCapabilities {
        self.host_capabilities
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

    fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        self.session.diagnostics()
    }

    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant> {
        self.clock
            .timestamp(display_time)
            .map_err(anyhow::Error::from)
    }

    fn capture_screenshot_reference(
        &self,
    ) -> anyhow::Result<
        mpsc::Receiver<
            Result<MacosScreenshotReferenceCapture, hypercolor_macos_capture::MacosCaptureError>,
        >,
    > {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.session
            .capture_screenshot_reference_with_identity(move |result| {
                let _ = result_tx.send(result);
            })?;
        Ok(result_rx)
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
    display_scale_bits: u64,
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
            colorimetry: capture_colorimetry(frame)?,
            pixel_format: frame.pixel_format,
            resource_generation,
            allocation_bytes: frame.surface.allocation_bytes,
            display_scale_bits: frame.geometry.display_scale_factor.get().to_bits(),
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
                capture_pixel_format(self.pixel_format),
                self.colorimetry,
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
        let pixel_format = capture_pixel_format(self.pixel_format);
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
    compute_capacity_policy: ScreenComputeCapacityPolicy,
    resolution_revision: AtomicU64,
}

impl MacosExactPublicationShared {
    #[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
    fn with_compute_capacity_policy(policy: ScreenComputeCapacityPolicy) -> Self {
        Self {
            compute_capacity_policy: policy,
            ..Self::default()
        }
    }

    fn advance_resolution_revision(&self) {
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("macOS screen publication resolution revision exhausted");
    }

    fn replace_source(&self, next: Option<MacosPublicationSource>) {
        let mut source = lock(&self.source);
        if *source == next {
            return;
        }
        tracing::debug!(
            shared = ?std::ptr::from_ref(self),
            installed = next.is_some(),
            "macOS exact publication source changed"
        );
        *source = next;
        self.advance_resolution_revision();
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

impl StagedCaptureWorker {
    fn commit(mut self) -> CaptureWorker {
        self.worker
            .take()
            .expect("staged capture worker commits exactly once")
    }
}

impl Drop for StagedCaptureWorker {
    fn drop(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        self.start.store(true, Ordering::Release);
        if let Some(join) = worker.join.take() {
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

fn production_stream_request(
    config: &CaptureConfig,
    demand: ScreenCaptureDemand,
    capabilities: NativeCaptureCapabilities,
) -> anyhow::Result<MacosStreamRequest> {
    let cadence = demand
        .cadence()
        .unwrap_or(ScreenCaptureCadence::Configured)
        .resolve(config.acquisition_cadence);
    let cadence = match cadence {
        ScreenCaptureCadence::Configured => MacosCaptureCadence::FramesPerSecond(config.target_fps),
        ScreenCaptureCadence::NativeRefresh => MacosCaptureCadence::NativeRefresh,
        ScreenCaptureCadence::FramesPerSecond(frames_per_second) => {
            MacosCaptureCadence::FramesPerSecond(frames_per_second.get())
        }
    };
    let cursor_composed = matches!(demand.cursor(), Some(ScreenCursorPolicy::Include));
    Ok(MacosStreamRequest::for_capabilities(
        cadence,
        cursor_composed,
        capabilities,
    )?)
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
    owner_conflict: Option<Arc<crate::input::MacosDaemonOwnerConflict>>,
    owner_designated_requirement_hash: Option<Arc<str>>,
    authorization: MacosAuthorizationState,
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

    /// Create a production source with shared memory and calibrated CPU fences.
    #[cfg(target_os = "macos")]
    pub fn with_admission_and_compute_capacity(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> anyhow::Result<Self> {
        let selector = MacosCaptureSelector::parse(&config.source)?;
        let host_capabilities = MacosScreenCaptureSession::capabilities()?;
        let request =
            production_stream_request(&config, ScreenCaptureDemand::Inactive, host_capabilities)?;
        let pool_coordinator = admission.clone();
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
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
        ))
    }

    #[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
    fn with_control_and_telemetry(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
        control: Arc<dyn MacosCaptureControl>,
        telemetry: Arc<MacosScreenRuntimeTelemetry>,
    ) -> Self {
        let consented = control.authorization() == MacosAuthorizationState::Authorized;
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
                "screen_capture_kit_cpu",
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
        self.status
            .set_platform(Some(SourcePlatformStatus::MacosScreen(
                MacosScreenPlatformStatus {
                    state: map_protected_state(state),
                    tcc: authorization,
                    owner: self.owner,
                    selection: map_selection(self.control.selection()),
                    selection_diagnostic_label: selection_diagnostic_label(
                        self.control.selection(),
                    ),
                    selection_revision: self.control.selection_revision(),
                    tahoe: map_tahoe_capabilities(self.control.host_capabilities(), self.metal4),
                    tahoe_selection: self
                        .control
                        .tahoe_selection_capabilities()
                        .map(map_tahoe_selection_capabilities),
                    owner_conflict: self.owner_conflict.clone(),
                    authorization_last_transition_at: self.authorization_last_transition_at,
                    owner_designated_requirement_hash: self
                        .owner_designated_requirement_hash
                        .clone(),
                    executable_architecture: executable_architecture(),
                    stream_state: Arc::from(stream_state_name(state)),
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
                    color_space: source.as_ref().map(|source| {
                        Arc::from(color_space_name(source.colorimetry.color_space()))
                    }),
                    transfer_function: source.as_ref().map(|source| {
                        Arc::from(transfer_function_name(
                            source.colorimetry.transfer_function(),
                        ))
                    }),
                    display_scale_bits: source.as_ref().map(|source| source.display_scale_bits),
                    native_width: source
                        .as_ref()
                        .map(|source| source.geometry.native_extent().width()),
                    native_height: source
                        .as_ref()
                        .map(|source| source.geometry.native_extent().height()),
                    queue_depth: hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH,
                    admitted_native_bytes: self
                        .telemetry
                        .admitted_native_bytes
                        .load(Ordering::Acquire),
                    pinned_generations: Some(
                        self.telemetry.pinned_generations.load(Ordering::Acquire),
                    ),
                    frames_received: diagnostics.frames_received,
                    frames_published: diagnostics.frames_published,
                    frames_superseded: diagnostics.superseded_deliveries,
                    frames_malformed: diagnostics.malformed_frames,
                    frames_dropped: frame_drop_counters(&diagnostics),
                    frames_stale: self.telemetry.stale_frames.load(Ordering::Acquire),
                    publication_path: self.telemetry.publication_path(),
                    fallback_reason: lock(&self.telemetry.fallback_reason).clone(),
                    timing,
                    callback_total_ns: timing.callback.total_ns,
                    callback_max_ns: timing.callback.max_ns,
                    retain_total_ns: timing.retain.total_ns,
                    retain_max_ns: timing.retain.max_ns,
                    conversion_total_ns: timing.conversion.total_ns,
                    conversion_max_ns: timing.conversion.max_ns,
                    cpu_reduction_total_ns: timing.cpu_reduction.total_ns,
                    cpu_reduction_max_ns: timing.cpu_reduction.max_ns,
                    native_import_total_ns: timing.native_import.total_ns,
                    native_import_max_ns: timing.native_import.max_ns,
                    native_reduction_submit_total_ns: timing.native_reduction_submit.total_ns,
                    native_reduction_submit_max_ns: timing.native_reduction_submit.max_ns,
                    publication_total_ns: timing.publication.total_ns,
                    publication_max_ns: timing.publication.max_ns,
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
                // The exit channel is only drained by the sampled analysis
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

    fn set_macos_daemon_ownership(
        &mut self,
        owner: MacosCapabilityOwner,
        conflict: Option<crate::input::MacosDaemonOwnerConflict>,
        designated_requirement_hash: Option<Arc<str>>,
    ) -> anyhow::Result<()> {
        self.owner = owner;
        self.owner_conflict = conflict.map(Arc::new);
        self.owner_designated_requirement_hash = designated_requirement_hash;
        self.refresh_platform_status()
    }

    fn set_macos_metal4_capability(&mut self, metal4: bool) -> anyhow::Result<()> {
        self.metal4 = metal4;
        self.refresh_platform_status()
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
        _events: &mut Vec<hypercolor_types::event::TimedInputEvent>,
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
        Some(ProtectedSourceAuthorizationAction::current_macos_process(
            Arc::new(move || {
                control.request_authorization();
                Ok(control.authorization() == MacosAuthorizationState::Authorized)
            }),
        ))
    }

    fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        let control = Arc::clone(&self.control);
        Some(ScreenSourcePickerAction::current_macos_process(Arc::new(
            move || control.present_picker(),
        )))
    }

    #[cfg(target_os = "macos")]
    fn macos_screenshot_reference_action(&self) -> Option<MacosScreenshotReferenceAction> {
        let control = Arc::clone(&self.control);
        Some(Arc::new(move || control.capture_screenshot_reference()))
    }
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
fn resolve_macos_publication_branch(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    resolve_macos_publication_branch_with_telemetry(source, demand, &telemetry)
}

fn resolve_macos_publication_branch_with_telemetry(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let selector = demand.request().selector();
    if !source.matches_selector(selector) {
        return Ok(None);
    }
    let selector = selector.clone();
    let capabilities = CpuReductionExecutor::supported_color_capabilities();
    if matches!(
        demand.request().executor(),
        ScreenPublicationExecutorRequest::Cpu
    ) {
        telemetry.set_cpu();
        return Ok(Some(demand.resolve_with_color_capabilities(
            &source.cpu_source(selector),
            capabilities,
        )?));
    }

    let ScreenPublicationExecutorRequest::SourceNative(target) = demand.request().executor() else {
        unreachable!("screen publication executor requests are exhaustive");
    };
    if target.accepted_api() != &PlatformGpuApi::Metal {
        telemetry.set_cpu_fallback("target_api_not_metal");
    } else if let Ok(native_source) =
        source.gpu_source(selector.clone(), target.physical_gpu_device().clone())
    {
        if let Ok(resolved) = demand.resolve_with_executor_capabilities(
            &native_source,
            ScreenExecutorColorCapabilities::new(capabilities, target.color_capabilities()),
        ) {
            if matches!(
                resolved.descriptor().executor(),
                ScreenPublicationExecutor::SourceNative(_)
            ) && MacosNativeTargetManifest::new(resolved.descriptor()).is_ok()
            {
                telemetry.set_native();
                return Ok(Some(resolved));
            }
            telemetry.set_cpu_fallback("native_contract_unavailable");
        } else {
            telemetry.set_cpu_fallback("native_descriptor_incompatible");
        }
    } else {
        telemetry.set_cpu_fallback("metal_device_mismatch");
    }

    Ok(Some(demand.resolve_with_color_capabilities(
        &source.cpu_source(selector),
        capabilities,
    )?))
}

fn macos_native_descriptor_is_identity(descriptor: &ResolvedScreenPublicationDescriptor) -> bool {
    descriptor.source_pixel_format() == CapturePixelFormat::Bgra8
        && descriptor.source().geometry().crop().is_none()
        && descriptor.geometry().output_extent() == descriptor.source().geometry().storage_extent()
        && descriptor.physical().reduction_extent()
            == descriptor.source().geometry().storage_extent()
        && descriptor.physical().target_pixel_format() == descriptor.source_pixel_format()
        && matches!(
            descriptor.physical().color_pipeline().transform(),
            super::ResolvedScreenColorTransform::PreserveEncodedSamples
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
    let executor = exact.cpu_executor()?;
    let compute_plan =
        CpuExactReductionWorkPlan::try_for_source(&candidate, ticket.source_id(), |_| true)?;
    let compute_plan = match exact.compute_capacity_policy.exact(executor.worker_count()) {
        Some(capacity) => compute_plan.admit(capacity)?,
        None => compute_plan,
    };
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
        let cpu_source =
            source.cpu_source(ScreenSourceSelector::Exact(source.epoch.source_id.clone()));
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
        let shared_resource_name = pending.target.shared_resource_name().cloned();
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
        let shared_lifetime = shared_resource_name
            .as_ref()
            .map(|resource_name| {
                lifetimes
                    .iter()
                    .find(|lifetime| lifetime.resource().name() == resource_name)
                    .cloned()
                    .ok_or_else(|| anyhow!("macOS native shared target lifetime is missing"))
            })
            .transpose()?;
        native_routes.push(MacosNativeRoute {
            descriptor: pending.descriptor,
            target: pending.target.bind_with_shared(lifetime, shared_lifetime)?,
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
    captured_at: Instant,
) -> anyhow::Result<Option<&'a mut MacosExactRuntime>> {
    let authority = hub.committed_state();
    let Some(current_binding) = authority.runtime_binding(&source.epoch.source_id) else {
        return Ok(None);
    };
    let Some(current_index) = runtimes
        .iter_mut()
        .position(|runtime| runtime.source == *source && runtime.binding.is_same(current_binding))
    else {
        return Ok(None);
    };
    let should_inherit = runtimes[current_index].fanout.is_none()
        && runtimes[current_index].fanout_candidate.is_some();
    runtimes[current_index].bind_if_current(hub)?;
    if should_inherit
        && let Some(previous_index) =
            runtimes
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, runtime)| {
                    (index != current_index
                        && runtime.binding.source_id() == current_binding.source_id()
                        && runtime.fanout.is_some())
                    .then_some(index)
                })
    {
        let (current, previous) = if current_index < previous_index {
            let (before_previous, previous_and_after) = runtimes.split_at_mut(previous_index);
            (
                &mut before_previous[current_index],
                &mut previous_and_after[0],
            )
        } else {
            let (before_current, current_and_after) = runtimes.split_at_mut(current_index);
            (
                &mut current_and_after[0],
                &mut before_current[previous_index],
            )
        };
        if let (Some(current), Some(previous)) = (current.fanout.as_mut(), previous.fanout.as_mut())
        {
            current.inherit_tone_map_transition_from(previous, captured_at);
        }
    }
    Ok(runtimes[current_index]
        .is_bound()
        .then_some(&mut runtimes[current_index]))
}

fn handle_worker_commands(
    command_rx: &mpsc::Receiver<WorkerCommand>,
    prepared: &mut PreparedWorker,
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
            WorkerCommand::ReconfigureProcessing {
                calibration,
                completion,
            } => {
                prepared.analyzer.set_led_tone_map_calibration(calibration);
                let _ = completion.send(Ok(()));
            }
        }
    }
}

fn update_pinned_generations(
    runtimes: &[MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) {
    let current = runtimes
        .iter()
        .map(|runtime| runtime.source.resource_generation)
        .max();
    let mut retained = runtimes
        .iter()
        .filter(|runtime| Some(runtime.source.resource_generation) != current)
        .map(|runtime| runtime.source.resource_generation)
        .collect::<Vec<_>>();
    retained.sort_unstable();
    retained.dedup();
    telemetry
        .pinned_generations
        .store(retained.len(), Ordering::Release);
}

fn with_current_macos_worker_authority<T>(
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
    operation: impl FnOnce(
        &ScreenPublicationHub,
        &ScreenWorkerBinding,
    ) -> Result<T, ScreenPublicationHubError>,
) -> anyhow::Result<Option<T>> {
    let Some(hub) = exact.hub() else {
        return Ok(None);
    };
    let authority = hub.committed_state();
    let Some(binding) = runtimes
        .iter()
        .map(|runtime| &runtime.binding)
        .find(|binding| authority.owns_runtime_binding(binding))
    else {
        return Ok(None);
    };
    match operation(&hub, binding) {
        Ok(value) => Ok(Some(value)),
        Err(ScreenPublicationHubError::WorkerAuthorityStale { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn report_macos_worker_health(
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
    health: ScreenPublicationHealth,
) -> anyhow::Result<()> {
    with_current_macos_worker_authority(exact, runtimes, |hub, binding| {
        hub.report_worker_delivery_health(binding, health)
    })?;
    Ok(())
}

fn invalidate_macos_worker(
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
) -> anyhow::Result<()> {
    with_current_macos_worker_authority(exact, runtimes, ScreenPublicationHub::invalidate_worker)?;
    Ok(())
}

fn synchronize_macos_invalidation_generation(
    observed: &mut u64,
    delivered: u64,
    publication: &Arc<Mutex<MacosPublication>>,
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
) -> anyhow::Result<bool> {
    if delivered < *observed {
        return Ok(false);
    }
    if delivered > *observed {
        lock(publication).latest = None;
        invalidate_macos_worker(exact, runtimes)?;
        *observed = delivered;
    }
    Ok(true)
}

fn run_worker(
    mut prepared: PreparedWorker,
    mailbox: MacosFrameMailbox,
    publication: Arc<Mutex<MacosPublication>>,
    exact: Arc<MacosExactPublicationShared>,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
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
    let mut invalidation_generation = 0;
    let result: anyhow::Result<()> = (|| {
        while !stop.load(Ordering::Acquire) {
            handle_worker_commands(&command_rx, &mut prepared, &mut exact_runtimes, &exact);
            update_pinned_generations(&exact_runtimes, &telemetry);
            let Some((_, delivery_invalidation_generation, delivery)) = mailbox
                .wait_latest_with_generation_while(WORKER_WAIT, || !stop.load(Ordering::Acquire))
            else {
                continue;
            };
            if !synchronize_macos_invalidation_generation(
                &mut invalidation_generation,
                delivery_invalidation_generation,
                &publication,
                &exact,
                &exact_runtimes,
            )? {
                continue;
            }
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
                        &telemetry,
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
                | Err(_) => {}
                Ok(MacosFrameEvent::RecoverableError(_)) => {
                    report_macos_worker_health(
                        &exact,
                        &exact_runtimes,
                        ScreenPublicationHealth::Recovering,
                    )?;
                }
                Ok(MacosFrameEvent::Lifecycle(_)) => {}
            }
        }
        Ok(())
    })();
    let invalidation = invalidate_macos_worker(&exact, &exact_runtimes);
    exact.replace_source(None);
    exact.clear_owned_sources();
    exact_runtimes.clear();
    telemetry.pinned_generations.store(0, Ordering::Release);
    prepared.analyzer.stop();
    result?;
    invalidation
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
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
    exact_runtimes: &mut [MacosExactRuntime],
    worker_generation: u64,
    target_fps: u32,
    status_session: &SourceSessionSlot,
    control: &Arc<dyn MacosCaptureControl>,
) -> anyhow::Result<()> {
    // ScreenCaptureKit's display time is the frame's intended display
    // vsync, which runs slightly ahead of callback delivery, so the raw
    // conversion can land in the future. A future capture instant makes
    // every publication timeline read backwards (published_at precedes
    // captured_at) and kills the pump; a capture time can never postdate
    // the moment we hold the frame.
    let captured_at = control.captured_at(frame.display_time)?.min(Instant::now());
    let fresh_until = captured_at
        .checked_add(Duration::from_nanos(
            2_000_000_000_u64.div_ceil(u64::from(target_fps)),
        ))
        .ok_or_else(|| anyhow!("macOS capture freshness deadline overflow"))?;
    if Instant::now() > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    let topology_generation = topology.observe(&frame)?;
    let resource_generation = resources.observe(&frame)?;
    let source = MacosPublicationSource::from_frame(
        source_id.clone(),
        topology_generation,
        resource_generation,
        &frame,
    )?;
    exact.replace_source(Some(source.clone()));
    let exact_delivery = publish_macos_native_exact_with_telemetry(
        &frame,
        captured_at,
        fresh_until,
        &source,
        exact,
        exact_runtimes,
        telemetry,
    )?;
    if exact_delivery.stale {
        return Ok(());
    }
    if exact_delivery.cpu {
        let capture =
            native_cpu_capture_frame(&frame, captured_at, fresh_until, &source, source_id.clone())?;
        if Instant::now() > fresh_until {
            telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        publish_macos_scalar_exact(&frame, &capture, &source, exact, exact_runtimes, telemetry)?;
    }
    // The exact lane feeds native compositing only; HTML effects read the
    // analyzed ScreenData that flows through the sampled publication slot.
    // Treating the lanes as exclusive turns every HTML screen effect black
    // the moment an exact plan commits, so sampled analysis always runs
    // alongside exact deliveries.
    let capture = analysis_cpu_capture_frame(
        prepared,
        &frame,
        captured_at,
        fresh_until,
        &source,
        source_id,
        topology_generation,
    )?;
    if Instant::now() > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        publish_macos_cpu_exact(&capture, &source, exact, exact_runtimes, telemetry)?;
    }
    let reduction_started = Instant::now();
    let snapshot = analyze_screen_frame(&mut prepared.analyzer, capture);
    telemetry.record_cpu_reduction(reduction_started.elapsed());
    let snapshot = snapshot?;
    if Instant::now() > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
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
    telemetry.record_converted_publication(captured_at);
    Ok(())
}

/// Ceiling on the surface handed to the sampled analyzer. The analyzer reduces
/// whatever it receives into the coarse ScreenData grid, so tone-mapping every
/// pixel of a native 4K HDR frame is wasted work that blows the publication
/// freshness budget and starves every HTML screen effect.
const ANALYSIS_MAX_WIDTH: u32 = 640;
const ANALYSIS_MAX_HEIGHT: u32 = 480;

fn analysis_decimation(extent: PixelExtent) -> u32 {
    extent
        .width()
        .div_ceil(ANALYSIS_MAX_WIDTH)
        .max(extent.height().div_ceil(ANALYSIS_MAX_HEIGHT))
}

#[allow(clippy::too_many_arguments)]
fn analysis_cpu_capture_frame(
    prepared: &mut PreparedWorker,
    frame: &MacosCaptureFrame,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    source_id: CaptureSourceId,
    topology_generation: u64,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let extent = source.geometry.storage_extent();
    let decimation = if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        // The Bgra8 plane also feeds the CPU-exact publication, which is
        // exact by contract, so it keeps every native pixel.
        1
    } else {
        analysis_decimation(extent)
    };
    let storage_extent = if decimation == 1 {
        extent
    } else {
        PixelExtent::new(
            extent.width().div_ceil(decimation),
            extent.height().div_ceil(decimation),
        )?
    };
    let row_stride = usize::try_from(storage_extent.width())
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| anyhow!("macOS capture row stride overflow"))?;
    let byte_len = row_stride
        .checked_mul(usize::try_from(storage_extent.height())?)
        .ok_or_else(|| anyhow!("macOS capture plane length overflow"))?;
    let mut plane = prepared.plane_pool.try_acquire(byte_len)?;
    plane.resize(byte_len, 0);
    let (pixel_format, colorimetry) = if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        frame.copy_bgra8_to(&mut plane, row_stride)?;
        (CapturePixelFormat::Bgra8, source.colorimetry)
    } else {
        let calibration = LedToneMapCalibration::try_new(
            prepared.analyzer.config().target_led_white_x,
            prepared.analyzer.config().target_led_white_y,
            prepared.analyzer.config().target_led_reference_white_nits,
            prepared.analyzer.config().target_led_peak_nits,
            prepared.analyzer.config().exposure_ev,
        )?;
        let tone_map = PreparedLedToneMap::prepare(
            source.colorimetry.try_known()?,
            KnownCaptureColorimetry::SRGB,
            calibration,
        )?;
        frame.with_cpu_source(|samples| -> anyhow::Result<()> {
            for y in 0..storage_extent.height() {
                let source_y = y
                    .checked_mul(decimation)
                    .ok_or_else(|| anyhow!("macOS analysis sample row overflow"))?;
                let row_start = usize::try_from(y)?
                    .checked_mul(row_stride)
                    .ok_or_else(|| anyhow!("macOS analysis row offset overflow"))?;
                for x in 0..storage_extent.width() {
                    let source_x = x
                        .checked_mul(decimation)
                        .ok_or_else(|| anyhow!("macOS analysis sample column overflow"))?;
                    let pixel_start = usize::try_from(x)?
                        .checked_mul(4)
                        .and_then(|offset| row_start.checked_add(offset))
                        .ok_or_else(|| anyhow!("macOS analysis pixel offset overflow"))?;
                    let pixel_end = pixel_start
                        .checked_add(4)
                        .ok_or_else(|| anyhow!("macOS analysis pixel end overflow"))?;
                    let source_pixel = samples.sample_rgba32f(source_x, source_y)?;
                    plane[pixel_start..pixel_end].copy_from_slice(
                        &tone_map.encode(tone_map.decode_and_map_source(source_pixel)),
                    );
                }
            }
            Ok(())
        })??;
        (CapturePixelFormat::Rgba8, CaptureColorimetry::SRGB)
    };
    let geometry = if decimation == 1 {
        source.geometry
    } else {
        super::CaptureGeometry::new(
            source.geometry.origin(),
            source.geometry.native_extent(),
            storage_extent,
            source.geometry.rotation(),
            source.geometry.crop(),
            source.geometry.source_scale(),
        )?
    };
    let damage = if decimation == 1 {
        CaptureDamage::new(
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
        )
    } else {
        // Native damage rects no longer address the decimated pixels, and the
        // analyzer resamples the whole surface anyway.
        CaptureDamage::new(
            vec![PixelRect::new(
                0,
                0,
                storage_extent.width(),
                storage_extent.height(),
            )?],
            Vec::new(),
        )
    };
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    Ok(CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry,
            colorimetry,
            cursor: CaptureCursor {
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
            },
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            plane.freeze(),
            pixel_format,
            i64::try_from(row_stride)?,
            0,
        )),
        damage,
    )?)
}

fn native_cpu_capture_frame(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    source_id: CaptureSourceId,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let surface = PlatformGpuSurface::new(
        PlatformGpuApi::Metal,
        u64::from(frame.surface.iosurface_id),
        source.geometry.storage_extent(),
        capture_pixel_format(frame.pixel_format),
        Arc::clone(frame),
    )?;
    Ok(CaptureFrame::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation: source.epoch.topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry: source.geometry,
            colorimetry: source.colorimetry,
            cursor: CaptureCursor {
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
            },
        },
        CaptureStorage::Gpu(surface),
        CaptureDamage::new(
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
        ),
    )?)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MacosExactDelivery {
    native: bool,
    cpu: bool,
    stale: bool,
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
fn publish_macos_native_exact(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
) -> anyhow::Result<(MacosExactDelivery, Arc<MacosScreenRuntimeTelemetry>)> {
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let delivery = publish_macos_native_exact_with_telemetry(
        frame,
        captured_at,
        fresh_until,
        source,
        exact,
        runtimes,
        &telemetry,
    )?;
    Ok((delivery, telemetry))
}

fn publish_macos_native_exact_with_telemetry(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
) -> anyhow::Result<MacosExactDelivery> {
    let Some(hub) = exact.hub() else {
        return Ok(MacosExactDelivery::default());
    };
    let Some(runtime) = bind_current_macos_exact_runtime(runtimes, source, &hub, captured_at)?
    else {
        return Ok(MacosExactDelivery::default());
    };
    let delivery = MacosExactDelivery {
        native: !runtime.native_routes.is_empty(),
        cpu: runtime.fanout.is_some(),
        stale: false,
    };
    let published_at = Instant::now();
    if published_at > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(MacosExactDelivery {
            stale: true,
            ..delivery
        });
    }
    let native_sequence = frame
        .sequence
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let mut native_published = false;
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
            source.geometry.storage_extent(),
            route.descriptor.source_pixel_format(),
            Arc::clone(frame),
        )?
        .with_timing_sink(Arc::clone(telemetry));
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
        let payload = if macos_native_descriptor_is_identity(&route.descriptor) {
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                ScreenPublicationColorimetry::new(
                    route.descriptor.physical().color_pipeline().output(),
                ),
                &surface,
            ))
        } else {
            ScreenBranchPayload::NativeWork(ScreenNativeWorkPayload::new(
                ScreenPublicationColorimetry::new(route.descriptor.source_colorimetry()),
                &surface,
            ))
        };
        match hub.publish(publisher, payload, &metadata) {
            Ok(_) => {
                native_published = true;
                telemetry
                    .publication_plan_generation
                    .store(publisher.plan_generation().get(), Ordering::Release);
                route.last_accepted_sequence = Some(frame.sequence);
                route.next_publish_at = route
                    .pacer
                    .advance_deadline(route.next_publish_at, published_at)?;
            }
            Err(ScreenPublicationHubError::PublicationPressure { .. }) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if native_published {
        telemetry.record_native_publication(captured_at);
    }
    Ok(delivery)
}

fn publish_macos_cpu_exact(
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)?
    else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        telemetry
            .publication_plan_generation
            .store(fanout.plan_generation().get(), Ordering::Release);
        let report = fanout.publish_due(
            &hub,
            Some(frame),
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )?;
        if report.published() > 0 {
            telemetry.record_converted_publication(frame.metadata().captured_at);
        }
    }
    Ok(())
}

fn publish_macos_scalar_exact(
    native_frame: &MacosCaptureFrame,
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)?
    else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        let reduction_started = Instant::now();
        telemetry
            .publication_plan_generation
            .store(fanout.plan_generation().get(), Ordering::Release);
        let report = fanout.publish_due_scalar(
            &hub,
            frame,
            Instant::now(),
            ScreenPublicationHealth::Healthy,
            |execute| {
                native_frame
                    .with_cpu_source(|samples| execute(&samples))
                    .map_err(|error| {
                        CpuPublicationFanoutError::ScalarSourceAccessFailed(error.to_string())
                    })?
            },
        )?;
        if report.published() > 0 {
            telemetry.record_cpu_reduction(reduction_started.elapsed());
            telemetry.record_converted_publication(frame.metadata().captured_at);
        }
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

fn capture_colorimetry(frame: &MacosCaptureFrame) -> anyhow::Result<CaptureColorimetry> {
    let color = frame.color;
    let color_space = match color.primaries {
        MacosColorPrimaries::Srgb => CaptureColorSpace::Srgb,
        MacosColorPrimaries::DisplayP3 => CaptureColorSpace::DisplayP3,
        MacosColorPrimaries::Rec2020 => CaptureColorSpace::Rec2020,
    };
    let transfer_function = match color.transfer {
        MacosTransferFunction::Srgb => CaptureTransferFunction::Srgb,
        MacosTransferFunction::Rec709 => CaptureTransferFunction::Rec709,
        MacosTransferFunction::Rec2020 => CaptureTransferFunction::Rec2020,
        MacosTransferFunction::Linear => CaptureTransferFunction::Linear,
        MacosTransferFunction::Pq => CaptureTransferFunction::Pq,
        MacosTransferFunction::Hlg => CaptureTransferFunction::Hlg,
    };
    let delivered = frame.delivered_metadata();
    let dynamic_range = if matches!(
        color.transfer,
        MacosTransferFunction::Pq | MacosTransferFunction::Hlg
    ) || delivered
        .is_some_and(|metadata| metadata.dynamic_range == MacosCaptureDynamicRange::Hdr)
        || matches!(
            frame.pixel_format,
            MacosCapturePixelFormat::Argb2101010 | MacosCapturePixelFormat::Rgba16Float
        ) {
        CaptureDynamicRange::High
    } else {
        CaptureDynamicRange::Standard
    };
    let luminance = if dynamic_range == CaptureDynamicRange::High {
        let delivered = delivered
            .ok_or_else(|| anyhow!("macOS HDR capture is missing delivered luminance metadata"))?;
        if delivered.pixel_format != frame.pixel_format
            || delivered.color != frame.color
            || delivered.dynamic_range != MacosCaptureDynamicRange::Hdr
        {
            return Err(anyhow!(
                "macOS HDR delivered metadata contradicts the capture frame"
            ));
        }
        // ScreenCaptureKit only sometimes attaches ContentLightLevelInfo and
        // IOSurfaceContentHeadroom; spec 76 requires a source reference white
        // regardless and treats headroom as best-effort. When the OS stays
        // silent, anchor reference white at BT.2408 diffuse white (203 nits,
        // the same default as the target LED calibration, so unsignalled
        // content maps through at unity) and assume one stop of highlight
        // headroom, matching the output headroom the tone map reserves.
        let reference_white = delivered
            .source_reference_white_nits
            .unwrap_or(DEFAULT_HDR_SOURCE_REFERENCE_WHITE_NITS);
        // EDR headroom is dynamic: macOS reports it relative to the current
        // display brightness, and at high brightness it legitimately reaches
        // 1.0 (no room above SDR white right now). No real pixel can exceed
        // reference white in that state, so giving the tone map its default
        // rolloff space is visually identity while keeping the luminance
        // contract well-formed.
        let headroom = delivered
            .content_headroom
            .filter(|headroom| *headroom > 1.0)
            .unwrap_or(DEFAULT_HDR_SOURCE_CONTENT_HEADROOM);
        let reference_white = CapturePositiveScalar::try_new(reference_white)?;
        let peak = CapturePositiveScalar::try_new(reference_white.value() * headroom)?;
        Some(CaptureLuminanceContext::new(reference_white, peak)?)
    } else {
        None
    };
    Ok(CaptureColorimetry::new(
        color_space,
        transfer_function,
        Some(dynamic_range),
        luminance,
    )?)
}

const fn capture_pixel_format(format: MacosCapturePixelFormat) -> CapturePixelFormat {
    match format {
        MacosCapturePixelFormat::Bgra8 => CapturePixelFormat::Bgra8,
        MacosCapturePixelFormat::Argb2101010 => CapturePixelFormat::Argb2101010,
        MacosCapturePixelFormat::Rgba16Float => CapturePixelFormat::Rgba16Float,
        MacosCapturePixelFormat::Yuv420VideoRange => CapturePixelFormat::Yuv420VideoRange,
        MacosCapturePixelFormat::Yuv420FullRange => CapturePixelFormat::Yuv420FullRange,
        MacosCapturePixelFormat::Yuv44410BiPlanar => CapturePixelFormat::Yuv44410BiPlanar,
    }
}

impl CpuScalarSource for MacosCpuSourceView<'_> {
    fn storage_extent(&self) -> PixelExtent {
        let extent = (*self).extent();
        PixelExtent::new(extent.width, extent.height)
            .expect("validated macOS CPU source has a non-empty extent")
    }

    fn pixel_format(&self) -> CapturePixelFormat {
        capture_pixel_format((*self).pixel_format())
    }

    fn sample_rgba32f(&self, x: u32, y: u32) -> Result<[f32; 4], CpuSamplingError> {
        (*self)
            .sample_rgba32f(x, y)
            .map_err(|_| CpuSamplingError::ScalarSourceReadFailed { x, y })
    }
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

fn selection_diagnostic_label(selection: MacosCaptureSelection) -> Option<Arc<str>> {
    match selection {
        MacosCaptureSelection::None => None,
        MacosCaptureSelection::Display { .. } => Some(Arc::from("display")),
        MacosCaptureSelection::SessionScoped { content_style } => {
            Some(Arc::from(match content_style {
                MacosCaptureContentStyle::Window => "window",
                MacosCaptureContentStyle::MultipleWindows => "multiple_windows",
                MacosCaptureContentStyle::Application => "application",
                MacosCaptureContentStyle::MultipleApplications => "multiple_applications",
                MacosCaptureContentStyle::Mixed => "mixed",
            }))
        }
    }
}

fn map_tahoe_selection_capabilities(
    capabilities: NativeTahoeSelectionCapabilities,
) -> MacosTahoeSelectionCapabilities {
    MacosTahoeSelectionCapabilities {
        source_id: capabilities.source_id,
        capture_session_generation: capabilities.capture_session_generation,
        hdr_capture: capabilities.hdr_capture,
        dual_range_screenshots: capabilities.dual_range_screenshots,
    }
}

fn map_tahoe_capabilities(
    capabilities: NativeCaptureCapabilities,
    metal4: bool,
) -> MacosTahoeCapabilities {
    MacosTahoeCapabilities {
        host_architecture: match capabilities.host_architecture {
            NativeHostArchitecture::AppleSilicon => MacosArchitecture::AppleSilicon,
            NativeHostArchitecture::Intel => MacosArchitecture::Intel,
        },
        translated_process: capabilities.translated_process,
        content_tone_mapping_info: capabilities.tahoe.content_tone_mapping_info.is_present(),
        metal4,
    }
}

const fn executable_architecture() -> MacosArchitecture {
    #[cfg(target_arch = "aarch64")]
    {
        MacosArchitecture::AppleSilicon
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        MacosArchitecture::Intel
    }
}

const fn stream_state_name(state: NativeProtectedSourceState) -> &'static str {
    match state {
        NativeProtectedSourceState::Starting | NativeProtectedSourceState::Live => "active",
        NativeProtectedSourceState::Interrupted
        | NativeProtectedSourceState::Revoked
        | NativeProtectedSourceState::Failed => "stopped",
        NativeProtectedSourceState::Disabled
        | NativeProtectedSourceState::NeedsUserAction
        | NativeProtectedSourceState::PermissionDenied
        | NativeProtectedSourceState::NeedsProcessRestart
        | NativeProtectedSourceState::NeedsSelection
        | NativeProtectedSourceState::ReadyIdle => "inactive",
    }
}

const fn nonzero_telemetry(value: u64) -> Option<u64> {
    if value == 0 { None } else { Some(value) }
}

const fn timing_status(
    sample_count: u64,
    total_ns: u64,
    max_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
) -> MacosTimingStatus {
    MacosTimingStatus {
        sample_count,
        total_ns,
        max_ns,
        p95_ns,
        p99_ns,
    }
}

const fn pixel_format_name(format: MacosCapturePixelFormat) -> &'static str {
    match format {
        MacosCapturePixelFormat::Bgra8 => "bgra8",
        MacosCapturePixelFormat::Argb2101010 => "argb2101010",
        MacosCapturePixelFormat::Rgba16Float => "rgba16_float",
        MacosCapturePixelFormat::Yuv420VideoRange => "yuv420_video_range",
        MacosCapturePixelFormat::Yuv420FullRange => "yuv420_full_range",
        MacosCapturePixelFormat::Yuv44410BiPlanar => "yuv44410_biplanar",
    }
}

const fn dynamic_range_name(range: CaptureDynamicRange) -> &'static str {
    match range {
        CaptureDynamicRange::Standard => "standard",
        CaptureDynamicRange::High => "high",
    }
}

const fn color_space_name(space: CaptureColorSpace) -> &'static str {
    match space {
        CaptureColorSpace::Srgb => "srgb",
        CaptureColorSpace::DisplayP3 => "display_p3",
        CaptureColorSpace::Rec2020 => "rec2020",
        CaptureColorSpace::Unknown => "unknown",
    }
}

const fn transfer_function_name(function: CaptureTransferFunction) -> &'static str {
    match function {
        CaptureTransferFunction::Srgb => "srgb",
        CaptureTransferFunction::Linear => "linear",
        CaptureTransferFunction::Rec709 => "rec709",
        CaptureTransferFunction::Rec2020 => "rec2020",
        CaptureTransferFunction::Pq => "pq",
        CaptureTransferFunction::Hlg => "hlg",
        CaptureTransferFunction::Unknown => "unknown",
    }
}

fn frame_drop_counters(diagnostics: &MacosCaptureCallbackDiagnostics) -> Arc<[(Arc<str>, u64)]> {
    MacosFrameDropReason::ALL
        .into_iter()
        .map(|reason| {
            let name = match reason {
                MacosFrameDropReason::InvalidSample => "invalid_sample",
                MacosFrameDropReason::DataNotReady => "data_not_ready",
                MacosFrameDropReason::UnexpectedOutput => "unexpected_output",
                MacosFrameDropReason::Attachment => "attachment",
                MacosFrameDropReason::UnsupportedFormat => "unsupported_format",
                MacosFrameDropReason::ColorMetadata => "color_metadata",
                MacosFrameDropReason::Surface => "surface",
                MacosFrameDropReason::Validation => "validation",
                MacosFrameDropReason::Resource => "resource",
            };
            (Arc::from(name), diagnostics.dropped(reason))
        })
        .collect::<Vec<_>>()
        .into()
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
    active_transitions: AtomicU64,
    status: Mutex<NativeProtectedSourceState>,
    selection: Mutex<MacosCaptureSelection>,
    selection_revision: AtomicU64,
    stream_request: Mutex<MacosStreamRequest>,
    pending_stream_request: Mutex<Option<FixturePendingStreamRequest>>,
    stream_request_transitions: AtomicU64,
    reject_next_stream_request: AtomicBool,
    defer_next_stream_request: AtomicBool,
    tahoe_selection: Mutex<Option<NativeTahoeSelectionCapabilities>>,
    host_capabilities: Mutex<NativeCaptureCapabilities>,
    captured_at: Mutex<Option<Instant>>,
    diagnostics: Mutex<MacosCaptureCallbackDiagnostics>,
}

#[cfg(feature = "macos-capture-fixtures")]
struct FixturePendingStreamRequest {
    generation: u64,
    request: MacosStreamRequest,
    completion: mpsc::SyncSender<anyhow::Result<()>>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl Default for FixtureControl {
    fn default() -> Self {
        Self {
            mailbox: MacosFrameMailbox::default(),
            active: AtomicBool::new(false),
            active_transitions: AtomicU64::new(0),
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            selection: Mutex::new(MacosCaptureSelection::None),
            selection_revision: AtomicU64::new(0),
            stream_request: Mutex::new(MacosStreamRequest::default()),
            pending_stream_request: Mutex::new(None),
            stream_request_transitions: AtomicU64::new(0),
            reject_next_stream_request: AtomicBool::new(false),
            defer_next_stream_request: AtomicBool::new(false),
            tahoe_selection: Mutex::new(None),
            host_capabilities: Mutex::new(NativeCaptureCapabilities::from_runtime(
                NativeHostArchitecture::AppleSilicon,
                true,
                MacosTahoeRuntimeProbes {
                    content_tone_mapping_info_symbol: MacosRuntimeCapability::Present,
                    screenshot_configuration_class: MacosRuntimeCapability::Present,
                    screenshot_dynamic_range_selector: MacosRuntimeCapability::Present,
                    screenshot_capture_selector: MacosRuntimeCapability::Present,
                },
            )),
            captured_at: Mutex::new(None),
            diagnostics: Mutex::new(MacosCaptureCallbackDiagnostics::default()),
        }
    }
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosCaptureControl for FixtureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.mailbox.clone()
    }

    fn set_active(&self, active: bool) {
        let previous = self.active.swap(active, Ordering::AcqRel);
        if previous == active {
            return;
        }
        self.active_transitions.fetch_add(1, Ordering::AcqRel);
        if !active {
            *lock(&self.tahoe_selection) = None;
            self.selection_revision.fetch_add(1, Ordering::AcqRel);
        }
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

    fn selection_revision(&self) -> u64 {
        self.selection_revision.load(Ordering::Acquire)
    }

    fn begin_stream_request(&self, request: MacosStreamRequest) -> anyhow::Result<StreamRequest> {
        if self
            .reject_next_stream_request
            .swap(false, Ordering::AcqRel)
        {
            anyhow::bail!("fixture rejected macOS stream request");
        }
        let generation = self
            .stream_request_transitions
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        if self.defer_next_stream_request.swap(false, Ordering::AcqRel) {
            let (completion, receiver) = mpsc::sync_channel(1);
            *lock(&self.pending_stream_request) = Some(FixturePendingStreamRequest {
                generation,
                request,
                completion,
            });
            return Ok(StreamRequest {
                generation,
                completion: Box::new(move || {
                    receiver
                        .recv()
                        .map_err(|_| anyhow!("fixture stream request completion was lost"))?
                }),
            });
        }
        *lock(&self.stream_request) = request;
        Ok(StreamRequest::completed(generation, Ok(())))
    }

    fn tahoe_selection_capabilities(&self) -> Option<NativeTahoeSelectionCapabilities> {
        lock(&self.tahoe_selection).clone()
    }

    fn host_capabilities(&self) -> NativeCaptureCapabilities {
        *lock(&self.host_capabilities)
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

    fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        *lock(&self.diagnostics)
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
        Self::source_with_compute_capacity_policy(
            config,
            admission,
            ScreenComputeCapacityPolicy::UNBOUNDED,
        )
    }

    pub fn source_with_compute_capacity_policy(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> (MacosScreenCaptureInput, Self) {
        let control = Arc::new(FixtureControl {
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            ..FixtureControl::default()
        });
        let source = MacosScreenCaptureInput::with_control_and_telemetry(
            config,
            admission,
            compute_capacity_policy,
            control.clone(),
            Arc::new(MacosScreenRuntimeTelemetry::default()),
        );
        (source, Self { control })
    }

    pub fn publish(&self, frame: MacosCaptureFrame) {
        *lock(&self.control.status) = NativeProtectedSourceState::Live;
        let mut diagnostics = lock(&self.control.diagnostics);
        diagnostics.frames_received = diagnostics.frames_received.saturating_add(1);
        diagnostics.frames_published = diagnostics.frames_published.saturating_add(1);
        drop(diagnostics);
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
        *lock(&self.control.tahoe_selection) = None;
        let mut current = lock(&self.control.selection);
        if *current != selection {
            *current = selection;
            self.control
                .selection_revision
                .fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn selection_revision(&self) -> u64 {
        self.control.selection_revision.load(Ordering::Acquire)
    }

    pub fn stream_request(&self) -> MacosStreamRequest {
        *lock(&self.control.stream_request)
    }

    pub fn stream_request_transitions(&self) -> u64 {
        self.control
            .stream_request_transitions
            .load(Ordering::Acquire)
    }

    pub fn active_transitions(&self) -> u64 {
        self.control.active_transitions.load(Ordering::Acquire)
    }

    pub fn reject_next_stream_request(&self) {
        self.control
            .reject_next_stream_request
            .store(true, Ordering::Release);
    }

    pub fn defer_next_stream_request(&self) {
        self.control
            .defer_next_stream_request
            .store(true, Ordering::Release);
    }

    pub fn pending_stream_request(&self) -> Option<MacosStreamRequest> {
        lock(&self.control.pending_stream_request)
            .as_ref()
            .map(|pending| pending.request)
    }

    pub fn commit_pending_stream_request(&self) {
        let pending = lock(&self.control.pending_stream_request)
            .take()
            .expect("fixture stream request should be pending");
        *lock(&self.control.stream_request) = pending.request;
        let _ = pending.completion.send(Ok(()));
    }

    pub fn fail_pending_stream_request(&self) {
        let pending = lock(&self.control.pending_stream_request)
            .take()
            .expect("fixture stream request should be pending");
        let _ = pending.completion.send(Err(anyhow!(
            "fixture stream request generation {} failed asynchronously",
            pending.generation
        )));
    }

    pub fn set_tahoe_selection_capabilities(
        &self,
        capabilities: Option<NativeTahoeSelectionCapabilities>,
    ) {
        *lock(&self.control.tahoe_selection) = capabilities;
    }

    pub fn set_host_capabilities(&self, capabilities: NativeCaptureCapabilities) {
        *lock(&self.control.host_capabilities) = capabilities;
    }
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
mod tests {
    use super::*;
    use crate::input::screen::{
        CpuReductionLayout, CpuReductionRequest, InputPublicationDemandRevision,
        PreparedLedToneMap, ResolvedScreenColorTransform, ScreenAdmissionCapacity,
        ScreenAspectPolicy, ScreenBranchDeliveryLifecycle, ScreenBranchPublication,
        ScreenExtentRequest, ScreenHdrPolicy, ScreenInputGraphGeneration,
        ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativeTargetPreparation,
        ScreenNativeTargetPreparer, ScreenPayloadKind, ScreenPlanBuilder, ScreenProcessingProfile,
        ScreenProcessingProfileConfig, ScreenProfileScalar, ScreenPublicationFreshness,
        ScreenPublicationKind, ScreenPublicationRequest, ScreenReductionFilter,
        ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenToneMapOperator, ScreenToneMapPolicy,
    };
    use hypercolor_macos_capture::{
        MacosAttachment, MacosCaptureColorimetry, MacosCaptureSurface, MacosColorRange,
        MacosDeliveredFrameMetadata, MacosFrameDecoder, MacosPixelExtent, MacosPointRect,
        MacosRawCapturePlane, MacosRawCaptureSample, MacosRawCompleteFrame,
        MacosRawFrameAttachments,
    };

    const BGRA8: u32 = 0x4247_5241;
    const ARGB2101010: u32 = 0x6c31_3072;
    const RGBA16_FLOAT: u32 = 0x5247_6841;
    const YUV420_VIDEO_RANGE: u32 = 0x3432_3076;
    const YUV420_FULL_RANGE: u32 = 0x3432_3066;
    const YUV44410_FULL_RANGE: u32 = 0x7866_3434;

    #[test]
    fn runtime_timing_percentiles_are_bounded_by_the_exact_maximum() {
        let timing = AtomicTimingHistogram::default();
        timing.record(Duration::from_nanos(1));
        timing.record(Duration::from_micros(250));

        let snapshot = timing.snapshot();
        assert_eq!(snapshot.sample_count, 2);
        assert_eq!(snapshot.total_ns, 250_001);
        assert_eq!(snapshot.max_ns, 250_000);
        assert_eq!(snapshot.p95_ns, 250_000);
        assert_eq!(snapshot.p99_ns, 250_000);
    }

    #[test]
    fn runtime_timing_snapshot_retries_when_population_changes() {
        let timing = AtomicTimingHistogram::default();
        timing.record(Duration::from_nanos(40));
        let mut injected = false;

        let snapshot = timing.snapshot_with_hooks(
            || {},
            || {
                if !injected {
                    timing.record(Duration::from_nanos(70));
                    injected = true;
                }
            },
        );

        assert_eq!(snapshot.sample_count, 2);
        assert_eq!(snapshot.total_ns, 110);
        assert_eq!(snapshot.max_ns, 70);
        assert_eq!(snapshot.p95_ns, 70);
        assert_eq!(snapshot.p99_ns, 70);
    }

    #[test]
    fn worker_invalidation_is_atomic_across_branches_and_stale_safe() {
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        let hub = builder.publication_hub();
        *lock(&exact.hub) = Some(Arc::clone(&hub));
        let mut runtimes = Vec::new();
        let source = source(&frame());
        exact.replace_source(Some(source.clone()));
        let surface = resolve_macos_publication_branch(
            &source,
            &cpu_demand_for_kind(
                ScreenProcessingProfile::default(),
                ScreenPublicationKind::Surface,
            ),
        )
        .expect("surface demand resolves")
        .expect("configured source owns surface demand");
        let zones = resolve_macos_publication_branch(
            &source,
            &cpu_demand_for_kind(
                ScreenProcessingProfile::default(),
                ScreenPublicationKind::Zones {
                    columns: NonZeroU32::new(2).expect("nonzero columns"),
                    rows: NonZeroU32::new(1).expect("nonzero rows"),
                },
            ),
        )
        .expect("zone demand resolves")
        .expect("configured source owns zone demand");
        let descriptors = commit_cpu_runtimes(
            &mut builder,
            &exact,
            &source,
            [surface, zones],
            &mut runtimes,
        );
        let bound_at = Instant::now();
        bind_current_macos_exact_runtime(&mut runtimes, &source, &hub, bound_at)
            .expect("current runtime binds")
            .expect("current runtime exists");
        let captured_at = bound_at + Duration::from_millis(20);
        let first = cpu_capture_frame(&source, 1, captured_at, [32, 64, 96, 255]);
        publish_cpu_frame(&exact, &mut runtimes, &source, &first);
        let leases = descriptors
            .iter()
            .map(|descriptor| hub.lease(descriptor).expect("branch lease remains live"))
            .collect::<Vec<_>>();
        let initial = leases
            .iter()
            .map(|lease| lease.observe(captured_at))
            .collect::<Vec<_>>();
        assert!(initial.iter().all(|(publication, delivery)| {
            publication.is_some()
                && delivery.lifecycle() == ScreenBranchDeliveryLifecycle::Live
                && delivery.freshness() == Some(ScreenPublicationFreshness::Fresh)
                && delivery.source_health() == Some(ScreenPublicationHealth::Healthy)
                && delivery.invalidation_epoch() == 0
        }));

        report_macos_worker_health(&exact, &runtimes, ScreenPublicationHealth::Recovering)
            .expect("recoverable health report succeeds");
        for (lease, (publication, _)) in leases.iter().zip(&initial) {
            let (retained, delivery) = lease.observe(captured_at);
            assert!(
                retained
                    .as_ref()
                    .zip(publication.as_ref())
                    .is_some_and(|(retained, publication)| Arc::ptr_eq(retained, publication))
            );
            assert_eq!(delivery.lifecycle(), ScreenBranchDeliveryLifecycle::Live);
            assert_eq!(
                delivery.source_health(),
                Some(ScreenPublicationHealth::Recovering)
            );
            assert_eq!(delivery.invalidation_epoch(), 0);
        }

        let old_binding = runtimes
            .last()
            .expect("committed runtime exists")
            .binding
            .clone();
        let publisher = hub
            .publisher(&descriptors[0], &old_binding)
            .expect("current worker owns surface publisher");
        let prepared_at = captured_at + Duration::from_millis(10);
        let intent = ScreenPublicationMetadata::try_intent(
            source.epoch.clone(),
            publisher.plan_generation(),
            NonZeroU64::new(2).expect("nonzero sequence"),
            prepared_at,
            prepared_at + Duration::from_secs(1),
        )
        .expect("pre-invalidation intent is valid");
        let prepared = hub
            .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &intent)
            .expect("pre-invalidation publication reserves");
        let worker_publication = Arc::new(Mutex::new(MacosPublication::default()));
        let mut observed_invalidation_generation = 0;
        assert!(
            synchronize_macos_invalidation_generation(
                &mut observed_invalidation_generation,
                1,
                &worker_publication,
                &exact,
                &runtimes,
            )
            .expect("new terminal generation invalidates")
        );
        assert!(matches!(
            hub.finalize_writable_publication(
                prepared,
                prepared_at + Duration::from_millis(1),
                ScreenPublicationHealth::Healthy,
            ),
            Err(ScreenPublicationHubError::PublicationInvalidated)
        ));
        let invalidated = leases
            .iter()
            .map(|lease| lease.observe(captured_at))
            .collect::<Vec<_>>();
        let invalidation_epoch = invalidated[0].1.invalidation_epoch();
        assert_ne!(invalidation_epoch, 0);
        assert!(invalidated.iter().all(|(publication, delivery)| {
            publication.is_none()
                && delivery.lifecycle() == ScreenBranchDeliveryLifecycle::Pending
                && delivery.freshness().is_none()
                && delivery.source_health() == Some(ScreenPublicationHealth::Failed)
                && delivery.invalidation_epoch() == invalidation_epoch
        }));
        assert!(
            synchronize_macos_invalidation_generation(
                &mut observed_invalidation_generation,
                1,
                &worker_publication,
                &exact,
                &runtimes,
            )
            .expect("duplicate terminal generation is accepted without reinvalidation")
        );
        assert!(leases.iter().all(|lease| {
            lease.observe(captured_at).1.invalidation_epoch() == invalidation_epoch
        }));
        assert!(
            !synchronize_macos_invalidation_generation(
                &mut observed_invalidation_generation,
                0,
                &worker_publication,
                &exact,
                &runtimes,
            )
            .expect("pre-terminal frame generation is rejected")
        );

        let recovered_at = captured_at + Duration::from_millis(20);
        let recovered = cpu_capture_frame(&source, 2, recovered_at, [96, 64, 32, 255]);
        publish_cpu_frame(&exact, &mut runtimes, &source, &recovered);
        assert!(leases.iter().all(|lease| {
            let (publication, delivery) = lease.observe(recovered_at);
            publication.is_some()
                && delivery.lifecycle() == ScreenBranchDeliveryLifecycle::Live
                && delivery.source_health() == Some(ScreenPublicationHealth::Healthy)
                && delivery.invalidation_epoch() == invalidation_epoch
        }));

        let replacement =
            resolve_macos_publication_branch(&source, &cpu_demand(transition_profile(false)))
                .expect("replacement demand resolves")
                .expect("configured source owns replacement demand");
        let replacement_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &source, replacement, &mut runtimes);
        let replacement_bound_at = recovered_at + Duration::from_millis(20);
        bind_current_macos_exact_runtime(&mut runtimes, &source, &hub, replacement_bound_at)
            .expect("replacement runtime binds")
            .expect("replacement runtime exists");
        let replacement_at = replacement_bound_at + Duration::from_millis(20);
        let replacement_frame = cpu_capture_frame(&source, 3, replacement_at, [48, 48, 48, 255]);
        publish_cpu_frame(&exact, &mut runtimes, &source, &replacement_frame);
        let replacement_lease = hub
            .lease(&replacement_descriptor)
            .expect("replacement lease is committed");
        let replacement_publication = replacement_lease
            .read()
            .expect("replacement branch published");
        assert!(matches!(
            hub.invalidate_worker(&old_binding),
            Err(ScreenPublicationHubError::WorkerAuthorityStale { .. })
        ));
        let after_stale_invalidation = replacement_lease
            .read()
            .expect("stale worker cannot clear replacement publication");
        assert!(Arc::ptr_eq(
            &replacement_publication,
            &after_stale_invalidation
        ));
    }

    #[test]
    fn cpu_reduction_timing_excludes_frames_when_branch_cadence_is_not_due() {
        let native_frame = frame();
        let native_source = source(&native_frame);
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        exact.replace_source(Some(native_source.clone()));
        let demand = cpu_demand_for_kind_at_hz(
            ScreenProcessingProfile::default(),
            ScreenPublicationKind::Surface,
            NonZeroU32::MIN,
        );
        let resolved = resolve_macos_publication_branch(&native_source, &demand)
            .expect("CPU demand resolves")
            .expect("configured source owns CPU demand");
        let mut runtimes = Vec::new();
        commit_cpu_runtime(
            &mut builder,
            &exact,
            &native_source,
            resolved,
            &mut runtimes,
        );
        let telemetry = MacosScreenRuntimeTelemetry::default();
        let captured_at = Instant::now();
        let first = native_cpu_capture_frame(
            &native_frame,
            captured_at,
            captured_at + Duration::from_secs(2),
            &native_source,
            native_source.epoch.source_id.clone(),
        )
        .expect("first native scalar envelope is valid");
        publish_macos_scalar_exact(
            &native_frame,
            &first,
            &native_source,
            &exact,
            &mut runtimes,
            &telemetry,
        )
        .expect("first due CPU branch publishes");
        assert_eq!(telemetry.cpu_reduction_timing.snapshot().sample_count, 1);

        let mut next_native_frame = (*native_frame).clone();
        next_native_frame.sequence = next_native_frame
            .sequence
            .checked_add(1)
            .expect("fixture sequence advances");
        let next_native_frame = Arc::new(next_native_frame);
        let next = native_cpu_capture_frame(
            &next_native_frame,
            captured_at,
            captured_at + Duration::from_secs(2),
            &native_source,
            native_source.epoch.source_id.clone(),
        )
        .expect("second native scalar envelope is valid");
        publish_macos_scalar_exact(
            &next_native_frame,
            &next,
            &native_source,
            &exact,
            &mut runtimes,
            &telemetry,
        )
        .expect("not-due CPU branch is skipped");
        assert_eq!(telemetry.cpu_reduction_timing.snapshot().sample_count, 1);
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
        frame_with_color(
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            BGRA8,
            &[0, 0, 255, 255],
            None,
        )
    }

    fn frame_with_color(
        color: MacosCaptureColorimetry,
        pixel_format_fourcc: u32,
        encoded_pixel: &[u8],
        delivered: Option<MacosDeliveredFrameMetadata>,
    ) -> Arc<MacosCaptureFrame> {
        let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
        let byte_len = u64::try_from(encoded_pixel.len() * 8).expect("fixture length fits");
        let mut surface = MacosCaptureSurface::new_cpu_fixture(
            7,
            byte_len,
            1,
            vec![Arc::<[u8]>::from(encoded_pixel.repeat(8))],
        )
        .expect("fixture surface is valid");
        if let Some(delivered) = delivered {
            surface = surface
                .with_delivery_metadata(delivered)
                .expect("fixture delivery metadata is valid");
        }
        let sample = MacosRawCaptureSample {
            frame: Some(MacosRawCompleteFrame {
                storage_extent: extent,
                planes: vec![MacosRawCapturePlane {
                    index: 0,
                    extent,
                    bytes_per_row: encoded_pixel.len() * 4,
                    length_bytes: byte_len,
                }],
                pixel_format_fourcc,
                color,
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

    fn frame_with_planes(
        color: MacosCaptureColorimetry,
        pixel_format_fourcc: u32,
        planes: &[(&[u8], MacosPixelExtent, usize)],
        delivered: Option<MacosDeliveredFrameMetadata>,
    ) -> Arc<MacosCaptureFrame> {
        let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
        let allocation_bytes = planes
            .iter()
            .try_fold(0_u64, |total, (bytes, _, _)| {
                total.checked_add(u64::try_from(bytes.len()).ok()?)
            })
            .expect("fixture allocation fits");
        let mut surface = MacosCaptureSurface::new_cpu_fixture(
            7,
            allocation_bytes,
            1,
            planes
                .iter()
                .map(|(bytes, _, _)| Arc::<[u8]>::from(*bytes))
                .collect(),
        )
        .expect("fixture surface is valid");
        if let Some(delivered) = delivered {
            surface = surface
                .with_delivery_metadata(delivered)
                .expect("fixture delivery metadata is valid");
        }
        let sample = MacosRawCaptureSample {
            frame: Some(MacosRawCompleteFrame {
                storage_extent: extent,
                planes: planes
                    .iter()
                    .enumerate()
                    .map(|(index, (bytes, extent, stride))| MacosRawCapturePlane {
                        index: u32::try_from(index).expect("fixture plane index fits"),
                        extent: *extent,
                        bytes_per_row: *stride,
                        length_bytes: u64::try_from(bytes.len()).expect("fixture length fits"),
                    })
                    .collect(),
                pixel_format_fourcc,
                color,
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

    fn cpu_demand(profile: ScreenProcessingProfile) -> RegisteredScreenBranchDemand {
        cpu_demand_for_kind(profile, ScreenPublicationKind::Surface)
    }

    fn cpu_demand_for_kind(
        profile: ScreenProcessingProfile,
        kind: ScreenPublicationKind,
    ) -> RegisteredScreenBranchDemand {
        cpu_demand_for_kind_at_hz(profile, kind, NonZeroU32::new(60).expect("nonzero cadence"))
    }

    fn cpu_demand_for_kind_at_hz(
        profile: ScreenProcessingProfile,
        kind: ScreenPublicationKind,
        requested_hz: NonZeroU32,
    ) -> RegisteredScreenBranchDemand {
        RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                kind,
                ScreenPublicationExecutorRequest::Cpu,
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Cover,
                Arc::new(profile),
            ),
            requested_hz,
        )
    }

    fn execute_resolved_cpu(
        source: &MacosPublicationSource,
        descriptor: &ResolvedScreenPublicationDescriptor,
        encoded_bgra: [u8; 4],
    ) -> Vec<u8> {
        let source_extent = source.logical_extent;
        let source_bytes = Arc::<[u8]>::from(
            encoded_bgra.repeat(
                usize::try_from(source_extent.width() * source_extent.height())
                    .expect("fixture pixel count fits"),
            ),
        );
        let storage = CpuCaptureStorage::new(
            source_bytes,
            CapturePixelFormat::Bgra8,
            i64::from(source_extent.width()) * 4,
            0,
        );
        let layout =
            CpuReductionLayout::new(source_extent, descriptor.physical().reduction_extent())
                .expect("fixture reduction layout is valid");
        let mut output = vec![0; layout.target_byte_len_usize()];
        CpuReductionExecutor::new(NonZeroUsize::MIN, NonZeroU32::MIN)
            .expect("fixture executor prepares")
            .reduce(
                CpuReductionRequest::new(
                    &storage,
                    layout,
                    descriptor.physical().target_pixel_format(),
                    descriptor.physical().reduction_filter(),
                    descriptor.physical().color_pipeline(),
                ),
                &mut output,
            )
            .expect("resolved macOS CPU color pipeline executes");
        output
    }

    fn commit_cpu_runtime(
        builder: &mut ScreenPlanBuilder,
        exact: &MacosExactPublicationShared,
        source: &MacosPublicationSource,
        resolved: ResolvedScreenBranchDemand,
        runtimes: &mut Vec<MacosExactRuntime>,
    ) -> ResolvedScreenPublicationDescriptor {
        commit_cpu_runtimes(builder, exact, source, [resolved], runtimes)
            .pop()
            .expect("single-demand fixture commits one descriptor")
    }

    fn commit_cpu_runtimes(
        builder: &mut ScreenPlanBuilder,
        exact: &MacosExactPublicationShared,
        source: &MacosPublicationSource,
        resolved: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
        runtimes: &mut Vec<MacosExactRuntime>,
    ) -> Vec<ResolvedScreenPublicationDescriptor> {
        let resolved = resolved.into_iter().collect::<Vec<_>>();
        let descriptors = resolved
            .iter()
            .map(|demand| demand.descriptor().clone())
            .collect();
        let revision = builder
            .current()
            .demand_revision()
            .next()
            .expect("fixture demand revision advances");
        let graph = ScreenInputGraphGeneration::new(1);
        let mut preparing = builder
            .prepare(
                resolved,
                None,
                revision,
                graph,
                ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
            )
            .expect("macOS CPU candidate plan prepares");
        let ticket = preparing
            .worker_ticket(&source.epoch.source_id)
            .expect("macOS source owns the candidate worker");
        let (token, runtime) = prepare_macos_exact_runtime(ticket, Some(source), exact)
            .expect("macOS CPU runtime prepares");
        let (runtime, owned_source) = runtime.expect("CPU plan owns a runtime");
        exact.register_owned_source(owned_source);
        runtimes.push(runtime);
        preparing
            .acknowledge(token)
            .expect("macOS CPU worker token matches candidate");
        let armed = preparing
            .arm(builder.current().generation(), revision, graph)
            .unwrap_or_else(|failure| panic!("macOS CPU plan arms: {}", failure.error()));
        let committed = builder
            .commit(armed, revision, graph)
            .unwrap_or_else(|failure| panic!("macOS CPU plan commits: {}", failure.error()));
        let (_, retirement) = committed.into_parts();
        drop(retirement);
        descriptors
    }

    fn cpu_capture_frame(
        source: &MacosPublicationSource,
        sequence: u64,
        captured_at: Instant,
        encoded_bgra: [u8; 4],
    ) -> CaptureFrame<RawCaptureSurface> {
        let byte_len = usize::try_from(
            u64::from(source.geometry.storage_extent().width())
                * u64::from(source.geometry.storage_extent().height())
                * 4,
        )
        .expect("fixture CPU bytes fit");
        CaptureFrame::new(
            CaptureFrameMetadata {
                source_id: source.epoch.source_id.clone(),
                topology_generation: source.epoch.topology_generation,
                session_generation: source.epoch.session_generation,
                sequence,
                captured_at,
                fresh_until: captured_at + Duration::from_secs(1),
                geometry: source.geometry,
                colorimetry: source.colorimetry,
                cursor: CaptureCursor {
                    visible: false,
                    position: None,
                    hotspot: None,
                    shape_extent: None,
                    shape_generation: None,
                    content: CaptureCursorContent::Hidden,
                },
            },
            CaptureStorage::Cpu(CpuCaptureStorage::new(
                Arc::from(encoded_bgra.repeat(byte_len / 4)),
                CapturePixelFormat::Bgra8,
                i64::from(source.geometry.storage_extent().width()) * 4,
                0,
            )),
            CaptureDamage::new(Vec::new(), Vec::new()),
        )
        .expect("fixture CPU frame is valid")
    }

    fn publish_cpu_bytes(
        exact: &MacosExactPublicationShared,
        runtimes: &mut [MacosExactRuntime],
        source: &MacosPublicationSource,
        descriptor: &ResolvedScreenPublicationDescriptor,
        frame: &CaptureFrame<RawCaptureSurface>,
    ) -> Vec<u8> {
        publish_cpu_frame(exact, runtimes, source, frame);
        published_surface_bytes(exact, descriptor)
    }

    fn publish_cpu_frame(
        exact: &MacosExactPublicationShared,
        runtimes: &mut [MacosExactRuntime],
        source: &MacosPublicationSource,
        frame: &CaptureFrame<RawCaptureSurface>,
    ) {
        let hub = exact.hub().expect("fixture hub remains installed");
        let runtime =
            bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)
                .expect("current macOS runtime binds")
                .expect("committed runtime is current");
        let report = runtime
            .fanout
            .as_mut()
            .expect("CPU runtime owns a fanout")
            .publish_due(
                &hub,
                Some(frame),
                frame.metadata().captured_at,
                ScreenPublicationHealth::Healthy,
            )
            .expect("CPU fanout publishes");
        assert!(
            report.published() > 0,
            "CPU fixture had no due branch: {report:?}"
        );
    }

    fn publish_scalar_frame(
        exact: &MacosExactPublicationShared,
        runtimes: &mut [MacosExactRuntime],
        source: &MacosPublicationSource,
        frame: &Arc<MacosCaptureFrame>,
        captured_at: Instant,
    ) {
        let capture = native_cpu_capture_frame(
            frame,
            captured_at,
            captured_at + Duration::from_secs(1),
            source,
            source.epoch.source_id.clone(),
        )
        .expect("native scalar fixture envelope is valid");
        let hub = exact.hub().expect("fixture hub remains installed");
        let runtime = bind_current_macos_exact_runtime(runtimes, source, &hub, captured_at)
            .expect("current macOS runtime binds")
            .expect("committed runtime is current");
        let report = runtime
            .fanout
            .as_mut()
            .expect("CPU runtime owns a fanout")
            .publish_due_scalar(
                &hub,
                &capture,
                captured_at,
                ScreenPublicationHealth::Healthy,
                |execute| {
                    frame
                        .with_cpu_source(|samples| execute(&samples))
                        .map_err(|error| {
                            CpuPublicationFanoutError::ScalarSourceAccessFailed(error.to_string())
                        })?
                },
            )
            .expect("native scalar fanout publishes");
        assert!(report.published() > 0);
    }

    fn active_tone_map_transition_count(
        exact: &MacosExactPublicationShared,
        runtimes: &mut [MacosExactRuntime],
        source: &MacosPublicationSource,
        captured_at: Instant,
    ) -> usize {
        let hub = exact.hub().expect("fixture hub remains installed");
        bind_current_macos_exact_runtime(runtimes, source, &hub, captured_at)
            .expect("current macOS runtime binds")
            .expect("committed runtime is current")
            .fanout
            .as_ref()
            .expect("CPU runtime owns a fanout")
            .active_tone_map_transition_count()
    }

    fn published_surface_bytes(
        exact: &MacosExactPublicationShared,
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Vec<u8> {
        let hub = exact.hub().expect("fixture hub remains installed");
        let lease = hub
            .lease(descriptor)
            .expect("committed Surface branch has a lease");
        let publication = lease.read().expect("Surface branch has published bytes");
        let ScreenBranchPayload::Surface(surface) = publication.payload() else {
            panic!("fixture branch publishes Surface bytes");
        };
        surface.pixels().to_vec()
    }

    fn published_zone_colors(
        exact: &MacosExactPublicationShared,
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Vec<[u8; 3]> {
        let hub = exact.hub().expect("fixture hub remains installed");
        let lease = hub
            .lease(descriptor)
            .expect("committed Zones branch has a lease");
        let publication = lease.read().expect("Zones branch has published colors");
        let ScreenBranchPayload::Zones(zones) = publication.payload() else {
            panic!("fixture branch publishes zone colors");
        };
        zones.colors().to_vec()
    }

    fn transition_profile(hdr: bool) -> ScreenProcessingProfile {
        transition_profile_with_smoothing(hdr, ScreenSmoothingPolicy::Disabled)
    }

    fn transition_profile_with_smoothing(
        hdr: bool,
        smoothing: ScreenSmoothingPolicy,
    ) -> ScreenProcessingProfile {
        let calibration = LedToneMapCalibration::DEFAULT;
        transition_profile_with_calibration(hdr, smoothing, calibration)
    }

    fn transition_profile_with_calibration(
        hdr: bool,
        smoothing: ScreenSmoothingPolicy,
        calibration: LedToneMapCalibration,
    ) -> ScreenProcessingProfile {
        ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            smoothing,
            hdr: if hdr {
                ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                    ScreenToneMapOperator::Bt2390Eetf,
                    calibration,
                ))
            } else {
                ScreenHdrPolicy::Reject
            },
            ..ScreenProcessingProfileConfig::default()
        })
        .with_led_tone_map(calibration)
    }

    fn hdr_transition_source(sdr_source: &MacosPublicationSource) -> MacosPublicationSource {
        let hdr_color = CaptureColorimetry::new(
            CaptureColorSpace::Srgb,
            CaptureTransferFunction::Pq,
            Some(CaptureDynamicRange::High),
            Some(
                CaptureLuminanceContext::new(
                    CapturePositiveScalar::try_new(203.0).expect("reference white is valid"),
                    CapturePositiveScalar::try_new(1_000.0).expect("peak is valid"),
                )
                .expect("HDR luminance is ordered"),
            ),
        )
        .expect("HDR fixture colorimetry is valid");
        MacosPublicationSource {
            colorimetry: hdr_color,
            ..sdr_source.clone()
        }
    }

    #[test]
    fn delivered_hdr_luminance_is_required_and_mapped_exactly() {
        assert_eq!(
            capture_colorimetry(&frame()).expect("SDR remains valid without delivery luminance"),
            CaptureColorimetry::SRGB
        );
        let hdr_color = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Rec2020,
            transfer: MacosTransferFunction::Pq,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        };
        let headroom = 1_000.0 / 203.0;
        let delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            hdr_color,
            Some(203.0),
            Some(headroom),
        )
        .expect("complete HDR metadata is valid");
        let hdr = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(delivered));
        let colorimetry = capture_colorimetry(&hdr).expect("complete HDR colorimetry maps");
        let luminance = colorimetry.luminance().expect("HDR luminance is retained");
        assert_eq!(luminance.reference_white_nits().value(), 203.0);
        assert_eq!(luminance.peak_nits().value(), 203.0 * headroom);

        let linear_hdr_color = MacosCaptureColorimetry {
            transfer: MacosTransferFunction::Linear,
            ..hdr_color
        };
        let linear_delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            linear_hdr_color,
            Some(203.0),
            Some(headroom),
        )
        .expect("extended-linear HDR metadata is valid");
        let linear_hdr = frame_with_color(
            linear_hdr_color,
            RGBA16_FLOAT,
            &[0; 8],
            Some(linear_delivered),
        );
        let linear_colorimetry =
            capture_colorimetry(&linear_hdr).expect("extended-linear HDR colorimetry maps");
        assert_eq!(
            linear_colorimetry.transfer_function(),
            CaptureTransferFunction::Linear
        );
        assert_eq!(
            linear_colorimetry.dynamic_range(),
            Some(CaptureDynamicRange::High)
        );
        assert_eq!(linear_colorimetry.luminance(), colorimetry.luminance());
        let missing_linear = frame_with_color(linear_hdr_color, RGBA16_FLOAT, &[0; 8], None);
        assert!(capture_colorimetry(&missing_linear).is_err());

        let missing = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], None);
        assert!(capture_colorimetry(&missing).is_err());

        let no_reference_white = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            hdr_color,
            None,
            Some(headroom),
        )
        .expect("capture layer admits optional reference white");
        let no_reference_white =
            frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(no_reference_white));
        assert!(capture_colorimetry(&no_reference_white).is_err());

        let no_headroom = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            hdr_color,
            Some(203.0),
            None,
        )
        .expect("capture layer admits optional headroom");
        let no_headroom = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(no_headroom));
        assert!(capture_colorimetry(&no_headroom).is_err());

        let no_peak_headroom = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            hdr_color,
            Some(203.0),
            Some(1.0),
        )
        .expect("capture layer admits unity headroom");
        let no_peak_headroom =
            frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(no_peak_headroom));
        assert!(capture_colorimetry(&no_peak_headroom).is_err());

        let contradictory_color = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::DisplayP3,
            ..hdr_color
        };
        let contradictory = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            contradictory_color,
            Some(203.0),
            Some(headroom),
        )
        .expect("alternate HDR metadata is valid in isolation");
        let contradictory = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(contradictory));
        assert!(capture_colorimetry(&contradictory).is_err());
    }

    #[test]
    fn macos_cpu_resolves_p3_and_full_precision_hdr() {
        let p3_color = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::DisplayP3,
            transfer: MacosTransferFunction::Linear,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        };
        let p3_frame = frame_with_color(p3_color, BGRA8, &[255, 0, 255, 255], None);
        let p3_source = source(&p3_frame);
        let p3_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            ..ScreenProcessingProfileConfig::default()
        });
        let p3 = resolve_macos_publication_branch(&p3_source, &cpu_demand(p3_profile))
            .expect("P3 macOS demand resolves")
            .expect("configured source owns P3 demand");
        assert!(matches!(
            p3.descriptor().physical().color_pipeline().transform(),
            ResolvedScreenColorTransform::LinearRelativeColorimetric { .. }
        ));
        assert_eq!(
            &execute_resolved_cpu(&p3_source, p3.descriptor(), [255, 0, 255, 255])[..4],
            [255, 59, 242, 255]
        );

        let hdr_color = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Rec2020,
            transfer: MacosTransferFunction::Pq,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        };
        let delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            hdr_color,
            Some(203.0),
            Some(1_000.0 / 203.0),
        )
        .expect("HDR delivery metadata is valid");
        let hdr_frame = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(delivered));
        let hdr_source = source(&hdr_frame);
        let calibration = LedToneMapCalibration::DEFAULT;
        let hdr_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                ScreenToneMapOperator::Bt2390Eetf,
                calibration,
            )),
            ..ScreenProcessingProfileConfig::default()
        })
        .with_led_tone_map(calibration);
        let hdr = resolve_macos_publication_branch(&hdr_source, &cpu_demand(hdr_profile))
            .expect("full-precision HDR CPU demand resolves")
            .expect("configured source owns HDR demand");
        assert_eq!(
            hdr.descriptor().source_pixel_format(),
            CapturePixelFormat::Rgba16Float
        );
        assert!(matches!(
            hdr.descriptor().physical().color_pipeline().transform(),
            ResolvedScreenColorTransform::ToneMap(_)
        ));
    }

    #[test]
    fn macos_publication_transition_is_deterministic_at_zero_midpoint_and_completion() {
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let mut runtimes = Vec::new();
        let base_frame = frame();
        let sdr_source = source(&base_frame);
        exact.replace_source(Some(sdr_source.clone()));
        let sdr =
            resolve_macos_publication_branch(&sdr_source, &cpu_demand(transition_profile(false)))
                .expect("SDR transition branch resolves")
                .expect("configured source owns SDR transition branch");
        let sdr_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &sdr_source, sdr, &mut runtimes);
        let started = Instant::now() + Duration::from_millis(20);
        let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [148, 148, 148, 255]);
        let sdr_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &sdr_source,
            &sdr_descriptor,
            &sdr_frame,
        );
        assert_eq!(&sdr_bytes[..4], [148, 148, 148, 255]);

        let hdr_source = hdr_transition_source(&sdr_source);
        exact.replace_source(Some(hdr_source.clone()));
        let hdr =
            resolve_macos_publication_branch(&hdr_source, &cpu_demand(transition_profile(true)))
                .expect("HDR transition branch resolves")
                .expect("configured source owns HDR transition branch");
        let hdr_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &hdr_source, hdr, &mut runtimes);
        let at_zero = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
        let zero_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &hdr_source,
            &hdr_descriptor,
            &at_zero,
        );
        let at_midpoint = cpu_capture_frame(
            &hdr_source,
            3,
            started + Duration::from_millis(125),
            [148, 148, 148, 255],
        );
        let midpoint_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &hdr_source,
            &hdr_descriptor,
            &at_midpoint,
        );
        let at_complete = cpu_capture_frame(
            &hdr_source,
            4,
            started + Duration::from_millis(250),
            [148, 148, 148, 255],
        );
        let complete_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &hdr_source,
            &hdr_descriptor,
            &at_complete,
        );
        assert_eq!(&zero_bytes[..4], [255, 255, 255, 255]);
        assert_eq!(&midpoint_bytes[..4], [223, 223, 223, 255]);
        assert_eq!(&complete_bytes[..4], [187, 187, 187, 255]);
    }

    #[test]
    fn macos_transition_inheritance_skips_matching_routes_without_curve_state() {
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let mut runtimes = Vec::new();
        let base_frame = frame();
        let sdr_source = source(&base_frame);
        exact.replace_source(Some(sdr_source.clone()));
        let identity_profile = ScreenProcessingProfile::new(
            ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Bgra8),
        );
        let calibration = LedToneMapCalibration::DEFAULT;
        let managed_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            target_pixel_format: CapturePixelFormat::Bgra8,
            ..ScreenProcessingProfileConfig::default()
        })
        .with_led_tone_map(calibration);
        let identity = resolve_macos_publication_branch(&sdr_source, &cpu_demand(identity_profile))
            .expect("encoded-identity branch resolves")
            .expect("configured source owns encoded-identity branch");
        let managed = resolve_macos_publication_branch(&sdr_source, &cpu_demand(managed_profile))
            .expect("managed SDR branch resolves")
            .expect("configured source owns managed SDR branch");
        let sdr_descriptors = commit_cpu_runtimes(
            &mut builder,
            &exact,
            &sdr_source,
            [identity, managed],
            &mut runtimes,
        );
        let started = Instant::now() + Duration::from_millis(20);
        let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [148, 148, 148, 255]);
        publish_cpu_frame(&exact, &mut runtimes, &sdr_source, &sdr_frame);
        assert_eq!(sdr_descriptors.len(), 2);

        let hdr_source = hdr_transition_source(&sdr_source);
        exact.replace_source(Some(hdr_source.clone()));
        let hdr_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            target_pixel_format: CapturePixelFormat::Bgra8,
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                ScreenToneMapOperator::Bt2390Eetf,
                calibration,
            )),
            ..ScreenProcessingProfileConfig::default()
        })
        .with_led_tone_map(calibration);
        let hdr = resolve_macos_publication_branch(&hdr_source, &cpu_demand(hdr_profile))
            .expect("managed HDR branch resolves")
            .expect("configured source owns managed HDR branch");
        let hdr_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &hdr_source, hdr, &mut runtimes);
        let transition_start = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
        assert_eq!(
            &publish_cpu_bytes(
                &exact,
                &mut runtimes,
                &hdr_source,
                &hdr_descriptor,
                &transition_start,
            )[..4],
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn macos_publication_transition_restarts_from_its_midpoint_curve() {
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let mut runtimes = Vec::new();
        let base_frame = frame();
        let sdr_source = source(&base_frame);
        exact.replace_source(Some(sdr_source.clone()));
        let sdr =
            resolve_macos_publication_branch(&sdr_source, &cpu_demand(transition_profile(false)))
                .expect("SDR transition branch resolves")
                .expect("configured source owns SDR transition branch");
        let sdr_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &sdr_source, sdr, &mut runtimes);
        let started = Instant::now() + Duration::from_millis(20);
        let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [148, 148, 148, 255]);
        assert_eq!(
            &publish_cpu_bytes(
                &exact,
                &mut runtimes,
                &sdr_source,
                &sdr_descriptor,
                &sdr_frame,
            )[..4],
            [148, 148, 148, 255]
        );

        let hdr_source = hdr_transition_source(&sdr_source);
        exact.replace_source(Some(hdr_source.clone()));
        let hdr =
            resolve_macos_publication_branch(&hdr_source, &cpu_demand(transition_profile(true)))
                .expect("HDR transition branch resolves")
                .expect("configured source owns HDR transition branch");
        let hdr_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &hdr_source, hdr, &mut runtimes);
        let at_zero = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
        assert_eq!(
            &publish_cpu_bytes(
                &exact,
                &mut runtimes,
                &hdr_source,
                &hdr_descriptor,
                &at_zero,
            )[..4],
            [255, 255, 255, 255]
        );
        let at_midpoint = cpu_capture_frame(
            &hdr_source,
            3,
            started + Duration::from_millis(125),
            [148, 148, 148, 255],
        );
        assert_eq!(
            &publish_cpu_bytes(
                &exact,
                &mut runtimes,
                &hdr_source,
                &hdr_descriptor,
                &at_midpoint,
            )[..4],
            [223, 223, 223, 255]
        );

        exact.replace_source(Some(sdr_source.clone()));
        let restarted_sdr =
            resolve_macos_publication_branch(&sdr_source, &cpu_demand(transition_profile(false)))
                .expect("restarted SDR branch resolves")
                .expect("configured source owns restarted SDR branch");
        let restarted_descriptor = commit_cpu_runtime(
            &mut builder,
            &exact,
            &sdr_source,
            restarted_sdr,
            &mut runtimes,
        );
        let restart_boundary = cpu_capture_frame(
            &sdr_source,
            5,
            started + Duration::from_millis(125),
            [255, 255, 255, 255],
        );
        let restart_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &sdr_source,
            &restarted_descriptor,
            &restart_boundary,
        );
        let restart_midpoint = cpu_capture_frame(
            &sdr_source,
            6,
            started + Duration::from_millis(250),
            [255, 255, 255, 255],
        );
        let restart_midpoint_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &sdr_source,
            &restarted_descriptor,
            &restart_midpoint,
        );
        let restart_complete = cpu_capture_frame(
            &sdr_source,
            7,
            started + Duration::from_millis(375),
            [255, 255, 255, 255],
        );
        let restart_complete_bytes = publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &sdr_source,
            &restarted_descriptor,
            &restart_complete,
        );
        assert_eq!(&restart_bytes[..4], [224, 224, 224, 255]);
        assert_eq!(&restart_midpoint_bytes[..4], [238, 238, 238, 255]);
        assert_eq!(&restart_complete_bytes[..4], [255, 255, 255, 255]);
    }

    #[test]
    fn sdr_exposure_reconfiguration_swaps_atomically_without_transition() {
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let mut runtimes = Vec::new();
        let source = source(&frame());
        exact.replace_source(Some(source.clone()));
        let initial =
            resolve_macos_publication_branch(&source, &cpu_demand(transition_profile(false)))
                .expect("initial SDR branch resolves")
                .expect("configured source owns the initial SDR branch");
        let initial_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &source, initial, &mut runtimes);
        let started = Instant::now() + Duration::from_millis(20);
        let initial_frame = cpu_capture_frame(&source, 1, started, [96, 96, 96, 255]);
        publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &source,
            &initial_descriptor,
            &initial_frame,
        );

        let default = LedToneMapCalibration::DEFAULT;
        let calibration = LedToneMapCalibration::try_new(
            default.target_white_x(),
            default.target_white_y(),
            default.target_reference_white_nits(),
            default.target_peak_nits(),
            1.0,
        )
        .expect("updated SDR exposure is valid");
        let next = resolve_macos_publication_branch(
            &source,
            &cpu_demand(transition_profile_with_calibration(
                false,
                ScreenSmoothingPolicy::Disabled,
                calibration,
            )),
        )
        .expect("updated SDR branch resolves")
        .expect("configured source owns the updated SDR branch");
        let next_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &source, next, &mut runtimes);
        let boundary = started + Duration::from_millis(20);
        assert_eq!(
            active_tone_map_transition_count(&exact, &mut runtimes, &source, boundary),
            0
        );
        let encoded = [96, 96, 96, 255];
        let expected = execute_resolved_cpu(&source, &next_descriptor, encoded);
        let at_zero = cpu_capture_frame(&source, 2, boundary, encoded);
        assert_eq!(
            publish_cpu_bytes(&exact, &mut runtimes, &source, &next_descriptor, &at_zero,),
            expected
        );
        let at_midpoint =
            cpu_capture_frame(&source, 3, boundary + Duration::from_millis(125), encoded);
        assert_eq!(
            publish_cpu_bytes(
                &exact,
                &mut runtimes,
                &source,
                &next_descriptor,
                &at_midpoint,
            ),
            expected
        );
        assert_eq!(
            active_tone_map_transition_count(
                &exact,
                &mut runtimes,
                &source,
                boundary + Duration::from_millis(125),
            ),
            0
        );
    }

    #[test]
    fn hdr_calibration_reconfiguration_swaps_atomically_without_transition() {
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let mut runtimes = Vec::new();
        let source = hdr_transition_source(&source(&frame()));
        exact.replace_source(Some(source.clone()));
        let initial =
            resolve_macos_publication_branch(&source, &cpu_demand(transition_profile(true)))
                .expect("initial HDR branch resolves")
                .expect("configured source owns the initial HDR branch");
        let initial_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &source, initial, &mut runtimes);
        let started = Instant::now() + Duration::from_millis(20);
        let initial_frame = cpu_capture_frame(&source, 1, started, [148, 148, 148, 255]);
        publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &source,
            &initial_descriptor,
            &initial_frame,
        );

        let default = LedToneMapCalibration::DEFAULT;
        let calibration = LedToneMapCalibration::try_new(
            default.target_white_x(),
            default.target_white_y(),
            160.0,
            640.0,
            default.exposure_ev(),
        )
        .expect("updated HDR calibration is valid");
        let next = resolve_macos_publication_branch(
            &source,
            &cpu_demand(transition_profile_with_calibration(
                true,
                ScreenSmoothingPolicy::Disabled,
                calibration,
            )),
        )
        .expect("updated HDR branch resolves")
        .expect("configured source owns the updated HDR branch");
        let next_descriptor =
            commit_cpu_runtime(&mut builder, &exact, &source, next, &mut runtimes);
        let boundary = started + Duration::from_millis(20);
        assert_eq!(
            active_tone_map_transition_count(&exact, &mut runtimes, &source, boundary),
            0
        );
        let encoded = [148, 148, 148, 255];
        let expected = execute_resolved_cpu(&source, &next_descriptor, encoded);
        let at_zero = cpu_capture_frame(&source, 2, boundary, encoded);
        assert_eq!(
            publish_cpu_bytes(&exact, &mut runtimes, &source, &next_descriptor, &at_zero,),
            expected
        );
        let at_midpoint =
            cpu_capture_frame(&source, 3, boundary + Duration::from_millis(125), encoded);
        assert_eq!(
            publish_cpu_bytes(
                &exact,
                &mut runtimes,
                &source,
                &next_descriptor,
                &at_midpoint,
            ),
            expected
        );
        assert_eq!(
            active_tone_map_transition_count(
                &exact,
                &mut runtimes,
                &source,
                boundary + Duration::from_millis(125),
            ),
            0
        );
    }

    #[test]
    fn macos_publication_samples_once_and_suppresses_both_scene_cut_paths() {
        let smoothing = ScreenSmoothingPolicy::Exponential {
            time_constant: Duration::from_mins(1),
            scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
                threshold: ScreenProfileScalar::try_new(0.01)
                    .expect("scene-cut threshold is valid"),
            },
        };
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        let mut runtimes = Vec::new();
        let base_frame = frame();
        let sdr_source = source(&base_frame);
        exact.replace_source(Some(sdr_source.clone()));
        let sdr_profile = transition_profile_with_smoothing(false, smoothing);
        let sdr_surface = resolve_macos_publication_branch(
            &sdr_source,
            &cpu_demand_for_kind(sdr_profile.clone(), ScreenPublicationKind::Surface),
        )
        .expect("SDR Surface branch resolves")
        .expect("configured source owns SDR Surface branch");
        let sdr_zones = resolve_macos_publication_branch(
            &sdr_source,
            &cpu_demand_for_kind(
                sdr_profile,
                ScreenPublicationKind::Zones {
                    columns: NonZeroU32::MIN,
                    rows: NonZeroU32::MIN,
                },
            ),
        )
        .expect("SDR Zones branch resolves")
        .expect("configured source owns SDR Zones branch");
        let sdr_descriptors = commit_cpu_runtimes(
            &mut builder,
            &exact,
            &sdr_source,
            [sdr_surface, sdr_zones],
            &mut runtimes,
        );
        assert_eq!(sdr_descriptors.len(), 2);
        assert_eq!(sdr_descriptors[0].physical(), sdr_descriptors[1].physical());
        let started = Instant::now() + Duration::from_millis(20);
        let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [255, 255, 255, 255]);
        publish_cpu_frame(&exact, &mut runtimes, &sdr_source, &sdr_frame);
        assert_eq!(
            &published_surface_bytes(&exact, &sdr_descriptors[0])[..4],
            [255, 255, 255, 255]
        );
        assert_eq!(
            published_zone_colors(&exact, &sdr_descriptors[1])[0],
            [255, 255, 255]
        );

        let hdr_source = hdr_transition_source(&sdr_source);
        exact.replace_source(Some(hdr_source.clone()));
        let hdr_profile = transition_profile_with_smoothing(true, smoothing);
        let hdr_surface = resolve_macos_publication_branch(
            &hdr_source,
            &cpu_demand_for_kind(hdr_profile.clone(), ScreenPublicationKind::Surface),
        )
        .expect("HDR Surface branch resolves")
        .expect("configured source owns HDR Surface branch");
        let hdr_zones = resolve_macos_publication_branch(
            &hdr_source,
            &cpu_demand_for_kind(
                hdr_profile,
                ScreenPublicationKind::Zones {
                    columns: NonZeroU32::MIN,
                    rows: NonZeroU32::MIN,
                },
            ),
        )
        .expect("HDR Zones branch resolves")
        .expect("configured source owns HDR Zones branch");
        let hdr_descriptors = commit_cpu_runtimes(
            &mut builder,
            &exact,
            &hdr_source,
            [hdr_surface, hdr_zones],
            &mut runtimes,
        );
        assert_eq!(hdr_descriptors.len(), 2);
        assert_eq!(hdr_descriptors[0].physical(), hdr_descriptors[1].physical());
        let transition_start = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
        publish_cpu_frame(&exact, &mut runtimes, &hdr_source, &transition_start);
        assert_eq!(
            &published_surface_bytes(&exact, &hdr_descriptors[0])[..4],
            [255, 255, 255, 255]
        );
        assert_eq!(
            published_zone_colors(&exact, &hdr_descriptors[1])[0],
            [255, 255, 255]
        );

        let midpoint = cpu_capture_frame(
            &hdr_source,
            3,
            started + Duration::from_millis(125),
            [148, 148, 148, 255],
        );
        publish_cpu_frame(&exact, &mut runtimes, &hdr_source, &midpoint);
        let surface = published_surface_bytes(&exact, &hdr_descriptors[0]);
        let zones = published_zone_colors(&exact, &hdr_descriptors[1]);
        assert!(surface[0] > 250);
        assert!(zones[0][0] > 250);
        assert_eq!(&surface[..3], zones[0]);
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
        native_demand_for_format(target, CapturePixelFormat::Bgra8)
    }

    fn native_demand_for_format(
        target: &ScreenNativeExecutionTarget,
        format: CapturePixelFormat,
    ) -> RegisteredScreenBranchDemand {
        RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenPublicationExecutorRequest::SourceNative(target.clone()),
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Contain,
                Arc::new(ScreenProcessingProfile::new(
                    ScreenProcessingProfileConfig::exact_encoded_identity(format),
                )),
            ),
            NonZeroU32::new(60).expect("nonzero cadence"),
        )
    }

    fn reduced_native_demand(target: &ScreenNativeExecutionTarget) -> RegisteredScreenBranchDemand {
        RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenPublicationExecutorRequest::SourceNative(target.clone()),
                ScreenExtentRequest::bounded(
                    NonZeroU32::new(2),
                    NonZeroU32::new(1),
                    super::super::ScreenUpscalePolicy::Never,
                ),
                ScreenAspectPolicy::Contain,
                Arc::new(ScreenProcessingProfile::default()),
            ),
            NonZeroU32::new(60).expect("nonzero cadence"),
        )
    }

    fn publish_native_fixture(
        frame: &Arc<MacosCaptureFrame>,
        source: &MacosPublicationSource,
        resolved: ResolvedScreenBranchDemand,
    ) -> (
        Arc<ScreenBranchPublication>,
        Arc<MacosScreenRuntimeTelemetry>,
    ) {
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
        let (token, runtime) = prepare_macos_exact_runtime(ticket, Some(source), &exact)
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
        let (_, telemetry) = publish_macos_native_exact(
            frame,
            now,
            now + Duration::from_secs(1),
            source,
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
        (publication, telemetry)
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

        let (publication, telemetry) = publish_native_fixture(&frame, &source, resolved);
        assert_eq!(publication.native_sequence(), NonZeroU64::MIN);
        let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
            panic!("identity macOS native branch publishes its GPU surface");
        };
        let surface = payload.surface();
        assert_eq!(surface.api(), &PlatformGpuApi::Metal);
        assert_eq!(surface.handle_id(), 7);
        assert_eq!(surface.format(), CapturePixelFormat::Bgra8);
        assert_eq!(surface.extent(), source.geometry.storage_extent());
        assert_eq!(payload.colorimetry().value(), source.colorimetry);
        assert!(surface.owner::<MacosCaptureFrame>().is_some());
        assert!(surface.timing_sink().is_some());
        assert!(surface.retained_owner::<TestPreparedTarget>().is_some());
        assert!(surface.resource_lifetime().is_some());
        assert!(surface.capture_resource_lifetime().is_some());
        assert_eq!(
            telemetry
                .capture_to_native_publication_timing
                .snapshot()
                .sample_count,
            1
        );
    }

    #[test]
    fn every_extended_native_format_publishes_deferred_work_without_masquerading() {
        let mappings = [
            (
                MacosCapturePixelFormat::Argb2101010,
                CapturePixelFormat::Argb2101010,
            ),
            (
                MacosCapturePixelFormat::Rgba16Float,
                CapturePixelFormat::Rgba16Float,
            ),
            (
                MacosCapturePixelFormat::Yuv420VideoRange,
                CapturePixelFormat::Yuv420VideoRange,
            ),
            (
                MacosCapturePixelFormat::Yuv420FullRange,
                CapturePixelFormat::Yuv420FullRange,
            ),
            (
                MacosCapturePixelFormat::Yuv44410BiPlanar,
                CapturePixelFormat::Yuv44410BiPlanar,
            ),
        ];
        for (native, core) in mappings {
            assert_eq!(capture_pixel_format(native), core);
            let mut native_frame = (*frame()).clone();
            native_frame.pixel_format = native;
            let native_frame = Arc::new(native_frame);
            let mut native_source = source(&frame());
            native_source.pixel_format = native;
            let demand = native_demand_for_format(&target(), core);
            let resolved = resolve_macos_publication_branch(&native_source, &demand)
                .expect("extended native demand resolves")
                .expect("configured macOS source owns extended native demand");
            assert!(matches!(
                resolved.descriptor().executor(),
                ScreenPublicationExecutor::SourceNative(_)
            ));
            assert!(!macos_native_descriptor_is_identity(resolved.descriptor()));
            let (publication, _) = publish_native_fixture(&native_frame, &native_source, resolved);
            let ScreenBranchPayload::NativeWork(payload) = publication.payload() else {
                panic!("extended native source must publish deferred work");
            };
            assert_eq!(payload.source().format(), core);
            assert_eq!(
                payload.source().extent(),
                native_source.geometry.storage_extent()
            );
        }
    }

    #[test]
    fn rec709_and_rec2020_transfer_metadata_remain_exact() {
        for (native, core) in [
            (
                MacosTransferFunction::Rec709,
                CaptureTransferFunction::Rec709,
            ),
            (
                MacosTransferFunction::Rec2020,
                CaptureTransferFunction::Rec2020,
            ),
        ] {
            let frame = frame_with_color(
                MacosCaptureColorimetry {
                    primaries: MacosColorPrimaries::Rec2020,
                    transfer: native,
                    matrix: None,
                    range: MacosColorRange::Full,
                    chroma_location: None,
                },
                BGRA8,
                &[0, 0, 255, 255],
                None,
            );
            assert_eq!(
                capture_colorimetry(&frame)
                    .expect("exact SDR transfer maps")
                    .transfer_function(),
                core
            );
        }
    }

    #[test]
    fn rgba16float_cpu_publication_matches_the_shared_scalar_oracle() {
        let color = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Rec2020,
            transfer: MacosTransferFunction::Linear,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        };
        let headroom = 1_000.0 / 203.0;
        let delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Rgba16Float,
            color,
            Some(203.0),
            Some(headroom),
        )
        .expect("extended-linear HDR delivery metadata is valid");
        let encoded = [0x00, 0x38, 0x00, 0x3c, 0x00, 0x40, 0x00, 0x3c];
        let native_frame = frame_with_color(color, RGBA16_FLOAT, &encoded, Some(delivered));
        let native_source = source(&native_frame);
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        exact.replace_source(Some(native_source.clone()));
        let resolved =
            resolve_macos_publication_branch(&native_source, &cpu_demand(transition_profile(true)))
                .expect("extended-linear CPU demand resolves")
                .expect("configured source owns extended-linear CPU demand");
        let mut runtimes = Vec::new();
        let descriptor = commit_cpu_runtime(
            &mut builder,
            &exact,
            &native_source,
            resolved,
            &mut runtimes,
        );
        let captured_at = Instant::now() + Duration::from_millis(20);
        publish_scalar_frame(
            &exact,
            &mut runtimes,
            &native_source,
            &native_frame,
            captured_at,
        );
        let output = published_surface_bytes(&exact, &descriptor);
        let pipeline = descriptor.physical().color_pipeline();
        let prepared = PreparedLedToneMap::prepare(
            pipeline
                .effective_source()
                .expect("managed pipeline retains source"),
            pipeline
                .output()
                .try_known()
                .expect("managed output is known"),
            pipeline.calibration().expect("managed calibration exists"),
        )
        .expect("shared scalar oracle prepares");
        let expected = prepared.encode(prepared.decode_and_map_source([0.5, 1.0, 2.0, 1.0]));
        assert_eq!(&output[..4], &expected);
    }

    #[test]
    fn malformed_native_planes_fail_before_cpu_publication() {
        let native_frame = frame();
        let native_source = source(&native_frame);
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        exact.replace_source(Some(native_source.clone()));
        let resolved = resolve_macos_publication_branch(
            &native_source,
            &cpu_demand(ScreenProcessingProfile::default()),
        )
        .expect("CPU demand resolves")
        .expect("configured source owns CPU demand");
        let mut runtimes = Vec::new();
        let descriptor = commit_cpu_runtime(
            &mut builder,
            &exact,
            &native_source,
            resolved,
            &mut runtimes,
        );
        let mut malformed = (*native_frame).clone();
        let mut planes = malformed.planes.to_vec();
        planes[0].bytes_per_row = 1;
        malformed.planes = planes.into();
        let captured_at = Instant::now() + Duration::from_millis(20);
        let capture = native_cpu_capture_frame(
            &Arc::new(malformed.clone()),
            captured_at,
            captured_at + Duration::from_secs(1),
            &native_source,
            native_source.epoch.source_id.clone(),
        )
        .expect("malformed plane metadata does not alter native ownership envelope");
        assert!(
            publish_macos_scalar_exact(
                &malformed,
                &capture,
                &native_source,
                &exact,
                &mut runtimes,
                &MacosScreenRuntimeTelemetry::default(),
            )
            .is_err()
        );
        let hub = exact.hub().expect("fixture hub remains installed");
        let lease = hub
            .lease(&descriptor)
            .expect("committed branch has a lease");
        assert!(lease.read().is_none());
    }

    #[test]
    fn every_retained_format_cpu_publication_matches_the_shared_scalar_oracle() {
        let sdr_rgb = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Srgb,
            transfer: MacosTransferFunction::Srgb,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        };
        let hdr_linear = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Rec2020,
            transfer: MacosTransferFunction::Linear,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        };
        let yuv_video = MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Rec2020,
            transfer: MacosTransferFunction::Pq,
            matrix: Some(hypercolor_macos_capture::MacosYuvMatrix::Bt2020),
            range: MacosColorRange::Video,
            chroma_location: Some(hypercolor_macos_capture::MacosChromaLocation::Left),
        };
        let yuv_full = MacosCaptureColorimetry {
            transfer: MacosTransferFunction::Hlg,
            range: MacosColorRange::Full,
            chroma_location: Some(hypercolor_macos_capture::MacosChromaLocation::Center),
            ..yuv_video
        };
        let hdr_delivery = |format, color| {
            MacosDeliveredFrameMetadata::new(format, color, Some(203.0), Some(1_000.0 / 203.0))
                .expect("HDR delivery metadata is valid")
        };
        let bgra = frame_with_planes(
            sdr_rgb,
            BGRA8,
            &[(
                &[32, 64, 128, 255].repeat(8),
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                16,
            )],
            None,
        );
        let packed_l10r = (3_u32 << 30) | (512 << 20) | (256 << 10) | 128;
        let l10r_bytes = packed_l10r.to_le_bytes().repeat(8);
        let l10r = frame_with_planes(
            hdr_linear,
            ARGB2101010,
            &[(
                &l10r_bytes,
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                16,
            )],
            Some(hdr_delivery(
                MacosCapturePixelFormat::Argb2101010,
                hdr_linear,
            )),
        );
        let rgba16_pixel = [0x00, 0x38, 0x00, 0x3c, 0x00, 0x40, 0x00, 0x3c];
        let rgba16_bytes = rgba16_pixel.repeat(8);
        let rgba16 = frame_with_planes(
            hdr_linear,
            RGBA16_FLOAT,
            &[(
                &rgba16_bytes,
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                32,
            )],
            Some(hdr_delivery(
                MacosCapturePixelFormat::Rgba16Float,
                hdr_linear,
            )),
        );
        let y_plane_video = vec![126; 8];
        let chroma_video = vec![96, 160, 96, 160];
        let yuv420v = frame_with_planes(
            yuv_video,
            YUV420_VIDEO_RANGE,
            &[
                (
                    &y_plane_video,
                    MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                    4,
                ),
                (
                    &chroma_video,
                    MacosPixelExtent::new(2, 1).expect("fixture extent is valid"),
                    4,
                ),
            ],
            Some(hdr_delivery(
                MacosCapturePixelFormat::Yuv420VideoRange,
                yuv_video,
            )),
        );
        let y_plane_full = vec![128; 8];
        let chroma_full = vec![96, 160, 96, 160];
        let yuv420f = frame_with_planes(
            yuv_full,
            YUV420_FULL_RANGE,
            &[
                (
                    &y_plane_full,
                    MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                    4,
                ),
                (
                    &chroma_full,
                    MacosPixelExtent::new(2, 1).expect("fixture extent is valid"),
                    4,
                ),
            ],
            Some(hdr_delivery(
                MacosCapturePixelFormat::Yuv420FullRange,
                yuv_full,
            )),
        );
        let yuv444_color = MacosCaptureColorimetry {
            chroma_location: Some(hypercolor_macos_capture::MacosChromaLocation::TopLeft),
            ..yuv_full
        };
        let y10 = (512_u16 << 6).to_le_bytes();
        let cb10 = (384_u16 << 6).to_le_bytes();
        let cr10 = (640_u16 << 6).to_le_bytes();
        let y444 = y10.repeat(8);
        let mut chroma444 = Vec::new();
        for _ in 0..8 {
            chroma444.extend_from_slice(&cb10);
            chroma444.extend_from_slice(&cr10);
        }
        let yuv444 = frame_with_planes(
            yuv444_color,
            YUV44410_FULL_RANGE,
            &[
                (
                    &y444,
                    MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                    8,
                ),
                (
                    &chroma444,
                    MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                    16,
                ),
            ],
            Some(hdr_delivery(
                MacosCapturePixelFormat::Yuv44410BiPlanar,
                yuv444_color,
            )),
        );

        for frame in [bgra, l10r, rgba16, yuv420v, yuv420f, yuv444] {
            assert_scalar_publication_matches_oracle(&frame);
        }
    }

    fn assert_scalar_publication_matches_oracle(frame: &Arc<MacosCaptureFrame>) {
        let native_source = source(frame);
        let hdr = native_source.colorimetry.dynamic_range() == Some(CaptureDynamicRange::High);
        let mut builder = ScreenPlanBuilder::new();
        let exact = MacosExactPublicationShared::default();
        *lock(&exact.hub) = Some(builder.publication_hub());
        exact.replace_source(Some(native_source.clone()));
        let resolved =
            resolve_macos_publication_branch(&native_source, &cpu_demand(transition_profile(hdr)))
                .expect("native scalar CPU demand resolves")
                .expect("configured source owns native scalar demand");
        let mut runtimes = Vec::new();
        let descriptor = commit_cpu_runtime(
            &mut builder,
            &exact,
            &native_source,
            resolved,
            &mut runtimes,
        );
        let source_sample = frame
            .with_cpu_source(|samples| samples.sample_rgba32f(0, 0))
            .expect("native scalar source validates")
            .expect("first source sample decodes");
        let captured_at = Instant::now() + Duration::from_millis(20);
        publish_scalar_frame(&exact, &mut runtimes, &native_source, frame, captured_at);
        let output = published_surface_bytes(&exact, &descriptor);
        let pipeline = descriptor.physical().color_pipeline();
        let prepared = PreparedLedToneMap::prepare(
            pipeline
                .effective_source()
                .expect("managed pipeline retains source"),
            pipeline
                .output()
                .try_known()
                .expect("managed output is known"),
            pipeline.calibration().expect("managed calibration exists"),
        )
        .expect("shared scalar oracle prepares");
        assert_eq!(
            &output[..4],
            &prepared.encode(prepared.decode_and_map_source(source_sample))
        );
    }

    #[test]
    fn reduced_rgba_demand_falls_back_until_native_reducer_exists() {
        let frame = frame();
        let source = source(&frame);
        let demand = reduced_native_demand(&target());
        let resolved = resolve_macos_publication_branch(&source, &demand)
            .expect("reduced demand resolves")
            .expect("configured macOS source owns reduced demand");
        assert!(matches!(
            resolved.descriptor().executor(),
            ScreenPublicationExecutor::Cpu
        ));

        let capable_target =
            target().with_color_capabilities(CpuReductionExecutor::supported_color_capabilities());
        let capable =
            resolve_macos_publication_branch(&source, &reduced_native_demand(&capable_target))
                .expect("capable reduced demand resolves")
                .expect("configured macOS source owns capable demand");
        assert!(matches!(
            capable.descriptor().executor(),
            ScreenPublicationExecutor::SourceNative(_)
        ));
        assert!(!macos_native_descriptor_is_identity(capable.descriptor()));
        let output_extent = capable.descriptor().geometry().output_extent();
        let (publication, _) = publish_native_fixture(&frame, &source, capable);
        let ScreenBranchPayload::NativeWork(payload) = publication.payload() else {
            panic!("reduced macOS native branch publishes deferred GPU work");
        };
        assert_eq!(payload.source().extent(), source.geometry.storage_extent());
        assert_ne!(payload.source().extent(), output_extent);
        assert_eq!(payload.source().format(), CapturePixelFormat::Bgra8);
        assert_eq!(payload.source_colorimetry().value(), source.colorimetry);
    }

    #[test]
    fn processing_reconfiguration_preserves_the_native_capture_runtime() {
        let admission =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
        let (mut input, fixture) =
            MacosScreenCaptureFixture::source(CaptureConfig::default(), admission);
        let native_source = source(&frame());
        input.exact.replace_source(Some(native_source));
        fixture.control.set_active(true);
        let active_transitions = fixture.control.active_transitions.load(Ordering::Acquire);
        let worker_generation = input.worker_generation;
        let revision = input.screen_publication_resolution_revision();
        let mut config = input.config.clone();
        config.target_led_white_x = 0.3000;
        config.target_led_white_y = 0.3200;
        config.target_led_reference_white_nits = 180.0;
        config.target_led_peak_nits = 500.0;
        config.exposure_ev = 1.25;

        input
            .reconfigure_screen_processing(&config)
            .expect("valid calibration updates without rebuilding capture");

        assert_eq!(input.worker_generation, worker_generation);
        assert!(fixture.is_active());
        assert_eq!(
            fixture.control.active_transitions.load(Ordering::Acquire),
            active_transitions
        );
        assert_eq!(input.screen_publication_resolution_revision(), revision + 1);
        let resolved = input
            .resolve_screen_publication_branch(&RegisteredScreenBranchDemand::new(
                ScreenPublicationRequest::new(
                    ScreenSourceSelector::Configured,
                    ScreenPublicationKind::Surface,
                    ScreenPublicationExecutorRequest::Cpu,
                    ScreenExtentRequest::bounded(
                        NonZeroU32::new(2),
                        NonZeroU32::new(1),
                        super::super::ScreenUpscalePolicy::Never,
                    ),
                    ScreenAspectPolicy::Contain,
                    Arc::new(ScreenProcessingProfile::default()),
                ),
                NonZeroU32::new(60).expect("nonzero cadence"),
            ))
            .expect("calibrated branch resolves")
            .expect("configured macOS source owns the demand");
        assert_eq!(
            resolved
                .descriptor()
                .physical()
                .color_pipeline()
                .calibration(),
            Some(
                LedToneMapCalibration::try_new(0.3000, 0.3200, 180.0, 500.0, 1.25)
                    .expect("fixture calibration is valid")
            )
        );
    }

    #[test]
    fn analysis_decimation_keeps_the_surface_near_the_analyzer_budget() {
        // Native 4K HDR display (retina 2x): must decimate hard enough to fit
        // the tone-map conversion inside the publication freshness budget.
        let native_4k = PixelExtent::new(4112, 2658).expect("valid extent");
        assert_eq!(analysis_decimation(native_4k), 7);
        // Surfaces already at or under the analyzer budget pass through exact.
        let analyzer_sized = PixelExtent::new(640, 480).expect("valid extent");
        assert_eq!(analysis_decimation(analyzer_sized), 1);
        let small_window = PixelExtent::new(320, 200).expect("valid extent");
        assert_eq!(analysis_decimation(small_window), 1);
        // The limiting axis wins: a wide-but-short surface decimates by width.
        let ultrawide = PixelExtent::new(5120, 400).expect("valid extent");
        assert_eq!(analysis_decimation(ultrawide), 8);
        // Every decimated sample position stays inside the source surface.
        for extent in [native_4k, analyzer_sized, small_window, ultrawide] {
            let step = analysis_decimation(extent);
            let last_x = (extent.width().div_ceil(step) - 1) * step;
            let last_y = (extent.height().div_ceil(step) - 1) * step;
            assert!(last_x < extent.width());
            assert!(last_y < extent.height());
        }
    }

    #[test]
    fn invalid_processing_reconfiguration_preserves_the_active_profile() {
        let admission =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
        let (mut input, fixture) =
            MacosScreenCaptureFixture::source(CaptureConfig::default(), admission);
        fixture.control.set_active(true);
        let revision = input.screen_publication_resolution_revision();
        let previous = input.config.clone();
        let mut invalid = previous.clone();
        invalid.exposure_ev = f32::INFINITY;

        assert!(input.reconfigure_screen_processing(&invalid).is_err());
        assert_eq!(input.config, previous);
        assert_eq!(input.screen_publication_resolution_revision(), revision);
        assert!(fixture.is_active());
    }
}
