use std::sync::Arc;

use hypercolor_windows_capture::{
    GpuAdapterLuid, GpuSurfaceDescriptor, GpuSurfacePlanGeneration, GpuSurfacePublication,
};
use thiserror::Error;

/// Result type for D3D11On12 screen-copy interop.
pub type ScreenInteropResult<T> = std::result::Result<T, D3d11On12ScreenInteropError>;

/// Failures while binding screen interop to a renderer-owned DX12 queue.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum D3d11On12ScreenInteropError {
    /// D3D11On12 screen-copy interop is Windows-only.
    #[error("D3D11On12 screen interop is only available on Windows")]
    UnsupportedPlatform,
}

/// Renderer-local GPU texture carrying one exact native screen result.
#[derive(Clone, Debug)]
pub struct ScreenTextureCopy {
    /// Stable identity of the renderer-owned texture allocation.
    pub storage_id: u64,
    /// Monotonic content identity.
    pub content_generation: u64,
    /// Exact output width.
    pub width: u32,
    /// Exact output height.
    pub height: u32,
    /// Renderer-owned ordinary wgpu texture.
    pub texture: Arc<wgpu::Texture>,
    /// Default shader-resource view.
    pub view: Arc<wgpu::TextureView>,
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
    /// Exact physical bytes retained by live renderer targets.
    pub retained_target_bytes: u64,
}

/// Opaque Windows renderer-target lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedScreenCopyTarget {
    allocation: ScreenCopyTargetAllocation,
}

impl PreparedScreenCopyTarget {
    /// Exact renderer allocation retained by this lease.
    #[must_use]
    pub const fn allocation(&self) -> ScreenCopyTargetAllocation {
        self.allocation
    }

    /// Stable identity of the renderer-owned texture allocation.
    #[must_use]
    pub const fn storage_id(&self) -> u64 {
        self.allocation.storage_id
    }

    /// Physical RGBA bytes retained by this renderer target.
    #[must_use]
    pub const fn retained_bytes(&self) -> u64 {
        self.allocation.retained_bytes
    }

    /// Exact target width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.allocation.width
    }

    /// Exact target height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.allocation.height
    }
}

/// D3D11On12 interfaces permanently bound to one renderer device and queue.
pub struct D3d11On12ScreenDevice {
    adapter_luid: GpuAdapterLuid,
}

/// Windows-only D3D11On12 screen-copy bridge.
#[derive(Clone)]
pub struct D3d11On12ScreenBridge {
    adapter_luid: GpuAdapterLuid,
}

impl D3d11On12ScreenBridge {
    /// Reject construction outside Windows.
    ///
    /// # Errors
    ///
    /// Always returns [`D3d11On12ScreenInteropError::UnsupportedPlatform`].
    pub fn new(_device: wgpu::Device, _queue: wgpu::Queue) -> ScreenInteropResult<Self> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// Adapter identity retained by an initialized bridge.
    #[must_use]
    pub const fn adapter_luid(&self) -> GpuAdapterLuid {
        self.adapter_luid
    }

    /// Reject target preparation outside Windows.
    ///
    /// # Errors
    ///
    /// Always returns [`D3d11On12ScreenInteropError::UnsupportedPlatform`].
    pub fn prepare_target(
        &self,
        _plan_generation: GpuSurfacePlanGeneration,
        _descriptor: &GpuSurfaceDescriptor,
    ) -> ScreenInteropResult<PreparedScreenCopyTarget> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// Reject cache inspection outside Windows.
    ///
    /// # Errors
    ///
    /// Always returns [`D3d11On12ScreenInteropError::UnsupportedPlatform`].
    pub fn cache_stats(&self) -> ScreenInteropResult<ScreenInteropCacheStats> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// Reject a native publication outside Windows.
    ///
    /// # Errors
    ///
    /// Always returns [`D3d11On12ScreenInteropError::UnsupportedPlatform`].
    pub fn copy_publication(
        &self,
        _prepared: &PreparedScreenCopyTarget,
        _publication: &Arc<GpuSurfacePublication>,
    ) -> ScreenInteropResult<ScreenTextureCopy> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// No-op outside Windows.
    pub fn retire_source(
        &self,
        _source_id: &str,
        _topology_generation: u64,
        _plan_generation: GpuSurfacePlanGeneration,
    ) {
    }
}

impl D3d11On12ScreenDevice {
    /// Reject construction outside Windows.
    ///
    /// # Errors
    ///
    /// Always returns [`D3d11On12ScreenInteropError::UnsupportedPlatform`].
    pub fn new(_device: &wgpu::Device, _queue: &wgpu::Queue) -> ScreenInteropResult<Self> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// Adapter identity retained by an initialized device.
    #[must_use]
    pub const fn adapter_luid(&self) -> GpuAdapterLuid {
        self.adapter_luid
    }
}
