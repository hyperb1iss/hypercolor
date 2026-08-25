use hypercolor_gpu_frame::{GpuFrameImportError, GpuFrameImportFallbackReason};
use std::ffi::{CStr, c_void};
use thiserror::Error;

use crate::{ImportedEffectFrame, ImportedFrameFormat};

/// Result type for Linux GPU interop operations.
pub type Result<T> = std::result::Result<T, LinuxGpuInteropError>;

/// Errors raised while preparing or importing Linux GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinuxGpuInteropError {
    /// The current platform is not Linux.
    #[error("Linux GPU interop is only available on Linux")]
    UnsupportedPlatform,

    /// Frame dimensions are not usable by GL or wgpu.
    #[error("invalid import dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested frame width.
        width: u32,
        /// Requested frame height.
        height: u32,
    },

    /// The neutral frame format is not supported by the Linux import path.
    #[error("unsupported Linux import frame format {format:?}")]
    UnsupportedFrameFormat {
        /// Requested frame format.
        format: ImportedFrameFormat,
    },

    /// Every pooled import slot is still referenced by downstream GPU work.
    #[error("all {slot_count} GPU import slots are still in use")]
    ImportSlotsExhausted {
        /// Number of slots in the import pool.
        slot_count: usize,
    },
}

impl GpuFrameImportError for LinuxGpuInteropError {
    fn fallback_reason(&self) -> GpuFrameImportFallbackReason {
        match self {
            Self::UnsupportedPlatform => GpuFrameImportFallbackReason::UnsupportedPlatform,
            Self::InvalidDimensions { .. } => GpuFrameImportFallbackReason::InvalidDimensions,
            Self::UnsupportedFrameFormat { .. } => GpuFrameImportFallbackReason::Other,
            Self::ImportSlotsExhausted { .. } => GpuFrameImportFallbackReason::ImportSlotsExhausted,
        }
    }
}

/// Description of a Servo GL framebuffer import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxGlFramebufferImportDescriptor {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame pixel format.
    pub format: ImportedFrameFormat,
}

impl LinuxGlFramebufferImportDescriptor {
    /// Creates a validated import descriptor.
    pub const fn new(width: u32, height: u32, format: ImportedFrameFormat) -> Result<Self> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            Err(LinuxGpuInteropError::InvalidDimensions { width, height })
        } else if !matches!(format, ImportedFrameFormat::Rgba8Unorm) {
            Err(LinuxGpuInteropError::UnsupportedFrameFormat { format })
        } else {
            Ok(Self {
                width,
                height,
                format,
            })
        }
    }
}

/// Reusable importer for repeatedly copying one GL framebuffer into wgpu.
pub struct LinuxGlFramebufferImporter {
    descriptor: LinuxGlFramebufferImportDescriptor,
}

impl LinuxGlFramebufferImporter {
    /// Creates a pooled importer using GL entry points loaded from the process.
    pub fn new_from_process(
        _device: &wgpu::Device,
        _gl: &glow::Context,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<Self> {
        let _descriptor = LinuxGlFramebufferImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    }

    /// Creates a pooled importer using the supplied GL external-memory entry points.
    pub fn new(
        _device: &wgpu::Device,
        _gl: &glow::Context,
        _gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        _slot_count: usize,
    ) -> Result<Self> {
        let _descriptor = LinuxGlFramebufferImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> LinuxGlFramebufferImportDescriptor {
        self.descriptor
    }

    /// Returns the number of reusable import slots.
    #[must_use]
    pub const fn slot_count(&self) -> usize {
        0
    }

    /// Imports the current GL framebuffer contents into a pooled wgpu texture.
    pub fn import_framebuffer(
        &mut self,
        _gl: &glow::Context,
        _source_framebuffer: GlFramebufferSource,
    ) -> Result<ImportedEffectFrame> {
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    }

    /// Queues a GL framebuffer blit and returns the newest completed import.
    pub fn import_framebuffer_pipelined(
        &mut self,
        _gl: &glow::Context,
        _source_framebuffer: GlFramebufferSource,
    ) -> Result<ImportedEffectFrame> {
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    }

    /// Deletes pooled GL objects while their context is current.
    pub fn destroy_gl_resources(&mut self, _gl: &glow::Context) {}
}

/// Capability report for the Linux zero-copy import path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxGpuImportCapabilities {
    /// Whether the active wgpu device exposes its Vulkan HAL.
    pub wgpu_vulkan_device: bool,
    /// Whether the Vulkan device has `VK_KHR_external_memory_fd`.
    pub vulkan_external_memory_fd: bool,
    /// Whether the GL context exposes `GL_EXT_memory_object_fd`.
    pub gl_memory_object_fd: bool,
    /// Missing capabilities, in stable diagnostic order.
    pub missing: Vec<&'static str>,
}

impl LinuxGpuImportCapabilities {
    /// Returns `true` when all required capabilities are present.
    #[must_use]
    pub fn supported(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Loaded GL entry points for `GL_EXT_memory_object_fd`.
#[derive(Clone, Copy)]
pub struct GlExternalMemoryFunctions;

impl GlExternalMemoryFunctions {
    /// Loads required entry points from a current GL context.
    pub fn load_from(_get_proc_address: impl FnMut(&CStr) -> *const c_void) -> Result<Self> {
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    }

    /// Loads required entry points from libGL/libEGL process loaders.
    pub fn load_from_process() -> Result<Self> {
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    }
}

/// Reports missing GL entry points for `GL_EXT_memory_object_fd`.
#[must_use]
pub fn missing_gl_external_memory_functions(
    _get_proc_address: impl FnMut(&CStr) -> *const c_void,
) -> Vec<&'static str> {
    vec![
        "glCreateMemoryObjectsEXT",
        "glMemoryObjectParameterivEXT",
        "glImportMemoryFdEXT",
        "glTexStorageMem2DEXT",
        "glDeleteMemoryObjectsEXT",
    ]
}

/// Reports missing `GL_EXT_memory_object_fd` entry points through libGL/libEGL.
#[must_use]
pub fn missing_process_gl_external_memory_functions() -> Vec<&'static str> {
    missing_gl_external_memory_functions(|_| std::ptr::null())
}

/// Verifies that the active wgpu device can expose Vulkan external memory FDs.
pub fn check_wgpu_vulkan_external_memory_fd(_device: &wgpu::Device) -> Result<()> {
    Err(LinuxGpuInteropError::UnsupportedPlatform)
}

/// Collects the import capability status for the active GL and wgpu contexts.
#[must_use]
pub fn report_linux_gpu_import_capabilities(
    _device: &wgpu::Device,
    _get_proc_address: impl FnMut(&CStr) -> *const c_void,
) -> LinuxGpuImportCapabilities {
    LinuxGpuImportCapabilities {
        wgpu_vulkan_device: false,
        vulkan_external_memory_fd: false,
        gl_memory_object_fd: false,
        missing: vec!["linux"],
    }
}

/// Source framebuffer selection for the GL import blit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlFramebufferSource {
    /// Use the framebuffer currently bound to `READ_FRAMEBUFFER`.
    CurrentRead,
    /// Bind and read the supplied framebuffer. `None` means the default FBO.
    Framebuffer(Option<glow::NativeFramebuffer>),
}

/// Imports a GL framebuffer into the supplied wgpu device without CPU readback.
pub fn import_gl_framebuffer_to_wgpu(
    _device: &wgpu::Device,
    _gl: &glow::Context,
    _gl_external_memory: GlExternalMemoryFunctions,
    _source_framebuffer: GlFramebufferSource,
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> Result<ImportedEffectFrame> {
    let _ = LinuxGlFramebufferImportDescriptor::new(
        descriptor.width,
        descriptor.height,
        descriptor.format,
    )?;
    Err(LinuxGpuInteropError::UnsupportedPlatform)
}

/// Imports a GL framebuffer using libGL/libEGL to resolve extension functions.
pub fn import_gl_framebuffer_to_wgpu_from_process(
    _device: &wgpu::Device,
    _gl: &glow::Context,
    _source_framebuffer: GlFramebufferSource,
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> Result<ImportedEffectFrame> {
    let _ = LinuxGlFramebufferImportDescriptor::new(
        descriptor.width,
        descriptor.height,
        descriptor.format,
    )?;
    Err(LinuxGpuInteropError::UnsupportedPlatform)
}
