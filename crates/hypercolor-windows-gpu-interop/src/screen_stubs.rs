use std::sync::Arc;

use hypercolor_windows_capture::{
    GpuAdapterLuid, GpuSurfaceDescriptor, GpuSurfaceDescriptorId, GpuSurfacePlanGeneration,
    GpuSurfacePublication,
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

/// D3D11On12 interfaces permanently bound to one renderer device and queue.
pub struct D3d11On12ScreenDevice {
    adapter_luid: GpuAdapterLuid,
}

/// Windows-only D3D11On12 screen-copy bridge.
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
        &mut self,
        _descriptor: &GpuSurfaceDescriptor,
    ) -> ScreenInteropResult<ScreenCopyTargetAllocation> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// Reject a native publication outside Windows.
    ///
    /// # Errors
    ///
    /// Always returns [`D3d11On12ScreenInteropError::UnsupportedPlatform`].
    pub fn copy_publication(
        &mut self,
        _publication: &Arc<GpuSurfacePublication>,
    ) -> ScreenInteropResult<ScreenTextureCopy> {
        Err(D3d11On12ScreenInteropError::UnsupportedPlatform)
    }

    /// No-op outside Windows.
    pub fn retire_source(
        &mut self,
        _source_id: &str,
        _topology_generation: u64,
        _plan_generation: GpuSurfacePlanGeneration,
    ) {
    }

    /// No-op outside Windows.
    pub fn retire_descriptor(&mut self, _descriptor_id: GpuSurfaceDescriptorId) {}
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
