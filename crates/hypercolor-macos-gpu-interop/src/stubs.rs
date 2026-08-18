use std::sync::Arc;

use thiserror::Error;

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

    /// The supplied pixel buffer does not match the IOSurface dimensions.
    #[error("pixel buffer length mismatch: expected {expected_len} bytes, got {actual_len}")]
    PixelBufferSizeMismatch {
        /// Expected byte length.
        expected_len: usize,
        /// Actual byte length.
        actual_len: usize,
    },
}

/// Family-selected Metal storage mode for imported IOSurfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosMetalStorageMode {
    /// Coherent shared storage on Apple-family GPUs.
    Shared,
    /// Managed storage required by non-Apple-family GPUs.
    Managed,
}

/// Pixel format shared by the IOSurface and imported wgpu texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImportedFrameFormat {
    /// 8-bit normalized BGRA.
    Bgra8Unorm,
    /// 16-bit floating-point RGBA.
    Rgba16Float,
    /// One 8-bit normalized component.
    R8Unorm,
    /// Two 8-bit normalized components.
    Rg8Unorm,
    /// One 16-bit normalized component.
    R16Unorm,
    /// Two 16-bit normalized components.
    Rg16Unorm,
}

impl ImportedFrameFormat {
    /// Returns the matching wgpu texture format.
    #[must_use]
    pub const fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            Self::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            Self::R8Unorm => wgpu::TextureFormat::R8Unorm,
            Self::Rg8Unorm => wgpu::TextureFormat::Rg8Unorm,
            Self::R16Unorm => wgpu::TextureFormat::R16Unorm,
            Self::Rg16Unorm => wgpu::TextureFormat::Rg16Unorm,
        }
    }

    const fn bytes_per_texel(self) -> u32 {
        match self {
            Self::Bgra8Unorm => 4,
            Self::Rgba16Float => 8,
            Self::R8Unorm => 1,
            Self::Rg8Unorm | Self::R16Unorm => 2,
            Self::Rg16Unorm => 4,
        }
    }
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
    /// Monotonically increasing content version; contents changed iff this
    /// changed. Does NOT imply distinct GPU storage — the same IOSurface (and
    /// cached wgpu texture) can carry many successive versions.
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
