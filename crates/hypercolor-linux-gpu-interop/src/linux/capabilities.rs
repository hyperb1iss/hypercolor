use std::ffi::{CStr, c_void};

use ash::khr;

use super::loader::{lookup_process_gl_symbol, process_gl_loader_available};
use super::{LinuxGpuInteropError, Result};

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
    if !process_gl_loader_available() {
        return vec!["libGL.so.1", "libEGL.so.1"];
    }
    missing_gl_external_memory_functions(lookup_process_gl_symbol)
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
