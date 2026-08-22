use hypercolor_gpu_frame::{GpuFrameImportError, GpuFrameImportFallbackReason};
use thiserror::Error;

use crate::ImportedFrameFormat;

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

    /// The neutral frame format is not supported by the Windows import path.
    #[error("unsupported Windows import frame format {format:?}")]
    UnsupportedFrameFormat {
        /// Requested frame format.
        format: ImportedFrameFormat,
    },
}

impl GpuFrameImportError for WindowsGpuInteropError {
    fn fallback_reason(&self) -> GpuFrameImportFallbackReason {
        match self {
            Self::UnsupportedPlatform | Self::UnsupportedFrameFormat { .. } => {
                GpuFrameImportFallbackReason::Other
            }
            Self::MissingWgpuVulkanDevice => GpuFrameImportFallbackReason::MissingWgpuVulkanDevice,
            Self::MissingVulkanExternalMemoryWin32 => {
                GpuFrameImportFallbackReason::MissingVulkanExternalMemoryWin32
            }
            Self::MissingWindowsAngleContext => {
                GpuFrameImportFallbackReason::MissingWindowsAngleContext
            }
            Self::InvalidDimensions { .. } => GpuFrameImportFallbackReason::InvalidDimensions,
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
        } else if !matches!(
            format,
            ImportedFrameFormat::Rgba8Unorm | ImportedFrameFormat::Bgra8Unorm
        ) {
            Err(WindowsGpuInteropError::UnsupportedFrameFormat { format })
        } else {
            Ok(Self {
                width,
                height,
                format,
            })
        }
    }
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
