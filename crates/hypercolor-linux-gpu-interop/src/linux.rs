use std::ffi::{CStr, c_void};
use std::num::NonZeroU32;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use ash::{khr, vk};
use glow::HasContext;
use thiserror::Error;

const GL_DEDICATED_MEMORY_OBJECT_EXT: u32 = 0x9581;
const GL_HANDLE_TYPE_OPAQUE_FD_EXT: u32 = 0x9586;
const DEFAULT_IMPORT_SLOT_COUNT: usize = 8;
static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS_GL_LOADER: OnceLock<Option<ProcessGlLoader>> = OnceLock::new();

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
            slots.push(ImportedFrameSlot::create(
                device,
                &hal_device,
                gl,
                gl_external_memory,
                descriptor,
            )?);
        }

        Ok(Self {
            gl_external_memory,
            descriptor,
            slots,
            next_slot: 0,
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

    /// Imports the current GL framebuffer contents into a pooled wgpu texture.
    pub fn import_framebuffer(
        &mut self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> Result<ImportedEffectFrame> {
        let total_start = Instant::now();
        let gl_external_memory = self.gl_external_memory;
        let descriptor = self.descriptor;
        let slot = self.next_slot()?;
        let mut timings =
            slot.blit_from_framebuffer(gl, gl_external_memory, source_framebuffer, descriptor)?;
        timings.total_us = elapsed_micros(total_start);
        Ok(slot.frame(descriptor, timings))
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

type GlCreateMemoryObjectsExt = unsafe extern "system" fn(i32, *mut u32);
type GlMemoryObjectParameterivExt = unsafe extern "system" fn(u32, u32, *const i32);
type GlImportMemoryFdExt = unsafe extern "system" fn(u32, u64, u32, i32);
type GlTexStorageMem2DExt = unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64);
type GlDeleteMemoryObjectsExt = unsafe extern "system" fn(i32, *const u32);

/// Loaded GL entry points for `GL_EXT_memory_object_fd`.
#[derive(Clone, Copy)]
pub struct GlExternalMemoryFunctions {
    /// `glCreateMemoryObjectsEXT`
    pub create_memory_objects_ext: GlCreateMemoryObjectsExt,
    /// `glMemoryObjectParameterivEXT`
    pub memory_object_parameteriv_ext: GlMemoryObjectParameterivExt,
    /// `glImportMemoryFdEXT`
    pub import_memory_fd_ext: GlImportMemoryFdExt,
    /// `glTexStorageMem2DEXT`
    pub tex_storage_mem_2d_ext: GlTexStorageMem2DExt,
    /// `glDeleteMemoryObjectsEXT`
    pub delete_memory_objects_ext: GlDeleteMemoryObjectsExt,
}

impl GlExternalMemoryFunctions {
    /// Loads required entry points from a current GL context.
    ///
    /// The callback should return the address for the supplied symbol name, or
    /// a null pointer when the symbol is unavailable.
    pub fn load_from(mut get_proc_address: impl FnMut(&CStr) -> *const c_void) -> Result<Self> {
        let create_memory_objects_ext = get_required_proc_address(
            c"glCreateMemoryObjectsEXT",
            "glCreateMemoryObjectsEXT",
            &mut get_proc_address,
        )?;
        let memory_object_parameteriv_ext = get_required_proc_address(
            c"glMemoryObjectParameterivEXT",
            "glMemoryObjectParameterivEXT",
            &mut get_proc_address,
        )?;
        let import_memory_fd_ext = get_required_proc_address(
            c"glImportMemoryFdEXT",
            "glImportMemoryFdEXT",
            &mut get_proc_address,
        )?;
        let tex_storage_mem_2d_ext = get_required_proc_address(
            c"glTexStorageMem2DEXT",
            "glTexStorageMem2DEXT",
            &mut get_proc_address,
        )?;
        let delete_memory_objects_ext = get_required_proc_address(
            c"glDeleteMemoryObjectsEXT",
            "glDeleteMemoryObjectsEXT",
            &mut get_proc_address,
        )?;

        Ok(Self {
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            create_memory_objects_ext: unsafe {
                std::mem::transmute::<*const c_void, GlCreateMemoryObjectsExt>(
                    create_memory_objects_ext,
                )
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            memory_object_parameteriv_ext: unsafe {
                std::mem::transmute::<*const c_void, GlMemoryObjectParameterivExt>(
                    memory_object_parameteriv_ext,
                )
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object_fd.
            import_memory_fd_ext: unsafe {
                std::mem::transmute::<*const c_void, GlImportMemoryFdExt>(import_memory_fd_ext)
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            tex_storage_mem_2d_ext: unsafe {
                std::mem::transmute::<*const c_void, GlTexStorageMem2DExt>(tex_storage_mem_2d_ext)
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            delete_memory_objects_ext: unsafe {
                std::mem::transmute::<*const c_void, GlDeleteMemoryObjectsExt>(
                    delete_memory_objects_ext,
                )
            },
        })
    }

    /// Loads required entry points from libGL/libEGL process loaders.
    pub fn load_from_process() -> Result<Self> {
        let loader = PROCESS_GL_LOADER
            .get_or_init(ProcessGlLoader::load)
            .ok_or(LinuxGpuInteropError::GlProcLoaderUnavailable)?;
        Self::load_from(|symbol| loader.lookup(symbol))
    }
}

/// Reports missing GL entry points for `GL_EXT_memory_object_fd`.
#[must_use]
pub fn missing_gl_external_memory_functions(
    mut get_proc_address: impl FnMut(&CStr) -> *const c_void,
) -> Vec<&'static str> {
    [
        (c"glCreateMemoryObjectsEXT", "glCreateMemoryObjectsEXT"),
        (
            c"glMemoryObjectParameterivEXT",
            "glMemoryObjectParameterivEXT",
        ),
        (c"glImportMemoryFdEXT", "glImportMemoryFdEXT"),
        (c"glTexStorageMem2DEXT", "glTexStorageMem2DEXT"),
        (c"glDeleteMemoryObjectsEXT", "glDeleteMemoryObjectsEXT"),
    ]
    .into_iter()
    .filter_map(|(symbol, name)| get_proc_address(symbol).is_null().then_some(name))
    .collect()
}

/// Reports missing `GL_EXT_memory_object_fd` entry points through libGL/libEGL.
#[must_use]
pub fn missing_process_gl_external_memory_functions() -> Vec<&'static str> {
    let Some(loader) = PROCESS_GL_LOADER.get_or_init(ProcessGlLoader::load) else {
        return vec!["libGL.so.1", "libEGL.so.1"];
    };
    missing_gl_external_memory_functions(|symbol| loader.lookup(symbol))
}

/// Verifies that the active wgpu device can expose Vulkan external memory FDs.
pub fn check_wgpu_vulkan_external_memory_fd(device: &wgpu::Device) -> Result<()> {
    // SAFETY: this only borrows the underlying HAL device long enough to inspect
    // immutable device-extension metadata; no raw handles are retained.
    let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
        .ok_or(LinuxGpuInteropError::MissingWgpuVulkanDevice)?;

    if hal_device
        .enabled_device_extensions()
        .contains(&khr::external_memory_fd::NAME)
    {
        Ok(())
    } else {
        Err(LinuxGpuInteropError::MissingVulkanDeviceExtension(
            "VK_KHR_external_memory_fd",
        ))
    }
}

/// Collects the import capability status for the active GL and wgpu contexts.
#[must_use]
pub fn report_linux_gpu_import_capabilities(
    device: &wgpu::Device,
    get_proc_address: impl FnMut(&CStr) -> *const c_void,
) -> LinuxGpuImportCapabilities {
    let vulkan_result = check_wgpu_vulkan_external_memory_fd(device);
    let missing_gl = missing_gl_external_memory_functions(get_proc_address);

    let mut missing = Vec::new();
    let wgpu_vulkan_device = !matches!(
        vulkan_result,
        Err(LinuxGpuInteropError::MissingWgpuVulkanDevice)
    );
    let vulkan_external_memory_fd = vulkan_result.is_ok();

    if !wgpu_vulkan_device {
        missing.push("wgpu_vulkan_device");
    }
    if wgpu_vulkan_device && !vulkan_external_memory_fd {
        missing.push("VK_KHR_external_memory_fd");
    }
    missing.extend(missing_gl.iter().copied());

    LinuxGpuImportCapabilities {
        wgpu_vulkan_device,
        vulkan_external_memory_fd,
        gl_memory_object_fd: missing_gl.is_empty(),
        missing,
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

fn get_required_proc_address(
    symbol: &'static CStr,
    name: &'static str,
    get_proc_address: &mut impl FnMut(&CStr) -> *const c_void,
) -> Result<*const c_void> {
    let ptr = get_proc_address(symbol);
    if ptr.is_null() {
        Err(LinuxGpuInteropError::MissingGlFunction(name))
    } else {
        Ok(ptr)
    }
}

struct ExportableVulkanImage {
    raw_device: ash::Device,
    image: Option<vk::Image>,
    memory: Option<vk::DeviceMemory>,
    memory_fd: OwnedFd,
    allocation_size: u64,
}

struct ImportedFrameSlot {
    gl_binding: GlImportedImageBinding,
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
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
        })
    }

    fn is_available(&self) -> bool {
        Arc::strong_count(&self.texture) == 1 && Arc::strong_count(&self.view) == 1
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

    fn frame(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
        timings: ImportedFrameTimings,
    ) -> ImportedEffectFrame {
        ImportedEffectFrame {
            width: descriptor.width,
            height: descriptor.height,
            format: descriptor.format,
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            texture: Arc::clone(&self.texture),
            view: Arc::clone(&self.view),
            timings,
        }
    }

    fn destroy_gl_resources(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
    ) {
        self.gl_binding.destroy(gl, gl_external_memory);
    }
}

struct ImportedWgpuTexture {
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
}

impl ExportableVulkanImage {
    fn create(
        hal_device: &wgpu_hal::vulkan::Device,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<Self> {
        if !hal_device
            .enabled_device_extensions()
            .contains(&khr::external_memory_fd::NAME)
        {
            return Err(LinuxGpuInteropError::MissingVulkanDeviceExtension(
                "VK_KHR_external_memory_fd",
            ));
        }

        let raw_device = hal_device.raw_device().clone();
        let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(descriptor.format.vk_format())
            .extent(vk::Extent3D {
                width: descriptor.width,
                height: descriptor.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::COLOR_ATTACHMENT,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external_memory);

        // SAFETY: image_info is fully initialized and uses the active wgpu
        // Vulkan device; allocation ownership is transferred to Self.
        let image = unsafe { raw_device.create_image(&image_info, None) }.map_err(|result| {
            LinuxGpuInteropError::Vulkan {
                operation: "create_image",
                result,
            }
        })?;

        // SAFETY: image was created on raw_device and is valid until cleanup.
        let requirements = unsafe { raw_device.get_image_memory_requirements(image) };
        let memory_type_index = find_memory_type_index(
            hal_device.shared_instance().raw_instance(),
            hal_device.raw_physical_device(),
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;

        let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut export_info = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index)
            .push_next(&mut dedicated_info)
            .push_next(&mut export_info);

        // SAFETY: memory_info requests a dedicated exportable allocation for
        // the image created above on the same device.
        let memory =
            unsafe { raw_device.allocate_memory(&memory_info, None) }.map_err(|result| {
                // SAFETY: image was created on raw_device and has not been bound or
                // handed to wgpu yet.
                unsafe { raw_device.destroy_image(image, None) };
                LinuxGpuInteropError::Vulkan {
                    operation: "allocate_memory",
                    result,
                }
            })?;

        // SAFETY: image and memory were created on the same device, and the
        // allocation satisfies the image memory requirements.
        if let Err(result) = unsafe { raw_device.bind_image_memory(image, memory, 0) } {
            // SAFETY: both resources were created on raw_device and have not
            // been handed to wgpu yet.
            unsafe {
                raw_device.free_memory(memory, None);
                raw_device.destroy_image(image, None);
            }
            return Err(LinuxGpuInteropError::Vulkan {
                operation: "bind_image_memory",
                result,
            });
        }

        let external_memory_fd = khr::external_memory_fd::Device::new(
            hal_device.shared_instance().raw_instance(),
            &raw_device,
        );
        let fd_info = vk::MemoryGetFdInfoKHR::default()
            .memory(memory)
            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        // SAFETY: memory was allocated with ExportMemoryAllocateInfo for
        // OPAQUE_FD, and the returned descriptor becomes owned by OwnedFd.
        let memory_fd =
            unsafe { external_memory_fd.get_memory_fd(&fd_info) }.map_err(|result| {
                // SAFETY: both resources were created on raw_device and have not
                // been handed to wgpu yet.
                unsafe {
                    raw_device.free_memory(memory, None);
                    raw_device.destroy_image(image, None);
                }
                LinuxGpuInteropError::Vulkan {
                    operation: "get_memory_fd",
                    result,
                }
            })?;
        // SAFETY: Vulkan returned a newly owned POSIX file descriptor.
        let memory_fd = unsafe { OwnedFd::from_raw_fd(memory_fd) };

        Ok(Self {
            raw_device,
            image: Some(image),
            memory: Some(memory),
            memory_fd,
            allocation_size: requirements.size,
        })
    }

    fn wrap_as_wgpu_texture(
        &mut self,
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedWgpuTexture> {
        let image = self.image.take().ok_or(LinuxGpuInteropError::Vulkan {
            operation: "wrap_image",
            result: vk::Result::ERROR_UNKNOWN,
        })?;
        let memory = self.memory.take().ok_or(LinuxGpuInteropError::Vulkan {
            operation: "wrap_memory",
            result: vk::Result::ERROR_UNKNOWN,
        })?;

        let raw_device = self.raw_device.clone();
        let drop_callback: wgpu_hal::DropCallback = Box::new(move || {
            // SAFETY: ownership of image and memory moved into this callback,
            // which wgpu-hal invokes after all GPU uses are complete.
            unsafe {
                raw_device.destroy_image(image, None);
                raw_device.free_memory(memory, None);
            }
        });

        let hal_desc = hal_texture_descriptor(descriptor);
        // SAFETY: image was created from hal_device's raw Vulkan device, bound
        // to exportable memory, and initialized by the completed GL blit.
        let hal_texture = unsafe {
            hal_device.texture_from_raw(
                image,
                &hal_desc,
                Some(drop_callback),
                wgpu_hal::vulkan::TextureMemory::External,
            )
        };
        let wgpu_desc = wgpu_texture_descriptor(descriptor);
        // SAFETY: hal_texture was created from this wgpu device's Vulkan HAL
        // and matches wgpu_desc.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(hal_texture, &wgpu_desc)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(ImportedWgpuTexture {
            texture: Arc::new(texture),
            view: Arc::new(view),
        })
    }
}

impl Drop for ExportableVulkanImage {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            // SAFETY: memory was created on raw_device and was not handed to
            // wgpu because it is still present here.
            unsafe { self.raw_device.free_memory(memory, None) };
        }
        if let Some(image) = self.image.take() {
            // SAFETY: image was created on raw_device and was not handed to
            // wgpu because it is still present here.
            unsafe { self.raw_device.destroy_image(image, None) };
        }
    }
}

fn find_memory_type_index(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32> {
    // SAFETY: physical_device comes from instance through the active wgpu HAL.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };

    memory_properties
        .memory_types_as_slice()
        .iter()
        .enumerate()
        .find_map(|(index, memory_type)| {
            if index >= u32::BITS as usize {
                return None;
            }
            let type_supported = (type_bits & (1_u32 << index)) != 0;
            let flags_supported = memory_type.property_flags.contains(flags);
            (type_supported && flags_supported).then_some(index as u32)
        })
        .ok_or(LinuxGpuInteropError::MemoryTypeUnavailable)
}

struct GlImportedImageBinding {
    memory_object: u32,
    texture: Option<glow::NativeTexture>,
    framebuffer: Option<glow::NativeFramebuffer>,
}

impl GlImportedImageBinding {
    fn create(
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        memory_fd: i32,
        allocation_size: u64,
    ) -> Result<Self> {
        let bindings = capture_gl_bindings(gl);
        clear_gl_errors(gl);
        let mut memory_object = 0;
        let mut texture = None;
        let mut framebuffer = None;
        let result = (|| {
            // SAFETY: the function pointer was loaded from the current GL context,
            // and memory_object points to valid writable storage for one object.
            unsafe { (gl_external_memory.create_memory_objects_ext)(1, &mut memory_object) };
            check_gl_error(gl, "glCreateMemoryObjectsEXT")?;

            let dedicated = i32::from(glow::TRUE);
            unsafe {
                (gl_external_memory.memory_object_parameteriv_ext)(
                    memory_object,
                    GL_DEDICATED_MEMORY_OBJECT_EXT,
                    &dedicated,
                );
            }
            check_gl_error(gl, "glMemoryObjectParameterivEXT")?;

            let gl_fd = duplicate_fd(memory_fd)?;
            // SAFETY: gl_fd is a duplicate of the Vulkan memory FD; GL consumes
            // the duplicate while Rust keeps ownership of the original FD.
            unsafe {
                (gl_external_memory.import_memory_fd_ext)(
                    memory_object,
                    allocation_size,
                    GL_HANDLE_TYPE_OPAQUE_FD_EXT,
                    gl_fd,
                );
            }
            check_gl_error(gl, "glImportMemoryFdEXT")?;

            // SAFETY: a current GL context is required by the public import API.
            let imported_texture = unsafe { gl.create_texture() }.map_err(|message| {
                LinuxGpuInteropError::GlCreateResource {
                    resource: "texture",
                    message,
                }
            })?;
            texture = Some(imported_texture);

            // SAFETY: imported_texture belongs to this context and is valid until
            // cleanup at the end of this function.
            unsafe { gl.bind_texture(glow::TEXTURE_2D, texture) };
            // SAFETY: memory_object names external memory imported above, and the
            // texture bound to TEXTURE_2D receives storage from that memory.
            unsafe {
                (gl_external_memory.tex_storage_mem_2d_ext)(
                    glow::TEXTURE_2D,
                    1,
                    descriptor.format.gl_internal_format(),
                    descriptor.width_i32(),
                    descriptor.height_i32(),
                    memory_object,
                    0,
                );
            }
            check_gl_error(gl, "glTexStorageMem2DEXT")?;

            // SAFETY: a current GL context is required by the public import API.
            let draw_framebuffer = unsafe { gl.create_framebuffer() }.map_err(|message| {
                LinuxGpuInteropError::GlCreateResource {
                    resource: "framebuffer",
                    message,
                }
            })?;
            framebuffer = Some(draw_framebuffer);

            // SAFETY: framebuffer and texture are valid GL objects owned by this
            // context for the duration of the blit.
            unsafe {
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, framebuffer);
                gl.framebuffer_texture_2d(
                    glow::DRAW_FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    texture,
                    0,
                );
            }
            // SAFETY: DRAW_FRAMEBUFFER is bound above.
            let framebuffer_status = unsafe { gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) };
            if framebuffer_status != glow::FRAMEBUFFER_COMPLETE {
                return Err(LinuxGpuInteropError::GlFramebufferIncomplete {
                    status: framebuffer_status,
                });
            }

            Ok(Self {
                memory_object,
                texture,
                framebuffer,
            })
        })();

        let result = match result {
            Ok(binding) => Ok(binding),
            Err(error) => {
                cleanup_gl_import_resources(
                    gl,
                    gl_external_memory,
                    framebuffer,
                    texture,
                    memory_object,
                );
                Err(error)
            }
        };
        restore_gl_bindings(gl, bindings);
        result
    }

    fn blit_from_framebuffer(
        &self,
        gl: &glow::Context,
        _gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedFrameTimings> {
        let bindings = capture_gl_bindings(gl);
        clear_gl_errors(gl);
        let result = (|| {
            let blit_start = Instant::now();
            // SAFETY: source_framebuffer is supplied by the current GL context;
            // the destination framebuffer is complete and backed by external memory.
            unsafe {
                if let GlFramebufferSource::Framebuffer(source_framebuffer) = source_framebuffer {
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, source_framebuffer);
                }
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.framebuffer);
                gl.blit_framebuffer(
                    0,
                    0,
                    descriptor.width_i32(),
                    descriptor.height_i32(),
                    0,
                    descriptor.height_i32(),
                    descriptor.width_i32(),
                    0,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
                gl.flush();
            }
            let blit_us = elapsed_micros(blit_start);

            let sync_start = Instant::now();
            // SAFETY: the current GL context owns the queued blit and `finish`
            // only blocks until that context has completed prior commands.
            unsafe {
                gl.finish();
            }
            let sync_us = elapsed_micros(sync_start);
            check_gl_error(gl, "glBlitFramebuffer")?;

            Ok(ImportedFrameTimings {
                blit_us,
                sync_us,
                total_us: 0,
            })
        })();

        restore_gl_bindings(gl, bindings);
        result
    }

    fn destroy(&mut self, gl: &glow::Context, gl_external_memory: GlExternalMemoryFunctions) {
        cleanup_gl_import_resources(
            gl,
            gl_external_memory,
            self.framebuffer.take(),
            self.texture.take(),
            std::mem::take(&mut self.memory_object),
        );
    }
}

fn duplicate_fd(fd: i32) -> Result<i32> {
    // SAFETY: dup does not take ownership of fd and returns a new descriptor or
    // -1 with errno set.
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate >= 0 {
        Ok(duplicate)
    } else {
        Err(LinuxGpuInteropError::DuplicateFdFailed {
            errno: std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default(),
        })
    }
}

#[derive(Clone, Copy)]
struct ProcessGlLoader {
    lib_gl: Option<usize>,
    lib_egl: Option<usize>,
    glx_get_proc_address: Option<GlxGetProcAddress>,
    egl_get_proc_address: Option<EglGetProcAddress>,
}

type GlxGetProcAddress = unsafe extern "C" fn(*const u8) -> *const c_void;
type EglGetProcAddress = unsafe extern "C" fn(*const i8) -> *const c_void;

impl ProcessGlLoader {
    fn load() -> Option<Self> {
        let lib_gl = open_library(c"libGL.so.1").or_else(|| open_library(c"libGL.so"));
        let lib_egl = open_library(c"libEGL.so.1").or_else(|| open_library(c"libEGL.so"));
        let glx_get_proc_address = lib_gl.and_then(|handle| {
            lookup_raw_symbol(handle, c"glXGetProcAddressARB")
                .or_else(|| lookup_raw_symbol(handle, c"glXGetProcAddress"))
                .map(|ptr| {
                    // SAFETY: symbol names are the GLX resolver entry points
                    // with the standard C ABI.
                    unsafe { std::mem::transmute::<*const c_void, GlxGetProcAddress>(ptr) }
                })
        });
        let egl_get_proc_address = lib_egl.and_then(|handle| {
            lookup_raw_symbol(handle, c"eglGetProcAddress").map(|ptr| {
                // SAFETY: symbol name is the EGL resolver entry point with the
                // standard C ABI.
                unsafe { std::mem::transmute::<*const c_void, EglGetProcAddress>(ptr) }
            })
        });

        (lib_gl.is_some()
            || lib_egl.is_some()
            || glx_get_proc_address.is_some()
            || egl_get_proc_address.is_some())
        .then_some(Self {
            lib_gl,
            lib_egl,
            glx_get_proc_address,
            egl_get_proc_address,
        })
    }

    fn lookup(&self, symbol: &CStr) -> *const c_void {
        self.lib_gl
            .and_then(|handle| lookup_raw_symbol(handle, symbol))
            .or_else(|| {
                self.lib_egl
                    .and_then(|handle| lookup_raw_symbol(handle, symbol))
            })
            .or_else(|| {
                self.glx_get_proc_address.and_then(|get_proc_address| {
                    // SAFETY: GLX resolver accepts a NUL-terminated GL symbol
                    // name and returns null when unavailable.
                    let ptr = unsafe { get_proc_address(symbol.as_ptr().cast::<u8>()) };
                    (!ptr.is_null()).then_some(ptr)
                })
            })
            .or_else(|| {
                self.egl_get_proc_address.and_then(|get_proc_address| {
                    // SAFETY: EGL resolver accepts a NUL-terminated GL symbol
                    // name and returns null when unavailable.
                    let ptr = unsafe { get_proc_address(symbol.as_ptr()) };
                    (!ptr.is_null()).then_some(ptr)
                })
            })
            .unwrap_or(std::ptr::null())
    }
}

fn open_library(name: &CStr) -> Option<usize> {
    // SAFETY: dlopen receives a static NUL-terminated library name. Handles are
    // intentionally retained for process lifetime so resolved function pointers
    // remain valid while Servo's GL context is alive.
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    (!handle.is_null()).then_some(handle as usize)
}

fn lookup_raw_symbol(handle: usize, symbol: &CStr) -> Option<*const c_void> {
    // SAFETY: handle came from dlopen and symbol is NUL-terminated.
    let ptr = unsafe { libc::dlsym(handle as *mut c_void, symbol.as_ptr()) };
    (!ptr.is_null()).then_some(ptr.cast_const())
}

fn cleanup_gl_import_resources(
    gl: &glow::Context,
    gl_external_memory: GlExternalMemoryFunctions,
    framebuffer: Option<glow::NativeFramebuffer>,
    texture: Option<glow::NativeTexture>,
    memory_object: u32,
) {
    // SAFETY: the objects were created in this context when present. Deleting
    // zero memory objects is skipped because zero is not a valid object name.
    unsafe {
        if let Some(framebuffer) = framebuffer {
            gl.delete_framebuffer(framebuffer);
        }
        if let Some(texture) = texture {
            gl.delete_texture(texture);
        }
        if memory_object != 0 {
            (gl_external_memory.delete_memory_objects_ext)(1, &memory_object);
        }
    }
}

#[derive(Clone, Copy)]
struct GlBindingSnapshot {
    read_framebuffer: Option<glow::NativeFramebuffer>,
    draw_framebuffer: Option<glow::NativeFramebuffer>,
    texture_2d: Option<glow::NativeTexture>,
}

fn capture_gl_bindings(gl: &glow::Context) -> GlBindingSnapshot {
    // SAFETY: these queries read binding state from the current GL context.
    let read_framebuffer =
        unsafe { framebuffer_from_binding(gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING)) };
    // SAFETY: these queries read binding state from the current GL context.
    let draw_framebuffer =
        unsafe { framebuffer_from_binding(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING)) };
    // SAFETY: these queries read binding state from the current GL context.
    let texture_2d =
        unsafe { texture_from_binding(gl.get_parameter_i32(glow::TEXTURE_BINDING_2D)) };

    GlBindingSnapshot {
        read_framebuffer,
        draw_framebuffer,
        texture_2d,
    }
}

fn restore_gl_bindings(gl: &glow::Context, bindings: GlBindingSnapshot) {
    // SAFETY: the captured object names came from this same current GL context.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, bindings.read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, bindings.draw_framebuffer);
        gl.bind_texture(glow::TEXTURE_2D, bindings.texture_2d);
    }
}

fn framebuffer_from_binding(binding: i32) -> Option<glow::NativeFramebuffer> {
    u32::try_from(binding)
        .ok()
        .and_then(NonZeroU32::new)
        .map(glow::NativeFramebuffer)
}

fn texture_from_binding(binding: i32) -> Option<glow::NativeTexture> {
    u32::try_from(binding)
        .ok()
        .and_then(NonZeroU32::new)
        .map(glow::NativeTexture)
}

fn clear_gl_errors(gl: &glow::Context) {
    for _ in 0..16 {
        // SAFETY: this reads and clears the current GL error flag.
        if unsafe { gl.get_error() } == glow::NO_ERROR {
            break;
        }
    }
}

fn check_gl_error(gl: &glow::Context, operation: &'static str) -> Result<()> {
    // SAFETY: this reads the current GL error flag after a GL operation.
    let code = unsafe { gl.get_error() };
    if code == glow::NO_ERROR {
        Ok(())
    } else {
        Err(LinuxGpuInteropError::GlOperation { operation, code })
    }
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn wgpu_texture_descriptor(
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("hypercolor-linux-servo-import"),
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
    descriptor: LinuxGlFramebufferImportDescriptor,
) -> wgpu_hal::TextureDescriptor<'static> {
    wgpu_hal::TextureDescriptor {
        label: Some("hypercolor-linux-servo-import"),
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
