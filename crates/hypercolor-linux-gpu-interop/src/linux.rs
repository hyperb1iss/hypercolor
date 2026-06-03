use std::fmt;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use ash::vk;
use thiserror::Error;

const DEFAULT_IMPORT_SLOT_COUNT: usize = 8;
const MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION: u32 = 16;
static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

mod capabilities;
mod fence;
mod gl_external_memory;
mod loader;
mod vulkan_export;

pub use capabilities::{
    LinuxGpuImportCapabilities, check_wgpu_vulkan_external_memory_fd,
    missing_gl_external_memory_functions, missing_process_gl_external_memory_functions,
    report_linux_gpu_import_capabilities,
};
use fence::{GlFenceStatus, delete_gl_fence, poll_gl_fence, wait_for_gl_fence_completion};
pub use gl_external_memory::GlExternalMemoryFunctions;
use gl_external_memory::{GlImportedImageBinding, clear_gl_errors, current_gl_framebuffer_state};
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
    slots: Vec<ImportedFrameSlot>,
    next_slot: usize,
    recoverable_poll_errors_without_completion: u32,
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
        let slot_count = slot_count.max(1);

        // SAFETY: the guard is held while raw Vulkan images are created and
        // wrapped; raw handles only escape inside wgpu's drop callbacks.
        let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
            .ok_or(LinuxGpuInteropError::MissingWgpuVulkanDevice)?;
        let mut slots = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            match ImportedFrameSlot::create(device, &hal_device, gl, gl_external_memory, descriptor)
            {
                Ok(slot) => slots.push(slot),
                Err(error) => {
                    for slot in &mut slots {
                        slot.destroy_gl_resources(gl, gl_external_memory);
                    }
                    return Err(error);
                }
            }
        }

        Ok(Self {
            gl_external_memory,
            descriptor,
            slots,
            next_slot: 0,
            recoverable_poll_errors_without_completion: 0,
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
        self.slots.len()
    }

    /// Returns a cheap snapshot of pooled import slot state.
    #[must_use]
    pub fn state_snapshot(&self) -> LinuxGlImporterStateSnapshot {
        let oldest_pending_age_ms = self
            .slots
            .iter()
            .filter_map(ImportedFrameSlot::pending_age_ms)
            .max();
        LinuxGlImporterStateSnapshot {
            slot_count: self.slots.len(),
            pending_slots: self
                .slots
                .iter()
                .filter(|slot| slot.pending.is_some())
                .count(),
            completed_slots: self
                .slots
                .iter()
                .filter(|slot| slot.completed.is_some())
                .count(),
            available_slots: self.slots.iter().filter(|slot| slot.is_available()).count(),
            oldest_pending_age_ms,
            latest_completed_storage_id: self.latest_completed_storage_id(),
        }
    }

    /// Captures the exact read/draw framebuffer state used by the import blit.
    #[must_use]
    pub fn framebuffer_state_for_blit(
        &self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> GlFramebufferStateSnapshot {
        self.slots.first().map_or_else(
            || current_gl_framebuffer_state(gl),
            |slot| slot.framebuffer_state_for_blit(gl, source_framebuffer),
        )
    }

    /// Imports the current GL framebuffer contents into a pooled wgpu texture.
    pub fn import_framebuffer(
        &mut self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> Result<ImportedEffectFrame> {
        let total_start = Instant::now();
        self.poll_pending_imports(gl)?;
        let gl_external_memory = self.gl_external_memory;
        let descriptor = self.descriptor;
        let slot = self.next_slot()?;
        let mut timings =
            slot.blit_from_framebuffer(gl, gl_external_memory, source_framebuffer, descriptor)?;
        timings.total_us = elapsed_micros(total_start);
        Ok(slot.frame(descriptor, timings))
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
        self.poll_pending_imports(gl)?;

        let issued_slot = if let Some(index) = self.next_pipelined_slot() {
            let total_start = Instant::now();
            let gl_external_memory = self.gl_external_memory;
            let descriptor = self.descriptor;
            let slot = &mut self.slots[index];
            let mut pending = slot.blit_from_framebuffer_pipelined(
                gl,
                gl_external_memory,
                source_framebuffer,
                descriptor,
            )?;
            pending.timings.total_us = elapsed_micros(total_start);
            slot.completed = None;
            slot.pending = Some(pending);
            self.next_slot = (index + 1) % self.slots.len();
            Some(index)
        } else {
            None
        };

        self.poll_pending_imports(gl)?;
        if let Some(frame) = self.latest_completed_frame() {
            return Ok(frame);
        }

        if let Some(index) = issued_slot {
            self.slots[index].wait_pending_import(gl)?;
            return self.slots[index].completed_frame(self.descriptor).ok_or(
                LinuxGpuInteropError::ImportSlotsExhausted {
                    slot_count: self.slots.len(),
                },
            );
        }

        Err(LinuxGpuInteropError::ImportSlotsExhausted {
            slot_count: self.slots.len(),
        })
    }

    /// Deletes pooled GL objects while their context is current.
    pub fn destroy_gl_resources(&mut self, gl: &glow::Context) {
        for slot in &mut self.slots {
            slot.destroy_gl_resources(gl, self.gl_external_memory);
        }
    }

    fn next_slot(&mut self) -> Result<&mut ImportedFrameSlot> {
        let slot_count = self.slots.len();
        let preferred = self.next_slot;
        let selected = (0..slot_count)
            .map(|offset| (preferred + offset) % slot_count)
            .find(|&index| self.slots[index].is_available())
            .ok_or(LinuxGpuInteropError::ImportSlotsExhausted { slot_count })?;
        self.next_slot = (selected + 1) % slot_count;
        Ok(&mut self.slots[selected])
    }

    fn next_pipelined_slot(&self) -> Option<usize> {
        let slot_count = self.slots.len();
        let preferred = self.next_slot;
        let latest_completed_index = self.latest_completed_index();
        let mut fallback = None;
        for index in (0..slot_count).map(|offset| (preferred + offset) % slot_count) {
            if !self.slots[index].is_available() {
                continue;
            }
            if Some(index) != latest_completed_index {
                return Some(index);
            }
            fallback = Some(index);
        }
        fallback
    }

    fn latest_completed_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.completed.map(|completed| (index, completed)))
            .max_by_key(|(_, completed)| completed.storage_id)
            .map(|(index, _)| index)
    }

    fn latest_completed_storage_id(&self) -> Option<u64> {
        self.slots
            .iter()
            .filter_map(|slot| slot.completed.map(|completed| completed.storage_id))
            .max()
    }

    fn latest_completed_frame(&self) -> Option<ImportedEffectFrame> {
        self.latest_completed_index()
            .and_then(|index| self.slots[index].completed_frame(self.descriptor))
    }

    fn poll_pending_imports(&mut self, gl: &glow::Context) -> Result<()> {
        let latest_before = self.latest_completed_storage_id();
        let mut first_error = None;
        for slot in &mut self.slots {
            if let Err(error) = slot.poll_pending_import(gl)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        let latest_after = self.latest_completed_storage_id();
        let completed_advanced = latest_after != latest_before && latest_after.is_some();
        if completed_advanced {
            self.recoverable_poll_errors_without_completion = 0;
        }
        if let Some(error) = first_error
            && poll_error_should_propagate(
                &error,
                latest_after,
                completed_advanced,
                &mut self.recoverable_poll_errors_without_completion,
            )
        {
            return Err(error);
        }
        Ok(())
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
    let mut slot = ImportedFrameSlot::create_from_image(
        device,
        &hal_device,
        gl,
        gl_external_memory,
        descriptor,
        &mut image,
    )?;
    let mut timings =
        slot.blit_from_framebuffer(gl, gl_external_memory, source_framebuffer, descriptor)?;
    timings.total_us = elapsed_micros(total_start);
    let frame = slot.frame(descriptor, timings);
    slot.destroy_gl_resources(gl, gl_external_memory);
    Ok(frame)
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

struct ImportedFrameSlot {
    gl_binding: GlImportedImageBinding,
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
    pending: Option<PendingImport>,
    completed: Option<CompletedImport>,
}

struct PendingImport {
    fence: glow::NativeFence,
    storage_id: u64,
    issued_at: Instant,
    timings: ImportedFrameTimings,
}

#[derive(Clone, Copy)]
struct CompletedImport {
    storage_id: u64,
    timings: ImportedFrameTimings,
}

impl ImportedFrameSlot {
    fn create(
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<Self> {
        let mut image = ExportableVulkanImage::create(hal_device, descriptor)?;
        Self::create_from_image(
            device,
            hal_device,
            gl,
            gl_external_memory,
            descriptor,
            &mut image,
        )
    }

    fn create_from_image(
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        image: &mut ExportableVulkanImage,
    ) -> Result<Self> {
        let gl_binding = GlImportedImageBinding::create(
            gl,
            gl_external_memory,
            descriptor,
            image.memory_fd.as_raw_fd(),
            image.allocation_size,
        )?;
        let texture = image.wrap_as_wgpu_texture(device, hal_device, descriptor)?;
        Ok(Self {
            gl_binding,
            texture: texture.texture,
            view: texture.view,
            pending: None,
            completed: None,
        })
    }

    fn is_available(&self) -> bool {
        self.pending.is_none()
            && Arc::strong_count(&self.texture) == 1
            && Arc::strong_count(&self.view) == 1
    }

    fn blit_from_framebuffer(
        &self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedFrameTimings> {
        self.gl_binding.blit_from_framebuffer(
            gl,
            gl_external_memory,
            source_framebuffer,
            descriptor,
        )
    }

    fn blit_from_framebuffer_pipelined(
        &self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<PendingImport> {
        let timings = self.gl_binding.blit_from_framebuffer_pipelined(
            gl,
            gl_external_memory,
            source_framebuffer,
            descriptor,
        )?;
        Ok(PendingImport {
            fence: timings.fence,
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            issued_at: Instant::now(),
            timings: timings.timings,
        })
    }

    fn framebuffer_state_for_blit(
        &self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> GlFramebufferStateSnapshot {
        self.gl_binding
            .framebuffer_state_for_blit(gl, source_framebuffer)
    }

    fn frame(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
        timings: ImportedFrameTimings,
    ) -> ImportedEffectFrame {
        self.frame_with_storage_id(
            descriptor,
            NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            timings,
        )
    }

    fn completed_frame(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Option<ImportedEffectFrame> {
        self.completed.map(|completed| {
            self.frame_with_storage_id(descriptor, completed.storage_id, completed.timings)
        })
    }

    fn frame_with_storage_id(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
        storage_id: u64,
        timings: ImportedFrameTimings,
    ) -> ImportedEffectFrame {
        ImportedEffectFrame {
            width: descriptor.width,
            height: descriptor.height,
            format: descriptor.format,
            storage_id,
            texture: Arc::clone(&self.texture),
            view: Arc::clone(&self.view),
            timings,
        }
    }

    fn poll_pending_import(&mut self, gl: &glow::Context) -> Result<()> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };

        let sync_start = Instant::now();
        let status = match poll_gl_fence(gl, pending.fence) {
            Ok(status) => status,
            Err(error) => {
                delete_gl_fence(gl, pending.fence);
                clear_gl_errors(gl);
                return Err(error);
            }
        };
        pending.timings.sync_us = pending
            .timings
            .sync_us
            .saturating_add(elapsed_micros(sync_start));

        match status {
            GlFenceStatus::Complete => {
                delete_gl_fence(gl, pending.fence);
                self.completed = Some(CompletedImport {
                    storage_id: pending.storage_id,
                    timings: pending.timings,
                });
            }
            GlFenceStatus::Pending => {
                self.pending = Some(pending);
            }
        }
        Ok(())
    }

    fn wait_pending_import(&mut self, gl: &glow::Context) -> Result<()> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        let sync_result = wait_for_gl_fence_completion(gl, pending.fence);
        delete_gl_fence(gl, pending.fence);
        let sync_us = sync_result?;
        pending.timings.sync_us = pending.timings.sync_us.saturating_add(sync_us);
        self.completed = Some(CompletedImport {
            storage_id: pending.storage_id,
            timings: pending.timings,
        });
        Ok(())
    }

    fn pending_age_ms(&self) -> Option<u64> {
        self.pending
            .as_ref()
            .map(|pending| elapsed_millis(pending.issued_at))
    }

    fn destroy_gl_resources(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
    ) {
        if let Some(pending) = self.pending.take() {
            delete_gl_fence(gl, pending.fence);
        }
        self.gl_binding.destroy(gl, gl_external_memory);
    }
}

fn is_recoverable_poll_error(error: &LinuxGpuInteropError) -> bool {
    matches!(
        error,
        LinuxGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code: glow::INVALID_OPERATION,
        }
    )
}

fn should_escalate_recoverable_poll_error(errors_without_completion: u32) -> bool {
    errors_without_completion > MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION
}

fn poll_error_should_propagate(
    error: &LinuxGpuInteropError,
    latest_completed_storage_id: Option<u64>,
    completed_advanced: bool,
    errors_without_completion: &mut u32,
) -> bool {
    if latest_completed_storage_id.is_none() || !is_recoverable_poll_error(error) {
        return true;
    }
    if completed_advanced {
        return false;
    }

    *errors_without_completion = errors_without_completion.saturating_add(1);
    should_escalate_recoverable_poll_error(*errors_without_completion)
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn elapsed_millis(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gl_client_wait_sync_poll_errors_are_recoverable() {
        assert!(is_recoverable_poll_error(
            &LinuxGpuInteropError::GlOperation {
                operation: "glClientWaitSync",
                code: glow::INVALID_OPERATION,
            }
        ));
        assert!(!is_recoverable_poll_error(
            &LinuxGpuInteropError::GlOperation {
                operation: "glBlitFramebuffer",
                code: glow::INVALID_OPERATION,
            }
        ));
        assert!(!is_recoverable_poll_error(
            &LinuxGpuInteropError::ImportSlotsExhausted { slot_count: 8 }
        ));
    }

    #[test]
    fn recoverable_poll_errors_escalate_after_streak_limit() {
        assert!(!should_escalate_recoverable_poll_error(
            MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION,
        ));
        assert!(should_escalate_recoverable_poll_error(
            MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION + 1,
        ));
    }

    #[test]
    fn poll_error_decision_tracks_progress_and_escalation() {
        let recoverable = LinuxGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code: glow::INVALID_OPERATION,
        };
        let non_recoverable = LinuxGpuInteropError::GlOperation {
            operation: "glBlitFramebuffer",
            code: glow::INVALID_OPERATION,
        };
        let mut streak = 0;

        assert!(poll_error_should_propagate(
            &recoverable,
            None,
            false,
            &mut streak,
        ));
        assert_eq!(streak, 0);

        assert!(!poll_error_should_propagate(
            &recoverable,
            Some(41),
            true,
            &mut streak,
        ));
        assert_eq!(streak, 0);

        for _ in 0..MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION {
            assert!(!poll_error_should_propagate(
                &recoverable,
                Some(41),
                false,
                &mut streak,
            ));
        }
        assert_eq!(streak, MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION);
        assert!(poll_error_should_propagate(
            &recoverable,
            Some(41),
            false,
            &mut streak,
        ));

        streak = 0;
        assert!(poll_error_should_propagate(
            &non_recoverable,
            Some(41),
            false,
            &mut streak,
        ));
        assert_eq!(streak, 0);
    }
}
