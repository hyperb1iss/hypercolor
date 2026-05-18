use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use objc2_core_foundation::{
    CFDictionary, CFIndex, CFNumber, CFString, kCFAllocatorDefault, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use objc2_io_surface::{
    IOSurfaceLockOptions, IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow,
    kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};
use thiserror::Error;

const BYTES_PER_PIXEL: u32 = 4;
const PIXEL_FORMAT_BGRA: i32 = u32::from_be_bytes(*b"BGRA") as i32;
static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Result type for macOS GPU interop operations.
pub type Result<T> = std::result::Result<T, MacosGpuInteropError>;

/// Errors raised while preparing or importing macOS GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacosGpuInteropError {
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

    /// The macOS Servo hardware context has no bound Surfman surface.
    #[error("macOS Servo hardware context has no bound Surfman surface")]
    MissingServoSurface,

    /// Metal could not create a texture from the IOSurface.
    #[error("Metal failed to create texture from IOSurface")]
    MetalTextureCreateFailed,
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

    const fn metal_format(self) -> MTLPixelFormat {
        match self {
            Self::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
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
    pub fn new(device: &wgpu::Device, descriptor: MacosIosurfaceImportDescriptor) -> Result<Self> {
        let descriptor = MacosIosurfaceImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        require_metal_device(device)?;
        Ok(Self { descriptor })
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> MacosIosurfaceImportDescriptor {
        self.descriptor
    }

    /// Imports an IOSurface into the supplied wgpu device.
    pub fn import_iosurface(
        &mut self,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
    ) -> Result<ImportedEffectFrame> {
        validate_iosurface_shape(self.descriptor, iosurface)?;

        let total_start = Instant::now();
        let wrap_start = Instant::now();
        let metal_texture = {
            let hal_device = require_metal_device(device)?;
            let descriptor = metal_texture_descriptor(self.descriptor);
            hal_device
                .raw_device()
                .newTextureWithDescriptor_iosurface_plane(&descriptor, iosurface, 0)
                .ok_or(MacosGpuInteropError::MetalTextureCreateFailed)?
        };
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

        Ok(ImportedEffectFrame {
            width: self.descriptor.width,
            height: self.descriptor.height,
            format: self.descriptor.format,
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            texture: Arc::new(texture),
            view: Arc::new(view),
            timings: ImportedFrameTimings {
                wrap_us,
                total_us: elapsed_micros(total_start),
            },
        })
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
    let expected_len = width as usize * height as usize * BYTES_PER_PIXEL as usize;
    if pixels.len() != expected_len {
        return Err(MacosGpuInteropError::PixelBufferSizeMismatch {
            expected_len,
            actual_len: pixels.len(),
        });
    }
    validate_iosurface_shape(
        MacosIosurfaceImportDescriptor::new(width, height, ImportedFrameFormat::Bgra8Unorm)?,
        iosurface,
    )?;

    let lock = IosurfaceLockGuard::lock(iosurface)?;
    let bytes_per_row = iosurface.bytes_per_row();
    let row_len = width as usize * BYTES_PER_PIXEL as usize;
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

fn create_iosurface(
    descriptor: MacosIosurfaceImportDescriptor,
) -> Result<objc2_core_foundation::CFRetained<IOSurfaceRef>> {
    let bytes_per_row = descriptor.width * BYTES_PER_PIXEL;
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
        &*CFNumber::new_i32(BYTES_PER_PIXEL as i32),
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
) -> objc2::rc::Retained<MTLTextureDescriptor> {
    // SAFETY: descriptor dimensions are validated by
    // MacosIosurfaceImportDescriptor::new.
    let texture_descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            descriptor.format.metal_format(),
            descriptor.width as usize,
            descriptor.height as usize,
            false,
        )
    };
    texture_descriptor.setTextureType(MTLTextureType::Type2D);
    texture_descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    texture_descriptor.setStorageMode(MTLStorageMode::Shared);
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
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    }
}

fn validate_iosurface_shape(
    descriptor: MacosIosurfaceImportDescriptor,
    iosurface: &IOSurfaceRef,
) -> Result<()> {
    let actual_width = iosurface.width();
    let actual_height = iosurface.height();
    if actual_width == descriptor.width as usize && actual_height == descriptor.height as usize {
        Ok(())
    } else {
        Err(MacosGpuInteropError::IosurfaceShapeMismatch {
            expected_width: descriptor.width,
            expected_height: descriptor.height,
            actual_width,
            actual_height,
        })
    }
}

fn require_metal_device(
    device: &wgpu::Device,
) -> Result<impl std::ops::Deref<Target = wgpu_hal::metal::Device> + '_> {
    // SAFETY: we only inspect whether this wgpu device is backed by Metal and
    // borrow the HAL device for the duration of the immediate import call.
    unsafe { device.as_hal::<wgpu_hal::api::Metal>() }
        .ok_or(MacosGpuInteropError::MissingWgpuMetalDevice)
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
