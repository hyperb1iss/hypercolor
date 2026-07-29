use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, WAIT_ABANDONED, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_UNORDERED_ACCESS, D3D11_BUFFER_DESC,
    D3D11_CPU_ACCESS_FLAG, D3D11_FENCE_FLAG_SHARED, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device, ID3D11Device5,
    ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UINT, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIKeyedMutex, IDXGIResource1,
};
use windows::core::{Interface, PCWSTR};

use super::gpu_reduction::{
    GpuReductionError, checked_rgba_row_pitch, create_srv, create_surface_compute_shader,
    create_texture, create_uav, normalized_pointer, pointer_kind_code, rotation_code,
};
use super::{CaptureMetadata, PointerState};
use crate::shared::checked_gpu_surface_bytes;
use crate::{
    CaptureError, CaptureExtent, CaptureResult, GpuAdapterLuid, GpuSharedHandle,
    GpuSurfaceAdmission, GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace,
    GpuSurfaceCursorPolicy, GpuSurfaceDescriptor, GpuSurfaceDescriptorId, GpuSurfaceFormat,
    GpuSurfacePlanGeneration, GpuSurfaceProvenance, GpuSurfaceSlotId, GpuSurfaceSourceColorSpace,
    GpuSurfaceSynchronization, GpuSurfaceUnsupportedReason,
};

const THREAD_GROUP: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy)]
struct SurfaceShaderParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    stride: u32,
    rotation: u32,
    pointer_kind: u32,
    pointer_visible: u32,
    pointer_x: i32,
    pointer_y: i32,
    pointer_width: u32,
    pointer_height: u32,
    region_x: u32,
    region_y: u32,
    region_width: u32,
    region_height: u32,
}

#[derive(Debug)]
struct OwnedHandle(HANDLE);

// SAFETY: Windows kernel handles are process-wide values. This wrapper owns one
// handle, never mutates it, and closes it after the final Arc-held resource drops.
unsafe impl Send for OwnedHandle {}
// SAFETY: the borrowed numeric value is immutable; CloseHandle cannot race
// while a shared slot retains this owner.
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: HANDLE) -> CaptureResult<Self> {
        if handle.is_invalid() {
            return Err(CaptureError::windows(
                "create shared GPU Surface handle",
                "Windows returned an invalid handle",
            ));
        }
        Ok(Self(handle))
    }

    fn borrowed(&self) -> GpuSharedHandle<'_> {
        GpuSharedHandle::from_raw(self.0.0 as isize)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this object exclusively owns a valid handle returned by a
        // D3D11/DXGI CreateSharedHandle call.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct SharedSurfaceSlot {
    _texture: ID3D11Texture2D,
    keyed_mutex: IDXGIKeyedMutex,
    fence: ID3D11Fence,
    texture_handle: OwnedHandle,
    fence_handle: OwnedHandle,
}

const USE_IDLE: u8 = 0;
const USE_PREPARED: u8 = 1;
const USE_UNCLAIMED: u8 = 2;
const USE_CLAIMED: u8 = 3;
const USE_RELEASE_QUEUED: u8 = 4;
const USE_POISONED: u8 = 5;

/// Claimed D3D11 texture hand-off for one exact result.
///
/// The Windows bridge opens both handles on a D3D11On12 device created from
/// the renderer's D3D12 device and queue. It waits the ready fence, acquires
/// key 1, copies into a renderer-owned wrapped D3D12 texture, releases key 0,
/// signals the release fence, and then marks the release queued. Wgpu never
/// imports the keyed source directly.
pub struct GpuSurfaceLease {
    publication: Arc<GpuSurfacePublication>,
    native_acquired: bool,
    finalized: bool,
}

impl std::fmt::Debug for GpuSurfaceLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuSurfaceLease")
            .field("texture_handle", &self.texture_handle())
            .field("fence_handle", &self.fence_handle())
            .field("synchronization", &self.publication.synchronization)
            .field("provenance", &self.publication.provenance())
            .field("native_acquired", &self.native_acquired)
            .finish_non_exhaustive()
    }
}

impl GpuSurfaceLease {
    /// Borrowed NT handle for the shareable `R8G8B8A8_UNORM` texture.
    #[must_use]
    pub fn texture_handle(&self) -> GpuSharedHandle<'_> {
        self.publication.shared.texture_handle.borrowed()
    }

    /// Borrowed NT handle for the shared D3D11 fence.
    #[must_use]
    pub fn fence_handle(&self) -> GpuSharedHandle<'_> {
        self.publication.shared.fence_handle.borrowed()
    }

    /// Producer-ready and consumer-release values for this slot use.
    #[must_use]
    pub fn synchronization(&self) -> GpuSurfaceSynchronization {
        self.publication.synchronization
    }

    /// Immutable production-time metadata for this claimed hand-off.
    #[must_use]
    pub fn provenance(&self) -> &GpuSurfaceProvenance {
        self.publication.provenance()
    }

    /// Mark the instant the bridge acquires the native consumer key.
    ///
    /// Call this immediately after a successful keyed-mutex acquire. From
    /// this point, dropping the lease before queuing the matching release
    /// poisons the slot so the producer can never overwrite consumer-owned
    /// storage.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::GpuSurfaceUseUnavailable`] if this lease no
    /// longer owns the active hand-off or already marked native acquisition.
    pub fn mark_native_acquired(&mut self) -> CaptureResult<()> {
        if self.native_acquired
            || self.finalized
            || self.publication.state.load(Ordering::Acquire) != USE_CLAIMED
        {
            return Err(self.unavailable());
        }
        self.native_acquired = true;
        Ok(())
    }

    /// Return an unacquired reservation to the publication.
    ///
    /// This is the non-poisoning exit for adapter mismatch, import failure,
    /// or any other bridge error that occurs before keyed ownership changes.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::GpuSurfaceUseUnavailable`] after native
    /// acquisition or when this lease no longer owns the active hand-off.
    pub fn abandon_before_acquire(mut self) -> CaptureResult<()> {
        if self.native_acquired || self.finalized {
            return Err(self.unavailable());
        }
        self.publication
            .state
            .compare_exchange(
                USE_CLAIMED,
                USE_UNCLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| self.unavailable())?;
        self.finalized = true;
        Ok(())
    }

    /// Record that key 0 release and the consumer fence signal are queued.
    ///
    /// The D3D11On12 bridge must call this only after queuing both operations.
    /// This marker never performs GPU synchronization itself; native reuse
    /// remains gated on the published consumer-release fence value.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::GpuSurfaceUseUnavailable`] if the guard no
    /// longer owns the active hand-off.
    pub fn mark_release_queued(mut self) -> CaptureResult<()> {
        if !self.native_acquired || self.finalized {
            return Err(self.unavailable());
        }
        self.publication
            .state
            .compare_exchange(
                USE_CLAIMED,
                USE_RELEASE_QUEUED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| self.unavailable())?;
        self.finalized = true;
        Ok(())
    }

    fn unavailable(&self) -> CaptureError {
        CaptureError::GpuSurfaceUseUnavailable {
            descriptor_id: self.provenance().descriptor.id(),
            source_sequence: self.provenance().source_sequence,
        }
    }
}

impl Drop for GpuSurfaceLease {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let next = if self.native_acquired {
            USE_POISONED
        } else {
            USE_UNCLAIMED
        };
        let _ = self.publication.state.compare_exchange(
            USE_CLAIMED,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.finalized = true;
    }
}

/// One exact GPU Surface result with production-time provenance.
pub struct GpuSurfacePublication {
    provenance: Option<GpuSurfaceProvenance>,
    shared: SharedSurfaceSlot,
    synchronization: GpuSurfaceSynchronization,
    slot_id: GpuSurfaceSlotId,
    use_id: u64,
    state: AtomicU8,
}

impl std::fmt::Debug for GpuSurfacePublication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuSurfacePublication")
            .field("provenance", &self.provenance)
            .field("slot_id", &self.slot_id)
            .field("use_id", &self.use_id)
            .field("state", &self.state.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl GpuSurfacePublication {
    /// Immutable metadata captured with this exact result.
    #[must_use]
    pub fn provenance(&self) -> &GpuSurfaceProvenance {
        self.provenance
            .as_ref()
            .expect("only initialized publications leave the native plan")
    }

    /// Claim the sole native hand-off for a D3D11On12 bridge copy.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::GpuSurfaceUseUnavailable`] when this result was
    /// already claimed, expired, superseded, or reclaimed by the producer.
    pub fn claim(self: &Arc<Self>) -> CaptureResult<GpuSurfaceLease> {
        if Instant::now() >= self.provenance().freshness_deadline {
            return Err(CaptureError::GpuSurfaceUseUnavailable {
                descriptor_id: self.provenance().descriptor.id(),
                source_sequence: self.provenance().source_sequence,
            });
        }
        self.state
            .compare_exchange(
                USE_UNCLAIMED,
                USE_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| CaptureError::GpuSurfaceUseUnavailable {
                descriptor_id: self.provenance().descriptor.id(),
                source_sequence: self.provenance().source_sequence,
            })?;
        Ok(GpuSurfaceLease {
            publication: Arc::clone(self),
            native_acquired: false,
            finalized: false,
        })
    }
}

/// Per-descriptor outcome from one native acquisition.
#[derive(Debug)]
pub enum GpuSurfacePublishOutcome {
    /// An exact Surface was dispatched and fenced for consumption.
    Published(Arc<GpuSurfacePublication>),
    /// Every reusable slot remains owned by an earlier consumer.
    Busy(GpuSurfaceDescriptorId),
}

/// Allocation-free summary of one descriptor fanout pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuSurfaceBatchInfo {
    source_sequence: u64,
    published: usize,
    busy: usize,
}

impl GpuSurfaceBatchInfo {
    /// Native acquisition sequence shared by every outcome in this batch.
    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    /// Number of exact publications emitted to the callback.
    #[must_use]
    pub const fn published(&self) -> usize {
        self.published
    }

    /// Number of descriptors whose native slots remained occupied.
    #[must_use]
    pub const fn busy(&self) -> usize {
        self.busy
    }
}

struct SurfaceSlot {
    publication: Arc<GpuSurfacePublication>,
    uav: ID3D11UnorderedAccessView,
    next_signal_value: u64,
    next_use_id: u64,
    required_release_value: Option<u64>,
}

struct SurfaceRoute {
    descriptor: Arc<GpuSurfaceDescriptor>,
    slots: Vec<SurfaceSlot>,
    write_index: usize,
    pending: PendingRouteOutcome,
    pending_source_sequence: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PendingRouteOutcome {
    #[default]
    None,
    Busy,
    Published(usize),
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectedSurfaceFault {
    AfterProducerAcquire,
    AfterProducerRelease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyedMutexAcquire {
    Acquired,
    Timeout,
    Abandoned,
}

fn acquire_keyed_mutex(
    mutex: &IDXGIKeyedMutex,
    key: u64,
    timeout_ms: u32,
    context: &'static str,
) -> CaptureResult<KeyedMutexAcquire> {
    // SAFETY: the live COM interface and its vtable remain valid for the call.
    // The generated wrapper discards positive wait statuses through
    // HRESULT::ok, so this path must preserve the raw return value.
    let status = unsafe {
        (Interface::vtable(mutex).AcquireSync)(Interface::as_raw(mutex), key, timeout_ms)
    };
    classify_keyed_mutex_status(status, context)
}

fn classify_keyed_mutex_status(
    status: windows::core::HRESULT,
    context: &'static str,
) -> CaptureResult<KeyedMutexAcquire> {
    match status.0.cast_unsigned() {
        0 => Ok(KeyedMutexAcquire::Acquired),
        value if value == WAIT_TIMEOUT.0 => Ok(KeyedMutexAcquire::Timeout),
        value if value == WAIT_ABANDONED.0 => Ok(KeyedMutexAcquire::Abandoned),
        _ if status.is_err() => Err(CaptureError::windows(
            context,
            windows::core::Error::from_hresult(status),
        )),
        value => Err(CaptureError::windows(
            context,
            format_args!("unexpected keyed-mutex wait status {value:#x}"),
        )),
    }
}

#[cfg(test)]
fn require_keyed_mutex(
    mutex: &IDXGIKeyedMutex,
    key: u64,
    timeout_ms: u32,
    context: &'static str,
) -> CaptureResult<()> {
    match acquire_keyed_mutex(mutex, key, timeout_ms, context)? {
        KeyedMutexAcquire::Acquired => Ok(()),
        KeyedMutexAcquire::Timeout => Err(CaptureError::Timeout),
        KeyedMutexAcquire::Abandoned => Err(CaptureError::windows(
            context,
            "keyed-mutex ownership was abandoned",
        )),
    }
}

struct PointerResource {
    generation: u64,
    width: u32,
    height: u32,
    byte_len: u64,
    srv: ID3D11ShaderResourceView,
}

/// Allocation-complete descriptor-keyed D3D11 Surface plan.
pub struct PreparedGpuSurfacePlan {
    plan_generation: GpuSurfacePlanGeneration,
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
    adapter_luid: GpuAdapterLuid,
    native_source_extent: CaptureExtent,
    logical_source_extent: CaptureExtent,
    source_rotation: crate::DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    context4: ID3D11DeviceContext4,
    shader: ID3D11ComputeShader,
    params: ID3D11Buffer,
    clean: ID3D11Texture2D,
    clean_srv: ID3D11ShaderResourceView,
    clean_metadata: Option<CaptureMetadata>,
    pointer: Option<PointerResource>,
    routes: Vec<SurfaceRoute>,
    texture_budget: u64,
    allocation_byte_len: u64,
    #[cfg(test)]
    injected_fault: Option<InjectedSurfaceFault>,
}

impl std::fmt::Debug for PreparedGpuSurfacePlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedGpuSurfacePlan")
            .field("plan_generation", &self.plan_generation)
            .field("source_id", &self.source_id)
            .field("topology_generation", &self.topology_generation)
            .field("duplication_generation", &self.duplication_generation)
            .field("adapter_luid", &self.adapter_luid)
            .field("native_source_extent", &self.native_source_extent)
            .field("logical_source_extent", &self.logical_source_extent)
            .field("source_rotation", &self.source_rotation)
            .field("source_color_space", &self.source_color_space)
            .field("descriptor_count", &self.routes.len())
            .field("allocation_byte_len", &self.allocation_byte_len)
            .finish_non_exhaustive()
    }
}

impl PreparedGpuSurfacePlan {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        plan_generation: GpuSurfacePlanGeneration,
        source_id: Arc<str>,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        native_source_extent: CaptureExtent,
        logical_source_extent: CaptureExtent,
        source_rotation: crate::DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
        descriptors: &[GpuSurfaceDescriptor],
        admission: GpuSurfaceAdmission,
    ) -> CaptureResult<Self> {
        let allocation_byte_len = admission.admit(logical_source_extent, descriptors)?;
        for descriptor in descriptors {
            if descriptor.source_rotation() != source_rotation {
                return Err(CaptureError::GpuSurfaceRotationMismatch {
                    descriptor_id: descriptor.id(),
                    descriptor_rotation: descriptor.source_rotation(),
                    source_rotation,
                });
            }
            if descriptor.source_color_space() != source_color_space {
                return Err(CaptureError::UnsupportedGpuSurface {
                    descriptor_id: descriptor.id(),
                    reason: GpuSurfaceUnsupportedReason::SourceColorSpace(source_color_space),
                });
            }
        }

        let context4 = context
            .cast::<ID3D11DeviceContext4>()
            .map_err(|_| unsupported_fence_error(descriptors))?;
        let device5 = device
            .cast::<ID3D11Device5>()
            .map_err(|_| unsupported_fence_error(descriptors))?;
        let shader = create_surface_compute_shader(device).map_err(capture_gpu_error)?;
        let params = create_surface_params(device).map_err(capture_gpu_error)?;
        let clean_desc = texture_desc(
            native_source_extent,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
            0,
        );
        let clean = create_texture(device, &clean_desc, None).map_err(capture_gpu_error)?;
        let clean_srv = create_srv(device, &clean).map_err(capture_gpu_error)?;

        let mut routes = Vec::new();
        routes.try_reserve_exact(descriptors.len()).map_err(|_| {
            CaptureError::ResourceExhausted {
                operation: "allocate GPU Surface routes",
                requested_bytes: descriptors.len().saturating_mul(size_of::<SurfaceRoute>()),
            }
        })?;
        let mut next_slot_id = 1_u64;
        for descriptor in descriptors {
            routes.push(create_route(
                device,
                &device5,
                descriptor,
                admission.slots_per_descriptor().get(),
                &mut next_slot_id,
            )?);
        }

        Ok(Self {
            plan_generation,
            source_id,
            topology_generation,
            duplication_generation,
            adapter_luid,
            native_source_extent,
            logical_source_extent,
            source_rotation,
            source_color_space,
            device: device.clone(),
            context: context.clone(),
            context4,
            shader,
            params,
            clean,
            clean_srv,
            clean_metadata: None,
            pointer: None,
            routes,
            texture_budget: admission.max_texture_bytes(),
            allocation_byte_len,
            #[cfg(test)]
            injected_fault: None,
        })
    }

    /// Committed plan generation stamped into every result.
    #[must_use]
    pub const fn plan_generation(&self) -> GpuSurfacePlanGeneration {
        self.plan_generation
    }

    /// Exact descriptors in stable publication order.
    #[must_use]
    pub fn descriptors(&self) -> impl ExactSizeIterator<Item = &GpuSurfaceDescriptor> {
        self.routes.iter().map(|route| route.descriptor.as_ref())
    }

    /// Checked texture bytes retained by the prepared plan.
    #[must_use]
    pub const fn allocation_byte_len(&self) -> u64 {
        self.allocation_byte_len
    }

    /// GPU Surface publication performs no staging readback.
    #[must_use]
    pub const fn readback_byte_len(&self) -> u64 {
        0
    }

    /// GPU Surface callback fanout retains no per-frame outcome buffer.
    #[must_use]
    pub const fn publication_buffer_byte_len(&self) -> usize {
        0
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn matches_source(
        &self,
        source_id: &str,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        native_source_extent: CaptureExtent,
        logical_source_extent: CaptureExtent,
        source_rotation: crate::DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
    ) -> bool {
        self.source_id.as_ref() == source_id
            && self.topology_generation == topology_generation
            && self.duplication_generation == duplication_generation
            && self.adapter_luid == adapter_luid
            && self.native_source_extent == native_source_extent
            && self.logical_source_extent == logical_source_extent
            && self.source_rotation == source_rotation
            && self.source_color_space == source_color_space
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_source(
        &self,
        source_id: &str,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        native_source_extent: CaptureExtent,
        logical_source_extent: CaptureExtent,
        source_rotation: crate::DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
    ) -> CaptureResult<()> {
        if self.matches_source(
            source_id,
            topology_generation,
            duplication_generation,
            adapter_luid,
            native_source_extent,
            logical_source_extent,
            source_rotation,
            source_color_space,
        ) {
            Ok(())
        } else {
            Err(CaptureError::GpuSurfacePlanInvalidated)
        }
    }

    pub(super) fn has_clean_desktop(&self) -> bool {
        self.clean_metadata.is_some()
    }

    pub(super) fn has_pending_routes(&self) -> bool {
        self.routes
            .iter()
            .any(|route| route.pending_source_sequence.is_some())
    }

    pub(super) fn publish<F>(
        &mut self,
        texture: Option<&ID3D11Texture2D>,
        metadata: CaptureMetadata,
        duplication_generation: u64,
        emit: F,
    ) -> CaptureResult<GpuSurfaceBatchInfo>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        let logical_source_extent = logical_extent(
            metadata.source_width,
            metadata.source_height,
            metadata.rotation,
        )?;
        self.validate_source(
            &metadata.source_id,
            metadata.topology_generation,
            duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(metadata.source_width, metadata.source_height)?,
            logical_source_extent,
            metadata.rotation,
            metadata.source_color_space,
        )?;
        for route in &self.routes {
            metadata
                .captured_at
                .checked_add(route.descriptor.freshness())
                .ok_or(CaptureError::GpuSurfaceFreshnessOverflow)?;
        }
        self.update_clean(texture, &metadata)?;
        for route in &mut self.routes {
            route.pending_source_sequence = Some(metadata.sequence);
        }
        self.validate_cursor_shape(&metadata)?;
        if self
            .routes
            .iter()
            .any(|route| route.descriptor.cursor() == GpuSurfaceCursorPolicy::Include)
        {
            self.ensure_pointer(&metadata.pointer)?;
        }
        self.fanout_pending(&metadata, emit)
    }

    pub(super) fn retry_pending<F>(&mut self, mut emit: F) -> CaptureResult<GpuSurfaceBatchInfo>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        let metadata = self.clean_metadata.clone().ok_or_else(|| {
            CaptureError::windows(
                "retry exact GPU Surface",
                "no retained clean desktop is available",
            )
        })?;
        self.validate_cursor_shape(&metadata)?;
        if self
            .routes
            .iter()
            .any(|route| route.descriptor.cursor() == GpuSurfaceCursorPolicy::Include)
        {
            self.ensure_pointer(&metadata.pointer)?;
        }
        self.fanout_pending(&metadata, &mut emit)
    }

    fn validate_cursor_shape(&self, metadata: &CaptureMetadata) -> CaptureResult<()> {
        if metadata.pointer.visible
            && metadata.pointer.shape.is_none()
            && let Some(route) = self
                .routes
                .iter()
                .find(|route| route.descriptor.cursor() == GpuSurfaceCursorPolicy::Include)
        {
            return Err(CaptureError::GpuSurfaceCursorShapeUnavailable {
                descriptor_id: route.descriptor.id(),
                source_sequence: metadata.sequence,
            });
        }
        Ok(())
    }

    fn fanout_pending<F>(
        &mut self,
        metadata: &CaptureMetadata,
        mut emit: F,
    ) -> CaptureResult<GpuSurfaceBatchInfo>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        self.reclaim_abandoned()?;
        for route in &mut self.routes {
            route.pending = PendingRouteOutcome::None;
        }
        let mut published = 0;
        let mut busy = 0;
        let mut publish_error = None;
        for route_index in 0..self.routes.len() {
            if self.routes[route_index].pending_source_sequence != Some(metadata.sequence) {
                continue;
            }
            match self.publish_route(route_index, metadata) {
                Ok(outcome) => {
                    match outcome {
                        PendingRouteOutcome::Published(_) => published += 1,
                        PendingRouteOutcome::Busy => busy += 1,
                        PendingRouteOutcome::None => {
                            unreachable!("publication route always records an outcome")
                        }
                    }
                    self.routes[route_index].pending = outcome;
                }
                Err(error) => {
                    publish_error = Some(error);
                    break;
                }
            }
        }
        if published != 0 {
            // SAFETY: Flush submits the fanout and fence signals without
            // waiting for GPU completion or serializing consumer work.
            unsafe { self.context.Flush() };
            self.activate_pending_publications();
        }
        if let Some(error) = publish_error {
            return Err(error);
        }
        for route in &mut self.routes {
            match route.pending {
                PendingRouteOutcome::None => {}
                PendingRouteOutcome::Busy => {
                    emit(GpuSurfacePublishOutcome::Busy(route.descriptor.id()));
                }
                PendingRouteOutcome::Published(slot_index) => {
                    route.pending_source_sequence = None;
                    emit(GpuSurfacePublishOutcome::Published(Arc::clone(
                        &route.slots[slot_index].publication,
                    )));
                }
            }
        }
        Ok(GpuSurfaceBatchInfo {
            source_sequence: metadata.sequence,
            published,
            busy,
        })
    }

    fn activate_pending_publications(&mut self) {
        let published_at = Instant::now();
        for route in &mut self.routes {
            let PendingRouteOutcome::Published(slot_index) = route.pending else {
                continue;
            };
            let publication = Arc::get_mut(&mut route.slots[slot_index].publication)
                .expect("a submitted publication has no external owner before callback fanout");
            publication
                .provenance
                .as_mut()
                .expect("a submitted publication has initialized provenance")
                .published_at = published_at;
            publication.state.store(USE_UNCLAIMED, Ordering::Release);
        }
    }

    /// Reclaim uniquely owned publications that were superseded unclaimed.
    ///
    /// Claimed publications remain fence-gated. A claim guard dropped before
    /// queuing its release poisons the plan rather than risking a conflicting
    /// native owner.
    ///
    /// # Errors
    ///
    /// Returns a typed device or poisoned-plan error when ownership cannot be
    /// recovered safely.
    pub fn reclaim_abandoned(&mut self) -> CaptureResult<usize> {
        let mut reclaimed = 0;
        for route in &mut self.routes {
            for slot in &mut route.slots {
                if Arc::strong_count(&slot.publication) != 1 {
                    continue;
                }
                let state = slot.publication.state.load(Ordering::Acquire);
                match state {
                    USE_IDLE => {}
                    USE_UNCLAIMED => {
                        let Some(release) = slot.required_release_value else {
                            slot.publication
                                .state
                                .store(USE_POISONED, Ordering::Release);
                            return Err(CaptureError::GpuSurfacePlanPoisoned {
                                descriptor_id: route.descriptor.id(),
                                use_id: slot.publication.use_id,
                            });
                        };
                        match acquire_keyed_mutex(
                            &slot.publication.shared.keyed_mutex,
                            1,
                            0,
                            "acquire abandoned GPU Surface",
                        )? {
                            KeyedMutexAcquire::Acquired => {}
                            KeyedMutexAcquire::Timeout => continue,
                            KeyedMutexAcquire::Abandoned => {
                                poison_surface_slot(slot);
                                return Err(CaptureError::GpuSurfacePlanPoisoned {
                                    descriptor_id: route.descriptor.id(),
                                    use_id: slot.publication.use_id,
                                });
                            }
                        }
                        // SAFETY: the successful acquire above owns key 1.
                        unsafe { slot.publication.shared.keyed_mutex.ReleaseSync(0) }.map_err(
                            |error| CaptureError::windows("release abandoned GPU Surface", error),
                        )?;
                        // SAFETY: this signal is ordered after producer-side
                        // reclamation released key 0 for the next slot use.
                        unsafe {
                            self.context4
                                .Signal(&slot.publication.shared.fence, release)
                        }
                        .map_err(|error| {
                            CaptureError::windows("signal abandoned GPU Surface release", error)
                        })?;
                        slot.publication
                            .state
                            .store(USE_RELEASE_QUEUED, Ordering::Release);
                        reclaimed += 1;
                    }
                    USE_CLAIMED => {
                        slot.publication
                            .state
                            .store(USE_POISONED, Ordering::Release);
                        return Err(CaptureError::GpuSurfacePlanPoisoned {
                            descriptor_id: route.descriptor.id(),
                            use_id: slot.publication.use_id,
                        });
                    }
                    USE_RELEASE_QUEUED => {
                        let Some(required) = slot.required_release_value else {
                            slot.publication
                                .state
                                .store(USE_POISONED, Ordering::Release);
                            return Err(CaptureError::GpuSurfacePlanPoisoned {
                                descriptor_id: route.descriptor.id(),
                                use_id: slot.publication.use_id,
                            });
                        };
                        // SAFETY: the fence remains owned by this slot.
                        let completed =
                            unsafe { slot.publication.shared.fence.GetCompletedValue() };
                        if completed == u64::MAX {
                            return Err(CaptureError::DeviceLost);
                        }
                        if completed >= required {
                            slot.required_release_value = None;
                            slot.publication.state.store(USE_IDLE, Ordering::Release);
                        }
                    }
                    USE_POISONED => {
                        return Err(CaptureError::GpuSurfacePlanPoisoned {
                            descriptor_id: route.descriptor.id(),
                            use_id: slot.publication.use_id,
                        });
                    }
                    _ => {
                        slot.publication
                            .state
                            .store(USE_POISONED, Ordering::Release);
                        return Err(CaptureError::GpuSurfacePlanPoisoned {
                            descriptor_id: route.descriptor.id(),
                            use_id: slot.publication.use_id,
                        });
                    }
                }
            }
        }
        if reclaimed != 0 {
            // SAFETY: Flush submits queued release signals without waiting.
            unsafe { self.context.Flush() };
        }
        Ok(reclaimed)
    }

    fn update_clean(
        &mut self,
        texture: Option<&ID3D11Texture2D>,
        metadata: &CaptureMetadata,
    ) -> CaptureResult<()> {
        if let Some(texture) = texture {
            let mut observed = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: GetDesc fills caller-owned storage and cannot fail.
            unsafe { texture.GetDesc(&mut observed) };
            if observed.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                let descriptor_id = self
                    .routes
                    .first()
                    .map_or_else(fallback_descriptor_id, |route| route.descriptor.id());
                return Err(CaptureError::UnsupportedGpuSurface {
                    descriptor_id,
                    reason: GpuSurfaceUnsupportedReason::SourceFormat,
                });
            }
            if observed.Width != self.native_source_extent.width()
                || observed.Height != self.native_source_extent.height()
            {
                return Err(CaptureError::GpuSurfacePlanInvalidated);
            }
            // SAFETY: the descriptors match and both resources belong to the
            // same D3D11 device and immediate context.
            unsafe { self.context.CopyResource(&self.clean, texture) };
            self.clean_metadata = Some(metadata.clone());
        } else if self.clean_metadata.is_none() {
            return Err(CaptureError::windows(
                "publish exact GPU Surface",
                "no retained clean desktop is available",
            ));
        } else {
            self.clean_metadata = Some(metadata.clone());
        }
        Ok(())
    }

    fn ensure_pointer(&mut self, pointer: &PointerState) -> CaptureResult<()> {
        let Some(shape) = pointer.shape.as_ref() else {
            return Ok(());
        };
        let height = shape.visible_height();
        if self.pointer.as_ref().is_some_and(|resource| {
            resource.generation == pointer.shape_generation
                && resource.width == shape.width
                && resource.height == height
        }) {
            return Ok(());
        }
        let byte_len = checked_gpu_surface_bytes(CaptureExtent::try_new(shape.width, height)?)?;
        let previous = self
            .pointer
            .as_ref()
            .map_or(0, |resource| resource.byte_len);
        let replacement_total = self
            .allocation_byte_len
            .checked_sub(previous)
            .and_then(|bytes| bytes.checked_add(byte_len))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account GPU pointer texture",
                width: shape.width,
                height,
            })?;
        if replacement_total > self.texture_budget {
            return Err(CaptureError::GpuSurfaceBudgetExceeded {
                requested_bytes: replacement_total,
                budget_bytes: self.texture_budget,
            });
        }

        let pixels = normalized_pointer(shape).map_err(capture_gpu_error)?;
        let desc = texture_desc(
            CaptureExtent::try_new(shape.width, height)?,
            DXGI_FORMAT_R8G8B8A8_UINT,
            D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
            0,
        );
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: checked_rgba_row_pitch(shape.width, height, "create GPU pointer texture")
                .map_err(capture_gpu_error)?,
            SysMemSlicePitch: 0,
        };
        let texture =
            create_texture(&self.device, &desc, Some(&initial)).map_err(capture_gpu_error)?;
        let srv = create_srv(&self.device, &texture).map_err(capture_gpu_error)?;
        self.pointer = Some(PointerResource {
            generation: pointer.shape_generation,
            width: shape.width,
            height,
            byte_len,
            srv,
        });
        self.allocation_byte_len = replacement_total;
        Ok(())
    }

    fn publish_route(
        &mut self,
        route_index: usize,
        metadata: &CaptureMetadata,
    ) -> CaptureResult<PendingRouteOutcome> {
        #[cfg(test)]
        let injected_fault = self.injected_fault.take();
        let route = &mut self.routes[route_index];
        let descriptor = Arc::clone(&route.descriptor);
        let slot_count = route.slots.len();
        let mut slot_index = None;
        for offset in 0..slot_count {
            let candidate_index = (route.write_index + offset) % slot_count;
            let candidate = &route.slots[candidate_index];
            let available = Arc::strong_count(&candidate.publication) == 1
                && candidate.publication.state.load(Ordering::Acquire) == USE_IDLE
                && candidate.required_release_value.is_none();
            if available {
                slot_index = Some(candidate_index);
                break;
            }
        }
        let Some(slot_index) = slot_index else {
            return Ok(PendingRouteOutcome::Busy);
        };
        let slot = &mut route.slots[slot_index];
        let ready = slot
            .next_signal_value
            .checked_add(1)
            .ok_or(CaptureError::GpuSurfaceSynchronizationExhausted)?;
        let release = ready
            .checked_add(1)
            .ok_or(CaptureError::GpuSurfaceSynchronizationExhausted)?;
        let use_id = slot
            .next_use_id
            .checked_add(1)
            .ok_or(CaptureError::GpuSurfaceSynchronizationExhausted)?;
        let freshness_deadline = metadata
            .captured_at
            .checked_add(descriptor.freshness())
            .ok_or(CaptureError::GpuSurfaceFreshnessOverflow)?;

        Arc::get_mut(&mut slot.publication)
            .expect("an idle native slot has no external publication owners")
            .use_id = use_id;

        match acquire_keyed_mutex(
            &slot.publication.shared.keyed_mutex,
            0,
            0,
            "acquire GPU Surface producer key",
        )? {
            KeyedMutexAcquire::Acquired => {}
            KeyedMutexAcquire::Timeout => return Ok(PendingRouteOutcome::Busy),
            KeyedMutexAcquire::Abandoned => {
                poison_surface_slot(slot);
                return Err(CaptureError::GpuSurfacePlanPoisoned {
                    descriptor_id: descriptor.id(),
                    use_id,
                });
            }
        }
        #[cfg(test)]
        if injected_fault == Some(InjectedSurfaceFault::AfterProducerAcquire) {
            poison_surface_slot(slot);
            return Err(CaptureError::windows(
                "publish exact GPU Surface",
                "injected failure after producer acquire",
            ));
        }
        let pointer = &metadata.pointer;
        let shape = (descriptor.cursor() == GpuSurfaceCursorPolicy::Include)
            .then_some(pointer)
            .and_then(|pointer| pointer.shape.as_ref().filter(|_| pointer.visible));
        let output = descriptor.output_extent();
        let region = descriptor.source_region();
        let params = SurfaceShaderParams {
            source_width: metadata.source_width,
            source_height: metadata.source_height,
            output_width: output.width(),
            output_height: output.height(),
            stride: 0,
            rotation: rotation_code(metadata.rotation),
            pointer_kind: shape.map_or(0, |shape| pointer_kind_code(shape.kind)),
            pointer_visible: u32::from(shape.is_some()),
            pointer_x: pointer.position_x,
            pointer_y: pointer.position_y,
            pointer_width: shape.map_or(0, |shape| shape.width),
            pointer_height: shape.map_or(0, super::PointerShape::visible_height),
            region_x: region.origin_x(),
            region_y: region.origin_y(),
            region_width: region.width(),
            region_height: region.height(),
        };
        update_params(&self.context, &self.params, &params);
        let srvs = [
            Some(self.clean_srv.clone()),
            shape.and(self.pointer.as_ref().map(|resource| resource.srv.clone())),
        ];
        let uavs = [Some(slot.uav.clone())];
        // SAFETY: prepared descriptors, views, and constants match the shader
        // contract. HLSL clips dispatch overhang to the exact output extent.
        unsafe {
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(uavs.as_ptr()), None);
            self.context.CSSetShader(&self.shader, None::<&[Option<_>]>);
            self.context.Dispatch(
                output.width().div_ceil(THREAD_GROUP),
                output.height().div_ceil(THREAD_GROUP),
                1,
            );
        }
        unbind_compute_views(&self.context);
        // SAFETY: the producer owns key 0 and releases key 1 only after every
        // exact Surface write has been queued on the immediate context.
        if let Err(error) = unsafe { slot.publication.shared.keyed_mutex.ReleaseSync(1) } {
            poison_surface_slot(slot);
            return Err(CaptureError::windows(
                "release GPU Surface consumer key",
                error,
            ));
        }
        #[cfg(test)]
        if injected_fault == Some(InjectedSurfaceFault::AfterProducerRelease) {
            poison_surface_slot(slot);
            return Err(CaptureError::windows(
                "publish exact GPU Surface",
                "injected failure after producer release",
            ));
        }
        // SAFETY: the fence and immediate context belong to the same device;
        // Signal is queued after every command that writes this texture.
        if let Err(error) = unsafe { self.context4.Signal(&slot.publication.shared.fence, ready) } {
            poison_surface_slot(slot);
            return Err(CaptureError::windows(
                "signal GPU Surface ready fence",
                error,
            ));
        }

        slot.next_signal_value = release;
        slot.next_use_id = use_id;
        slot.required_release_value = Some(release);
        route.write_index = (slot_index + 1) % slot_count;
        let published_at = Instant::now();
        let cursor_composed = shape.is_some();
        let provenance = GpuSurfaceProvenance {
            descriptor,
            plan_generation: self.plan_generation,
            adapter_luid: self.adapter_luid,
            slot_id: slot.publication.slot_id,
            use_id,
            source_id: Arc::clone(&metadata.source_id),
            topology_generation: metadata.topology_generation,
            duplication_generation: self.duplication_generation,
            source_sequence: metadata.sequence,
            captured_at: metadata.captured_at,
            published_at,
            freshness_deadline,
            native_source_extent: self.native_source_extent,
            logical_source_extent: self.logical_source_extent,
            coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
            output_extent: output,
            source_format: GpuSurfaceFormat::Bgra8Unorm,
            source_color_space: self.source_color_space,
            output_format: GpuSurfaceFormat::Rgba8Unorm,
            color_pipeline: GpuSurfaceColorPipeline::PreserveEncoded,
            pending_rotation: crate::DisplayRotation::Identity,
            cursor_composed,
        };
        let publication = Arc::get_mut(&mut slot.publication)
            .expect("an idle native slot has no external publication owners");
        publication.provenance = Some(provenance);
        publication.synchronization = GpuSurfaceSynchronization {
            producer_acquire_key: 0,
            producer_release_key: 1,
            consumer_acquire_key: 1,
            consumer_release_key: 0,
            producer_ready_value: ready,
            consumer_release_value: release,
        };
        publication.state.store(USE_PREPARED, Ordering::Release);
        Ok(PendingRouteOutcome::Published(slot_index))
    }
}

fn poison_surface_slot(slot: &SurfaceSlot) {
    slot.publication
        .state
        .store(USE_POISONED, Ordering::Release);
}

fn create_route(
    device: &ID3D11Device,
    device5: &ID3D11Device5,
    descriptor: &GpuSurfaceDescriptor,
    slot_count: u32,
    next_slot_id: &mut u64,
) -> CaptureResult<SurfaceRoute> {
    let descriptor = Arc::new(descriptor.clone());
    let mut slots = Vec::new();
    let slot_count = usize::try_from(slot_count).map_err(|_| CaptureError::ResourceExhausted {
        operation: "allocate GPU Surface slots",
        requested_bytes: usize::MAX,
    })?;
    slots
        .try_reserve_exact(slot_count)
        .map_err(|_| CaptureError::ResourceExhausted {
            operation: "allocate GPU Surface slots",
            requested_bytes: slot_count.saturating_mul(size_of::<SurfaceSlot>()),
        })?;
    for _ in 0..slot_count {
        let slot_id = GpuSurfaceSlotId::new(
            std::num::NonZeroU64::new(*next_slot_id)
                .ok_or(CaptureError::GpuSurfaceSynchronizationExhausted)?,
        );
        *next_slot_id = next_slot_id
            .checked_add(1)
            .ok_or(CaptureError::GpuSurfaceSynchronizationExhausted)?;
        slots.push(create_surface_slot(
            device,
            device5,
            descriptor.output_extent(),
            slot_id,
        )?);
    }
    Ok(SurfaceRoute {
        descriptor,
        slots,
        write_index: 0,
        pending: PendingRouteOutcome::None,
        pending_source_sequence: None,
    })
}

fn create_surface_slot(
    device: &ID3D11Device,
    device5: &ID3D11Device5,
    extent: CaptureExtent,
    slot_id: GpuSurfaceSlotId,
) -> CaptureResult<SurfaceSlot> {
    let desc = texture_desc(
        extent,
        DXGI_FORMAT_R8G8B8A8_UNORM,
        (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS)
            .0
            .cast_unsigned(),
        (D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
            .0
            .cast_unsigned(),
    );
    let texture = create_texture(device, &desc, None).map_err(capture_gpu_error)?;
    let uav = create_uav(device, &texture).map_err(capture_gpu_error)?;
    let keyed_mutex = texture
        .cast::<IDXGIKeyedMutex>()
        .map_err(|error| CaptureError::windows("query GPU Surface keyed mutex", error))?;
    let resource = texture
        .cast::<IDXGIResource1>()
        .map_err(|error| CaptureError::windows("query shareable GPU Surface", error))?;
    // SAFETY: null security attributes and name create an unnamed NT handle
    // owned by this process with read/write sharing rights.
    let texture_handle = OwnedHandle::new(
        unsafe {
            resource.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                PCWSTR::null(),
            )
        }
        .map_err(|error| CaptureError::windows("create shared GPU Surface handle", error))?,
    )?;

    let mut fence = None;
    // SAFETY: the out-pointer is live and requests the documented shared-fence
    // interface from a device already queried as ID3D11Device5.
    unsafe { device5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence) }
        .map_err(|error| CaptureError::windows("create GPU Surface fence", error))?;
    let fence: ID3D11Fence = fence.ok_or_else(|| {
        CaptureError::windows("create GPU Surface fence", "D3D11 returned no fence")
    })?;
    // SAFETY: null security attributes and name create one process-owned NT
    // handle for the shared fence.
    let fence_handle = OwnedHandle::new(
        unsafe { fence.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()) }.map_err(
            |error| CaptureError::windows("create shared GPU Surface fence handle", error),
        )?,
    )?;

    Ok(SurfaceSlot {
        publication: Arc::new(GpuSurfacePublication {
            provenance: None,
            shared: SharedSurfaceSlot {
                _texture: texture,
                keyed_mutex,
                fence,
                texture_handle,
                fence_handle,
            },
            synchronization: GpuSurfaceSynchronization {
                producer_acquire_key: 0,
                producer_release_key: 1,
                consumer_acquire_key: 1,
                consumer_release_key: 0,
                producer_ready_value: 0,
                consumer_release_value: 0,
            },
            slot_id,
            use_id: 0,
            state: AtomicU8::new(USE_IDLE),
        }),
        uav,
        next_signal_value: 0,
        next_use_id: 0,
        required_release_value: None,
    })
}

fn texture_desc(
    extent: CaptureExtent,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    bind_flags: u32,
    misc_flags: u32,
) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: extent.width(),
        Height: extent.height(),
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind_flags,
        CPUAccessFlags: D3D11_CPU_ACCESS_FLAG(0).0.cast_unsigned(),
        MiscFlags: misc_flags,
    }
}

fn create_surface_params(device: &ID3D11Device) -> Result<ID3D11Buffer, GpuReductionError> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: u32::try_from(size_of::<SurfaceShaderParams>()).unwrap_or(u32::MAX),
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: windows::Win32::Graphics::Direct3D11::D3D11_BIND_CONSTANT_BUFFER
            .0
            .cast_unsigned(),
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    // SAFETY: the constant-buffer descriptor and out-pointer are live.
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(|error| {
        GpuReductionError::Windows {
            context: "create GPU Surface constants",
            message: error.to_string(),
        }
    })?;
    buffer.ok_or_else(|| GpuReductionError::Operation {
        message: "GPU Surface constant buffer creation returned no buffer".to_owned(),
    })
}

fn update_params(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    params: &SurfaceShaderParams,
) {
    // SAFETY: the buffer is exactly SurfaceShaderParams bytes and the source
    // pointer remains valid for the duration of UpdateSubresource.
    unsafe { context.UpdateSubresource(buffer, 0, None, std::ptr::from_ref(params).cast(), 0, 0) };
}

fn unbind_compute_views(context: &ID3D11DeviceContext) {
    let no_srvs = [None, None];
    let no_uavs = [None];
    // SAFETY: null bindings detach resources before subsequent writes or
    // external sharing; the arrays cover every slot used by the shader.
    unsafe {
        context.CSSetShaderResources(0, Some(&no_srvs));
        context.CSSetUnorderedAccessViews(0, 1, Some(no_uavs.as_ptr()), None);
        context.CSSetShader(None, None::<&[Option<_>]>);
    }
}

fn unsupported_fence_error(descriptors: &[GpuSurfaceDescriptor]) -> CaptureError {
    CaptureError::UnsupportedGpuSurface {
        descriptor_id: descriptors
            .first()
            .map_or_else(fallback_descriptor_id, GpuSurfaceDescriptor::id),
        reason: GpuSurfaceUnsupportedReason::SharedFenceUnavailable,
    }
}

fn fallback_descriptor_id() -> GpuSurfaceDescriptorId {
    GpuSurfaceDescriptorId::new(
        std::num::NonZeroU64::new(1).expect("one is a non-zero descriptor identity"),
    )
}

fn capture_gpu_error(error: GpuReductionError) -> CaptureError {
    error
        .as_capture_error()
        .unwrap_or_else(|| CaptureError::windows("prepare or publish exact GPU Surface", error))
}

fn logical_extent(
    native_width: u32,
    native_height: u32,
    rotation: crate::DisplayRotation,
) -> CaptureResult<CaptureExtent> {
    match rotation {
        crate::DisplayRotation::Identity | crate::DisplayRotation::Clockwise180 => {
            CaptureExtent::try_new(native_width, native_height)
        }
        crate::DisplayRotation::Clockwise90 | crate::DisplayRotation::Clockwise270 => {
            CaptureExtent::try_new(native_height, native_width)
        }
    }
}

#[cfg(test)]
pub(super) mod fixture {
    use std::num::{NonZeroU32, NonZeroU64};
    use std::time::{Duration, Instant};

    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_STAGING, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence,
        ID3D11Texture2D,
    };
    use windows::core::Interface;

    use super::*;
    use crate::duplication::gpu_reduction::checked_rgba_len;
    use crate::{CaptureRegion, GpuSurfaceDescriptorConfig, GpuSurfaceFilter};

    pub(crate) fn d3d11on12_bridges_a_keyed_surface_into_d3d12() -> CaptureResult<()> {
        use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, ID3D11Device, ID3D11Device1, ID3D11Device5,
            ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Resource,
        };
        use windows::Win32::Graphics::Direct3D11on12::{
            D3D11_RESOURCE_FLAGS, D3D11On12CreateDevice, ID3D11On12Device,
        };
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
            D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
            D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            D3D12_RESOURCE_STATE_COPY_DEST, D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12CreateDevice,
            ID3D12CommandQueue, ID3D12Device, ID3D12Resource,
        };
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory4,
        };
        use windows::core::IUnknown;

        // SAFETY: the factory call has no borrowed inputs or out-pointers.
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
            .map_err(|error| CaptureError::windows("create WARP DXGI factory", error))?;
        // SAFETY: the live factory returns a reference-counted adapter.
        let adapter: IDXGIAdapter1 = unsafe { factory.EnumWarpAdapter() }
            .map_err(|error| CaptureError::windows("enumerate WARP adapter", error))?;
        let (capture_device, capture_context) =
            super::super::gpu_reduction::test_device().map_err(capture_gpu_error)?;
        let capture_device5 = capture_device
            .cast::<ID3D11Device5>()
            .map_err(|error| CaptureError::windows("query capture fixture fence support", error))?;
        let capture_context4 = capture_context
            .cast::<ID3D11DeviceContext4>()
            .map_err(|error| CaptureError::windows("query capture fixture fence context", error))?;
        let source_slot = create_surface_slot(
            &capture_device,
            &capture_device5,
            CaptureExtent::try_new(4, 4)?,
            GpuSurfaceSlotId::new(NonZeroU64::MIN),
        )?;
        require_keyed_mutex(
            &source_slot.publication.shared.keyed_mutex,
            0,
            u32::MAX,
            "acquire capture fixture key",
        )?;
        // SAFETY: the fixture owns key 0 from the successful acquire.
        unsafe { source_slot.publication.shared.keyed_mutex.ReleaseSync(1) }
            .map_err(|error| CaptureError::windows("publish capture fixture key", error))?;
        // SAFETY: the fence belongs to this immediate context's device.
        unsafe { capture_context4.Signal(&source_slot.publication.shared.fence, 1) }
            .map_err(|error| CaptureError::windows("signal capture fixture fence", error))?;
        // SAFETY: Flush submits the fixture hand-off without waiting.
        unsafe { capture_context.Flush() };

        let mut d3d12_device = None;
        // SAFETY: the WARP adapter is live and the out-pointer is valid.
        unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut d3d12_device) }
            .map_err(|error| CaptureError::windows("create WARP D3D12 device", error))?;
        let d3d12_device: ID3D12Device = d3d12_device.ok_or_else(|| {
            CaptureError::windows("create WARP D3D12 device", "D3D12 returned no device")
        })?;
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            ..D3D12_COMMAND_QUEUE_DESC::default()
        };
        // SAFETY: queue_desc remains live and fully initialized for the call.
        let command_queue: ID3D12CommandQueue =
            unsafe { d3d12_device.CreateCommandQueue(&raw const queue_desc) }
                .map_err(|error| CaptureError::windows("create WARP D3D12 queue", error))?;

        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
            ..D3D12_HEAP_PROPERTIES::default()
        };
        let texture_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 4,
            Height: 4,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            ..D3D12_RESOURCE_DESC::default()
        };
        let mut d3d12_texture = None;
        // SAFETY: descriptors and the output slot remain live for the call.
        unsafe {
            d3d12_device.CreateCommittedResource(
                &raw const heap,
                D3D12_HEAP_FLAG_NONE,
                &raw const texture_desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut d3d12_texture,
            )
        }
        .map_err(|error| CaptureError::windows("create shared D3D12 texture", error))?;
        let d3d12_texture: ID3D12Resource = d3d12_texture.ok_or_else(|| {
            CaptureError::windows("create D3D12 copy target", "D3D12 returned no resource")
        })?;

        let queue_unknown = command_queue
            .cast::<IUnknown>()
            .map_err(|error| CaptureError::windows("cast WARP D3D12 queue for D3D11On12", error))?;
        let queues = [Some(queue_unknown)];
        let mut d3d11_device: Option<ID3D11Device> = None;
        let mut d3d11_context: Option<ID3D11DeviceContext> = None;
        // SAFETY: the D3D12 device, queue array, feature-level slice, and both
        // output slots remain live for the duration of device creation.
        unsafe {
            D3D11On12CreateDevice(
                &d3d12_device,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT.0,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                Some(&queues),
                0,
                Some(&mut d3d11_device),
                Some(&mut d3d11_context),
                None,
            )
        }
        .map_err(|error| CaptureError::windows("create WARP D3D11On12 device", error))?;
        let d3d11_device = d3d11_device.ok_or_else(|| {
            CaptureError::windows("create WARP D3D11On12 device", "D3D11 returned no device")
        })?;
        let d3d11_context = d3d11_context.ok_or_else(|| {
            CaptureError::windows(
                "create WARP D3D11On12 device",
                "D3D11 returned no immediate context",
            )
        })?;
        let d3d11on12 = d3d11_device
            .cast::<ID3D11On12Device>()
            .map_err(|error| CaptureError::windows("query D3D11On12 wrapping support", error))?;
        let device1 = d3d11_device.cast::<ID3D11Device1>().map_err(|error| {
            CaptureError::windows("query bridge shared-resource support", error)
        })?;
        // SAFETY: source_slot owns the shared handle throughout this fixture.
        let imported_source: ID3D11Texture2D =
            unsafe { device1.OpenSharedResource1(source_slot.publication.shared.texture_handle.0) }
                .map_err(|error| CaptureError::windows("open keyed source on D3D11On12", error))?;
        let imported_keyed_mutex = imported_source
            .cast::<IDXGIKeyedMutex>()
            .map_err(|error| CaptureError::windows("query imported source keyed mutex", error))?;
        let device5 = d3d11_device
            .cast::<ID3D11Device5>()
            .map_err(|error| CaptureError::windows("query bridge shared-fence support", error))?;
        let mut imported_fence = None;
        // SAFETY: source_slot owns the fence handle and the output slot is live.
        unsafe {
            device5.OpenSharedFence::<ID3D11Fence>(
                source_slot.publication.shared.fence_handle.0,
                &mut imported_fence,
            )
        }
        .map_err(|error| CaptureError::windows("open source fence on D3D11On12", error))?;
        let imported_fence = imported_fence.ok_or_else(|| {
            CaptureError::windows("open source fence on D3D11On12", "D3D11 returned no fence")
        })?;
        let bridge_context4 = d3d11_context
            .cast::<ID3D11DeviceContext4>()
            .map_err(|error| CaptureError::windows("query D3D11On12 fence context", error))?;

        // SAFETY: value 1 is the capture-side ready signal queued above.
        unsafe { bridge_context4.Wait(&imported_fence, 1) }
            .map_err(|error| CaptureError::windows("wait for imported source", error))?;
        require_keyed_mutex(
            &imported_keyed_mutex,
            1,
            u32::MAX,
            "acquire imported source key",
        )?;

        let resource_flags = D3D11_RESOURCE_FLAGS::default();
        let mut d3d11_texture = None;
        // SAFETY: the D3D12 texture and output slot stay live, and the declared
        // in/out state matches its COPY_DEST lifetime in this fixture.
        unsafe {
            d3d11on12.CreateWrappedResource::<_, ID3D11Texture2D>(
                &d3d12_texture,
                &raw const resource_flags,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COPY_DEST,
                &mut d3d11_texture,
            )
        }
        .map_err(|error| CaptureError::windows("wrap D3D12 texture for D3D11", error))?;
        let d3d11_texture = d3d11_texture.ok_or_else(|| {
            CaptureError::windows("wrap D3D12 texture for D3D11", "D3D11 returned no texture")
        })?;
        let resource = d3d11_texture
            .cast::<ID3D11Resource>()
            .map_err(|error| CaptureError::windows("cast wrapped D3D11 texture", error))?;
        // SAFETY: both textures are 4x4 RGBA8 resources; wrapped ownership is
        // acquired and released around the sole bridge copy.
        unsafe {
            d3d11on12.AcquireWrappedResources(&[Some(resource.clone())]);
            d3d11_context.CopyResource(&d3d11_texture, &imported_source);
            d3d11on12.ReleaseWrappedResources(&[Some(resource)]);
        }
        // SAFETY: the bridge owns key 1 and has queued its final source access.
        unsafe { imported_keyed_mutex.ReleaseSync(0) }
            .map_err(|error| CaptureError::windows("release imported source key", error))?;
        // SAFETY: the release signal follows the final source access and key release.
        unsafe { bridge_context4.Signal(&imported_fence, 2) }
            .map_err(|error| CaptureError::windows("signal imported source release", error))?;
        // SAFETY: Flush submits both wrapped-resource transitions and release.
        unsafe { d3d11_context.Flush() };
        Ok(())
    }

    pub(crate) struct PublishedFixture {
        pub(crate) plan: PreparedGpuSurfacePlan,
        pub(crate) info: GpuSurfaceBatchInfo,
        pub(crate) outcomes: Vec<GpuSurfacePublishOutcome>,
    }

    pub(crate) fn descriptor(
        id: u64,
        source_region: CaptureRegion,
        output_extent: CaptureExtent,
    ) -> GpuSurfaceDescriptor {
        descriptor_for_rotation(
            id,
            source_region,
            output_extent,
            crate::DisplayRotation::Identity,
        )
    }

    pub(crate) fn descriptor_for_rotation(
        id: u64,
        source_region: CaptureRegion,
        output_extent: CaptureExtent,
        source_rotation: crate::DisplayRotation,
    ) -> GpuSurfaceDescriptor {
        descriptor_with_freshness(
            id,
            source_region,
            output_extent,
            source_rotation,
            Duration::from_secs(1),
        )
    }

    pub(crate) fn descriptor_with_freshness(
        id: u64,
        source_region: CaptureRegion,
        output_extent: CaptureExtent,
        source_rotation: crate::DisplayRotation,
        freshness: Duration,
    ) -> GpuSurfaceDescriptor {
        descriptor_with_cursor(
            id,
            source_region,
            output_extent,
            source_rotation,
            GpuSurfaceCursorPolicy::Exclude,
            freshness,
        )
    }

    pub(crate) fn descriptor_with_cursor(
        id: u64,
        source_region: CaptureRegion,
        output_extent: CaptureExtent,
        source_rotation: crate::DisplayRotation,
        cursor: GpuSurfaceCursorPolicy,
        freshness: Duration,
    ) -> GpuSurfaceDescriptor {
        GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
            id: GpuSurfaceDescriptorId::new(NonZeroU64::new(id).expect("fixture id is non-zero")),
            source_region,
            coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
            source_rotation,
            source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
            output_extent,
            filter: GpuSurfaceFilter::Nearest,
            format: GpuSurfaceFormat::Rgba8Unorm,
            color_pipeline: GpuSurfaceColorPipeline::PreserveEncoded,
            cursor,
            algorithm_revision: NonZeroU32::new(1).expect("fixture revision is non-zero"),
            freshness,
        })
    }

    pub(crate) fn publish(
        bgra: &[u8],
        width: u32,
        height: u32,
        descriptors: &[GpuSurfaceDescriptor],
    ) -> CaptureResult<PublishedFixture> {
        publish_rotated(
            bgra,
            width,
            height,
            crate::DisplayRotation::Identity,
            descriptors,
        )
    }

    pub(crate) fn publish_rotated(
        bgra: &[u8],
        width: u32,
        height: u32,
        rotation: crate::DisplayRotation,
        descriptors: &[GpuSurfaceDescriptor],
    ) -> CaptureResult<PublishedFixture> {
        publish_rotated_with_pointer(
            bgra,
            width,
            height,
            rotation,
            descriptors,
            PointerState::default(),
        )
    }

    pub(crate) fn publish_with_pointer(
        bgra: &[u8],
        width: u32,
        height: u32,
        descriptors: &[GpuSurfaceDescriptor],
        pointer: PointerState,
    ) -> CaptureResult<PublishedFixture> {
        publish_rotated_with_pointer(
            bgra,
            width,
            height,
            crate::DisplayRotation::Identity,
            descriptors,
            pointer,
        )
    }

    fn publish_rotated_with_pointer(
        bgra: &[u8],
        width: u32,
        height: u32,
        rotation: crate::DisplayRotation,
        descriptors: &[GpuSurfaceDescriptor],
        pointer: PointerState,
    ) -> CaptureResult<PublishedFixture> {
        let (device, context) =
            super::super::gpu_reduction::test_device().map_err(capture_gpu_error)?;
        let source = super::super::gpu_reduction::test_source(&device, bgra, width, height)
            .map_err(capture_gpu_error)?;
        let source_extent = CaptureExtent::try_new(width, height)?;
        let logical_source_extent = logical_extent(width, height, rotation)?;
        let admission =
            GpuSurfaceAdmission::new(u64::MAX, NonZeroU32::new(2).expect("two is non-zero"));
        let allocation = admission.admit(logical_source_extent, descriptors)?;
        let pointer_bytes = pointer.shape.as_ref().map_or(Ok(0), |shape| {
            checked_gpu_surface_bytes(CaptureExtent::try_new(shape.width, shape.visible_height())?)
        })?;
        let allocation =
            allocation
                .checked_add(pointer_bytes)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account fixture GPU pointer texture",
                    width,
                    height,
                })?;
        let adapter_luid = device_adapter_luid(&device)?;
        let mut plan = PreparedGpuSurfacePlan::prepare(
            &device,
            &context,
            GpuSurfacePlanGeneration::new(
                NonZeroU64::new(7).expect("fixture generation is non-zero"),
            ),
            Arc::from("fixture:display"),
            3,
            5,
            adapter_luid,
            source_extent,
            logical_source_extent,
            rotation,
            GpuSurfaceSourceColorSpace::RgbFullG22P709,
            descriptors,
            GpuSurfaceAdmission::new(allocation, admission.slots_per_descriptor()),
        )?;
        let metadata = CaptureMetadata {
            source_id: Arc::from("fixture:display"),
            topology_generation: 3,
            sequence: 41,
            captured_at: Instant::now(),
            cursor: crate::CursorInfo::default(),
            pointer: Arc::new(pointer),
            source_width: width,
            source_height: height,
            origin_x: 0,
            origin_y: 0,
            rotation,
            source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
            region: CaptureRegion::full(width, height),
        };
        let mut outcomes = Vec::new();
        let info = plan.publish(Some(&source), metadata, 5, |outcome| outcomes.push(outcome))?;
        Ok(PublishedFixture {
            plan,
            info,
            outcomes,
        })
    }

    fn device_adapter_luid(device: &ID3D11Device) -> CaptureResult<GpuAdapterLuid> {
        use windows::Win32::Graphics::Dxgi::{IDXGIAdapter1, IDXGIDevice};

        let dxgi_device = device
            .cast::<IDXGIDevice>()
            .map_err(|error| CaptureError::windows("query fixture DXGI device", error))?;
        // SAFETY: the live DXGI device returns its reference-counted adapter.
        let adapter = unsafe { dxgi_device.GetAdapter() }
            .map_err(|error| CaptureError::windows("query fixture DXGI adapter", error))?
            .cast::<IDXGIAdapter1>()
            .map_err(|error| CaptureError::windows("query fixture DXGI adapter 1", error))?;
        super::super::adapter_luid(&adapter)
    }

    pub(crate) fn real_keyed_mutex_contention_times_out() -> CaptureResult<bool> {
        let (device, _context) =
            super::super::gpu_reduction::test_device().map_err(capture_gpu_error)?;
        let device5 = device
            .cast::<ID3D11Device5>()
            .map_err(|error| CaptureError::windows("query contention fixture device", error))?;
        let slot = create_surface_slot(
            &device,
            &device5,
            CaptureExtent::try_new(1, 1)?,
            GpuSurfaceSlotId::new(NonZeroU64::MIN),
        )?;
        let (holder_device, _holder_context) =
            super::super::gpu_reduction::test_device().map_err(capture_gpu_error)?;
        let holder_device1 = holder_device
            .cast::<ID3D11Device1>()
            .map_err(|error| CaptureError::windows("query contention fixture sharing", error))?;
        // SAFETY: the slot retains its NT handle for the entire fixture.
        let holder_texture: ID3D11Texture2D =
            unsafe { holder_device1.OpenSharedResource1(slot.publication.shared.texture_handle.0) }
                .map_err(|error| CaptureError::windows("open contention fixture texture", error))?;
        let holder_mutex = holder_texture
            .cast::<IDXGIKeyedMutex>()
            .map_err(|error| CaptureError::windows("query contention fixture mutex", error))?;
        let first = acquire_keyed_mutex(&holder_mutex, 0, 0, "acquire contention fixture owner")?;
        let contended = acquire_keyed_mutex(
            &slot.publication.shared.keyed_mutex,
            0,
            0,
            "probe contended fixture key",
        )?;
        // SAFETY: the first raw-vtable acquire above owns key 0.
        unsafe { holder_mutex.ReleaseSync(0) }
            .map_err(|error| CaptureError::windows("release contention fixture owner", error))?;
        Ok(first == KeyedMutexAcquire::Acquired && contended == KeyedMutexAcquire::Timeout)
    }

    pub(crate) fn keyed_mutex_status_classifier_rejects_errors_and_unknown_success() -> bool {
        matches!(
            classify_keyed_mutex_status(
                windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
                "classify fixture keyed mutex",
            ),
            Err(CaptureError::Windows { .. })
        ) && matches!(
            classify_keyed_mutex_status(windows::core::HRESULT(1), "classify fixture keyed mutex",),
            Err(CaptureError::Windows { .. })
        )
    }

    pub(crate) fn abandoned_keyed_mutex_owner_is_reported() -> CaptureResult<bool> {
        let (owner_device, owner_context) =
            super::super::gpu_reduction::test_device().map_err(capture_gpu_error)?;
        let owner_device5 = owner_device
            .cast::<ID3D11Device5>()
            .map_err(|error| CaptureError::windows("query abandon fixture device", error))?;
        let slot = create_surface_slot(
            &owner_device,
            &owner_device5,
            CaptureExtent::try_new(1, 1)?,
            GpuSurfaceSlotId::new(NonZeroU64::MIN),
        )?;
        let (holder_device, holder_context) =
            super::super::gpu_reduction::test_device().map_err(capture_gpu_error)?;
        let holder_device1 = holder_device
            .cast::<ID3D11Device1>()
            .map_err(|error| CaptureError::windows("query abandon fixture sharing", error))?;
        // SAFETY: the slot retains its NT handle for the entire fixture.
        let holder_texture: ID3D11Texture2D =
            unsafe { holder_device1.OpenSharedResource1(slot.publication.shared.texture_handle.0) }
                .map_err(|error| CaptureError::windows("open abandon fixture texture", error))?;
        let holder_mutex = holder_texture
            .cast::<IDXGIKeyedMutex>()
            .map_err(|error| CaptureError::windows("query abandon fixture mutex", error))?;
        let holder_status =
            acquire_keyed_mutex(&holder_mutex, 0, 0, "acquire abandon fixture owner")?;
        drop(holder_mutex);
        drop(holder_texture);
        drop(holder_device1);
        drop(holder_context);
        drop(holder_device);

        let abandoned = acquire_keyed_mutex(
            &slot.publication.shared.keyed_mutex,
            0,
            0,
            "probe abandoned fixture key",
        )?;
        drop(owner_context);
        Ok(holder_status == KeyedMutexAcquire::Acquired
            && abandoned == KeyedMutexAcquire::Abandoned)
    }

    pub(crate) fn release_on_producer_device(
        plan: &PreparedGpuSurfacePlan,
        publication: &Arc<GpuSurfacePublication>,
    ) -> CaptureResult<()> {
        let lease = publication.claim()?;
        release_lease_on_producer_device(plan, lease)
    }

    pub(crate) fn release_lease_on_producer_device(
        plan: &PreparedGpuSurfacePlan,
        mut lease: GpuSurfaceLease,
    ) -> CaptureResult<()> {
        let device5 = plan
            .device
            .cast::<ID3D11Device5>()
            .map_err(|error| CaptureError::windows("query fixture device fence support", error))?;
        let context4 = plan
            .context
            .cast::<ID3D11DeviceContext4>()
            .map_err(|error| CaptureError::windows("query fixture context fence support", error))?;
        let mut fence = None;
        // SAFETY: the borrowed handle remains live through the claimed lease.
        unsafe {
            device5.OpenSharedFence::<ID3D11Fence>(
                HANDLE(lease.fence_handle().as_raw() as *mut _),
                &mut fence,
            )
        }
        .map_err(|error| CaptureError::windows("open fixture shared fence", error))?;
        let fence = fence.ok_or_else(|| {
            CaptureError::windows("open fixture shared fence", "D3D11 returned no fence")
        })?;
        let synchronization = lease.synchronization();
        // SAFETY: waits and signals use the values published with this lease.
        // SAFETY: the shared values came from the publication bound to this fence.
        unsafe { context4.Wait(&fence, synchronization.producer_ready_value) }
            .map_err(|error| CaptureError::windows("wait for fixture GPU Surface", error))?;
        require_keyed_mutex(
            &lease.publication.shared.keyed_mutex,
            synchronization.consumer_acquire_key,
            u32::MAX,
            "acquire fixture GPU Surface key",
        )?;
        lease.mark_native_acquired()?;
        // SAFETY: the fixture performs no access after releasing producer key 0.
        unsafe {
            lease
                .publication
                .shared
                .keyed_mutex
                .ReleaseSync(synchronization.consumer_release_key)
        }
        .map_err(|error| CaptureError::windows("release fixture GPU Surface key", error))?;
        // SAFETY: the fence release is ordered after keyed-mutex release.
        unsafe { context4.Signal(&fence, synchronization.consumer_release_value) }
            .map_err(|error| CaptureError::windows("release fixture GPU Surface", error))?;
        // SAFETY: Flush only submits already validated immediate-context work.
        unsafe { plan.context.Flush() };
        lease.mark_release_queued()?;
        Ok(())
    }

    pub(crate) fn texture_handle(publication: &GpuSurfacePublication) -> isize {
        publication.shared.texture_handle.borrowed().as_raw()
    }

    pub(crate) fn slot_diagnostics(
        plan: &PreparedGpuSurfacePlan,
    ) -> Vec<(usize, u8, u64, Option<u64>)> {
        plan.routes
            .iter()
            .flat_map(|route| &route.slots)
            .map(|slot| {
                (
                    Arc::strong_count(&slot.publication),
                    slot.publication.state.load(Ordering::Acquire),
                    // SAFETY: the plan owns this slot fence for the diagnostic.
                    unsafe { slot.publication.shared.fence.GetCompletedValue() },
                    slot.required_release_value,
                )
            })
            .collect()
    }

    pub(crate) fn republish(
        plan: &mut PreparedGpuSurfacePlan,
        sequence: u64,
    ) -> CaptureResult<Vec<GpuSurfacePublishOutcome>> {
        let metadata = CaptureMetadata {
            source_id: Arc::clone(&plan.source_id),
            topology_generation: plan.topology_generation,
            sequence,
            captured_at: Instant::now(),
            cursor: crate::CursorInfo::default(),
            pointer: Arc::new(PointerState::default()),
            source_width: plan.native_source_extent.width(),
            source_height: plan.native_source_extent.height(),
            origin_x: 0,
            origin_y: 0,
            rotation: plan.source_rotation,
            source_color_space: plan.source_color_space,
            region: CaptureRegion::full(
                plan.native_source_extent.width(),
                plan.native_source_extent.height(),
            ),
        };
        let mut outcomes = Vec::new();
        plan.publish(None, metadata, plan.duplication_generation, |outcome| {
            outcomes.push(outcome);
        })?;
        Ok(outcomes)
    }

    pub(crate) fn retry_pending(
        plan: &mut PreparedGpuSurfacePlan,
    ) -> CaptureResult<Vec<GpuSurfacePublishOutcome>> {
        let mut outcomes = Vec::new();
        plan.retry_pending(|outcome| outcomes.push(outcome))?;
        Ok(outcomes)
    }

    pub(crate) fn retry_pending_for_duplication_epoch(
        plan: &mut PreparedGpuSurfacePlan,
        duplication_generation: u64,
    ) -> CaptureResult<Vec<GpuSurfacePublishOutcome>> {
        plan.validate_source(
            &plan.source_id,
            plan.topology_generation,
            duplication_generation,
            plan.adapter_luid,
            plan.native_source_extent,
            plan.logical_source_extent,
            plan.source_rotation,
            plan.source_color_space,
        )?;
        retry_pending(plan)
    }

    pub(crate) fn republish_with_fault(
        plan: &mut PreparedGpuSurfacePlan,
        sequence: u64,
        fault: InjectedSurfaceFault,
    ) -> CaptureResult<Vec<GpuSurfacePublishOutcome>> {
        plan.injected_fault = Some(fault);
        republish(plan, sequence)
    }

    pub(crate) fn callback_observes_only_submitted_publications(
        plan: &mut PreparedGpuSurfacePlan,
        sequence: u64,
    ) -> CaptureResult<bool> {
        let metadata = CaptureMetadata {
            source_id: Arc::clone(&plan.source_id),
            topology_generation: plan.topology_generation,
            sequence,
            captured_at: Instant::now(),
            cursor: crate::CursorInfo::default(),
            pointer: Arc::new(PointerState::default()),
            source_width: plan.native_source_extent.width(),
            source_height: plan.native_source_extent.height(),
            origin_x: 0,
            origin_y: 0,
            rotation: plan.source_rotation,
            source_color_space: plan.source_color_space,
            region: CaptureRegion::full(
                plan.native_source_extent.width(),
                plan.native_source_extent.height(),
            ),
        };
        let mut submitted = true;
        plan.publish(None, metadata, plan.duplication_generation, |outcome| {
            if let GpuSurfacePublishOutcome::Published(publication) = outcome {
                submitted &= publication.state.load(Ordering::Acquire) == USE_UNCLAIMED;
            }
        })?;
        Ok(submitted)
    }

    pub(crate) fn acquire_without_release(
        publication: &Arc<GpuSurfacePublication>,
    ) -> CaptureResult<()> {
        let mut lease = publication.claim()?;
        let synchronization = lease.synchronization();
        require_keyed_mutex(
            &lease.publication.shared.keyed_mutex,
            synchronization.consumer_acquire_key,
            u32::MAX,
            "acquire fixture GPU Surface key",
        )?;
        lease.mark_native_acquired()?;
        drop(lease);
        Ok(())
    }

    pub(crate) fn release_key_without_fence(
        publication: &Arc<GpuSurfacePublication>,
    ) -> CaptureResult<()> {
        let mut lease = publication.claim()?;
        let synchronization = lease.synchronization();
        require_keyed_mutex(
            &lease.publication.shared.keyed_mutex,
            synchronization.consumer_acquire_key,
            u32::MAX,
            "acquire fixture GPU Surface key",
        )?;
        lease.mark_native_acquired()?;
        // SAFETY: the fixture owns the consumer key and performs no later access.
        unsafe {
            lease
                .publication
                .shared
                .keyed_mutex
                .ReleaseSync(synchronization.consumer_release_key)
        }
        .map_err(|error| CaptureError::windows("release fixture GPU Surface key", error))?;
        lease.mark_release_queued()
    }

    pub(crate) fn readback_and_release(
        plan: &PreparedGpuSurfacePlan,
        publication: &Arc<GpuSurfacePublication>,
    ) -> CaptureResult<Vec<u8>> {
        let mut lease = publication.claim()?;
        let synchronization = lease.synchronization();
        // SAFETY: the shared fence belongs to this plan's immediate context.
        unsafe {
            plan.context4.Wait(
                &lease.publication.shared.fence,
                synchronization.producer_ready_value,
            )
        }
        .map_err(|error| CaptureError::windows("wait for fixture GPU Surface", error))?;
        require_keyed_mutex(
            &lease.publication.shared.keyed_mutex,
            synchronization.consumer_acquire_key,
            u32::MAX,
            "acquire fixture GPU Surface key",
        )?;
        lease.mark_native_acquired()?;

        let extent = lease.provenance().output_extent;
        let desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0.cast_unsigned(),
            MiscFlags: 0,
            ..texture_desc(extent, DXGI_FORMAT_R8G8B8A8_UNORM, 0, 0)
        };
        let staging = create_texture(&plan.device, &desc, None).map_err(capture_gpu_error)?;
        // SAFETY: both resources share exact extent and format on one device.
        unsafe {
            plan.context
                .CopyResource(&staging, &lease.publication.shared._texture)
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging is CPU-readable and remains live until Unmap.
        unsafe {
            plan.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| CaptureError::windows("map fixture GPU Surface", error))?;
        let row_bytes = checked_rgba_len(extent.width(), 1, "read fixture GPU Surface")
            .map_err(capture_gpu_error)?;
        let output_len =
            checked_rgba_len(extent.width(), extent.height(), "read fixture GPU Surface")
                .map_err(capture_gpu_error)?;
        let row_pitch = mapped.RowPitch as usize;
        if mapped.pData.is_null() || row_pitch < row_bytes {
            // SAFETY: pairs with the successful Map above.
            unsafe { plan.context.Unmap(&staging, 0) };
            return Err(CaptureError::InvalidBufferGeometry {
                operation: "map fixture GPU Surface",
                width: extent.width(),
                height: extent.height(),
                row_pitch,
            });
        }
        let mapped_len = row_pitch.checked_mul(extent.height() as usize).ok_or(
            CaptureError::GeometryOverflow {
                operation: "map fixture GPU Surface",
                width: extent.width(),
                height: extent.height(),
            },
        )?;
        // SAFETY: Map exposes row_pitch bytes for every output row.
        let source = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), mapped_len) };
        let mut output = vec![0; output_len];
        for row in 0..extent.height() as usize {
            let source_start = row * row_pitch;
            let output_start = row * row_bytes;
            output[output_start..output_start + row_bytes]
                .copy_from_slice(&source[source_start..source_start + row_bytes]);
        }
        // SAFETY: pairs with the successful Map above.
        unsafe { plan.context.Unmap(&staging, 0) };
        // SAFETY: the fixture queued its final copy before releasing producer key 0.
        unsafe {
            lease
                .publication
                .shared
                .keyed_mutex
                .ReleaseSync(synchronization.consumer_release_key)
        }
        .map_err(|error| CaptureError::windows("release fixture GPU Surface key", error))?;
        // SAFETY: the release signal is queued after the keyed-mutex hand-off.
        unsafe {
            plan.context4.Signal(
                &lease.publication.shared.fence,
                synchronization.consumer_release_value,
            )
        }
        .map_err(|error| CaptureError::windows("release fixture GPU Surface", error))?;
        // SAFETY: Flush only submits already validated immediate-context work.
        unsafe { plan.context.Flush() };
        lease.mark_release_queued()?;
        Ok(output)
    }

    pub(crate) fn handles_survive_plan_drop(fixture: PublishedFixture) -> CaptureResult<bool> {
        let device = fixture.plan.device.clone();
        let publication = fixture
            .outcomes
            .iter()
            .find_map(|outcome| match outcome {
                GpuSurfacePublishOutcome::Published(publication) => Some(publication.clone()),
                GpuSurfacePublishOutcome::Busy(_) => None,
            })
            .ok_or_else(|| {
                CaptureError::windows("inspect fixture GPU Surface", "no publication exists")
            })?;
        let lease = publication.claim()?;
        drop(fixture);

        let device1 = device
            .cast::<ID3D11Device1>()
            .map_err(|error| CaptureError::windows("query fixture shared-resource API", error))?;
        let device5 = device
            .cast::<ID3D11Device5>()
            .map_err(|error| CaptureError::windows("query fixture shared-fence API", error))?;
        // SAFETY: publication retains ownership of both borrowed handles.
        let _texture: ID3D11Texture2D = unsafe {
            device1.OpenSharedResource1(HANDLE(lease.texture_handle().as_raw() as *mut _))
        }
        .map_err(|error| CaptureError::windows("open retained GPU Surface texture", error))?;
        let mut fence = None;
        // SAFETY: publication retains ownership of the borrowed fence handle.
        unsafe {
            device5.OpenSharedFence::<ID3D11Fence>(
                HANDLE(lease.fence_handle().as_raw() as *mut _),
                &mut fence,
            )
        }
        .map_err(|error| CaptureError::windows("open retained GPU Surface fence", error))?;
        Ok(fence.is_some())
    }
}
