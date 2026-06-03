use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use ash::vk;
use thiserror::Error;

const DEFAULT_IMPORT_SLOT_COUNT: usize = 8;

mod capabilities;
mod fence;
mod gl_external_memory;
mod loader;
mod slot_pool;
mod vulkan_export;

pub use capabilities::{
    LinuxGpuImportCapabilities, check_wgpu_vulkan_external_memory_fd,
    missing_gl_external_memory_functions, missing_process_gl_external_memory_functions,
    report_linux_gpu_import_capabilities,
};
pub use gl_external_memory::GlExternalMemoryFunctions;
use slot_pool::{ImportedFrameSlotPool, SingleFrameImport, import_framebuffer_once};
use vulkan_export::ExportableVulkanImage;

/// Result type for Linux GPU interop operations.
pub type Result<T> = std::result::Result<T, LinuxGpuInteropError>;

/// Errors raised while preparing or importing Linux GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinuxGpuInteropError {
    /// The active wgpu device is not backed by Vulkan.
    #[error("wgpu device is not backed by the Vulkan HAL")]
    MissingWgpuVulkanDevice,

    /// The Vulkan device was created without a required extension.
    #[error("Vulkan device is missing required extension {0}")]
    MissingVulkanDeviceExtension(&'static str),

    /// The active GL context is missing a required entry point.
    #[error("OpenGL context is missing required function {0}")]
    MissingGlFunction(&'static str),

    /// No process GL loader could be found.
    #[error("failed to load libGL or libEGL process entry points")]
    GlProcLoaderUnavailable,

    /// Frame dimensions are not usable by GL or wgpu.
    #[error("invalid import dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested frame width.
        width: u32,
        /// Requested frame height.
        height: u32,
    },

    /// Vulkan failed while creating or exporting the backing image.
    #[error("Vulkan {operation} failed: {result:?}")]
    Vulkan {
        /// Failed Vulkan operation.
        operation: &'static str,
        /// Vulkan result code.
        result: vk::Result,
    },

    /// No compatible Vulkan memory type was available for the image.
    #[error("no compatible Vulkan memory type found for external image")]
    MemoryTypeUnavailable,

    /// Duplicating the exported memory FD failed.
    #[error("failed to duplicate external memory FD: errno {errno}")]
    DuplicateFdFailed {
        /// OS errno from `dup`.
        errno: i32,
    },

    /// OpenGL failed to create a temporary object.
    #[error("OpenGL failed to create {resource}: {message}")]
    GlCreateResource {
        /// GL resource kind.
        resource: &'static str,
        /// Driver error message.
        message: String,
    },

    /// OpenGL reported an error code after an interop operation.
    #[error("OpenGL {operation} failed with error 0x{code:04x}")]
    GlOperation {
        /// GL operation name.
        operation: &'static str,
        /// GL error code.
        code: u32,
    },

    /// The imported texture framebuffer was incomplete.
    #[error("import destination framebuffer is incomplete: 0x{status:04x}")]
    GlFramebufferIncomplete {
        /// GL framebuffer status.
        status: u32,
    },

    /// Every pooled import slot is still referenced by downstream GPU work.
    #[error("all {slot_count} GPU import slots are still in use")]
    ImportSlotsExhausted {
        /// Number of slots in the import pool.
        slot_count: usize,
    },
}

/// Pixel format shared by the GL source and imported wgpu texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportedFrameFormat {
    /// 8-bit normalized RGBA.
    Rgba8Unorm,
}

impl ImportedFrameFormat {
    /// Returns the matching wgpu texture format.
    #[must_use]
    pub const fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    /// Returns the matching GL internal format.
    #[must_use]
    pub const fn gl_internal_format(self) -> u32 {
        match self {
            Self::Rgba8Unorm => glow::RGBA8,
        }
    }

    /// Returns the matching Vulkan image format.
    #[must_use]
    pub const fn vk_format(self) -> vk::Format {
        match self {
            Self::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
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
        } else {
            Ok(Self {
                width,
                height,
                format,
            })
        }
    }

    fn width_i32(self) -> i32 {
        self.width as i32
    }

    fn height_i32(self) -> i32 {
        self.height as i32
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

/// Timing counters captured while importing a GL framebuffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedFrameTimings {
    /// Time spent issuing the GL framebuffer blit.
    pub blit_us: u64,
    /// Time spent in the conservative GL synchronization wait.
    pub sync_us: u64,
    /// Total import time, including Vulkan allocation and wgpu wrapping.
    pub total_us: u64,
}

/// Snapshot of the GL framebuffer state used by the import blit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GlFramebufferStateSnapshot {
    /// Currently bound read framebuffer object, where zero means default FBO.
    pub read_framebuffer: i32,
    /// Currently bound draw framebuffer object, where zero means default FBO.
    pub draw_framebuffer: i32,
    /// Active read color buffer selector.
    pub read_buffer: i32,
    /// Active first draw color buffer selector.
    pub draw_buffer0: i32,
    /// Completeness status for `READ_FRAMEBUFFER`.
    pub read_status: u32,
    /// Completeness status for `DRAW_FRAMEBUFFER`.
    pub draw_status: u32,
    /// Active GL viewport as x, y, width, height.
    pub viewport: [i32; 4],
}

impl fmt::Debug for GlFramebufferStateSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GlFramebufferStateSnapshot")
            .field("read_framebuffer", &self.read_framebuffer)
            .field("draw_framebuffer", &self.draw_framebuffer)
            .field("read_buffer", &format_args!("0x{:04x}", self.read_buffer))
            .field("draw_buffer0", &format_args!("0x{:04x}", self.draw_buffer0))
            .field("read_status", &format_args!("0x{:04x}", self.read_status))
            .field("draw_status", &format_args!("0x{:04x}", self.draw_status))
            .field("viewport", &self.viewport)
            .finish()
    }
}

/// Slot-level state for a reusable Linux GPU import pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxGlImporterStateSnapshot {
    /// Number of import slots in the pool.
    pub slot_count: usize,
    /// Slots currently waiting on a GL fence.
    pub pending_slots: usize,
    /// Completed slots that can be returned to wgpu consumers.
    pub completed_slots: usize,
    /// Slots not pending and not retained by downstream GPU work.
    pub available_slots: usize,
    /// Age in milliseconds of the oldest pending fence.
    pub oldest_pending_age_ms: Option<u64>,
    /// Monotonic storage id for the newest completed slot.
    pub latest_completed_storage_id: Option<u64>,
}

/// Reusable importer for repeatedly copying one GL framebuffer into wgpu.
///
/// Creating exportable Vulkan images and importing their memory into GL is
/// expensive enough to cause visible stalls when done every frame. This pool
/// performs that setup once per size and then only issues the GL blit/sync on
/// the hot path.
pub struct LinuxGlFramebufferImporter {
    gl_external_memory: GlExternalMemoryFunctions,
    descriptor: LinuxGlFramebufferImportDescriptor,
    slot_pool: ImportedFrameSlotPool,
}

impl LinuxGlFramebufferImporter {
    /// Creates a pooled importer using GL entry points loaded from the process.
    pub fn new_from_process(
        device: &wgpu::Device,
        gl: &glow::Context,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<Self> {
        Self::new(
            device,
            gl,
            GlExternalMemoryFunctions::load_from_process()?,
            descriptor,
            DEFAULT_IMPORT_SLOT_COUNT,
        )
    }

    /// Creates a pooled importer using the supplied GL external-memory entry points.
    pub fn new(
        device: &wgpu::Device,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        slot_count: usize,
    ) -> Result<Self> {
        let descriptor = LinuxGlFramebufferImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        // SAFETY: the guard is held while raw Vulkan images are created and
        // wrapped; raw handles only escape inside wgpu's drop callbacks.
        let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
            .ok_or(LinuxGpuInteropError::MissingWgpuVulkanDevice)?;
        let slot_pool = ImportedFrameSlotPool::create(
            device,
            &hal_device,
            gl,
            gl_external_memory,
            descriptor,
            slot_count,
        )?;

        Ok(Self {
            gl_external_memory,
            descriptor,
            slot_pool,
        })
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> LinuxGlFramebufferImportDescriptor {
        self.descriptor
    }

    /// Returns the number of reusable import slots.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slot_pool.slot_count()
    }

    /// Returns a cheap snapshot of pooled import slot state.
    #[must_use]
    pub fn state_snapshot(&self) -> LinuxGlImporterStateSnapshot {
        self.slot_pool.state_snapshot()
    }

    /// Captures the exact read/draw framebuffer state used by the import blit.
    #[must_use]
    pub fn framebuffer_state_for_blit(
        &self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> GlFramebufferStateSnapshot {
        self.slot_pool
            .framebuffer_state_for_blit(gl, source_framebuffer)
    }

    /// Imports the current GL framebuffer contents into a pooled wgpu texture.
    pub fn import_framebuffer(
        &mut self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> Result<ImportedEffectFrame> {
        self.slot_pool.import_framebuffer(
            gl,
            self.gl_external_memory,
            source_framebuffer,
            self.descriptor,
        )
    }

    /// Queues a GL framebuffer blit and returns the newest completed import.
    ///
    /// The first call blocks until its own blit completes. Later calls return a
    /// previously completed slot while the current blit finishes on the GPU.
    pub fn import_framebuffer_pipelined(
        &mut self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> Result<ImportedEffectFrame> {
        self.slot_pool.import_framebuffer_pipelined(
            gl,
            self.gl_external_memory,
            source_framebuffer,
            self.descriptor,
        )
    }

    /// Deletes pooled GL objects while their context is current.
    pub fn destroy_gl_resources(&mut self, gl: &glow::Context) {
        self.slot_pool
            .destroy_gl_resources(gl, self.gl_external_memory);
    }
}

/// Imports a GL framebuffer into the supplied wgpu device without CPU readback.
///
/// The caller must pass a GL context that is current on the calling thread.
/// `source_framebuffer` is read from the current context and vertically flipped
/// into a Vulkan-backed texture that belongs to `device`.
pub fn import_gl_framebuffer_to_wgpu(
    device: &wgpu::Device,
    gl: &glow::Context,
    gl_external_memory: GlExternalMemoryFunctions,
    source_framebuffer: GlFramebufferSource,
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> Result<ImportedEffectFrame> {
    let total_start = Instant::now();
    let descriptor = LinuxGlFramebufferImportDescriptor::new(
        descriptor.width,
        descriptor.height,
        descriptor.format,
    )?;

    // SAFETY: the guard is held for the duration of raw Vulkan image creation
    // and wrapping; raw handles are not retained outside the returned texture.
    let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
        .ok_or(LinuxGpuInteropError::MissingWgpuVulkanDevice)?;

    let mut image = ExportableVulkanImage::create(&hal_device, descriptor)?;
    import_framebuffer_once(SingleFrameImport {
        device,
        hal_device: &hal_device,
        gl,
        gl_external_memory,
        source_framebuffer,
        descriptor,
        image: &mut image,
        total_start,
    })
}

/// Imports a GL framebuffer using libGL/libEGL to resolve extension functions.
pub fn import_gl_framebuffer_to_wgpu_from_process(
    device: &wgpu::Device,
    gl: &glow::Context,
    source_framebuffer: GlFramebufferSource,
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> Result<ImportedEffectFrame> {
    import_gl_framebuffer_to_wgpu(
        device,
        gl,
        GlExternalMemoryFunctions::load_from_process()?,
        source_framebuffer,
        descriptor,
    )
}

/// Source framebuffer selection for the GL import blit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlFramebufferSource {
    /// Use the framebuffer currently bound to `READ_FRAMEBUFFER`.
    CurrentRead,
    /// Bind and read the supplied framebuffer. `None` means the default FBO.
    Framebuffer(Option<glow::NativeFramebuffer>),
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}
