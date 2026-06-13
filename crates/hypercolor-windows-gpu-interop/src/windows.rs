use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use thiserror::Error;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIAdapter, IDXGIAdapter1,
    IDXGIFactory1, IDXGIResource1,
};
use windows::core::{Interface, PCWSTR};

const BYTES_PER_PIXEL: u32 = 4;
static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Result type for Windows GPU interop operations.
pub type Result<T> = std::result::Result<T, WindowsGpuInteropError>;

/// Errors raised while preparing or importing Windows GPU surfaces.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowsGpuInteropError {
    /// The active wgpu device is not backed by Vulkan.
    #[error("wgpu device is not backed by the Vulkan HAL")]
    MissingWgpuVulkanDevice,

    /// The active wgpu device lacks Win32 external-memory support.
    #[error("wgpu Vulkan device is missing VULKAN_EXTERNAL_MEMORY_WIN32")]
    MissingVulkanExternalMemoryWin32,

    /// A Windows ANGLE rendering context is required before import can run.
    #[error("Windows ANGLE rendering context is unavailable")]
    MissingWindowsAngleContext,

    /// The Windows ANGLE context had no published shared texture.
    #[error("Windows ANGLE shared-texture frame is not ready")]
    WindowsImportStaleFrame,

    /// Frame dimensions are not usable by D3D11 or wgpu.
    #[error("invalid import dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested frame width.
        width: u32,
        /// Requested frame height.
        height: u32,
    },

    /// The supplied D3D11 shared handle is null.
    #[error("D3D11 shared texture handle is null")]
    InvalidSharedHandle,

    /// Vulkan failed while importing the D3D11 shared handle.
    #[error("Vulkan D3D11 shared-handle import failed")]
    VulkanD3d11ImportFailed,

    /// DXGI could not create a factory for adapter selection.
    #[error("DXGI factory creation failed with HRESULT {hresult:#010x}")]
    DxgiFactoryCreateFailed {
        /// Failing HRESULT.
        hresult: i32,
    },

    /// DXGI adapter enumeration or description failed.
    #[error("DXGI adapter query failed with HRESULT {hresult:#010x}")]
    DxgiAdapterQueryFailed {
        /// Failing HRESULT.
        hresult: i32,
    },

    /// No hardware DXGI adapter matched the requested identifiers.
    #[error("DXGI adapter not found for vendor {vendor_id:?} device {device_id:?}")]
    DxgiAdapterNotFound {
        /// Requested PCI vendor identifier.
        vendor_id: Option<u32>,
        /// Requested PCI device identifier.
        device_id: Option<u32>,
    },

    /// D3D11 device creation failed.
    #[error("D3D11 device creation failed with HRESULT {hresult:#010x}")]
    D3d11DeviceCreateFailed {
        /// Failing HRESULT.
        hresult: i32,
    },

    /// D3D11 did not return an immediate context.
    #[error("D3D11 immediate context is unavailable")]
    D3d11ImmediateContextUnavailable,

    /// Creating a D3D11 texture failed.
    #[error("D3D11 shared texture creation failed with HRESULT {hresult:#010x}")]
    D3d11SharedTextureCreateFailed {
        /// Failing HRESULT.
        hresult: i32,
    },

    /// Querying a D3D11 texture for another COM interface failed.
    #[error("D3D11 texture interface query failed with HRESULT {hresult:#010x}")]
    D3d11TextureInterfaceQueryFailed {
        /// Failing HRESULT.
        hresult: i32,
    },

    /// Creating an NT shared handle for a D3D11 texture failed.
    #[error("D3D11 shared handle creation failed with HRESULT {hresult:#010x}")]
    D3d11SharedHandleCreateFailed {
        /// Failing HRESULT.
        hresult: i32,
    },

    /// A packed pixel upload did not match the texture shape.
    #[error("pixel buffer size mismatch: expected {expected_len} bytes, got {actual_len}")]
    PixelBufferSizeMismatch {
        /// Expected byte length.
        expected_len: usize,
        /// Actual byte length.
        actual_len: usize,
    },

    /// Creating or operating the Servo ANGLE rendering context failed.
    #[error("Servo ANGLE context {operation} failed: {message}")]
    ServoContext {
        /// Failing operation.
        operation: &'static str,
        /// Platform error detail.
        message: String,
    },

    /// Creating a GL resource for the publish pipeline failed.
    #[error("failed to create GL {resource}: {message}")]
    GlCreateResource {
        /// Resource kind that failed to allocate.
        resource: &'static str,
        /// Driver-reported detail.
        message: String,
    },

    /// A GL operation in the publish pipeline failed.
    #[error("GL {operation} failed with error {code:#06x}")]
    GlOperation {
        /// Failing GL operation.
        operation: &'static str,
        /// GL error code.
        code: u32,
    },

    /// A publish framebuffer was incomplete.
    #[error("GL framebuffer incomplete with status {status:#06x}")]
    GlFramebufferIncomplete {
        /// GL framebuffer status.
        status: u32,
    },

    /// Waiting on a published Servo frame fence timed out.
    #[error("timed out waiting for a published Servo frame fence")]
    PublishFenceTimeout,
}

/// Pixel format shared by the D3D11 texture and imported wgpu texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportedFrameFormat {
    /// 8-bit normalized RGBA.
    Rgba8Unorm,
    /// 8-bit normalized BGRA.
    Bgra8Unorm,
}

impl ImportedFrameFormat {
    /// Returns the matching wgpu texture format.
    #[must_use]
    pub const fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        }
    }

    const fn dxgi_format(self) -> DXGI_FORMAT {
        match self {
            Self::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
            Self::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        }
    }
}

/// Description of a Windows D3D11 shared-texture import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsD3d11SharedTextureImportDescriptor {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame pixel format.
    pub format: ImportedFrameFormat,
}

impl WindowsD3d11SharedTextureImportDescriptor {
    /// Creates a validated import descriptor.
    pub const fn new(width: u32, height: u32, format: ImportedFrameFormat) -> Result<Self> {
        if width == 0
            || height == 0
            || width > i32::MAX as u32 / BYTES_PER_PIXEL
            || height > i32::MAX as u32
        {
            Err(WindowsGpuInteropError::InvalidDimensions { width, height })
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
    /// changed. Does NOT imply distinct GPU storage — the same D3D11 shared
    /// texture (and cached wgpu texture) can carry many successive versions.
    pub storage_id: u64,
    /// Imported wgpu texture.
    pub texture: Arc<wgpu::Texture>,
    /// Default view over `texture`.
    pub view: Arc<wgpu::TextureView>,
    /// Import timing counters for observability.
    pub timings: ImportedFrameTimings,
}

/// Timing counters captured while importing a D3D11 shared texture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedFrameTimings {
    /// Time spent wrapping the D3D11 shared handle as a wgpu texture.
    pub wrap_us: u64,
    /// Time spent waiting for producer-side synchronization.
    pub sync_us: u64,
    /// Total import time, including wgpu wrapping.
    pub total_us: u64,
}

/// D3D11 device and immediate context used to create shared textures.
pub struct WindowsD3d11Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

impl WindowsD3d11Device {
    /// Creates a D3D11 device on the first non-software DXGI adapter.
    pub fn new_hardware() -> Result<Self> {
        let adapter = find_dxgi_adapter(None, None)?;
        Self::from_dxgi_adapter(&adapter)
    }

    /// Creates a D3D11 device on the adapter matching a wgpu adapter.
    pub fn new_for_wgpu_adapter(vendor_id: u32, device_id: u32) -> Result<Self> {
        let adapter = find_dxgi_adapter(Some(vendor_id), Some(device_id))?;
        Self::from_dxgi_adapter(&adapter)
    }

    #[cfg(feature = "servo-context")]
    pub(crate) unsafe fn from_owned_raw_d3d11_device(raw: *mut std::ffi::c_void) -> Result<Self> {
        if raw.is_null() {
            return Err(WindowsGpuInteropError::D3d11DeviceCreateFailed { hresult: 0 });
        }
        // SAFETY: raw is an owned ID3D11Device reference returned by Surfman.
        let device = unsafe { ID3D11Device::from_raw(raw) };
        // SAFETY: device is a live D3D11 device and returns its immediate context.
        let context = unsafe {
            device.GetImmediateContext().map_err(|error| {
                WindowsGpuInteropError::D3d11DeviceCreateFailed {
                    hresult: hresult(error),
                }
            })?
        };
        Ok(Self { device, context })
    }

    /// Creates a shared NT-handle texture on this D3D11 device.
    pub fn create_shared_texture(
        &self,
        descriptor: WindowsD3d11SharedTextureImportDescriptor,
    ) -> Result<WindowsD3d11SharedTexture> {
        let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        let desc = d3d11_texture_descriptor(descriptor);
        let mut texture = None;
        // SAFETY: desc is fully initialized and requests a single 2D texture.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(
                    |error| WindowsGpuInteropError::D3d11SharedTextureCreateFailed {
                        hresult: hresult(error),
                    },
                )?;
        }
        let texture =
            texture.ok_or(WindowsGpuInteropError::D3d11SharedTextureCreateFailed { hresult: 0 })?;
        let dxgi_resource: IDXGIResource1 = texture.cast().map_err(|error| {
            WindowsGpuInteropError::D3d11TextureInterfaceQueryFailed {
                hresult: hresult(error),
            }
        })?;
        let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
        // SAFETY: the texture was created with D3D11_RESOURCE_MISC_SHARED_NTHANDLE.
        let shared_handle = unsafe {
            dxgi_resource
                .CreateSharedHandle(None, access, PCWSTR::null())
                .map_err(
                    |error| WindowsGpuInteropError::D3d11SharedHandleCreateFailed {
                        hresult: hresult(error),
                    },
                )?
        };
        Ok(WindowsD3d11SharedTexture {
            descriptor,
            texture,
            shared_handle,
        })
    }

    /// Writes packed pixels into a shared texture and flushes producer work.
    pub fn write_pixels(&self, texture: &WindowsD3d11SharedTexture, pixels: &[u8]) -> Result<()> {
        let expected_len = texture.descriptor.width as usize
            * texture.descriptor.height as usize
            * BYTES_PER_PIXEL as usize;
        if pixels.len() != expected_len {
            return Err(WindowsGpuInteropError::PixelBufferSizeMismatch {
                expected_len,
                actual_len: pixels.len(),
            });
        }

        let resource: ID3D11Resource = texture.texture.cast().map_err(|error| {
            WindowsGpuInteropError::D3d11TextureInterfaceQueryFailed {
                hresult: hresult(error),
            }
        })?;
        let row_pitch = texture.descriptor.width * BYTES_PER_PIXEL;
        let depth_pitch = row_pitch * texture.descriptor.height;
        // SAFETY: resource is the destination texture, pixels covers every row,
        // and row/depth pitch match the validated descriptor.
        unsafe {
            self.context.UpdateSubresource(
                &resource,
                0,
                None,
                pixels.as_ptr().cast(),
                row_pitch,
                depth_pitch,
            );
            self.context.Flush();
        }
        Ok(())
    }

    fn from_dxgi_adapter(adapter: &IDXGIAdapter1) -> Result<Self> {
        let adapter: IDXGIAdapter =
            adapter
                .cast()
                .map_err(|error| WindowsGpuInteropError::DxgiAdapterQueryFailed {
                    hresult: hresult(error),
                })?;
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        let mut device = None;
        let mut context = None;
        // SAFETY: adapter is a live DXGI hardware adapter and the output
        // pointers remain valid until the call returns.
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
            .map_err(|error| WindowsGpuInteropError::D3d11DeviceCreateFailed {
                hresult: hresult(error),
            })?;
        }
        let device =
            device.ok_or(WindowsGpuInteropError::D3d11DeviceCreateFailed { hresult: 0 })?;
        let context = context.ok_or(WindowsGpuInteropError::D3d11ImmediateContextUnavailable)?;
        Ok(Self { device, context })
    }
}

/// A D3D11 texture carrying an NT shared handle for Vulkan import.
pub struct WindowsD3d11SharedTexture {
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
    texture: ID3D11Texture2D,
    shared_handle: HANDLE,
}

impl WindowsD3d11SharedTexture {
    /// Returns this texture's import descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> WindowsD3d11SharedTextureImportDescriptor {
        self.descriptor
    }

    /// Returns the raw D3D11 texture.
    #[must_use]
    pub const fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// Returns the NT shared handle. The texture owns and closes this handle.
    #[must_use]
    pub const fn shared_handle(&self) -> HANDLE {
        self.shared_handle
    }

    #[cfg(feature = "servo-context")]
    pub(crate) unsafe fn to_surfman_texture(
        &self,
    ) -> wio::com::ComPtr<winapi::um::d3d11::ID3D11Texture2D> {
        let raw = self
            .texture
            .as_raw()
            .cast::<winapi::um::d3d11::ID3D11Texture2D>();
        // SAFETY: raw is the live COM texture owned by self; AddRef gives
        // Surfman its own counted reference for the EGL surface.
        unsafe {
            (*raw).AddRef();
            wio::com::ComPtr::from_raw(raw)
        }
    }
}

impl Drop for WindowsD3d11SharedTexture {
    fn drop(&mut self) {
        if !self.shared_handle.is_invalid() {
            // SAFETY: shared_handle is owned by this texture wrapper.
            let _ = unsafe { CloseHandle(self.shared_handle) };
        }
    }
}

/// Reusable importer for wrapping D3D11 shared textures as wgpu textures.
pub struct WindowsD3d11SharedTextureImporter {
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
    imported_textures: HashMap<usize, ImportedSharedTexture>,
    last_ring_epoch: Option<u64>,
}

#[derive(Debug, Clone)]
struct ImportedSharedTexture {
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
}

impl WindowsD3d11SharedTextureImporter {
    /// Creates an importer for one shared-texture shape.
    pub fn new(
        device: &wgpu::Device,
        descriptor: WindowsD3d11SharedTextureImportDescriptor,
    ) -> Result<Self> {
        let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
            descriptor.width,
            descriptor.height,
            descriptor.format,
        )?;
        check_wgpu_vulkan_external_memory_win32(device)?;
        Ok(Self {
            descriptor,
            imported_textures: HashMap::new(),
            last_ring_epoch: None,
        })
    }

    /// Drops cached wgpu textures when the producer's texture ring changes.
    ///
    /// Closed NT handle values can be recycled by the OS, so a cache entry
    /// keyed on a handle from an earlier ring could alias a brand-new
    /// texture.
    pub fn reset_cache_for_ring_epoch(&mut self, ring_epoch: u64) {
        if self.last_ring_epoch != Some(ring_epoch) {
            self.imported_textures.clear();
            self.last_ring_epoch = Some(ring_epoch);
        }
    }

    /// Returns the descriptor this importer was built for.
    #[must_use]
    pub const fn descriptor(&self) -> WindowsD3d11SharedTextureImportDescriptor {
        self.descriptor
    }

    /// Imports a D3D11 NT shared handle into the supplied wgpu device.
    ///
    /// `content_generation` is the producer's monotonic content version for
    /// the texture contents and becomes the frame's `storage_id`. Repeated
    /// imports of the same shared handle reuse the cached wgpu texture.
    ///
    /// # Safety
    ///
    /// The handle must be a live NT handle from
    /// `IDXGIResource1::CreateSharedHandle`, created for a D3D11 texture whose
    /// dimensions and format match this importer's descriptor. The producer
    /// must have completed writes before this function is called.
    pub unsafe fn import_shared_handle(
        &mut self,
        device: &wgpu::Device,
        shared_handle: HANDLE,
        content_generation: u64,
        sync_us: u64,
    ) -> Result<ImportedEffectFrame> {
        if shared_handle.is_invalid() {
            return Err(WindowsGpuInteropError::InvalidSharedHandle);
        }

        let total_start = Instant::now();
        let shared_handle_key = shared_handle_key(shared_handle);
        if let Some(imported) = self.imported_textures.get(&shared_handle_key) {
            return Ok(ImportedEffectFrame {
                width: self.descriptor.width,
                height: self.descriptor.height,
                format: self.descriptor.format,
                storage_id: content_generation,
                texture: Arc::clone(&imported.texture),
                view: Arc::clone(&imported.view),
                timings: ImportedFrameTimings {
                    wrap_us: 0,
                    sync_us,
                    total_us: elapsed_micros(total_start),
                },
            });
        }

        let wrap_start = Instant::now();
        // SAFETY: this only borrows the underlying HAL device long enough to
        // import the caller-owned D3D11 NT handle; no raw Vulkan device handle
        // escapes this function.
        let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
            .ok_or(WindowsGpuInteropError::MissingWgpuVulkanDevice)?;
        let hal_desc = hal_texture_descriptor(self.descriptor);
        // SAFETY: the caller guarantees shared_handle is an NT D3D11 texture
        // handle matching hal_desc and completed producer synchronization.
        let hal_texture =
            unsafe { hal_device.texture_from_d3d11_shared_handle(shared_handle, &hal_desc) }
                .map_err(|_| WindowsGpuInteropError::VulkanD3d11ImportFailed)?;
        let wgpu_desc = wgpu_texture_descriptor(self.descriptor);
        // SAFETY: hal_texture was created from this wgpu device's Vulkan HAL
        // and matches wgpu_desc.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(hal_texture, &wgpu_desc)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let wrap_us = elapsed_micros(wrap_start);
        let texture = Arc::new(texture);
        let view = Arc::new(view);
        self.imported_textures.insert(
            shared_handle_key,
            ImportedSharedTexture {
                texture: Arc::clone(&texture),
                view: Arc::clone(&view),
            },
        );

        Ok(ImportedEffectFrame {
            width: self.descriptor.width,
            height: self.descriptor.height,
            format: self.descriptor.format,
            storage_id: content_generation,
            texture,
            view,
            timings: ImportedFrameTimings {
                wrap_us,
                sync_us,
                total_us: elapsed_micros(total_start),
            },
        })
    }

    /// Imports a shared handle that has no producer content generation.
    ///
    /// Allocates a fresh content version from a process-global counter, so
    /// every call reports changed contents. Intended for tests and
    /// compatibility probes built around synthetic D3D11 textures.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::import_shared_handle`].
    pub unsafe fn import_shared_handle_for_test(
        &mut self,
        device: &wgpu::Device,
        shared_handle: HANDLE,
    ) -> Result<ImportedEffectFrame> {
        let content_generation = NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded directly under the caller's contract.
        unsafe { self.import_shared_handle(device, shared_handle, content_generation, 0) }
    }
}

fn shared_handle_key(shared_handle: HANDLE) -> usize {
    shared_handle.0 as usize
}

/// Verifies that the active wgpu device can expose Vulkan external memory Win32 handles.
pub fn check_wgpu_vulkan_external_memory_win32(device: &wgpu::Device) -> Result<()> {
    // SAFETY: this only probes whether the wgpu device exposes the Vulkan HAL.
    if unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }.is_none() {
        return Err(WindowsGpuInteropError::MissingWgpuVulkanDevice);
    }
    if device
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32)
    {
        Ok(())
    } else {
        Err(WindowsGpuInteropError::MissingVulkanExternalMemoryWin32)
    }
}

fn find_dxgi_adapter(vendor_id: Option<u32>, device_id: Option<u32>) -> Result<IDXGIAdapter1> {
    // SAFETY: CreateDXGIFactory1 has no borrowed inputs and initializes a COM interface.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|error| {
        WindowsGpuInteropError::DxgiFactoryCreateFailed {
            hresult: hresult(error),
        }
    })?;

    let mut index = 0;
    loop {
        // SAFETY: factory is live and index is advanced until DXGI reports no match.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                return Err(WindowsGpuInteropError::DxgiAdapterQueryFailed {
                    hresult: hresult(error),
                });
            }
        };
        // SAFETY: adapter is a live DXGI adapter returned by the factory.
        let desc = unsafe { adapter.GetDesc1() }.map_err(|error| {
            WindowsGpuInteropError::DxgiAdapterQueryFailed {
                hresult: hresult(error),
            }
        })?;
        let is_software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;
        let vendor_matches = vendor_id.is_none_or(|id| id == desc.VendorId);
        let device_matches = device_id.is_none_or(|id| id == desc.DeviceId);
        if !is_software && vendor_matches && device_matches {
            return Ok(adapter);
        }
        index += 1;
    }

    Err(WindowsGpuInteropError::DxgiAdapterNotFound {
        vendor_id,
        device_id,
    })
}

fn d3d11_texture_descriptor(
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: descriptor.width,
        Height: descriptor.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: descriptor.format.dxgi_format(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
    }
}

fn wgpu_texture_descriptor(
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("hypercolor-windows-servo-import"),
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
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
) -> wgpu_hal::TextureDescriptor<'static> {
    wgpu_hal::TextureDescriptor {
        label: Some("hypercolor-windows-servo-import"),
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

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn hresult(error: windows::core::Error) -> i32 {
    error.code().0
}
