use std::sync::Arc;

use thiserror::Error;

const BYTES_PER_PIXEL: u32 = 4;

/// Result type for Windows GPU interop operations.
pub type Result<T> = std::result::Result<T, WindowsGpuInteropError>;

/// Errors raised while preparing or importing Windows GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowsGpuInteropError {
    /// The current platform is not Windows.
    #[error("Windows GPU interop is only available on Windows")]
    UnsupportedPlatform,

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
        _device: &wgpu::Device,
        descriptor: WindowsD3d11SharedTextureImportDescriptor,
    ) -> Result<Self> {
        let _descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        Err(WindowsGpuInteropError::UnsupportedPlatform)
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> WindowsD3d11SharedTextureImportDescriptor {
        self.descriptor
    }
}
