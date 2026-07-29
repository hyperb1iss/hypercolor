//! Non-Windows stand-in so downstream crates compile unconditionally.

use std::sync::Arc;
use std::time::Duration;

use crate::shared::{
    CaptureError, CaptureExtent, CaptureRegion, CaptureResult, Frame, GpuSharedHandle,
    GpuSurfaceAdmission, GpuSurfaceDescriptor, GpuSurfaceDescriptorId, GpuSurfacePlanGeneration,
    GpuSurfaceProvenance, GpuSurfaceSynchronization, MonitorSelector, ReductionTelemetry,
};

#[derive(Clone, Copy, Debug)]
enum Never {}

/// Uninhabited non-Windows stand-in for a shareable D3D11 texture lease.
#[derive(Debug)]
pub struct GpuSurfaceLease {
    never: Never,
}

impl GpuSurfaceLease {
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

    /// No GPU lease exists on this platform.
    pub fn claim(self: &Arc<Self>) -> CaptureResult<GpuSurfaceLease> {
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

/// Uninhabited non-Windows stand-in for a prepared D3D11 Surface plan.
#[derive(Debug)]
pub struct PreparedGpuSurfacePlan {
    never: Never,
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
