use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use hypercolor_windows_capture::{
    CaptureError, DisplayRotation, GpuAdapterLuid, GpuSurfaceColorPipeline,
    GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy, GpuSurfaceDescriptor,
    GpuSurfaceDescriptorId, GpuSurfaceFormat, GpuSurfaceLease, GpuSurfacePlanGeneration,
    GpuSurfaceProvenance, GpuSurfacePublication, GpuSurfaceSlotId, GpuSurfaceTargetPreparation,
    GpuSurfaceTargetPreparationSlot,
};
use thiserror::Error;
use windows::Win32::Foundation::{HANDLE, S_OK, WAIT_ABANDONED, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D::{
    D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_TEXTURE2D_DESC,
    ID3D11Device1, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Direct3D11on12::{
    D3D11_RESOURCE_FLAGS, D3D11On12CreateDevice, ID3D11On12Device,
};
use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_COPY_DEST;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;
use windows::core::{IUnknown, Interface};

/// Result type for D3D11On12 screen-copy interop.
pub type ScreenInteropResult<T> = std::result::Result<T, D3d11On12ScreenInteropError>;

/// Failures while binding screen interop to a renderer-owned DX12 queue.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum D3d11On12ScreenInteropError {
    /// The renderer device does not expose the DX12 HAL.
    #[error("wgpu device is not backed by the DX12 HAL")]
    MissingWgpuDx12Device,
    /// The renderer queue does not expose the DX12 HAL.
    #[error("wgpu queue is not backed by the DX12 HAL")]
    MissingWgpuDx12Queue,
    /// D3D11On12 could not bind to the renderer device and queue.
    #[error("D3D11On12 device creation failed with HRESULT {hresult:#010x}")]
    DeviceCreateFailed {
        /// Failing HRESULT.
        hresult: i32,
    },
    /// D3D11On12 returned no device or immediate context.
    #[error("D3D11On12 device creation returned no {resource}")]
    MissingCreatedResource {
        /// Missing output resource.
        resource: &'static str,
    },
    /// A required D3D11 interop interface was unavailable.
    #[error("D3D11On12 interface {interface} is unavailable with HRESULT {hresult:#010x}")]
    MissingInterface {
        /// Required COM interface.
        interface: &'static str,
        /// Failing HRESULT.
        hresult: i32,
    },
    /// Capture and renderer devices belong to different DXGI adapters.
    #[error("screen publication adapter {publication:?} does not match renderer {renderer:?}")]
    AdapterMismatch {
        /// Capture publication adapter.
        publication: GpuAdapterLuid,
        /// Renderer adapter.
        renderer: GpuAdapterLuid,
    },
    /// Native provenance does not match the bridge's exact copy contract.
    #[error("unsupported native screen publication contract: {field}")]
    UnsupportedPublicationContract {
        /// Contradictory provenance field.
        field: &'static str,
    },
    /// The exact output exceeds the active renderer device limit.
    #[error("screen output {width}x{height} exceeds renderer 2D texture limit {limit}")]
    RendererDimensionLimit {
        /// Requested output width.
        width: u32,
        /// Requested output height.
        height: u32,
        /// Active device limit.
        limit: u32,
    },
    /// A bridge cache could not reserve metadata for another exact resource.
    #[error("screen interop cache allocation failed")]
    CacheAllocationFailed,
    /// A bridge-local texture or content identity exhausted its integer domain.
    #[error("screen interop identity exhausted")]
    IdentityExhausted,
    /// The requested renderer target byte geometry cannot be represented.
    #[error("screen target {width}x{height} byte geometry overflowed")]
    TargetByteLengthOverflow {
        /// Exact output width.
        width: u32,
        /// Exact output height.
        height: u32,
    },
    /// Wgpu rejected the renderer-owned target allocation.
    #[error("screen target {width}x{height} allocation failed: {source}")]
    TargetAllocationFailed {
        /// Exact output width.
        width: u32,
        /// Exact output height.
        height: u32,
        /// Wgpu allocation error captured before its fatal default handler.
        #[source]
        source: wgpu::Error,
    },
    /// A copy arrived before its renderer target was transactionally prepared.
    #[error(
        "screen target for plan {plan_generation:?}, descriptor {descriptor_id:?} was not prepared at {width}x{height}"
    )]
    TargetNotPrepared {
        /// Exact committed plan whose target is absent.
        plan_generation: GpuSurfacePlanGeneration,
        /// Exact descriptor whose target is absent or stale.
        descriptor_id: GpuSurfaceDescriptorId,
        /// Required output width.
        width: u32,
        /// Required output height.
        height: u32,
    },
    /// A prepared target belongs to another renderer bridge.
    #[error("prepared screen target belongs to another renderer bridge")]
    ForeignPreparedTarget,
    /// A prepared target does not match the publication's exact plan identity.
    #[error("prepared screen target does not match publication field {field}")]
    PreparedTargetMismatch {
        /// Contradictory identity field.
        field: &'static str,
    },
    /// A publication references a slot absent from its prepared target.
    #[error("native screen slot {slot_id:?} was not opened during target preparation")]
    NativeSurfaceNotPrepared {
        /// Exact capture slot missing from the prepared target.
        slot_id: GpuSurfaceSlotId,
    },
    /// Internal bridge state was poisoned by a panicking caller.
    #[error("screen interop {state} state is poisoned")]
    PoisonedState {
        /// Poisoned synchronization domain.
        state: &'static str,
    },
    /// D3D11 could not open a resource shared by the capture device.
    #[error("{operation} failed with HRESULT {hresult:#010x}")]
    WindowsOperation {
        /// Failing interop operation.
        operation: &'static str,
        /// Failing HRESULT.
        hresult: i32,
    },
    /// The producer had not released the keyed-mutex handoff without blocking.
    #[error("capture keyed mutex is not ready")]
    KeyedMutexTimeout,
    /// DXGI reported abandoned keyed ownership and the native slot was poisoned.
    #[error("capture keyed mutex ownership was abandoned")]
    KeyedMutexAbandoned,
    /// DXGI returned a non-error status with unknown ownership semantics.
    #[error("capture keyed mutex returned unexpected status {status:#010x}")]
    UnexpectedKeyedMutexStatus {
        /// Raw HRESULT value returned by DXGI.
        status: i32,
    },
    /// A successful open call returned no interface.
    #[error("{operation} returned no {resource}")]
    MissingOpenedResource {
        /// Successful operation with a missing output.
        operation: &'static str,
        /// Missing interface or resource.
        resource: &'static str,
    },
    /// Native preparation failed and returning the unacquired claim also failed.
    #[error(
        "{operation} failed and the native publication claim could not be abandoned: {cleanup}"
    )]
    PreAcquireCleanupFailed {
        /// Original failing operation.
        operation: &'static str,
        /// Claim cleanup failure.
        cleanup: CaptureError,
    },
    /// The capture publication rejected a claim or lifecycle transition.
    #[error(transparent)]
    Capture(#[from] CaptureError),
}

/// D3D11On12 interfaces permanently bound to one renderer device and queue.
pub struct D3d11On12ScreenDevice {
    adapter_luid: GpuAdapterLuid,
    pub(crate) on12: ID3D11On12Device,
    pub(crate) device1: ID3D11Device1,
    pub(crate) device5: ID3D11Device5,
    pub(crate) context: ID3D11DeviceContext,
    pub(crate) context4: ID3D11DeviceContext4,
}

/// Renderer-local GPU texture carrying one exact native screen result.
#[derive(Clone, Debug)]
pub struct ScreenTextureCopy {
    /// Stable identity of the renderer-owned texture allocation.
    pub storage_id: u64,
    /// Monotonic content identity. New pixels always advance this value.
    pub content_generation: u64,
    /// Exact output width.
    pub width: u32,
    /// Exact output height.
    pub height: u32,
    /// Renderer-owned ordinary wgpu texture.
    pub texture: Arc<wgpu::Texture>,
    /// Default shader-resource view over [`Self::texture`].
    pub view: Arc<wgpu::TextureView>,
    _target_lease: PreparedScreenCopyTarget,
}

/// Renderer allocation retained for one exact screen descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenCopyTargetAllocation {
    /// Stable identity of the renderer-owned texture allocation.
    pub storage_id: u64,
    /// Exact output width.
    pub width: u32,
    /// Exact output height.
    pub height: u32,
    /// Physical RGBA bytes retained by the renderer target.
    pub retained_bytes: u64,
}

/// Live renderer-target and native-surface cache occupancy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenInteropCacheStats {
    /// Prepared renderer targets with at least one live lease.
    pub prepared_targets: usize,
    /// Native capture surfaces opened for live prepared targets.
    pub opened_surfaces: usize,
    /// Cumulative native surface opens performed during target preparation.
    pub native_surface_opens: u64,
    /// Exact physical bytes retained by live renderer targets.
    pub retained_target_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ScreenCopyTargetKey {
    plan_generation: GpuSurfacePlanGeneration,
    descriptor_id: GpuSurfaceDescriptorId,
    width: u32,
    height: u32,
    adapter_luid: GpuAdapterLuid,
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
}

impl ScreenCopyTargetKey {
    fn new(preparation: &GpuSurfaceTargetPreparation) -> Self {
        let descriptor = preparation.descriptor();
        Self {
            plan_generation: preparation.plan_generation(),
            descriptor_id: descriptor.id(),
            width: descriptor.output_extent().width(),
            height: descriptor.output_extent().height(),
            adapter_luid: preparation.adapter_luid(),
            source_id: Arc::from(preparation.source_id()),
            topology_generation: preparation.topology_generation(),
            duplication_generation: preparation.duplication_generation(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct NativeSurfaceKey {
    target: ScreenCopyTargetKey,
    adapter_luid: GpuAdapterLuid,
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
    slot_id: GpuSurfaceSlotId,
    opaque_handle_id: NonZeroU64,
    use_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedNativeIdentity {
    adapter_luid: GpuAdapterLuid,
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
}

struct OpenedSurface {
    slot_id: GpuSurfaceSlotId,
    opaque_handle_id: NonZeroU64,
    resource: ID3D11Resource,
    keyed_mutex: IDXGIKeyedMutex,
    fence: ID3D11Fence,
}

struct ScreenCopyTarget {
    content_generation: u64,
    last_native_surface: Option<NativeSurfaceKey>,
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
    wrapped_resource: ID3D11Resource,
}

struct PreparedScreenCopyTargetInner {
    bridge: Weak<D3d11On12ScreenBridgeInner>,
    key: ScreenCopyTargetKey,
    native_identity: PreparedNativeIdentity,
    opened_surfaces: Box<[OpenedSurface]>,
    allocation: ScreenCopyTargetAllocation,
    target: Mutex<ScreenCopyTarget>,
}

/// Cloneable lease retaining one exact renderer-owned screen-copy target.
///
/// The final lease or texture-reader drop releases the renderer target and
/// every native surface opened during preparation.
#[derive(Clone)]
pub struct PreparedScreenCopyTarget {
    inner: Arc<PreparedScreenCopyTargetInner>,
}

impl PreparedScreenCopyTarget {
    /// Exact renderer allocation retained by this lease.
    #[must_use]
    pub fn allocation(&self) -> ScreenCopyTargetAllocation {
        self.inner.allocation
    }

    /// Stable identity of the renderer-owned texture allocation.
    #[must_use]
    pub fn storage_id(&self) -> u64 {
        self.inner.allocation.storage_id
    }

    /// Physical RGBA bytes retained by this renderer target.
    #[must_use]
    pub fn retained_bytes(&self) -> u64 {
        self.inner.allocation.retained_bytes
    }

    /// Exact target width.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.inner.allocation.width
    }

    /// Exact target height.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.inner.allocation.height
    }
}

impl std::fmt::Debug for PreparedScreenCopyTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedScreenCopyTarget")
            .field("allocation", &self.inner.allocation)
            .finish_non_exhaustive()
    }
}

impl PartialEq for PreparedScreenCopyTarget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for PreparedScreenCopyTarget {}

impl Drop for PreparedScreenCopyTargetInner {
    fn drop(&mut self) {
        let Some(bridge) = self.bridge.upgrade() else {
            return;
        };
        let mut state = bridge
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let is_current = state
            .targets
            .get(&self.key)
            .is_some_and(|target| std::ptr::eq(target.as_ptr(), std::ptr::from_ref(self)));
        if is_current {
            state.targets.remove(&self.key);
        }
    }
}

struct D3d11On12ScreenBridgeState {
    targets: HashMap<ScreenCopyTargetKey, Weak<PreparedScreenCopyTargetInner>>,
    next_storage_id: u64,
    native_surface_opens: u64,
}

struct D3d11On12ScreenBridgeInner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    interop: D3d11On12ScreenDevice,
    state: Mutex<D3d11On12ScreenBridgeState>,
    context_gate: Mutex<()>,
    next_content_generation: AtomicU64,
}

/// Copy bridge from exact D3D11 capture publications into ordinary wgpu textures.
///
/// Clones share one bridge permanently bound to a wgpu DX12 device and queue.
/// Prepared leases own renderer targets, while the bridge retains weak dedupe
/// entries and serializes immediate-context work for exact native copies.
#[derive(Clone)]
pub struct D3d11On12ScreenBridge {
    inner: Arc<D3d11On12ScreenBridgeInner>,
}

impl D3d11On12ScreenBridge {
    /// Bind a screen-copy bridge to the renderer's exact DX12 queue.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the renderer is not using DX12 or D3D11On12
    /// cannot expose the required shared-resource and fence interfaces.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> ScreenInteropResult<Self> {
        let interop = D3d11On12ScreenDevice::new(&device, &queue)?;
        Ok(Self {
            inner: Arc::new(D3d11On12ScreenBridgeInner {
                device,
                queue,
                interop,
                state: Mutex::new(D3d11On12ScreenBridgeState {
                    targets: HashMap::new(),
                    next_storage_id: 0,
                    native_surface_opens: 0,
                }),
                context_gate: Mutex::new(()),
                next_content_generation: AtomicU64::new(1),
            }),
        })
    }

    /// Renderer adapter accepted by this bridge.
    #[must_use]
    pub fn adapter_luid(&self) -> GpuAdapterLuid {
        self.inner.interop.adapter_luid()
    }

    /// Prepare or reuse a renderer target for one exact plan descriptor.
    ///
    /// This performs every allocation and initialization submission outside
    /// the frame-copy hot path and reports the physical bytes retained.
    ///
    /// # Errors
    ///
    /// Rejects unsupported descriptor semantics, renderer limits, byte
    /// overflow, allocation metadata exhaustion, and native wrapping errors.
    pub fn prepare_target(
        &self,
        preparation: &GpuSurfaceTargetPreparation,
    ) -> ScreenInteropResult<PreparedScreenCopyTarget> {
        let descriptor = preparation.descriptor();
        validate_descriptor_contract(descriptor, self.inner.device.limits())?;
        if preparation.adapter_luid() != self.adapter_luid() {
            return Err(D3d11On12ScreenInteropError::AdapterMismatch {
                publication: preparation.adapter_luid(),
                renderer: self.adapter_luid(),
            });
        }
        if preparation.slots().is_empty() {
            return Err(
                D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                    field: "preparation.slots",
                },
            );
        }
        let key = ScreenCopyTargetKey::new(preparation);
        let native_identity = PreparedNativeIdentity {
            adapter_luid: preparation.adapter_luid(),
            source_id: Arc::clone(&key.source_id),
            topology_generation: preparation.topology_generation(),
            duplication_generation: preparation.duplication_generation(),
        };
        let retained_bytes = checked_target_bytes(key.width, key.height)?;
        let storage_id = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| D3d11On12ScreenInteropError::PoisonedState { state: "bridge" })?;
            if let Some(target) = state.targets.get(&key).and_then(Weak::upgrade) {
                validate_matching_preparation(&target, &native_identity, preparation.slots())?;
                return Ok(PreparedScreenCopyTarget { inner: target });
            }
            state.targets.remove(&key);
            state
                .targets
                .try_reserve(1)
                .map_err(|_| D3d11On12ScreenInteropError::CacheAllocationFailed)?;
            let storage_id = state
                .next_storage_id
                .checked_add(1)
                .ok_or(D3d11On12ScreenInteropError::IdentityExhausted)?;
            state.next_storage_id = storage_id;
            storage_id
        };
        let mut opened_surfaces = Vec::new();
        opened_surfaces
            .try_reserve_exact(preparation.slots().len())
            .map_err(|_| D3d11On12ScreenInteropError::CacheAllocationFailed)?;
        for slot in preparation.slots() {
            opened_surfaces.push(open_surface(&self.inner.interop, slot, descriptor)?);
        }
        opened_surfaces.sort_unstable_by_key(|surface| surface.slot_id);
        if opened_surfaces
            .windows(2)
            .any(|slots| slots[0].slot_id == slots[1].slot_id)
        {
            return Err(
                D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                    field: "preparation.slot_id",
                },
            );
        }
        let target = create_target(
            &self.inner.device,
            &self.inner.queue,
            &self.inner.interop,
            key.width,
            key.height,
        )?;
        let target = Arc::new(PreparedScreenCopyTargetInner {
            bridge: Arc::downgrade(&self.inner),
            key: key.clone(),
            native_identity,
            opened_surfaces: opened_surfaces.into_boxed_slice(),
            allocation: ScreenCopyTargetAllocation {
                storage_id,
                width: key.width,
                height: key.height,
                retained_bytes,
            },
            target: Mutex::new(target),
        });
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| D3d11On12ScreenInteropError::PoisonedState { state: "bridge" })?;
        state.native_surface_opens = state
            .native_surface_opens
            .checked_add(
                u64::try_from(target.opened_surfaces.len())
                    .map_err(|_| D3d11On12ScreenInteropError::IdentityExhausted)?,
            )
            .ok_or(D3d11On12ScreenInteropError::IdentityExhausted)?;
        // Independent extents allocate in parallel. If the same key raced us,
        // discard this candidate and share the winner's exact lease.
        if let Some(existing) = state.targets.get(&key).and_then(Weak::upgrade) {
            validate_matching_preparation(&existing, &target.native_identity, preparation.slots())?;
            drop(state);
            return Ok(PreparedScreenCopyTarget { inner: existing });
        }
        state
            .targets
            .try_reserve(1)
            .map_err(|_| D3d11On12ScreenInteropError::CacheAllocationFailed)?;
        state.targets.insert(key, Arc::downgrade(&target));
        Ok(PreparedScreenCopyTarget { inner: target })
    }

    /// Snapshot live target and opened-surface cache occupancy.
    ///
    /// # Errors
    ///
    /// Returns an error when a prior panic poisoned bridge state.
    pub fn cache_stats(&self) -> ScreenInteropResult<ScreenInteropCacheStats> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| D3d11On12ScreenInteropError::PoisonedState { state: "bridge" })?;
        let native_surface_opens = state.native_surface_opens;
        let targets = state
            .targets
            .values()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        drop(state);
        let mut stats = ScreenInteropCacheStats {
            native_surface_opens,
            ..ScreenInteropCacheStats::default()
        };
        for target in targets {
            stats.prepared_targets += 1;
            stats.opened_surfaces = stats
                .opened_surfaces
                .saturating_add(target.opened_surfaces.len());
            stats.retained_target_bytes = stats
                .retained_target_bytes
                .saturating_add(target.allocation.retained_bytes);
        }
        Ok(stats)
    }

    /// Copy one native publication into its exact prepared renderer target.
    ///
    /// Duplicate reads of the same plan, slot, and use identity return the
    /// existing renderer-local texture generation without claiming twice.
    ///
    /// # Errors
    ///
    /// Rejects stale claims, adapter or provenance mismatches, outputs beyond
    /// renderer limits, shared-resource failures, and unsafe handoff failures.
    pub fn copy_publication(
        &self,
        prepared: &PreparedScreenCopyTarget,
        publication: &Arc<GpuSurfacePublication>,
    ) -> ScreenInteropResult<ScreenTextureCopy> {
        let provenance = publication.provenance();
        validate_publication_contract(provenance, self.adapter_luid(), self.inner.device.limits())?;
        self.validate_prepared_target(prepared, provenance)?;
        let target_key = prepared.inner.key.clone();
        let native_key = NativeSurfaceKey {
            target: target_key.clone(),
            adapter_luid: provenance.adapter_luid,
            source_id: Arc::clone(&provenance.source_id),
            topology_generation: provenance.topology_generation,
            duplication_generation: provenance.duplication_generation,
            slot_id: provenance.slot_id,
            opaque_handle_id: publication.opaque_handle_id(),
            use_id: provenance.use_id,
        };
        let mut target = prepared.inner.target.lock().map_err(|_| {
            D3d11On12ScreenInteropError::PoisonedState {
                state: "prepared target",
            }
        })?;
        if target.last_native_surface.as_ref() == Some(&native_key) {
            return Ok(Self::texture_copy(&target, prepared));
        }
        let mut lease = publication.claim()?;
        let opened = prepared
            .inner
            .opened_surfaces
            .binary_search_by_key(&provenance.slot_id, |surface| surface.slot_id)
            .ok()
            .and_then(|index| prepared.inner.opened_surfaces.get(index))
            .filter(|surface| surface.opaque_handle_id == publication.opaque_handle_id());
        let Some(opened) = opened else {
            return Err(abandon_before_acquire(
                lease,
                D3d11On12ScreenInteropError::NativeSurfaceNotPrepared {
                    slot_id: provenance.slot_id,
                },
            ));
        };
        let content_generation = self
            .inner
            .next_content_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| D3d11On12ScreenInteropError::IdentityExhausted)?;
        self.transition_target_to_copy_destination(&target);
        let synchronization = lease.synchronization();
        let _context_guard = self.inner.context_gate.lock().map_err(|_| {
            D3d11On12ScreenInteropError::PoisonedState {
                state: "D3D11 immediate context",
            }
        })?;
        // SAFETY: the shared fence was opened from the claimed publication,
        // and its ready value was emitted with this exact slot use.
        if let Err(error) = unsafe {
            self.inner
                .interop
                .context4
                .Wait(&opened.fence, synchronization.producer_ready_value)
        } {
            return Err(abandon_windows_error(
                lease,
                "queue capture ready-fence wait",
                error,
            ));
        }
        // SAFETY: claim() reserved the sole consumer handoff. The helper reads
        // the raw HRESULT because windows-rs collapses positive WAIT statuses
        // into Ok. A zero timeout keeps the render thread non-blocking.
        match unsafe {
            acquire_keyed_mutex(&opened.keyed_mutex, synchronization.consumer_acquire_key, 0)
        } {
            KeyedAcquireStatus::Acquired => {}
            KeyedAcquireStatus::Timeout => {
                return Err(abandon_before_acquire(
                    lease,
                    D3d11On12ScreenInteropError::KeyedMutexTimeout,
                ));
            }
            KeyedAcquireStatus::Failed(hresult) => {
                return Err(abandon_before_acquire(
                    lease,
                    D3d11On12ScreenInteropError::WindowsOperation {
                        operation: "acquire capture keyed mutex",
                        hresult,
                    },
                ));
            }
            KeyedAcquireStatus::Abandoned => {
                lease.mark_native_acquired()?;
                return Err(D3d11On12ScreenInteropError::KeyedMutexAbandoned);
            }
            KeyedAcquireStatus::Unexpected(status) => {
                lease.mark_native_acquired()?;
                return Err(D3d11On12ScreenInteropError::UnexpectedKeyedMutexStatus { status });
            }
        }
        lease.mark_native_acquired()?;

        let wrapped = [Some(target.wrapped_resource.clone())];
        // SAFETY: the wrapped resource was created from this bridge's exact
        // D3D12 device in COPY_DEST state and remains live in the target cache.
        unsafe { self.inner.interop.on12.AcquireWrappedResources(&wrapped) };
        // SAFETY: both resources are single-sample R8G8B8A8 textures with the
        // exact descriptor extent. Keyed ownership and ready-fence ordering
        // protect the source; wgpu transitioned the destination beforehand.
        unsafe {
            self.inner
                .interop
                .context
                .CopyResource(&target.wrapped_resource, &opened.resource)
        };
        // SAFETY: pairs with AcquireWrappedResources above and returns the
        // destination to COPY_DEST for wgpu's tracked next use.
        unsafe { self.inner.interop.on12.ReleaseWrappedResources(&wrapped) };
        // SAFETY: the final source read was queued before handing key 0 back.
        unsafe {
            opened
                .keyed_mutex
                .ReleaseSync(synchronization.consumer_release_key)
        }
        .map_err(|error| windows_error("release capture keyed mutex", error))?;
        // SAFETY: the release fence is queued after the final copy and keyed
        // mutex release on the same D3D11On12 immediate context.
        unsafe {
            self.inner
                .interop
                .context4
                .Signal(&opened.fence, synchronization.consumer_release_value)
        }
        .map_err(|error| windows_error("signal capture release fence", error))?;
        // SAFETY: Flush submits the already ordered D3D11On12 wait/copy/release
        // work to the renderer's exact D3D12 command queue.
        unsafe { self.inner.interop.context.Flush() };
        lease.mark_release_queued()?;

        target.last_native_surface = Some(native_key);
        target.content_generation = content_generation;
        Ok(Self::texture_copy(&target, prepared))
    }

    fn validate_prepared_target(
        &self,
        prepared: &PreparedScreenCopyTarget,
        provenance: &GpuSurfaceProvenance,
    ) -> ScreenInteropResult<()> {
        if !Weak::ptr_eq(&prepared.inner.bridge, &Arc::downgrade(&self.inner)) {
            return Err(D3d11On12ScreenInteropError::ForeignPreparedTarget);
        }
        let key = &prepared.inner.key;
        if key.plan_generation != provenance.plan_generation {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
                field: "plan_generation",
            });
        }
        if key.descriptor_id != provenance.descriptor.id() {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
                field: "descriptor_id",
            });
        }
        if key.width != provenance.output_extent.width()
            || key.height != provenance.output_extent.height()
        {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
                field: "output_extent",
            });
        }
        let identity = &prepared.inner.native_identity;
        if identity.adapter_luid != provenance.adapter_luid {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
                field: "adapter_luid",
            });
        }
        if identity.source_id != provenance.source_id {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch { field: "source_id" });
        }
        if identity.topology_generation != provenance.topology_generation {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
                field: "topology_generation",
            });
        }
        if identity.duplication_generation != provenance.duplication_generation {
            return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
                field: "duplication_generation",
            });
        }
        Ok(())
    }

    fn transition_target_to_copy_destination(&self, target: &ScreenCopyTarget) {
        // Readers may leave this public texture in any declared wgpu usage.
        // Recording the transition through wgpu keeps its tracker synchronized
        // before D3D11On12 accesses the underlying resource externally.
        let mut encoder =
            self.inner
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("hypercolor screen native-copy transition"),
                });
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: target.texture.as_ref(),
                selector: None,
                state: wgpu::TextureUses::COPY_DST,
            }),
        );
        self.inner.queue.submit([encoder.finish()]);
    }

    fn texture_copy(
        target: &ScreenCopyTarget,
        prepared: &PreparedScreenCopyTarget,
    ) -> ScreenTextureCopy {
        ScreenTextureCopy {
            storage_id: prepared.storage_id(),
            content_generation: target.content_generation,
            width: prepared.width(),
            height: prepared.height(),
            texture: Arc::clone(&target.texture),
            view: Arc::clone(&target.view),
            _target_lease: prepared.clone(),
        }
    }
}

impl D3d11On12ScreenDevice {
    /// Bind D3D11On12 to the exact DX12 device and command queue owned by wgpu.
    ///
    /// # Errors
    ///
    /// Returns a typed error for non-DX12 renderers, D3D11On12 creation
    /// failure, or missing fence/shared-resource interfaces.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> ScreenInteropResult<Self> {
        let (raw_device, adapter_luid) = {
            // SAFETY: the COM clone is taken while the HAL guard is live; the
            // guard itself is dropped before any ordinary wgpu operation.
            let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Dx12>() }
                .ok_or(D3d11On12ScreenInteropError::MissingWgpuDx12Device)?;
            let raw_device = hal_device.raw_device().clone();
            // SAFETY: GetAdapterLuid only reads immutable device identity.
            let luid = unsafe { raw_device.GetAdapterLuid() };
            (raw_device, GpuAdapterLuid::new(luid.LowPart, luid.HighPart))
        };
        let raw_queue = {
            // SAFETY: the COM clone is taken while the HAL guard is live; the
            // cloned queue keeps the renderer command queue alive afterward.
            let hal_queue = unsafe { queue.as_hal::<wgpu_hal::api::Dx12>() }
                .ok_or(D3d11On12ScreenInteropError::MissingWgpuDx12Queue)?;
            hal_queue.as_raw().clone()
        };
        let command_queue: IUnknown =
            raw_queue
                .cast()
                .map_err(|error| D3d11On12ScreenInteropError::MissingInterface {
                    interface: "IUnknown command queue",
                    hresult: error.code().0,
                })?;
        let command_queues = [Some(command_queue)];
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut chosen_feature_level = D3D_FEATURE_LEVEL::default();
        let mut d3d11_device = None;
        let mut context = None;
        // SAFETY: both COM inputs are owned clones of the exact wgpu DX12
        // device and queue. Output pointers remain valid for the call.
        unsafe {
            D3D11On12CreateDevice(
                &raw_device,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT.0,
                Some(&feature_levels),
                Some(&command_queues),
                0,
                Some(&mut d3d11_device),
                Some(&mut context),
                Some(&mut chosen_feature_level),
            )
        }
        .map_err(|error| D3d11On12ScreenInteropError::DeviceCreateFailed {
            hresult: error.code().0,
        })?;
        let d3d11_device =
            d3d11_device.ok_or(D3d11On12ScreenInteropError::MissingCreatedResource {
                resource: "D3D11 device",
            })?;
        let context = context.ok_or(D3d11On12ScreenInteropError::MissingCreatedResource {
            resource: "D3D11 immediate context",
        })?;
        let on12 = cast_interface(&d3d11_device, "ID3D11On12Device")?;
        let device1 = cast_interface(&d3d11_device, "ID3D11Device1")?;
        let device5 = cast_interface(&d3d11_device, "ID3D11Device5")?;
        let context4 = cast_interface(&context, "ID3D11DeviceContext4")?;
        Ok(Self {
            adapter_luid,
            on12,
            device1,
            device5,
            context,
            context4,
        })
    }

    /// DXGI adapter identity shared by the renderer and capture publications.
    #[must_use]
    pub const fn adapter_luid(&self) -> GpuAdapterLuid {
        self.adapter_luid
    }
}

fn cast_interface<T, U>(source: &T, interface: &'static str) -> ScreenInteropResult<U>
where
    T: Interface,
    U: Interface,
{
    source
        .cast()
        .map_err(|error| D3d11On12ScreenInteropError::MissingInterface {
            interface,
            hresult: error.code().0,
        })
}

fn validate_publication_contract(
    provenance: &GpuSurfaceProvenance,
    renderer_adapter: GpuAdapterLuid,
    limits: wgpu::Limits,
) -> ScreenInteropResult<()> {
    if provenance.adapter_luid != renderer_adapter {
        return Err(D3d11On12ScreenInteropError::AdapterMismatch {
            publication: provenance.adapter_luid,
            renderer: renderer_adapter,
        });
    }
    validate_descriptor_contract(&provenance.descriptor, limits)?;
    let descriptor = &provenance.descriptor;
    if descriptor.coordinate_space() != provenance.coordinate_space {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "descriptor.coordinate_space",
            },
        );
    }
    if descriptor.source_color_space() != provenance.source_color_space {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "descriptor.source_color_space",
            },
        );
    }
    if descriptor.output_extent() != provenance.output_extent {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "descriptor.output_extent",
            },
        );
    }
    if descriptor.format() != provenance.output_format {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "descriptor.output_format",
            },
        );
    }
    if descriptor.color_pipeline() != provenance.color_pipeline {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "descriptor.color_pipeline",
            },
        );
    }
    if descriptor.cursor() == GpuSurfaceCursorPolicy::Exclude && provenance.cursor_composed {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "descriptor.cursor",
            },
        );
    }
    if provenance.coordinate_space != GpuSurfaceCoordinateSpace::LogicalDisplay {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "coordinate_space",
            },
        );
    }
    if provenance.pending_rotation != DisplayRotation::Identity {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "pending_rotation",
            },
        );
    }
    if provenance.output_format != GpuSurfaceFormat::Rgba8Unorm {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "output_format",
            },
        );
    }
    if provenance.color_pipeline != GpuSurfaceColorPipeline::PreserveEncoded {
        return Err(
            D3d11On12ScreenInteropError::UnsupportedPublicationContract {
                field: "color_pipeline",
            },
        );
    }
    Ok(())
}

fn validate_descriptor_contract(
    descriptor: &GpuSurfaceDescriptor,
    limits: wgpu::Limits,
) -> ScreenInteropResult<()> {
    descriptor.validate_exact_gpu()?;
    let width = descriptor.output_extent().width();
    let height = descriptor.output_extent().height();
    let limit = limits.max_texture_dimension_2d;
    if width > limit || height > limit {
        return Err(D3d11On12ScreenInteropError::RendererDimensionLimit {
            width,
            height,
            limit,
        });
    }
    checked_target_bytes(width, height)?;
    Ok(())
}

fn checked_target_bytes(width: u32, height: u32) -> ScreenInteropResult<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(D3d11On12ScreenInteropError::TargetByteLengthOverflow { width, height })
}

fn validate_matching_preparation(
    target: &PreparedScreenCopyTargetInner,
    identity: &PreparedNativeIdentity,
    slots: &[GpuSurfaceTargetPreparationSlot],
) -> ScreenInteropResult<()> {
    if target.native_identity != *identity {
        return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
            field: "preparation.native_identity",
        });
    }
    if target.opened_surfaces.len() != slots.len()
        || target
            .opened_surfaces
            .iter()
            .zip(slots)
            .any(|(opened, slot)| {
                opened.slot_id != slot.slot_id()
                    || opened.opaque_handle_id != slot.opaque_handle_id()
            })
    {
        return Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
            field: "preparation.slots",
        });
    }
    Ok(())
}

fn open_surface(
    interop: &D3d11On12ScreenDevice,
    slot: &GpuSurfaceTargetPreparationSlot,
    descriptor: &GpuSurfaceDescriptor,
) -> ScreenInteropResult<OpenedSurface> {
    let texture_handle = HANDLE(slot.texture_handle().as_raw() as *mut _);
    // SAFETY: the preparation manifest keeps its borrowed NT handle alive through
    // OpenSharedResource1 and the returned COM resource owns its reference.
    let texture: ID3D11Texture2D =
        unsafe { interop.device1.OpenSharedResource1(texture_handle) }
            .map_err(|error| windows_error("open capture shared texture", error))?;
    validate_opened_texture(&texture, descriptor)?;
    let resource = cast_interface(&texture, "ID3D11Resource capture texture")?;
    let keyed_mutex = cast_interface(&texture, "IDXGIKeyedMutex capture texture")?;

    let fence_handle = HANDLE(slot.fence_handle().as_raw() as *mut _);
    let mut fence = None;
    // SAFETY: the claimed lease keeps its borrowed fence handle alive through
    // OpenSharedFence and the returned COM fence owns its reference.
    unsafe {
        interop
            .device5
            .OpenSharedFence::<ID3D11Fence>(fence_handle, &mut fence)
    }
    .map_err(|error| windows_error("open capture shared fence", error))?;
    let fence = fence.ok_or(D3d11On12ScreenInteropError::MissingOpenedResource {
        operation: "open capture shared fence",
        resource: "ID3D11Fence",
    })?;
    Ok(OpenedSurface {
        slot_id: slot.slot_id(),
        opaque_handle_id: slot.opaque_handle_id(),
        resource,
        keyed_mutex,
        fence,
    })
}

fn validate_opened_texture(
    texture: &ID3D11Texture2D,
    descriptor: &GpuSurfaceDescriptor,
) -> ScreenInteropResult<()> {
    let mut observed = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: GetDesc fills caller-owned storage and cannot fail.
    unsafe { texture.GetDesc(&mut observed) };
    let expected = descriptor.output_extent();
    let invalid_field = if observed.Width != expected.width() {
        Some("native_texture.width")
    } else if observed.Height != expected.height() {
        Some("native_texture.height")
    } else if observed.Format != DXGI_FORMAT_R8G8B8A8_UNORM {
        Some("native_texture.format")
    } else if observed.MipLevels != 1 {
        Some("native_texture.mip_levels")
    } else if observed.ArraySize != 1 {
        Some("native_texture.array_size")
    } else if observed.SampleDesc.Count != 1 {
        Some("native_texture.sample_count")
    } else if observed.SampleDesc.Quality != 0 {
        Some("native_texture.sample_quality")
    } else {
        None
    };
    if let Some(field) = invalid_field {
        return Err(D3d11On12ScreenInteropError::UnsupportedPublicationContract { field });
    }
    Ok(())
}

fn create_target(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    interop: &D3d11On12ScreenDevice,
    width: u32,
    height: u32,
) -> ScreenInteropResult<ScreenCopyTarget> {
    let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hypercolor native screen copy target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let allocation_error = pollster::block_on(internal_scope.pop())
        .or_else(|| pollster::block_on(validation_scope.pop()))
        .or_else(|| pollster::block_on(out_of_memory_scope.pop()));
    if let Some(source) = allocation_error {
        return Err(D3d11On12ScreenInteropError::TargetAllocationFailed {
            width,
            height,
            source,
        });
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor native screen target initialization"),
    });
    {
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hypercolor native screen target clear"),
            color_attachments: &color_attachments,
            ..wgpu::RenderPassDescriptor::default()
        });
    }
    queue.submit([encoder.finish()]);

    let raw_resource = {
        // SAFETY: the COM clone is captured while the HAL texture guard is
        // live. Full clear submission above marks every subresource initialized
        // before native writes become invisible to wgpu's init tracker.
        let hal_texture = unsafe { texture.as_hal::<wgpu_hal::api::Dx12>() }
            .ok_or(D3d11On12ScreenInteropError::MissingWgpuDx12Device)?;
        // SAFETY: the guard proves this is the live DX12 resource for texture.
        unsafe { hal_texture.raw_resource() }.clone()
    };
    let flags = D3D11_RESOURCE_FLAGS {
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        MiscFlags: 0,
        CPUAccessFlags: 0,
        StructureByteStride: 0,
    };
    let mut wrapped_resource = None;
    // SAFETY: raw_resource belongs to the exact D3D12 device used to create
    // interop. Every wrapped acquire follows the explicit COPY_DST transition
    // in copy_publication, and both wrapped states agree with that transition.
    unsafe {
        interop.on12.CreateWrappedResource(
            &raw_resource,
            &flags,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_DEST,
            &mut wrapped_resource,
        )
    }
    .map_err(|error| windows_error("wrap renderer screen texture", error))?;
    let wrapped_resource =
        wrapped_resource.ok_or(D3d11On12ScreenInteropError::MissingOpenedResource {
            operation: "wrap renderer screen texture",
            resource: "ID3D11Resource",
        })?;
    Ok(ScreenCopyTarget {
        content_generation: 0,
        last_native_surface: None,
        texture: Arc::new(texture),
        view: Arc::new(view),
        wrapped_resource,
    })
}

fn windows_error(
    operation: &'static str,
    error: windows::core::Error,
) -> D3d11On12ScreenInteropError {
    D3d11On12ScreenInteropError::WindowsOperation {
        operation,
        hresult: error.code().0,
    }
}

fn abandon_windows_error(
    lease: GpuSurfaceLease,
    operation: &'static str,
    error: windows::core::Error,
) -> D3d11On12ScreenInteropError {
    abandon_before_acquire(lease, windows_error(operation, error))
}

fn abandon_before_acquire(
    lease: GpuSurfaceLease,
    error: D3d11On12ScreenInteropError,
) -> D3d11On12ScreenInteropError {
    let operation = match &error {
        D3d11On12ScreenInteropError::WindowsOperation { operation, .. }
        | D3d11On12ScreenInteropError::MissingOpenedResource { operation, .. } => *operation,
        _ => "prepare native screen publication",
    };
    match lease.abandon_before_acquire() {
        Ok(()) => error,
        Err(cleanup) => D3d11On12ScreenInteropError::PreAcquireCleanupFailed { operation, cleanup },
    }
}

enum KeyedAcquireStatus {
    Acquired,
    Timeout,
    Abandoned,
    Failed(i32),
    Unexpected(i32),
}

unsafe fn acquire_keyed_mutex(
    keyed_mutex: &IDXGIKeyedMutex,
    key: u64,
    timeout_ms: u32,
) -> KeyedAcquireStatus {
    // SAFETY: keyed_mutex is live, and this is the raw form of AcquireSync.
    // Reading the exact HRESULT is required because WAIT_TIMEOUT and
    // WAIT_ABANDONED are positive success-class values in this API.
    let result = unsafe {
        (Interface::vtable(keyed_mutex).AcquireSync)(
            Interface::as_raw(keyed_mutex),
            key,
            timeout_ms,
        )
    };
    if result == S_OK {
        KeyedAcquireStatus::Acquired
    } else if result.0 == WAIT_TIMEOUT.0 as i32 {
        KeyedAcquireStatus::Timeout
    } else if result.0 == WAIT_ABANDONED.0 as i32 {
        KeyedAcquireStatus::Abandoned
    } else if result.is_err() {
        KeyedAcquireStatus::Failed(result.0)
    } else {
        KeyedAcquireStatus::Unexpected(result.0)
    }
}
