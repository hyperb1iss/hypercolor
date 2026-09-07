use hypercolor_gpu_frame::{GpuFrameImportError, GpuFrameImportFallbackReason};
use thiserror::Error;

use crate::ImportedFrameFormat;

/// Result type for macOS GPU interop operations.
pub type Result<T> = std::result::Result<T, MacosGpuInteropError>;

/// Runtime facilities required by the direct Metal 4 reduction prototype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacosMetal4CapabilityProbe {
    /// Registry identity of the exact Metal device behind the wgpu device.
    pub metal_registry_id: u64,
    /// Whether the active device reports the Metal 4 GPU family.
    pub metal4_family: bool,
    /// Whether the active device exposes Metal 4 command allocators.
    pub command_allocator: bool,
    /// Whether the active device exposes Metal 4 command queues.
    pub command_queue: bool,
    /// Whether the active device exposes Metal 4 command buffers.
    pub command_buffer: bool,
    /// Whether the active device exposes Metal 4 argument tables.
    pub argument_table: bool,
    /// Whether the active device exposes residency-set creation.
    pub residency_set: bool,
    /// Whether the active device exposes shared events for completion timing.
    pub shared_event: bool,
    /// Whether the active device exposes command-buffer GPU interval feedback.
    pub commit_feedback: bool,
}

impl MacosMetal4CapabilityProbe {
    /// Whether every facility required by the prototype is callable.
    #[must_use]
    pub const fn all_required_facilities(self) -> bool {
        self.metal4_family
            && self.command_allocator
            && self.command_queue
            && self.command_buffer
            && self.argument_table
            && self.residency_set
            && self.shared_event
            && self.commit_feedback
    }

    /// Missing facilities in a stable order, padded with `None`.
    #[must_use]
    pub const fn missing_facilities(self) -> [Option<&'static str>; 8] {
        [
            if self.metal4_family {
                None
            } else {
                Some("metal4_family")
            },
            if self.command_allocator {
                None
            } else {
                Some("command_allocator")
            },
            if self.command_queue {
                None
            } else {
                Some("command_queue")
            },
            if self.command_buffer {
                None
            } else {
                Some("command_buffer")
            },
            if self.argument_table {
                None
            } else {
                Some("argument_table")
            },
            if self.residency_set {
                None
            } else {
                Some("residency_set")
            },
            if self.shared_event {
                None
            } else {
                Some("shared_event")
            },
            if self.commit_feedback {
                None
            } else {
                Some("commit_feedback")
            },
        ]
    }
}

/// Probe Metal 4 facilities on the exact Metal device behind a wgpu device.
pub fn probe_macos_metal4_capabilities(
    _device: &wgpu::Device,
) -> Result<MacosMetal4CapabilityProbe> {
    Err(MacosGpuInteropError::UnsupportedPlatform)
}

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

    /// The neutral frame format is not supported by the macOS import path.
    #[error("unsupported macOS import frame format {format:?}")]
    UnsupportedFrameFormat {
        /// Requested frame format.
        format: ImportedFrameFormat,
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

impl GpuFrameImportError for MacosGpuInteropError {
    fn fallback_reason(&self) -> GpuFrameImportFallbackReason {
        match self {
            Self::UnsupportedPlatform | Self::UnsupportedFrameFormat { .. } => {
                GpuFrameImportFallbackReason::Other
            }
            Self::InvalidDimensions { .. } | Self::PixelBufferSizeMismatch { .. } => {
                GpuFrameImportFallbackReason::InvalidDimensions
            }
        }
    }
}

/// Family-selected Metal storage mode for imported IOSurfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosMetalStorageMode {
    /// Coherent shared storage on Apple-family GPUs.
    Shared,
    /// Managed storage required by non-Apple-family GPUs.
    Managed,
}

/// Description of a macOS IOSurface import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        if width == 0
            || height == 0
            || width > i32::MAX as u32 / format.bytes_per_texel()
            || height > i32::MAX as u32
        {
            Err(MacosGpuInteropError::InvalidDimensions { width, height })
        } else if !matches!(
            format,
            ImportedFrameFormat::Bgra8Unorm
                | ImportedFrameFormat::Rgba16Float
                | ImportedFrameFormat::R8Unorm
                | ImportedFrameFormat::Rg8Unorm
                | ImportedFrameFormat::R16Unorm
                | ImportedFrameFormat::Rg16Unorm
        ) {
            Err(MacosGpuInteropError::UnsupportedFrameFormat { format })
        } else {
            Ok(Self {
                width,
                height,
                format,
            })
        }
    }
}

/// Reusable importer for wrapping IOSurfaces as wgpu textures.
pub struct MacosIosurfaceImporter {
    descriptor: MacosIosurfaceImportDescriptor,
    storage_mode: MacosMetalStorageMode,
    metal_registry_id: u64,
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

    /// Metal registry identity this importer is bound to.
    #[must_use]
    pub const fn metal_registry_id(&self) -> u64 {
        self.metal_registry_id
    }

    /// Family-selected storage mode used for IOSurface textures.
    #[must_use]
    pub const fn storage_mode(&self) -> MacosMetalStorageMode {
        self.storage_mode
    }
}
