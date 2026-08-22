#![deny(missing_docs)]

//! Platform-neutral vocabulary for imported GPU frames.

use std::fmt;
use std::sync::Arc;

/// Opaque identity of one imported GPU allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImportedFrameAllocationId(u64);

impl ImportedFrameAllocationId {
    /// Creates an allocation identity from a producer-scoped value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the producer-scoped identity value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Pixel format of an imported GPU frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImportedFrameFormat {
    /// 8-bit normalized RGBA.
    Rgba8Unorm,
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
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            Self::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            Self::R8Unorm => wgpu::TextureFormat::R8Unorm,
            Self::Rg8Unorm => wgpu::TextureFormat::Rg8Unorm,
            Self::R16Unorm => wgpu::TextureFormat::R16Unorm,
            Self::Rg16Unorm => wgpu::TextureFormat::Rg16Unorm,
        }
    }

    /// Returns the byte width of one texel.
    #[must_use]
    pub const fn bytes_per_texel(self) -> u32 {
        match self {
            Self::Rgba8Unorm | Self::Bgra8Unorm | Self::Rg16Unorm => 4,
            Self::Rgba16Float => 8,
            Self::R8Unorm => 1,
            Self::Rg8Unorm | Self::R16Unorm => 2,
        }
    }
}

/// Row-origin convention of an imported GPU frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameOrigin {
    /// The first row is the top row of the image.
    TopLeft,
    /// The first row is the bottom row of the image.
    BottomLeft,
}

/// Uniform timing phases captured while importing a GPU frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedFrameTimings {
    /// Time spent copying or blitting into importable storage.
    pub blit_us: Option<u64>,
    /// Time spent wrapping native storage as a wgpu texture.
    pub wrap_us: Option<u64>,
    /// Time spent waiting for producer synchronization.
    pub sync_us: Option<u64>,
    /// Total import time.
    pub total_us: u64,
}

/// Cloneable lifetime token for resources backing an imported frame.
#[derive(Clone)]
pub struct ImportedFrameLease {
    owner: Arc<dyn Send + Sync>,
}

impl ImportedFrameLease {
    /// Retains a platform-owned resource without exposing its native type.
    #[must_use]
    pub fn new<T>(owner: Arc<T>) -> Self
    where
        T: Send + Sync + 'static,
    {
        Self { owner }
    }

    /// Returns whether two tokens retain the same owner allocation.
    #[must_use]
    pub fn retains_same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
    }
}

impl fmt::Debug for ImportedFrameLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportedFrameLease")
            .finish_non_exhaustive()
    }
}

/// GPU-resident effect frame imported into Hypercolor's wgpu device.
#[derive(Debug, Clone)]
pub struct ImportedEffectFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame pixel format.
    pub format: ImportedFrameFormat,
    /// Stable identity of the underlying GPU allocation.
    pub allocation_id: ImportedFrameAllocationId,
    /// Monotonic version of the allocation's current contents.
    pub content_generation: u64,
    /// Row-origin convention of the imported contents.
    pub origin: FrameOrigin,
    /// Imported wgpu texture.
    pub texture: Arc<wgpu::Texture>,
    /// Default view over `texture`.
    pub view: Arc<wgpu::TextureView>,
    /// Native lifetime retained without exposing a platform handle.
    pub lease: ImportedFrameLease,
    /// Import timing counters for observability.
    pub timings: ImportedFrameTimings,
}

/// Stable reason for falling back after a GPU import failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GpuFrameImportFallbackReason {
    /// The render device is unavailable.
    DeviceUnavailable,
    /// The active wgpu device is not backed by Vulkan.
    MissingWgpuVulkanDevice,
    /// Vulkan external-memory file-descriptor support is unavailable.
    MissingVulkanExternalMemoryFd,
    /// A required OpenGL function is unavailable.
    MissingGlFunction,
    /// The OpenGL procedure loader is unavailable.
    GlProcLoaderUnavailable,
    /// Frame dimensions are invalid or inconsistent.
    InvalidDimensions,
    /// A Vulkan operation failed.
    Vulkan,
    /// An OpenGL resource could not be created.
    GlResource,
    /// An OpenGL operation failed.
    GlOperation,
    /// An OpenGL framebuffer is incomplete.
    GlFramebufferIncomplete,
    /// The current platform does not support this import path.
    UnsupportedPlatform,
    /// Every import slot is still retained by downstream work.
    ImportSlotsExhausted,
    /// The active wgpu device is not backed by Metal.
    MissingWgpuMetalDevice,
    /// The macOS Servo surface is unavailable.
    MissingMacosServoSurface,
    /// The IOSurface pixel format does not match the import contract.
    IosurfacePixelFormatMismatch,
    /// A Metal texture could not be created.
    MetalTextureCreateFailed,
    /// No more specific reason applies.
    Other,
    /// Vulkan external-memory Win32 support is unavailable.
    MissingVulkanExternalMemoryWin32,
    /// The Windows ANGLE context is unavailable.
    MissingWindowsAngleContext,
    /// A D3D11 device could not be created.
    D3d11DeviceCreateFailed,
    /// A shared D3D11 texture could not be created.
    D3d11SharedTextureCreateFailed,
    /// A shared D3D11 handle could not be created.
    D3d11SharedHandleCreateFailed,
    /// ANGLE could not create a client-buffer surface.
    AngleClientBufferSurfaceFailed,
    /// The D3D11 and wgpu adapters do not match.
    AdapterLuidMismatch,
    /// Vulkan could not import the D3D11 texture.
    VulkanD3d11ImportFailed,
    /// A Windows import publication became stale.
    WindowsImportStaleFrame,
}

impl GpuFrameImportFallbackReason {
    /// Returns the stable telemetry code for this reason.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        match self {
            Self::DeviceUnavailable => 1,
            Self::MissingWgpuVulkanDevice => 2,
            Self::MissingVulkanExternalMemoryFd => 3,
            Self::MissingGlFunction => 4,
            Self::GlProcLoaderUnavailable => 5,
            Self::InvalidDimensions => 6,
            Self::Vulkan => 7,
            Self::GlResource => 8,
            Self::GlOperation => 9,
            Self::GlFramebufferIncomplete => 10,
            Self::UnsupportedPlatform => 11,
            Self::ImportSlotsExhausted => 12,
            Self::MissingWgpuMetalDevice => 13,
            Self::MissingMacosServoSurface => 14,
            Self::IosurfacePixelFormatMismatch => 15,
            Self::MetalTextureCreateFailed => 16,
            Self::Other => 17,
            Self::MissingVulkanExternalMemoryWin32 => 18,
            Self::MissingWindowsAngleContext => 19,
            Self::D3d11DeviceCreateFailed => 20,
            Self::D3d11SharedTextureCreateFailed => 21,
            Self::D3d11SharedHandleCreateFailed => 22,
            Self::AngleClientBufferSurfaceFailed => 23,
            Self::AdapterLuidMismatch => 24,
            Self::VulkanD3d11ImportFailed => 25,
            Self::WindowsImportStaleFrame => 26,
        }
    }

    /// Decodes a stable telemetry code.
    #[must_use]
    pub const fn from_u64(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::DeviceUnavailable),
            2 => Some(Self::MissingWgpuVulkanDevice),
            3 => Some(Self::MissingVulkanExternalMemoryFd),
            4 => Some(Self::MissingGlFunction),
            5 => Some(Self::GlProcLoaderUnavailable),
            6 => Some(Self::InvalidDimensions),
            7 => Some(Self::Vulkan),
            8 => Some(Self::GlResource),
            9 => Some(Self::GlOperation),
            10 => Some(Self::GlFramebufferIncomplete),
            11 => Some(Self::UnsupportedPlatform),
            12 => Some(Self::ImportSlotsExhausted),
            13 => Some(Self::MissingWgpuMetalDevice),
            14 => Some(Self::MissingMacosServoSurface),
            15 => Some(Self::IosurfacePixelFormatMismatch),
            16 => Some(Self::MetalTextureCreateFailed),
            17 => Some(Self::Other),
            18 => Some(Self::MissingVulkanExternalMemoryWin32),
            19 => Some(Self::MissingWindowsAngleContext),
            20 => Some(Self::D3d11DeviceCreateFailed),
            21 => Some(Self::D3d11SharedTextureCreateFailed),
            22 => Some(Self::D3d11SharedHandleCreateFailed),
            23 => Some(Self::AngleClientBufferSurfaceFailed),
            24 => Some(Self::AdapterLuidMismatch),
            25 => Some(Self::VulkanD3d11ImportFailed),
            26 => Some(Self::WindowsImportStaleFrame),
            _ => None,
        }
    }

    /// Returns the stable diagnostic label for this reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceUnavailable => "device_unavailable",
            Self::MissingWgpuVulkanDevice => "missing_wgpu_vulkan_device",
            Self::MissingVulkanExternalMemoryFd => "missing_vulkan_external_memory_fd",
            Self::MissingGlFunction => "missing_gl_function",
            Self::GlProcLoaderUnavailable => "gl_proc_loader_unavailable",
            Self::InvalidDimensions => "invalid_dimensions",
            Self::Vulkan => "vulkan_error",
            Self::GlResource => "gl_resource_error",
            Self::GlOperation => "gl_operation_error",
            Self::GlFramebufferIncomplete => "gl_framebuffer_incomplete",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::ImportSlotsExhausted => "import_slots_exhausted",
            Self::MissingWgpuMetalDevice => "missing_wgpu_metal_device",
            Self::MissingMacosServoSurface => "missing_macos_servo_surface",
            Self::IosurfacePixelFormatMismatch => "iosurface_pixel_format_mismatch",
            Self::MetalTextureCreateFailed => "metal_texture_create_failed",
            Self::Other => "other",
            Self::MissingVulkanExternalMemoryWin32 => "missing_vulkan_external_memory_win32",
            Self::MissingWindowsAngleContext => "missing_windows_angle_context",
            Self::D3d11DeviceCreateFailed => "d3d11_device_create_failed",
            Self::D3d11SharedTextureCreateFailed => "d3d11_shared_texture_create_failed",
            Self::D3d11SharedHandleCreateFailed => "d3d11_shared_handle_create_failed",
            Self::AngleClientBufferSurfaceFailed => "angle_client_buffer_surface_failed",
            Self::AdapterLuidMismatch => "adapter_luid_mismatch",
            Self::VulkanD3d11ImportFailed => "vulkan_d3d11_import_failed",
            Self::WindowsImportStaleFrame => "windows_import_stale_frame",
        }
    }
}

/// Supplies the neutral fallback reason for a platform import error.
pub trait GpuFrameImportError {
    /// Returns the stable fallback reason for this error.
    fn fallback_reason(&self) -> GpuFrameImportFallbackReason;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_contract_distinguishes_allocation_from_content() {
        let allocation = ImportedFrameAllocationId::new(7);
        assert_eq!(allocation.get(), 7);
        assert_ne!(allocation.get(), 41);
    }

    #[test]
    fn timing_phases_are_explicitly_optional() {
        let timings = ImportedFrameTimings {
            blit_us: None,
            wrap_us: Some(11),
            sync_us: None,
            total_us: 17,
        };
        assert_eq!(timings.wrap_us, Some(11));
        assert_eq!(timings.blit_us, None);
    }

    #[test]
    fn lease_clone_retains_the_same_owner() {
        let lease = ImportedFrameLease::new(Arc::new(String::from("native owner")));
        assert!(lease.retains_same_owner(&lease.clone()));
    }

    #[test]
    fn formats_and_origins_are_platform_neutral() {
        assert_eq!(ImportedFrameFormat::Bgra8Unorm.bytes_per_texel(), 4);
        assert_eq!(ImportedFrameFormat::Rg16Unorm.bytes_per_texel(), 4);
        assert_ne!(FrameOrigin::TopLeft, FrameOrigin::BottomLeft);
    }

    #[test]
    fn fallback_reason_codes_remain_stable() {
        for value in 1..=26 {
            let reason = GpuFrameImportFallbackReason::from_u64(value)
                .expect("every assigned telemetry code decodes");
            assert_eq!(reason.as_u64(), value);
            assert!(!reason.as_str().is_empty());
        }
        assert_eq!(GpuFrameImportFallbackReason::from_u64(0), None);
        assert_eq!(
            GpuFrameImportFallbackReason::Vulkan.as_str(),
            "vulkan_error"
        );
        assert_eq!(
            GpuFrameImportFallbackReason::GlResource.as_str(),
            "gl_resource_error"
        );
        assert_eq!(
            GpuFrameImportFallbackReason::GlOperation.as_str(),
            "gl_operation_error"
        );
    }
}
