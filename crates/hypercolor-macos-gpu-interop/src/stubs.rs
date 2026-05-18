use std::sync::Arc;

use thiserror::Error;

/// Result type for macOS GPU interop operations.
pub type Result<T> = std::result::Result<T, MacosGpuInteropError>;

/// Errors raised while preparing or importing macOS GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacosGpuInteropError {
    /// The current platform is not macOS.
    #[error("macOS GPU interop is only available on macOS")]
    UnsupportedPlatform,

    /// Frame dimensions are not usable by IOSurface or wgpu.
    #[error("invalid import dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested frame width.
        width: u32,
        /// Requested frame height.
        height: u32,
    },

    /// The supplied pixel buffer does not match the IOSurface dimensions.
    #[error("pixel buffer length mismatch: expected {expected_len} bytes, got {actual_len}")]
    PixelBufferSizeMismatch {
        /// Expected byte length.
        expected_len: usize,
        /// Actual byte length.
        actual_len: usize,
    },
}

/// Pixel format shared by the IOSurface and imported wgpu texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportedFrameFormat {
    /// 8-bit normalized BGRA.
    Bgra8Unorm,
}

impl ImportedFrameFormat {
    /// Returns the matching wgpu texture format.
    #[must_use]
    pub const fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        }
    }
}

/// Description of a macOS IOSurface import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacosIosurfaceImportDescriptor {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame pixel format.
    pub format: ImportedFrameFormat,
}

impl MacosIosurfaceImportDescriptor {
    /// Creates a validated import descriptor.
    pub const fn new(width: u32, height: u32, format: ImportedFrameFormat) -> Result<Self> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            Err(MacosGpuInteropError::InvalidDimensions { width, height })
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

/// Timing counters captured while importing an IOSurface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedFrameTimings {
    /// Time spent creating the Metal texture wrapper.
    pub wrap_us: u64,
    /// Total import time, including wgpu wrapping.
    pub total_us: u64,
}

/// Reusable importer for wrapping IOSurfaces as wgpu textures.
pub struct MacosIosurfaceImporter {
    descriptor: MacosIosurfaceImportDescriptor,
}

impl MacosIosurfaceImporter {
    /// Creates an importer for one IOSurface shape.
    pub fn new(_device: &wgpu::Device, descriptor: MacosIosurfaceImportDescriptor) -> Result<Self> {
        let _descriptor = MacosIosurfaceImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        Err(MacosGpuInteropError::UnsupportedPlatform)
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> MacosIosurfaceImportDescriptor {
        self.descriptor
    }
}
