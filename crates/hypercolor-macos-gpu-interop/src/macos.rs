use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hypercolor_gpu_frame::{GpuFrameImportError, GpuFrameImportFallbackReason};
use objc2::{runtime::NSObjectProtocol, sel};
#[cfg(feature = "screen-capture")]
use objc2_core_foundation::CFRetained;
use objc2_core_foundation::{
    CFDictionary, CFIndex, CFNumber, CFString, kCFAllocatorDefault, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
#[cfg(feature = "screen-capture")]
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    kCVMetalTextureStorageMode, kCVMetalTextureUsage, kCVReturnSuccess,
};
use objc2_io_surface::{
    IOSurfaceLockOptions, IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow,
    kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
};
use objc2_metal::{
    MTL4CommitOptions, MTLCreateSystemDefaultDevice, MTLDevice, MTLGPUFamily, MTLPixelFormat,
    MTLResource, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use thiserror::Error;

use crate::{
    FrameOrigin, ImportedEffectFrame, ImportedFrameAllocationId, ImportedFrameFormat,
    ImportedFrameLease, ImportedFrameTimings,
};

const BGRA_BYTES_PER_PIXEL: u32 = 4;
const PIXEL_FORMAT_BGRA: i32 = u32::from_be_bytes(*b"BGRA") as i32;
/// Maximum cached wgpu wraps before the importer cache resets. The Servo
/// publish ring uses 3 IOSurfaces, so steady state stays well under this.
const MAX_CACHED_WRAPS: usize = 8;
/// Fallback content versions for fixture imports that have no producer
/// generation (see [`MacosIosurfaceImporter::import_iosurface_for_test`]).
static NEXT_ALLOCATION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONTENT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[cfg(feature = "screen-capture")]
type CoreVideoMetalTexturePlane = (
    objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn MTLTexture>>,
    CFRetained<CVMetalTexture>,
    Instant,
);

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

/// Identity and family facts for the system-default Metal device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosSystemMetalDeviceQualification {
    /// Human-readable device name emitted by native runner qualification.
    pub device_name: String,
    /// Global IORegistry identity of the system-default device.
    pub registry_id: u64,
    /// Whether the system-default device reports an Apple GPU family.
    pub apple_family: bool,
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
    device: &wgpu::Device,
) -> Result<MacosMetal4CapabilityProbe> {
    let (metal_registry_id, _) = metal_device_import_contract(device)?;
    // SAFETY: the HAL device is borrowed only for immediate capability queries
    // and never outlives the wgpu device.
    let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Metal>() }
        .ok_or(MacosGpuInteropError::MissingWgpuMetalDevice)?;
    let raw_device = hal_device.raw_device();
    let metal4_family = raw_device.supportsFamily(MTLGPUFamily::Metal4);
    let command_queue = raw_device.respondsToSelector(sel!(newMTL4CommandQueue));
    let commit_feedback = metal4_family
        && command_queue
        && raw_device.newMTL4CommandQueue().is_some_and(|queue| {
            queue.respondsToSelector(sel!(commit:count:options:))
                && MTL4CommitOptions::new().respondsToSelector(sel!(addFeedbackHandler:))
        });
    Ok(MacosMetal4CapabilityProbe {
        metal_registry_id,
        metal4_family,
        command_allocator: raw_device.respondsToSelector(sel!(newCommandAllocator)),
        command_queue,
        command_buffer: raw_device.respondsToSelector(sel!(newCommandBuffer)),
        argument_table: raw_device.respondsToSelector(sel!(newArgumentTableWithDescriptor:error:)),
        residency_set: raw_device.respondsToSelector(sel!(newResidencySetWithDescriptor:error:)),
        shared_event: raw_device.respondsToSelector(sel!(newSharedEvent)),
        commit_feedback,
    })
}

/// Query the system-default Metal device for native runner qualification.
pub fn qualify_macos_system_default_metal_device() -> Result<MacosSystemMetalDeviceQualification> {
    let device =
        MTLCreateSystemDefaultDevice().ok_or(MacosGpuInteropError::MissingSystemMetalDevice)?;
    Ok(MacosSystemMetalDeviceQualification {
        device_name: device.name().to_string(),
        registry_id: device.registryID(),
        apple_family: device.supportsFamily(MTLGPUFamily::Apple1),
    })
}

/// Errors raised while preparing or importing macOS GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacosGpuInteropError {
    /// `MTLCreateSystemDefaultDevice` did not return a usable device.
    #[error("MTLCreateSystemDefaultDevice returned no Metal device")]
    MissingSystemMetalDevice,

    /// The active wgpu device is not backed by Metal.
    #[error("wgpu device is not backed by the Metal HAL")]
    MissingWgpuMetalDevice,

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

    /// IOSurface creation failed.
    #[error("failed to create IOSurface")]
    IosurfaceCreateFailed,

    /// IOSurface locking or unlocking failed.
    #[error("IOSurface {operation} failed with kern_return_t {code}")]
    IosurfaceLock {
        /// Failed IOSurface operation.
        operation: &'static str,
        /// Kernel return code.
        code: libc::kern_return_t,
    },

    /// The IOSurface does not match the import descriptor.
    #[error(
        "IOSurface shape mismatch: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    IosurfaceShapeMismatch {
        /// Expected width in pixels.
        expected_width: u32,
        /// Expected height in pixels.
        expected_height: u32,
        /// Actual width in pixels.
        actual_width: usize,
        /// Actual height in pixels.
        actual_height: usize,
    },

    /// IOSurface allocation size cannot be represented by the cache identity.
    #[error("IOSurface allocation size {actual_bytes} exceeds u64")]
    IosurfaceAllocationSizeOverflow {
        /// Allocation size reported by IOSurface.
        actual_bytes: usize,
    },

    /// The supplied pixel buffer does not match the IOSurface dimensions.
    #[error("pixel buffer length mismatch: expected {expected_len} bytes, got {actual_len}")]
    PixelBufferSizeMismatch {
        /// Expected byte length.
        expected_len: usize,
        /// Actual byte length.
        actual_len: usize,
    },

    /// A macOS Servo hardware context operation failed.
    #[error("macOS Servo hardware context {operation} failed: {message}")]
    ServoContext {
        /// Failed context operation.
        operation: &'static str,
        /// Context error details.
        message: String,
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
        /// GL or CGL error code.
        code: u32,
    },

    /// The IOSurface-backed GL framebuffer was incomplete.
    #[error("IOSurface-backed framebuffer is incomplete: 0x{status:04x}")]
    GlFramebufferIncomplete {
        /// GL framebuffer status.
        status: u32,
    },

    /// The macOS Servo hardware context has no bound Surfman surface.
    #[error("macOS Servo hardware context has no bound Surfman surface")]
    MissingServoSurface,

    /// Waiting for an IOSurface ring slot blit fence timed out.
    ///
    /// Transient: the GPU has not finished the publish blit yet; retry on a
    /// later frame.
    #[error("timed out waiting for an IOSurface blit fence")]
    IosurfaceFenceTimeout,

    /// The IOSurface pixel format does not match the expected import format.
    #[error("IOSurface pixel format mismatch: expected 0x{expected:08x}, got 0x{actual:08x}")]
    IosurfacePixelFormatMismatch {
        /// Expected IOSurface pixel format.
        expected: u32,
        /// Actual IOSurface pixel format.
        actual: u32,
    },

    /// The requested IOSurface plane does not exist.
    #[error("IOSurface plane {requested} is unavailable; surface exposes {plane_count} planes")]
    IosurfacePlaneUnavailable {
        /// Requested plane index.
        requested: usize,
        /// Number of planes exposed by the IOSurface.
        plane_count: usize,
    },

    /// Metal could not create a texture from the IOSurface.
    #[error("Metal failed to create texture from IOSurface")]
    MetalTextureCreateFailed,

    /// The import used another physical Metal device.
    #[error("Metal registry identity mismatch: expected {expected}, got {actual}")]
    MetalRegistryIdMismatch {
        /// Registry identity captured when the importer was created.
        expected: u64,
        /// Registry identity observed during import.
        actual: u64,
    },

    /// Metal created a texture with another storage mode.
    #[error("Metal texture storage mode mismatch: expected {expected:?}, got {actual:?}")]
    MetalStorageModeMismatch {
        /// Family-selected storage mode.
        expected: MacosMetalStorageMode,
        /// Created texture storage mode.
        actual: MacosMetalStorageMode,
    },

    /// Metal returned a texture that names another IOSurface.
    #[error("Metal texture IOSurface mismatch: expected {expected}, got {actual}")]
    MetalIosurfaceIdentityMismatch {
        /// Source IOSurface identity.
        expected: u32,
        /// Created texture IOSurface identity.
        actual: u32,
    },

    /// Metal returned a texture that names another IOSurface plane.
    #[error("Metal texture IOSurface plane mismatch: expected {expected}, got {actual}")]
    MetalIosurfacePlaneMismatch {
        /// Requested IOSurface plane.
        expected: usize,
        /// Created texture IOSurface plane.
        actual: usize,
    },

    /// Metal returned a texture with another extent.
    #[error(
        "Metal texture extent mismatch: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
    )]
    MetalTextureExtentMismatch {
        /// Requested texture width.
        expected_width: u32,
        /// Requested texture height.
        expected_height: u32,
        /// Created texture width.
        actual_width: usize,
        /// Created texture height.
        actual_height: usize,
    },

    /// Metal returned a texture with another pixel format.
    #[error("Metal texture pixel format mismatch: expected {expected}, got {actual}")]
    MetalPixelFormatMismatch {
        /// Requested Metal pixel format.
        expected: usize,
        /// Created Metal pixel format.
        actual: usize,
    },

    /// Metal reported a storage mode outside the supported import contract.
    #[error("unsupported Metal texture storage mode {0}")]
    UnsupportedMetalStorageMode(usize),

    /// Core Video could not create the Metal texture cache.
    #[cfg(feature = "screen-capture")]
    #[error("Core Video Metal texture cache creation failed with CVReturn {0}")]
    CoreVideoTextureCacheCreateFailed(i32),

    /// Core Video could not create a texture wrapper for a pixel-buffer plane.
    #[cfg(feature = "screen-capture")]
    #[error("Core Video Metal texture creation failed with CVReturn {0}")]
    CoreVideoTextureCreateFailed(i32),
}

impl GpuFrameImportError for MacosGpuInteropError {
    fn fallback_reason(&self) -> GpuFrameImportFallbackReason {
        match self {
            Self::MissingWgpuMetalDevice => GpuFrameImportFallbackReason::MissingWgpuMetalDevice,
            Self::InvalidDimensions { .. }
            | Self::IosurfaceShapeMismatch { .. }
            | Self::PixelBufferSizeMismatch { .. } => {
                GpuFrameImportFallbackReason::InvalidDimensions
            }
            Self::ServoContext { .. } | Self::MissingServoSurface | Self::IosurfaceFenceTimeout => {
                GpuFrameImportFallbackReason::MissingMacosServoSurface
            }
            Self::GlCreateResource { .. } => GpuFrameImportFallbackReason::GlResource,
            Self::GlOperation { .. } => GpuFrameImportFallbackReason::GlOperation,
            Self::GlFramebufferIncomplete { .. } => {
                GpuFrameImportFallbackReason::GlFramebufferIncomplete
            }
            Self::IosurfacePixelFormatMismatch { .. } => {
                GpuFrameImportFallbackReason::IosurfacePixelFormatMismatch
            }
            Self::MetalTextureCreateFailed => {
                GpuFrameImportFallbackReason::MetalTextureCreateFailed
            }
            _ => GpuFrameImportFallbackReason::Other,
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

impl MacosMetalStorageMode {
    const fn native(self) -> MTLStorageMode {
        match self {
            Self::Shared => MTLStorageMode::Shared,
            Self::Managed => MTLStorageMode::Managed,
        }
    }

    fn from_native(mode: MTLStorageMode) -> Result<Self> {
        if mode == MTLStorageMode::Shared {
            Ok(Self::Shared)
        } else if mode == MTLStorageMode::Managed {
            Ok(Self::Managed)
        } else {
            Err(MacosGpuInteropError::UnsupportedMetalStorageMode(mode.0))
        }
    }
}

const fn metal_format(format: ImportedFrameFormat) -> Option<MTLPixelFormat> {
    match format {
        ImportedFrameFormat::Bgra8Unorm => Some(MTLPixelFormat::BGRA8Unorm),
        ImportedFrameFormat::Rgba16Float => Some(MTLPixelFormat::RGBA16Float),
        ImportedFrameFormat::R8Unorm => Some(MTLPixelFormat::R8Unorm),
        ImportedFrameFormat::Rg8Unorm => Some(MTLPixelFormat::RG8Unorm),
        ImportedFrameFormat::R16Unorm => Some(MTLPixelFormat::R16Unorm),
        ImportedFrameFormat::Rg16Unorm => Some(MTLPixelFormat::RG16Unorm),
        _ => None,
    }
}

const fn is_macos_import_format(format: ImportedFrameFormat) -> bool {
    matches!(
        format,
        ImportedFrameFormat::Bgra8Unorm
            | ImportedFrameFormat::Rgba16Float
            | ImportedFrameFormat::R8Unorm
            | ImportedFrameFormat::Rg8Unorm
            | ImportedFrameFormat::R16Unorm
            | ImportedFrameFormat::Rg16Unorm
    )
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
        } else if !is_macos_import_format(format) {
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

/// Cached wgpu texture/view pair wrapping one IOSurface identity.
struct CachedIosurfaceWrap {
    allocation_id: ImportedFrameAllocationId,
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
    capture_owner: Option<MacosCaptureCacheOwner>,
}

#[derive(Clone)]
pub(crate) struct MacosCaptureCacheOwner {
    _owner: Arc<dyn Send + Sync>,
}

impl MacosCaptureCacheOwner {
    pub(crate) fn new<T>(owner: Arc<T>) -> Self
    where
        T: Send + Sync + 'static,
    {
        Self { _owner: owner }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct IosurfaceWrapKey {
    capture_session_generation: u64,
    resource_generation: u64,
    surface_id: u32,
    plane: usize,
    width: u32,
    height: u32,
    bytes_per_row: usize,
    source_pixel_format: u32,
    allocation_bytes: u64,
    format: ImportedFrameFormat,
    storage_mode: MacosMetalStorageMode,
    metal_registry_id: u64,
}

/// Reusable importer for wrapping IOSurfaces as wgpu textures.
///
/// Wraps are cached per IOSurface identity, so re-importing a ring slot on
/// the frame hot path reuses the existing Metal/wgpu texture instead of
/// recreating it.
pub struct MacosIosurfaceImporter {
    descriptor: MacosIosurfaceImportDescriptor,
    storage_mode: MacosMetalStorageMode,
    metal_registry_id: u64,
    wraps: HashMap<IosurfaceWrapKey, CachedIosurfaceWrap>,
}

impl MacosIosurfaceImporter {
    /// Creates an importer for one IOSurface shape.
    pub fn new(device: &wgpu::Device, descriptor: MacosIosurfaceImportDescriptor) -> Result<Self> {
        let descriptor = MacosIosurfaceImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        let (metal_registry_id, storage_mode) = metal_device_import_contract(device)?;
        Ok(Self {
            descriptor,
            storage_mode,
            metal_registry_id,
            wraps: HashMap::new(),
        })
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

    /// Number of IOSurface wraps currently cached.
    #[must_use]
    pub fn cached_wrap_count(&self) -> usize {
        self.wraps.len()
    }

    /// Imports an IOSurface into the supplied wgpu device.
    ///
    /// `content_generation` is the producer's monotonic content version.
    /// Repeated imports of the same IOSurface retain one allocation identity.
    pub fn import_iosurface(
        &mut self,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
        content_generation: u64,
    ) -> Result<ImportedEffectFrame> {
        self.import_iosurface_scoped(
            device,
            iosurface,
            content_generation,
            FrameOrigin::BottomLeft,
            0,
            0,
        )
    }

    pub(crate) fn import_iosurface_scoped(
        &mut self,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
        content_generation: u64,
        origin: FrameOrigin,
        capture_session_generation: u64,
        resource_generation: u64,
    ) -> Result<ImportedEffectFrame> {
        self.import_iosurface_plane_scoped(
            device,
            iosurface,
            content_generation,
            origin,
            capture_session_generation,
            resource_generation,
            0,
            PIXEL_FORMAT_BGRA as u32,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import_iosurface_plane_scoped(
        &mut self,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
        content_generation: u64,
        origin: FrameOrigin,
        capture_session_generation: u64,
        resource_generation: u64,
        plane: usize,
        source_pixel_format: u32,
        capture_owner: Option<&MacosCaptureCacheOwner>,
    ) -> Result<ImportedEffectFrame> {
        validate_iosurface_shape(self.descriptor, iosurface, plane)?;
        validate_iosurface_format(iosurface, source_pixel_format)?;
        let (actual_registry_id, _) = metal_device_import_contract(device)?;
        if actual_registry_id != self.metal_registry_id {
            return Err(MacosGpuInteropError::MetalRegistryIdMismatch {
                expected: self.metal_registry_id,
                actual: actual_registry_id,
            });
        }

        let total_start = Instant::now();
        let surface_id = iosurface.id();
        let allocation_bytes = u64::try_from(iosurface.alloc_size()).map_err(|_| {
            MacosGpuInteropError::IosurfaceAllocationSizeOverflow {
                actual_bytes: iosurface.alloc_size(),
            }
        })?;
        let cache_key = IosurfaceWrapKey {
            capture_session_generation,
            resource_generation,
            surface_id,
            plane,
            width: self.descriptor.width,
            height: self.descriptor.height,
            bytes_per_row: iosurface_bytes_per_row(iosurface, plane),
            source_pixel_format,
            allocation_bytes,
            format: self.descriptor.format,
            storage_mode: self.storage_mode,
            metal_registry_id: self.metal_registry_id,
        };
        if let Some(cached) = self.wraps.get_mut(&cache_key) {
            if let Some(capture_owner) = capture_owner {
                cached.capture_owner = Some(capture_owner.clone());
            }
            return Ok(ImportedEffectFrame {
                width: self.descriptor.width,
                height: self.descriptor.height,
                format: self.descriptor.format,
                allocation_id: cached.allocation_id,
                content_generation,
                origin,
                texture: Arc::clone(&cached.texture),
                view: Arc::clone(&cached.view),
                lease: ImportedFrameLease::new(Arc::clone(&cached.texture)),
                timings: ImportedFrameTimings {
                    blit_us: None,
                    wrap_us: Some(0),
                    sync_us: None,
                    total_us: elapsed_micros(total_start),
                },
            });
        }

        let wrap_start = Instant::now();
        let metal_texture = {
            let hal_device = require_metal_device(device)?;
            let descriptor = metal_texture_descriptor(self.descriptor, self.storage_mode);
            hal_device
                .raw_device()
                .newTextureWithDescriptor_iosurface_plane(&descriptor, iosurface, plane)
                .ok_or(MacosGpuInteropError::MetalTextureCreateFailed)?
        };
        validate_metal_texture(
            &metal_texture,
            surface_id,
            plane,
            self.descriptor.width,
            self.descriptor.height,
            self.storage_mode,
            metal_format(self.descriptor.format).expect("validated macOS import format"),
        )?;
        let wrap_us = elapsed_micros(wrap_start);

        let wgpu_desc = wgpu_texture_descriptor(self.descriptor);
        let copy_size = wgpu_hal::CopyExtent {
            width: self.descriptor.width,
            height: self.descriptor.height,
            depth: 1,
        };
        // SAFETY: metal_texture was created from the same Metal device behind
        // this wgpu device, matches wgpu_desc, and retains IOSurface-backed
        // storage for the wrapped texture lifetime.
        let hal_texture = unsafe {
            wgpu_hal::metal::Device::texture_from_raw(
                metal_texture,
                self.descriptor.format.wgpu_format(),
                MTLTextureType::Type2D,
                1,
                1,
                copy_size,
            )
        };
        // SAFETY: hal_texture was created from this wgpu device's Metal HAL
        // and matches wgpu_desc.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Metal>(hal_texture, &wgpu_desc)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let texture = Arc::new(texture);
        let view = Arc::new(view);
        if self.wraps.len() >= MAX_CACHED_WRAPS {
            self.wraps.clear();
        }
        self.wraps.insert(
            cache_key,
            CachedIosurfaceWrap {
                allocation_id: ImportedFrameAllocationId::new(
                    NEXT_ALLOCATION_ID.fetch_add(1, Ordering::Relaxed),
                ),
                texture: Arc::clone(&texture),
                view: Arc::clone(&view),
                capture_owner: capture_owner.cloned(),
            },
        );

        let cached = self
            .wraps
            .get(&cache_key)
            .expect("new IOSurface wrap remains cached through frame construction");
        Ok(ImportedEffectFrame {
            width: self.descriptor.width,
            height: self.descriptor.height,
            format: self.descriptor.format,
            allocation_id: cached.allocation_id,
            content_generation,
            origin,
            texture,
            view,
            lease: ImportedFrameLease::new(Arc::clone(&cached.texture)),
            timings: ImportedFrameTimings {
                blit_us: None,
                wrap_us: Some(wrap_us),
                sync_us: None,
                total_us: elapsed_micros(total_start),
            },
        })
    }

    /// Imports an IOSurface fixture that has no producer content generation.
    ///
    /// Allocates a fresh content version from a process-global counter, so
    /// every call reports changed contents. Intended for tests and
    /// compatibility probes built around [`create_bgra_iosurface`].
    pub fn import_iosurface_for_test(
        &mut self,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
    ) -> Result<ImportedEffectFrame> {
        let content_generation = NEXT_CONTENT_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.import_iosurface(device, iosurface, content_generation)
    }
}

/// Creates a BGRA IOSurface fixture for import tests and compatibility probes.
pub fn create_bgra_iosurface(
    width: u32,
    height: u32,
) -> Result<objc2_core_foundation::CFRetained<IOSurfaceRef>> {
    let descriptor =
        MacosIosurfaceImportDescriptor::new(width, height, ImportedFrameFormat::Bgra8Unorm)?;
    create_iosurface(descriptor)
}

/// Writes packed BGRA pixels into an IOSurface fixture.
pub fn write_bgra_pixels(
    iosurface: &IOSurfaceRef,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<()> {
    let expected_len = width as usize * height as usize * BGRA_BYTES_PER_PIXEL as usize;
    if pixels.len() != expected_len {
        return Err(MacosGpuInteropError::PixelBufferSizeMismatch {
            expected_len,
            actual_len: pixels.len(),
        });
    }
    validate_iosurface_shape(
        MacosIosurfaceImportDescriptor::new(width, height, ImportedFrameFormat::Bgra8Unorm)?,
        iosurface,
        0,
    )?;

    let lock = IosurfaceLockGuard::lock(iosurface)?;
    let bytes_per_row = iosurface.bytes_per_row();
    let row_len = width as usize * BGRA_BYTES_PER_PIXEL as usize;
    let base_address = iosurface.base_address().as_ptr().cast::<u8>();
    for (row_index, row_pixels) in pixels.chunks_exact(row_len).enumerate() {
        // SAFETY: the IOSurface is locked for CPU writes, base_address points
        // to at least bytes_per_row * height bytes, and row_len fits the row.
        unsafe {
            let target = base_address.add(row_index * bytes_per_row);
            std::ptr::copy_nonoverlapping(row_pixels.as_ptr(), target, row_len);
        }
    }
    lock.unlock()
}

pub(crate) fn create_iosurface(
    descriptor: MacosIosurfaceImportDescriptor,
) -> Result<objc2_core_foundation::CFRetained<IOSurfaceRef>> {
    let bytes_per_row = descriptor.width * BGRA_BYTES_PER_PIXEL;
    // SAFETY: these are framework-provided constant CFString references.
    let keys = unsafe {
        [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfaceBytesPerRow,
            kIOSurfacePixelFormat,
        ]
    };
    let values = [
        &*CFNumber::new_i32(descriptor.width as i32),
        &*CFNumber::new_i32(descriptor.height as i32),
        &*CFNumber::new_i32(BGRA_BYTES_PER_PIXEL as i32),
        &*CFNumber::new_i32(bytes_per_row as i32),
        &*CFNumber::new_i32(PIXEL_FORMAT_BGRA),
    ];
    let len = keys.len() as CFIndex;
    let keys: *const &CFString = keys.as_ptr();
    let keys: *mut *const c_void = keys.cast_mut().cast();
    let values: *const &CFNumber = values.as_ptr();
    let values: *mut *const c_void = values.cast_mut().cast();

    // SAFETY: keys and values are CF types, and the dictionary retains them
    // using the standard CF callbacks before this function returns.
    let properties = unsafe {
        CFDictionary::new(
            kCFAllocatorDefault,
            keys,
            values,
            len,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    }
    .ok_or(MacosGpuInteropError::IosurfaceCreateFailed)?;

    // SAFETY: properties contains the IOSurface keys and CFNumber values
    // required by IOSurfaceCreate.
    unsafe { IOSurfaceRef::new(&properties) }.ok_or(MacosGpuInteropError::IosurfaceCreateFailed)
}

fn metal_texture_descriptor(
    descriptor: MacosIosurfaceImportDescriptor,
    storage_mode: MacosMetalStorageMode,
) -> objc2::rc::Retained<MTLTextureDescriptor> {
    // SAFETY: descriptor dimensions are validated by
    // MacosIosurfaceImportDescriptor::new.
    let texture_descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            metal_format(descriptor.format).expect("validated macOS import format"),
            descriptor.width as usize,
            descriptor.height as usize,
            false,
        )
    };
    texture_descriptor.setTextureType(MTLTextureType::Type2D);
    texture_descriptor.setUsage(MTLTextureUsage::ShaderRead);
    texture_descriptor.setStorageMode(storage_mode.native());
    texture_descriptor
}

fn wgpu_texture_descriptor(
    descriptor: MacosIosurfaceImportDescriptor,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("hypercolor-macos-iosurface-import"),
        size: wgpu::Extent3d {
            width: descriptor.width,
            height: descriptor.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: descriptor.format.wgpu_format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    }
}

fn validate_iosurface_shape(
    descriptor: MacosIosurfaceImportDescriptor,
    iosurface: &IOSurfaceRef,
    plane: usize,
) -> Result<()> {
    validate_iosurface_plane_extent(descriptor.width, descriptor.height, iosurface, plane)
}

fn validate_iosurface_plane_extent(
    expected_width: u32,
    expected_height: u32,
    iosurface: &IOSurfaceRef,
    plane: usize,
) -> Result<()> {
    let plane_count = iosurface.plane_count();
    if (plane_count == 0 && plane != 0) || (plane_count != 0 && plane >= plane_count) {
        return Err(MacosGpuInteropError::IosurfacePlaneUnavailable {
            requested: plane,
            plane_count,
        });
    }
    let (actual_width, actual_height) = if plane_count == 0 {
        (iosurface.width(), iosurface.height())
    } else {
        (
            iosurface.width_of_plane(plane),
            iosurface.height_of_plane(plane),
        )
    };
    if actual_width == expected_width as usize && actual_height == expected_height as usize {
        Ok(())
    } else {
        Err(MacosGpuInteropError::IosurfaceShapeMismatch {
            expected_width,
            expected_height,
            actual_width,
            actual_height,
        })
    }
}

fn validate_iosurface_format(iosurface: &IOSurfaceRef, expected: u32) -> Result<()> {
    let actual = iosurface.pixel_format();
    if actual == expected {
        Ok(())
    } else {
        Err(MacosGpuInteropError::IosurfacePixelFormatMismatch { expected, actual })
    }
}

fn validate_metal_texture(
    texture: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>,
    expected_surface_id: u32,
    expected_plane: usize,
    expected_width: u32,
    expected_height: u32,
    expected_storage_mode: MacosMetalStorageMode,
    expected_pixel_format: MTLPixelFormat,
) -> Result<()> {
    let actual_storage_mode = MacosMetalStorageMode::from_native(texture.storageMode())?;
    if actual_storage_mode != expected_storage_mode {
        return Err(MacosGpuInteropError::MetalStorageModeMismatch {
            expected: expected_storage_mode,
            actual: actual_storage_mode,
        });
    }
    let actual_surface_id = texture
        .iosurface()
        .ok_or(MacosGpuInteropError::MetalTextureCreateFailed)?
        .id();
    if actual_surface_id != expected_surface_id {
        return Err(MacosGpuInteropError::MetalIosurfaceIdentityMismatch {
            expected: expected_surface_id,
            actual: actual_surface_id,
        });
    }
    let actual_plane = texture.iosurfacePlane();
    if actual_plane != expected_plane {
        return Err(MacosGpuInteropError::MetalIosurfacePlaneMismatch {
            expected: expected_plane,
            actual: actual_plane,
        });
    }
    let actual_width = texture.width();
    let actual_height = texture.height();
    if actual_width != expected_width as usize || actual_height != expected_height as usize {
        return Err(MacosGpuInteropError::MetalTextureExtentMismatch {
            expected_width,
            expected_height,
            actual_width,
            actual_height,
        });
    }
    let actual_pixel_format = texture.pixelFormat();
    if actual_pixel_format != expected_pixel_format {
        return Err(MacosGpuInteropError::MetalPixelFormatMismatch {
            expected: expected_pixel_format.0,
            actual: actual_pixel_format.0,
        });
    }
    Ok(())
}

fn iosurface_bytes_per_row(iosurface: &IOSurfaceRef, plane: usize) -> usize {
    if iosurface.plane_count() == 0 {
        iosurface.bytes_per_row()
    } else {
        iosurface.bytes_per_row_of_plane(plane)
    }
}

pub(crate) fn metal_device_import_contract(
    device: &wgpu::Device,
) -> Result<(u64, MacosMetalStorageMode)> {
    let hal_device = require_metal_device(device)?;
    let raw_device = hal_device.raw_device();
    let storage_mode = if raw_device.supportsFamily(MTLGPUFamily::Apple1) {
        MacosMetalStorageMode::Shared
    } else {
        MacosMetalStorageMode::Managed
    };
    Ok((raw_device.registryID(), storage_mode))
}

fn require_metal_device(
    device: &wgpu::Device,
) -> Result<impl std::ops::Deref<Target = wgpu_hal::metal::Device> + '_> {
    // SAFETY: we only inspect whether this wgpu device is backed by Metal and
    // borrow the HAL device for the duration of the immediate import call.
    unsafe { device.as_hal::<wgpu_hal::api::Metal>() }
        .ok_or(MacosGpuInteropError::MissingWgpuMetalDevice)
}

#[cfg(feature = "screen-capture")]
pub(crate) fn create_core_video_texture_cache(
    device: &wgpu::Device,
    storage_mode: MacosMetalStorageMode,
) -> Result<objc2_core_foundation::CFRetained<CVMetalTextureCache>> {
    let hal_device = require_metal_device(device)?;
    let usage = CFNumber::new_i64(MTLTextureUsage::ShaderRead.bits() as i64);
    let storage_mode = CFNumber::new_i64(storage_mode.native().0 as i64);
    // SAFETY: these are framework-provided constant CFString references.
    let texture_attribute_keys = unsafe { [kCVMetalTextureUsage, kCVMetalTextureStorageMode] };
    let texture_attributes = CFDictionary::<CFString, CFNumber>::from_slices(
        &texture_attribute_keys,
        &[&usage, &storage_mode],
    );
    let mut raw_cache = std::ptr::null_mut();
    // SAFETY: the output pointer is valid, the Metal device outlives this call,
    // and the retained dictionary contains documented numeric Metal values.
    let result = unsafe {
        CVMetalTextureCache::create(
            None,
            None,
            hal_device.raw_device(),
            Some(texture_attributes.as_ref()),
            std::ptr::NonNull::from(&mut raw_cache),
        )
    };
    if result != kCVReturnSuccess {
        return Err(MacosGpuInteropError::CoreVideoTextureCacheCreateFailed(
            result,
        ));
    }
    let raw_cache = std::ptr::NonNull::new(raw_cache).ok_or(
        MacosGpuInteropError::CoreVideoTextureCacheCreateFailed(result),
    )?;
    // SAFETY: Core Video returned the created cache at +1 ownership.
    Ok(unsafe { objc2_core_foundation::CFRetained::from_raw(raw_cache) })
}

#[cfg(feature = "screen-capture")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_core_video_pixel_buffer_plane(
    device: &wgpu::Device,
    cache: &CVMetalTextureCache,
    pixel_buffer: &CVPixelBuffer,
    descriptor: MacosIosurfaceImportDescriptor,
    plane: usize,
    expected_surface_id: u32,
    expected_storage_mode: MacosMetalStorageMode,
    content_generation: u64,
) -> Result<(
    ImportedEffectFrame,
    objc2_core_foundation::CFRetained<CVMetalTexture>,
)> {
    let (metal_texture, wrapper, total_start) = import_core_video_metal_texture_plane(
        cache,
        pixel_buffer,
        descriptor.width,
        descriptor.height,
        plane,
        metal_format(descriptor.format).expect("validated macOS import format"),
        expected_surface_id,
        expected_storage_mode,
    )?;
    let wrap_us = elapsed_micros(total_start);
    let imported = wrap_metal_texture(
        device,
        metal_texture,
        descriptor,
        content_generation,
        wrap_us,
        total_start,
    );
    Ok((imported, wrapper))
}

#[cfg(feature = "screen-capture")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_core_video_metal_texture_plane(
    cache: &CVMetalTextureCache,
    pixel_buffer: &CVPixelBuffer,
    width: u32,
    height: u32,
    plane: usize,
    pixel_format: MTLPixelFormat,
    expected_surface_id: u32,
    expected_storage_mode: MacosMetalStorageMode,
) -> Result<CoreVideoMetalTexturePlane> {
    let total_start = Instant::now();
    let mut raw_wrapper = std::ptr::null_mut();
    // SAFETY: the output pointer is valid, the pixel buffer remains retained
    // by the capture owner, and the descriptor matches the validated plane.
    let result = unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            cache,
            pixel_buffer,
            None,
            pixel_format,
            width as usize,
            height as usize,
            plane,
            std::ptr::NonNull::from(&mut raw_wrapper),
        )
    };
    if result != kCVReturnSuccess {
        return Err(MacosGpuInteropError::CoreVideoTextureCreateFailed(result));
    }
    let raw_wrapper = std::ptr::NonNull::new(raw_wrapper)
        .ok_or(MacosGpuInteropError::CoreVideoTextureCreateFailed(result))?;
    // SAFETY: Core Video returned the created texture wrapper at +1 ownership.
    let wrapper = unsafe { objc2_core_foundation::CFRetained::from_raw(raw_wrapper) };
    let metal_texture =
        CVMetalTextureGetTexture(&wrapper).ok_or(MacosGpuInteropError::MetalTextureCreateFailed)?;
    validate_metal_texture(
        &metal_texture,
        expected_surface_id,
        plane,
        width,
        height,
        expected_storage_mode,
        pixel_format,
    )?;
    Ok((metal_texture, wrapper, total_start))
}

#[cfg(feature = "screen-capture")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_iosurface_metal_texture_plane(
    device: &wgpu::Device,
    iosurface: &IOSurfaceRef,
    width: u32,
    height: u32,
    plane: usize,
    source_pixel_format: u32,
    pixel_format: MTLPixelFormat,
    expected_storage_mode: MacosMetalStorageMode,
) -> Result<objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>>> {
    validate_iosurface_plane_extent(width, height, iosurface, plane)?;
    validate_iosurface_format(iosurface, source_pixel_format)?;
    let hal_device = require_metal_device(device)?;
    // SAFETY: the dimensions are validated by CapturePlaneImportDescriptor,
    // and the Metal pixel format is selected by the exact capture format.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            pixel_format,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setTextureType(MTLTextureType::Type2D);
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(expected_storage_mode.native());
    let texture = hal_device
        .raw_device()
        .newTextureWithDescriptor_iosurface_plane(&descriptor, iosurface, plane)
        .ok_or(MacosGpuInteropError::MetalTextureCreateFailed)?;
    validate_metal_texture(
        &texture,
        iosurface.id(),
        plane,
        width,
        height,
        expected_storage_mode,
        pixel_format,
    )?;
    Ok(texture)
}

#[cfg(feature = "screen-capture")]
fn wrap_metal_texture(
    device: &wgpu::Device,
    metal_texture: objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>>,
    descriptor: MacosIosurfaceImportDescriptor,
    content_generation: u64,
    wrap_us: u64,
    total_start: Instant,
) -> ImportedEffectFrame {
    let wgpu_desc = wgpu_texture_descriptor(descriptor);
    let copy_size = wgpu_hal::CopyExtent {
        width: descriptor.width,
        height: descriptor.height,
        depth: 1,
    };
    // SAFETY: the Metal texture came from the same device behind this wgpu
    // device, matches the descriptor, and remains retained by the wrapper.
    let hal_texture = unsafe {
        wgpu_hal::metal::Device::texture_from_raw(
            metal_texture,
            descriptor.format.wgpu_format(),
            MTLTextureType::Type2D,
            1,
            1,
            copy_size,
        )
    };
    // SAFETY: the HAL texture was created from this wgpu device and matches
    // the supplied descriptor.
    let texture =
        unsafe { device.create_texture_from_hal::<wgpu_hal::api::Metal>(hal_texture, &wgpu_desc) };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let texture = Arc::new(texture);
    ImportedEffectFrame {
        width: descriptor.width,
        height: descriptor.height,
        format: descriptor.format,
        allocation_id: ImportedFrameAllocationId::new(
            NEXT_ALLOCATION_ID.fetch_add(1, Ordering::Relaxed),
        ),
        content_generation,
        origin: FrameOrigin::TopLeft,
        texture: Arc::clone(&texture),
        view: Arc::new(view),
        lease: ImportedFrameLease::new(texture),
        timings: ImportedFrameTimings {
            blit_us: None,
            wrap_us: Some(wrap_us),
            sync_us: None,
            total_us: elapsed_micros(total_start),
        },
    }
}

fn lock_iosurface(iosurface: &IOSurfaceRef) -> Result<()> {
    // SAFETY: null seed is allowed by IOSurfaceLock.
    let code = unsafe { iosurface.lock(IOSurfaceLockOptions::empty(), std::ptr::null_mut()) };
    if code == 0 {
        Ok(())
    } else {
        Err(MacosGpuInteropError::IosurfaceLock {
            operation: "lock",
            code,
        })
    }
}

fn unlock_iosurface(iosurface: &IOSurfaceRef) -> Result<()> {
    // SAFETY: null seed is allowed by IOSurfaceUnlock.
    let code = unsafe { iosurface.unlock(IOSurfaceLockOptions::empty(), std::ptr::null_mut()) };
    if code == 0 {
        Ok(())
    } else {
        Err(MacosGpuInteropError::IosurfaceLock {
            operation: "unlock",
            code,
        })
    }
}

struct IosurfaceLockGuard<'a> {
    iosurface: &'a IOSurfaceRef,
    locked: bool,
}

impl<'a> IosurfaceLockGuard<'a> {
    fn lock(iosurface: &'a IOSurfaceRef) -> Result<Self> {
        lock_iosurface(iosurface)?;
        Ok(Self {
            iosurface,
            locked: true,
        })
    }

    fn unlock(mut self) -> Result<()> {
        unlock_iosurface(self.iosurface)?;
        self.locked = false;
        Ok(())
    }
}

impl Drop for IosurfaceLockGuard<'_> {
    fn drop(&mut self) {
        if self.locked {
            let _ = unlock_iosurface(self.iosurface);
        }
    }
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_key(source_pixel_format: u32, allocation_bytes: u64) -> IosurfaceWrapKey {
        IosurfaceWrapKey {
            capture_session_generation: 1,
            resource_generation: 2,
            surface_id: 3,
            plane: 0,
            width: 4,
            height: 5,
            bytes_per_row: 16,
            source_pixel_format,
            allocation_bytes,
            format: ImportedFrameFormat::Bgra8Unorm,
            storage_mode: MacosMetalStorageMode::Shared,
            metal_registry_id: 6,
        }
    }

    #[test]
    fn iosurface_wrap_key_retains_source_format_and_allocation_identity() {
        let baseline = wrap_key(u32::from_be_bytes(*b"420v"), 1_024);
        assert_ne!(baseline, wrap_key(u32::from_be_bytes(*b"420f"), 1_024));
        assert_ne!(baseline, wrap_key(u32::from_be_bytes(*b"420v"), 2_048));
    }

    #[test]
    fn capture_cache_owner_lives_until_explicit_cache_clear() {
        let external = Arc::new(());
        let retained = Arc::downgrade(&external);
        let mut cache = HashMap::from([(1_u8, MacosCaptureCacheOwner::new(external))]);

        assert!(retained.upgrade().is_some());
        cache.clear();
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn cache_reimport_replaces_owner_without_losing_live_ownership() {
        let first = Arc::new(());
        let first_retained = Arc::downgrade(&first);
        let second = Arc::new(());
        let second_retained = Arc::downgrade(&second);
        let mut cached = MacosCaptureCacheOwner::new(first);

        assert_eq!(Arc::strong_count(&cached._owner), 1);
        cached = MacosCaptureCacheOwner::new(second);

        assert!(first_retained.upgrade().is_none());
        assert!(second_retained.upgrade().is_some());
        drop(cached);
        assert!(second_retained.upgrade().is_none());
    }
}
