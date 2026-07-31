//! Non-Windows stand-in so downstream crates compile unconditionally.

use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::Duration;

use crate::shared::{
    CaptureError, CaptureExtent, CaptureLane, CaptureRegion, CaptureResourceAdmission,
    CaptureResult, CpuDesktopFrame, DisplayRotation, Frame, GpuAdapterLuid, GpuReductionAdmission,
    GpuReductionProvenance, GpuSharedHandle, GpuSurfaceAdmission, GpuSurfaceDescriptor,
    GpuSurfaceDescriptorId, GpuSurfacePlanGeneration, GpuSurfaceProvenance, GpuSurfaceSlotId,
    GpuSurfaceSourceColorSpace, GpuSurfaceSynchronization, MonitorSelector, ReductionTelemetry,
};

#[derive(Clone, Copy, Debug)]
enum Never {}

/// Uninhabited non-Windows stand-in for a shareable D3D11 texture lease.
#[derive(Debug)]
pub struct GpuSurfaceLease<'a> {
    never: Never,
    marker: PhantomData<&'a GpuSurfacePublication>,
}

impl GpuSurfaceLease<'_> {
    /// No borrowed D3D11 texture handle exists on this platform.
    #[must_use]
    pub fn texture_handle(&self) -> GpuSharedHandle<'_> {
        match self.never {}
    }

    /// No borrowed D3D11 fence handle exists on this platform.
    #[must_use]
    pub fn fence_handle(&self) -> GpuSharedHandle<'_> {
        match self.never {}
    }

    /// No synchronization hand-off exists on this platform.
    #[must_use]
    pub fn synchronization(&self) -> GpuSurfaceSynchronization {
        match self.never {}
    }

    /// No GPU provenance exists on this platform.
    #[must_use]
    pub fn provenance(&self) -> &GpuSurfaceProvenance {
        match self.never {}
    }

    /// No native ownership can be acquired on this platform.
    pub fn mark_native_acquired(&mut self) -> CaptureResult<()> {
        match self.never {}
    }

    /// No native reservation can be abandoned on this platform.
    pub fn abandon_before_acquire(self) -> CaptureResult<()> {
        match self.never {}
    }

    /// No native release can be queued on this platform.
    pub fn mark_release_queued(self) -> CaptureResult<()> {
        match self.never {}
    }
}

/// Uninhabited non-Windows stand-in for one exact GPU Surface result.
#[derive(Debug)]
pub struct GpuSurfacePublication {
    never: Never,
}

impl GpuSurfacePublication {
    /// No GPU provenance exists on this platform.
    #[must_use]
    pub fn provenance(&self) -> &GpuSurfaceProvenance {
        match self.never {}
    }

    /// No native texture identity exists on this platform.
    #[must_use]
    pub fn opaque_handle_id(&self) -> NonZeroU64 {
        match self.never {}
    }

    /// No GPU lease exists on this platform.
    pub fn claim(&self) -> CaptureResult<GpuSurfaceLease<'_>> {
        match self.never {}
    }
}

/// Uninhabited non-Windows stand-in for one native preparation slot.
#[derive(Debug)]
pub struct GpuSurfaceTargetPreparationSlot {
    never: Never,
}

impl GpuSurfaceTargetPreparationSlot {
    /// No native texture identity exists on this platform.
    #[must_use]
    pub fn opaque_handle_id(&self) -> NonZeroU64 {
        match self.never {}
    }

    /// No native slot identity exists on this platform.
    #[must_use]
    pub fn slot_id(&self) -> GpuSurfaceSlotId {
        match self.never {}
    }

    /// No borrowed D3D11 texture handle exists on this platform.
    #[must_use]
    pub fn texture_handle(&self) -> GpuSharedHandle<'_> {
        match self.never {}
    }

    /// No borrowed D3D11 fence handle exists on this platform.
    #[must_use]
    pub fn fence_handle(&self) -> GpuSharedHandle<'_> {
        match self.never {}
    }
}

/// Uninhabited non-Windows stand-in for a native preparation manifest.
#[derive(Debug)]
pub struct GpuSurfaceTargetPreparation {
    never: Never,
}

impl GpuSurfaceTargetPreparation {
    /// No committed plan generation exists on this platform.
    #[must_use]
    pub fn plan_generation(&self) -> GpuSurfacePlanGeneration {
        match self.never {}
    }

    /// No exact descriptor exists on this platform.
    #[must_use]
    pub fn descriptor(&self) -> &GpuSurfaceDescriptor {
        match self.never {}
    }

    /// No renderer adapter exists on this platform.
    #[must_use]
    pub fn adapter_luid(&self) -> GpuAdapterLuid {
        match self.never {}
    }

    /// No display source exists on this platform.
    #[must_use]
    pub fn source_id(&self) -> &str {
        match self.never {}
    }

    /// No output topology exists on this platform.
    #[must_use]
    pub fn topology_generation(&self) -> u64 {
        match self.never {}
    }

    /// No duplication session exists on this platform.
    #[must_use]
    pub fn duplication_generation(&self) -> u64 {
        match self.never {}
    }

    /// No route allocation exists on this platform.
    #[must_use]
    pub fn allocation_byte_len(&self) -> u64 {
        match self.never {}
    }

    /// No native slots exist on this platform.
    #[must_use]
    pub fn slots(&self) -> &[GpuSurfaceTargetPreparationSlot] {
        match self.never {}
    }
}

/// API-compatible non-Windows publication outcome.
#[derive(Debug)]
pub enum GpuSurfacePublishOutcome {
    /// Unreachable because no Windows GPU Surface can publish.
    Published(Arc<GpuSurfacePublication>),
    /// Unreachable because no Windows GPU Surface plan can prepare.
    Busy(GpuSurfaceDescriptorId),
}

/// API-compatible non-Windows publication feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuSurfacePublicationDisposition {
    /// Unreachable downstream acceptance.
    Accepted,
    /// Unreachable downstream retry request.
    Retry,
}

/// Non-Windows stand-in for one GPU Surface batch summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuSurfaceBatchInfo {
    source_sequence: u64,
    published: usize,
    busy: usize,
}

impl GpuSurfaceBatchInfo {
    /// No source sequence exists on this platform.
    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    /// No GPU Surface publications exist on this platform.
    #[must_use]
    pub const fn published(&self) -> usize {
        self.published
    }

    /// No GPU Surface slots are busy on this platform.
    #[must_use]
    pub const fn busy(&self) -> usize {
        self.busy
    }
}

/// API-compatible non-Windows GPU reduction feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuReductionPublicationDisposition {
    /// Unreachable downstream acceptance.
    Accepted,
    /// Unreachable downstream retry request.
    Retry,
}

/// Borrowed non-Windows stand-in for exact reduced bytes.
pub struct GpuReductionPublishOutcome<'a> {
    provenance: &'a GpuReductionProvenance,
    pixels: &'a [u8],
}

impl GpuReductionPublishOutcome<'_> {
    /// No reduction provenance exists on this platform.
    #[must_use]
    pub const fn provenance(&self) -> &GpuReductionProvenance {
        self.provenance
    }

    /// No reduced pixels exist on this platform.
    #[must_use]
    pub const fn pixels(&self) -> &[u8] {
        self.pixels
    }
}

/// Non-Windows stand-in for one GPU reduction batch summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuReductionBatchInfo {
    submitted: usize,
    completed: usize,
    busy: usize,
    readback_bytes: u64,
}

impl GpuReductionBatchInfo {
    /// No descriptor reductions execute on this platform.
    #[must_use]
    pub const fn submitted(self) -> usize {
        self.submitted
    }

    /// No readbacks complete on this platform.
    #[must_use]
    pub const fn completed(self) -> usize {
        self.completed
    }

    /// No reduction slots exist on this platform.
    #[must_use]
    pub const fn busy(self) -> usize {
        self.busy
    }

    /// No staging bytes are mapped on this platform.
    #[must_use]
    pub const fn readback_bytes(self) -> u64 {
        self.readback_bytes
    }
}

/// Uninhabited non-Windows stand-in for a prepared D3D11 Surface plan.
#[derive(Debug)]
pub struct PreparedGpuSurfacePlan {
    never: Never,
}

/// Uninhabited non-Windows descriptor-keyed reduction plan.
#[derive(Debug)]
pub struct PreparedGpuReductionPlan {
    never: Never,
}

/// Uninhabited non-Windows stand-in for native D3D11 CPU readback.
#[derive(Debug)]
pub struct PreparedCpuDesktopReadback {
    never: Never,
}

impl PreparedCpuDesktopReadback {
    /// No native readback is pending on this platform.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        match self.never {}
    }

    /// No native readback slots exist on this platform.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        match self.never {}
    }

    /// No native readback allocation exists on this platform.
    #[must_use]
    pub fn allocation_byte_len(&self) -> u64 {
        match self.never {}
    }

    /// No native staging surface can be mapped on this platform.
    #[must_use]
    pub fn mapped_byte_len(&self) -> u64 {
        match self.never {}
    }
}

/// Requested consumers for one unsupported capture pump cycle.
pub struct CapturePumpRequest<'a> {
    gpu: Option<&'a mut PreparedGpuSurfacePlan>,
    reduction: Option<&'a mut PreparedGpuReductionPlan>,
    cpu: Option<&'a mut PreparedCpuDesktopReadback>,
}

impl<'a> CapturePumpRequest<'a> {
    /// Request any combination of exact GPU and native CPU outputs.
    #[must_use]
    pub const fn new(
        gpu: Option<&'a mut PreparedGpuSurfacePlan>,
        cpu: Option<&'a mut PreparedCpuDesktopReadback>,
    ) -> Self {
        Self {
            gpu,
            reduction: None,
            cpu,
        }
    }

    /// Request any combination of unsupported capture lanes.
    #[must_use]
    pub const fn with_reduction(
        gpu: Option<&'a mut PreparedGpuSurfacePlan>,
        reduction: Option<&'a mut PreparedGpuReductionPlan>,
        cpu: Option<&'a mut PreparedCpuDesktopReadback>,
    ) -> Self {
        Self {
            gpu,
            reduction,
            cpu,
        }
    }

    /// Request only exact GPU publications.
    #[must_use]
    pub const fn gpu(plan: &'a mut PreparedGpuSurfacePlan) -> Self {
        Self::new(Some(plan), None)
    }

    /// Request only an exact native CPU frame.
    #[must_use]
    pub const fn cpu(readback: &'a mut PreparedCpuDesktopReadback) -> Self {
        Self::new(None, Some(readback))
    }

    /// Request only unsupported GPU reduction readback.
    #[must_use]
    pub const fn reduction(plan: &'a mut PreparedGpuReductionPlan) -> Self {
        Self::with_reduction(None, Some(plan), None)
    }

    /// Request exact GPU publications and native CPU readback together.
    #[must_use]
    pub const fn hybrid(
        plan: &'a mut PreparedGpuSurfacePlan,
        readback: &'a mut PreparedCpuDesktopReadback,
    ) -> Self {
        Self::new(Some(plan), Some(readback))
    }
}

/// Independent results for one unsupported capture pump cycle.
#[derive(Debug)]
pub struct CapturePumpReport {
    /// Always false on unsupported platforms.
    pub acquired: bool,
    /// Exact GPU lane outcome.
    pub gpu: CaptureLane<GpuSurfaceBatchInfo>,
    /// Exact GPU reduction lane outcome.
    pub reduction: CaptureLane<GpuReductionBatchInfo>,
    /// Native CPU lane outcome.
    pub cpu: CaptureLane<CpuDesktopFrame>,
}

impl PreparedGpuSurfacePlan {
    /// No committed D3D11 plan generation exists on this platform.
    #[must_use]
    pub fn plan_generation(&self) -> GpuSurfacePlanGeneration {
        match self.never {}
    }

    /// No D3D11 descriptors exist on this platform.
    pub fn descriptors(&self) -> std::iter::Empty<&GpuSurfaceDescriptor> {
        match self.never {}
    }

    /// No native routes exist to select on this platform.
    pub fn select_routes_for_next_acquisition<F>(&mut self, _select: F)
    where
        F: FnMut(&GpuSurfaceDescriptor) -> bool,
    {
        match self.never {}
    }

    /// No native acquisition can advance on this platform.
    #[must_use]
    pub fn has_selected_routes(&self) -> bool {
        match self.never {}
    }

    /// No native target can be prepared on this platform.
    pub fn target_preparation(
        &self,
        _descriptor_id: GpuSurfaceDescriptorId,
    ) -> CaptureResult<GpuSurfaceTargetPreparation> {
        match self.never {}
    }

    /// No D3D11 allocation exists on this platform.
    #[must_use]
    pub fn allocation_byte_len(&self) -> u64 {
        match self.never {}
    }

    /// No D3D11 staging readback exists on this platform.
    #[must_use]
    pub fn readback_byte_len(&self) -> u64 {
        match self.never {}
    }

    /// No D3D11 publication buffer exists on this platform.
    #[must_use]
    pub fn publication_buffer_byte_len(&self) -> usize {
        match self.never {}
    }

    /// No native ownership can be reclaimed on this platform.
    pub fn reclaim_abandoned(&mut self) -> CaptureResult<usize> {
        match self.never {}
    }
}

impl PreparedGpuReductionPlan {
    /// No D3D11 descriptors exist on this platform.
    pub fn descriptors(&self) -> std::iter::Empty<&GpuSurfaceDescriptor> {
        match self.never {}
    }

    /// No descriptor routes exist to select on this platform.
    pub fn select_routes_for_next_acquisition<F>(&mut self, _select: F)
    where
        F: FnMut(&GpuSurfaceDescriptor) -> bool,
    {
        match self.never {}
    }

    /// No native acquisition can advance on this platform.
    #[must_use]
    pub fn has_selected_routes(&self) -> bool {
        match self.never {}
    }

    /// No asynchronous readback can remain pending on this platform.
    #[must_use]
    pub fn has_pending_routes(&self) -> bool {
        match self.never {}
    }

    /// No D3D11 allocation exists on this platform.
    #[must_use]
    pub fn allocation_byte_len(&self) -> u64 {
        match self.never {}
    }

    /// No D3D11 staging allocation exists on this platform.
    #[must_use]
    pub fn readback_byte_len(&self) -> u64 {
        match self.never {}
    }

    /// No callback buffer exists on this platform.
    #[must_use]
    pub fn publication_buffer_byte_len(&self) -> usize {
        match self.never {}
    }
}

/// Desktop Duplication placeholder for platforms without the API.
pub struct DesktopDuplicator {
    requested_extent: CaptureExtent,
}

impl DesktopDuplicator {
    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn new(_monitor: usize, _requested_extent: CaptureExtent) -> CaptureResult<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn open(
        _selector: MonitorSelector,
        _requested_extent: CaptureExtent,
    ) -> CaptureResult<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn open_with_resource_admission(
        _selector: MonitorSelector,
        _requested_extent: CaptureExtent,
        _resource_admission: Arc<dyn CaptureResourceAdmission>,
    ) -> CaptureResult<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Monitor index this duplicator would be bound to.
    #[must_use]
    pub const fn monitor(&self) -> usize {
        0
    }

    /// Empty because no platform source can be opened.
    #[must_use]
    pub const fn source_id(&self) -> &str {
        ""
    }

    /// False because no output is bound.
    #[must_use]
    pub const fn is_primary(&self) -> bool {
        false
    }

    /// Zero because no platform topology exists.
    #[must_use]
    pub const fn topology_generation(&self) -> u64 {
        0
    }

    /// Native desktop dimensions.
    #[must_use]
    pub const fn native_extent(&self) -> (u32, u32) {
        (0, 0)
    }

    /// Logical desktop dimensions.
    #[must_use]
    pub const fn logical_extent(&self) -> (u32, u32) {
        (0, 0)
    }

    /// Zero because no duplication interface exists.
    #[must_use]
    pub const fn duplication_generation(&self) -> u64 {
        0
    }

    /// Empty adapter identity because no D3D11 session exists.
    #[must_use]
    pub const fn adapter_luid(&self) -> GpuAdapterLuid {
        GpuAdapterLuid::new(0, 0)
    }

    /// Always fails because no GPU adapter budget exists on this platform.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn available_gpu_memory_bytes(&self) -> CaptureResult<u64> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Empty virtual-desktop origin.
    #[must_use]
    pub const fn origin(&self) -> (i32, i32) {
        (0, 0)
    }

    /// Identity because no native pixels exist.
    #[must_use]
    pub const fn rotation(&self) -> DisplayRotation {
        DisplayRotation::Identity
    }

    /// Unknown because no DXGI output exists.
    #[must_use]
    pub const fn source_color_space(&self) -> GpuSurfaceSourceColorSpace {
        GpuSurfaceSourceColorSpace::Unknown
    }

    /// A stub has no live capture request.
    #[must_use]
    pub const fn requested_extent(&self) -> CaptureExtent {
        self.requested_extent
    }

    /// Ignore the requested extent on unsupported platforms.
    pub const fn set_requested_extent(&mut self, requested_extent: CaptureExtent) {
        self.requested_extent = requested_extent;
    }

    /// Always fails: no native capture extent exists on this platform.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn set_region(&mut self, _region: Option<CaptureRegion>) -> CaptureResult<()> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// CPU fallback telemetry for the unsupported platform stub.
    #[must_use]
    pub fn reduction_telemetry(&self) -> ReductionTelemetry {
        ReductionTelemetry {
            issue: Some(Arc::from(
                "desktop screen capture is only available on Windows",
            )),
            ..ReductionTelemetry::default()
        }
    }

    /// Always fails: shareable D3D11 resources are Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn prepare_gpu_surface_plan(
        &self,
        _plan_generation: GpuSurfacePlanGeneration,
        _descriptors: &[GpuSurfaceDescriptor],
        _admission: GpuSurfaceAdmission,
    ) -> CaptureResult<PreparedGpuSurfacePlan> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: native D3D11 CPU readback is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn prepare_cpu_desktop_readback(
        &self,
        _slot_count: NonZeroU32,
    ) -> CaptureResult<PreparedCpuDesktopReadback> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: descriptor-keyed D3D11 readback is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn prepare_gpu_reduction_plan(
        &self,
        _plan_generation: GpuSurfacePlanGeneration,
        _descriptors: &[GpuSurfaceDescriptor],
        _admission: GpuReductionAdmission,
    ) -> CaptureResult<PreparedGpuReductionPlan> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn pump<F>(
        &mut self,
        request: CapturePumpRequest<'_>,
        _timeout: Duration,
        _emit: F,
    ) -> CaptureResult<CapturePumpReport>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        let CapturePumpRequest {
            gpu: _gpu,
            reduction: _reduction,
            cpu: _cpu,
        } = request;
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn pump_with_feedback<F>(
        &mut self,
        request: CapturePumpRequest<'_>,
        _timeout: Duration,
        _emit: F,
    ) -> CaptureResult<CapturePumpReport>
    where
        F: FnMut(GpuSurfacePublishOutcome) -> GpuSurfacePublicationDisposition,
    {
        let CapturePumpRequest {
            gpu: _gpu,
            reduction: _reduction,
            cpu: _cpu,
        } = request;
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn pump_with_reduction_feedback<F, R>(
        &mut self,
        request: CapturePumpRequest<'_>,
        _timeout: Duration,
        _emit: F,
        _emit_reduction: R,
    ) -> CaptureResult<CapturePumpReport>
    where
        F: FnMut(GpuSurfacePublishOutcome) -> GpuSurfacePublicationDisposition,
        R: FnMut(GpuReductionPublishOutcome<'_>) -> GpuReductionPublicationDisposition,
    {
        let CapturePumpRequest {
            gpu: _gpu,
            reduction: _reduction,
            cpu: _cpu,
        } = request;
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: shareable D3D11 resources are Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn next_gpu_surfaces<F>(
        &mut self,
        _plan: &mut PreparedGpuSurfacePlan,
        _timeout: Duration,
        _emit: F,
    ) -> CaptureResult<Option<GpuSurfaceBatchInfo>>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn next_frame(&mut self, _timeout: Duration) -> CaptureResult<Option<Frame>> {
        Err(CaptureError::UnsupportedPlatform)
    }
}
