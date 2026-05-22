use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use thiserror::Error;
use windows::Win32::Foundation::HANDLE;

const BYTES_PER_PIXEL: u32 = 4;
static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Result type for Windows GPU interop operations.
pub type Result<T> = std::result::Result<T, WindowsGpuInteropError>;

/// Errors raised while preparing or importing Windows GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowsGpuInteropError {
    /// The active wgpu device is not backed by Vulkan.
    #[error("wgpu device is not backed by the Vulkan HAL")]
    MissingWgpuVulkanDevice,

    /// The active wgpu device lacks Win32 external-memory support.
    #[error("wgpu Vulkan device is missing VULKAN_EXTERNAL_MEMORY_WIN32")]
    MissingVulkanExternalMemoryWin32,

    /// A Windows ANGLE rendering context is required before import can run.
    #[error("Windows ANGLE rendering context is unavailable")]
    MissingWindowsAngleContext,

    /// Frame dimensions are not usable by D3D11 or wgpu.
    #[error("invalid import dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested frame width.
        width: u32,
        /// Requested frame height.
        height: u32,
    },

    /// The supplied D3D11 shared handle is null.
    #[error("D3D11 shared texture handle is null")]
    InvalidSharedHandle,

    /// Vulkan failed while importing the D3D11 shared handle.
    #[error("Vulkan D3D11 shared-handle import failed")]
    VulkanD3d11ImportFailed,
}

/// Pixel format shared by the D3D11 texture and imported wgpu texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportedFrameFormat {
    /// 8-bit normalized RGBA.
    Rgba8Unorm,
    /// 8-bit normalized BGRA.
    Bgra8Unorm,
}

impl ImportedFrameFormat {
    /// Returns the matching wgpu texture format.
    #[must_use]
    pub const fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        }
    }
}

/// Description of a Windows D3D11 shared-texture import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsD3d11SharedTextureImportDescriptor {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame pixel format.
    pub format: ImportedFrameFormat,
}

impl WindowsD3d11SharedTextureImportDescriptor {
    /// Creates a validated import descriptor.
    pub const fn new(width: u32, height: u32, format: ImportedFrameFormat) -> Result<Self> {
        if width == 0
            || height == 0
            || width > i32::MAX as u32 / BYTES_PER_PIXEL
            || height > i32::MAX as u32
        {
            Err(WindowsGpuInteropError::InvalidDimensions { width, height })
        } else {
            Ok(Self {
                width,
                height,
                format,
            })
        }
    }
}

/// GPU-resident Servo effect frame imported into Hypercolor's wgpu device.
#[derive(Debug, Clone)]
pub struct ImportedEffectFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame pixel format.
    pub format: ImportedFrameFormat,
    /// Monotonic storage identity for cache comparisons.
    pub storage_id: u64,
    /// Imported wgpu texture.
    pub texture: Arc<wgpu::Texture>,
    /// Default view over `texture`.
    pub view: Arc<wgpu::TextureView>,
    /// Import timing counters for observability.
    pub timings: ImportedFrameTimings,
}

/// Timing counters captured while importing a D3D11 shared texture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedFrameTimings {
    /// Time spent wrapping the D3D11 shared handle as a wgpu texture.
    pub wrap_us: u64,
    /// Time spent waiting for producer-side synchronization.
    pub sync_us: u64,
    /// Total import time, including wgpu wrapping.
    pub total_us: u64,
}

/// Reusable importer for wrapping D3D11 shared textures as wgpu textures.
pub struct WindowsD3d11SharedTextureImporter {
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
}

impl WindowsD3d11SharedTextureImporter {
    /// Creates an importer for one shared-texture shape.
    pub fn new(
        device: &wgpu::Device,
        descriptor: WindowsD3d11SharedTextureImportDescriptor,
    ) -> Result<Self> {
        let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        check_wgpu_vulkan_external_memory_win32(device)?;
        Ok(Self { descriptor })
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> WindowsD3d11SharedTextureImportDescriptor {
        self.descriptor
    }

    /// Imports a D3D11 NT shared handle into the supplied wgpu device.
    ///
    /// # Safety
    ///
    /// The handle must be a live NT handle from
    /// `IDXGIResource1::CreateSharedHandle`, created for a D3D11 texture whose
    /// dimensions and format match this importer's descriptor. The producer
    /// must have completed writes before this function is called.
    pub unsafe fn import_shared_handle(
        &mut self,
        device: &wgpu::Device,
        shared_handle: HANDLE,
        sync_us: u64,
    ) -> Result<ImportedEffectFrame> {
        if shared_handle.is_invalid() {
            return Err(WindowsGpuInteropError::InvalidSharedHandle);
        }

        let total_start = Instant::now();
        let wrap_start = Instant::now();
        // SAFETY: this only borrows the underlying HAL device long enough to
        // import the caller-owned D3D11 NT handle; no raw Vulkan device handle
        // escapes this function.
        let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
            .ok_or(WindowsGpuInteropError::MissingWgpuVulkanDevice)?;
        let hal_desc = hal_texture_descriptor(self.descriptor);
        // SAFETY: the caller guarantees shared_handle is an NT D3D11 texture
        // handle matching hal_desc and completed producer synchronization.
        let hal_texture =
            unsafe { hal_device.texture_from_d3d11_shared_handle(shared_handle, &hal_desc) }
                .map_err(|_| WindowsGpuInteropError::VulkanD3d11ImportFailed)?;
        let wgpu_desc = wgpu_texture_descriptor(self.descriptor);
        // SAFETY: hal_texture was created from this wgpu device's Vulkan HAL
        // and matches wgpu_desc.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(hal_texture, &wgpu_desc)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let wrap_us = elapsed_micros(wrap_start);

        Ok(ImportedEffectFrame {
            width: self.descriptor.width,
            height: self.descriptor.height,
            format: self.descriptor.format,
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            texture: Arc::new(texture),
            view: Arc::new(view),
            timings: ImportedFrameTimings {
                wrap_us,
                sync_us,
                total_us: elapsed_micros(total_start),
            },
        })
    }
}

/// Verifies that the active wgpu device can expose Vulkan external memory Win32 handles.
pub fn check_wgpu_vulkan_external_memory_win32(device: &wgpu::Device) -> Result<()> {
    // SAFETY: this only probes whether the wgpu device exposes the Vulkan HAL.
    if unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }.is_none() {
        return Err(WindowsGpuInteropError::MissingWgpuVulkanDevice);
    }
    if device
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32)
    {
        Ok(())
    } else {
        Err(WindowsGpuInteropError::MissingVulkanExternalMemoryWin32)
    }
}

fn wgpu_texture_descriptor(
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("hypercolor-windows-servo-import"),
        size: wgpu::Extent3d {
            width: descriptor.width,
            height: descriptor.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: descriptor.format.wgpu_format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    }
}

fn hal_texture_descriptor(
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
) -> wgpu_hal::TextureDescriptor<'static> {
    wgpu_hal::TextureDescriptor {
        label: Some("hypercolor-windows-servo-import"),
        size: wgpu::Extent3d {
            width: descriptor.width,
            height: descriptor.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: descriptor.format.wgpu_format(),
        usage: wgpu::TextureUses::RESOURCE
            | wgpu::TextureUses::COPY_SRC
            | wgpu::TextureUses::COLOR_TARGET,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    }
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}
