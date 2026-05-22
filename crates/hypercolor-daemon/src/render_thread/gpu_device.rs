use std::sync::Arc;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct GpuRenderDevice {
    inner: Arc<GpuRenderDeviceInner>,
}

#[derive(Debug)]
struct GpuRenderDeviceInner {
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: GpuRenderDeviceInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuRenderDeviceInfo {
    pub(crate) adapter_name: String,
    pub(crate) backend: wgpu::Backend,
    pub(crate) vulkan_external_memory_win32: bool,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuBackendPreference {
    Default,
    VulkanRequiredForServoImport,
}

impl GpuRenderDevice {
    pub(crate) fn new(label: &'static str) -> Result<Self> {
        Self::new_with_backend_preference(label, GpuBackendPreference::Default)
    }

    pub(crate) fn new_with_backend_preference(
        label: &'static str,
        backend_preference: GpuBackendPreference,
    ) -> Result<Self> {
        let mut instance_descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        if matches!(
            backend_preference,
            GpuBackendPreference::VulkanRequiredForServoImport
        ) {
            instance_descriptor.backends = wgpu::Backends::VULKAN;
        }
        let instance = wgpu::Instance::new(instance_descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .with_context(|| format!("no compatible wgpu adapter was available for {label}"))?;

        let adapter_features = adapter.features();
        let mut required_features = wgpu::Features::CLEAR_TEXTURE;
        let adapter_info = adapter.get_info();
        let vulkan_external_memory_win32 =
            adapter_features.contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32);
        if cfg!(target_os = "windows")
            && matches!(adapter_info.backend, wgpu::Backend::Vulkan)
            && vulkan_external_memory_win32
        {
            required_features |= wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
        }
        if matches!(
            backend_preference,
            GpuBackendPreference::VulkanRequiredForServoImport
        ) {
            required_features |= wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_features,
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .with_context(|| format!("failed to create a {label} wgpu device"))?;

        let limits = device.limits();
        let info = GpuRenderDeviceInfo {
            adapter_name: adapter_info.name,
            backend: adapter_info.backend,
            vulkan_external_memory_win32: device
                .features()
                .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32),
            max_texture_dimension_2d: limits.max_texture_dimension_2d,
            max_storage_textures_per_shader_stage: limits.max_storage_textures_per_shader_stage,
        };

        Ok(Self {
            inner: Arc::new(GpuRenderDeviceInner {
                _instance: instance,
                adapter,
                device,
                queue,
                info,
            }),
        })
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.inner.device
    }

    #[cfg(all(
        any(target_os = "linux", target_os = "macos", target_os = "windows"),
        feature = "servo-gpu-import"
    ))]
    pub(crate) fn device_handle(&self) -> wgpu::Device {
        self.inner.device.clone()
    }

    pub(crate) fn queue(&self) -> &wgpu::Queue {
        &self.inner.queue
    }

    pub(crate) fn info(&self) -> GpuRenderDeviceInfo {
        self.inner.info.clone()
    }

    pub(crate) fn require_texture_usage(
        &self,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Result<()> {
        let format_features = self.inner.adapter.get_texture_format_features(format);
        if format_features.allowed_usages.contains(usage) {
            return Ok(());
        }

        anyhow::bail!(
            "adapter does not support {usage:?} for {}",
            texture_format_name(format)
        );
    }
}

impl GpuRenderDeviceInfo {
    #[cfg_attr(
        not(feature = "servo-gpu-import"),
        allow(
            dead_code,
            reason = "Servo GPU import backend checks are used only when zero-copy import is enabled"
        )
    )]
    pub(crate) const fn servo_gpu_import_backend_compatible(&self) -> bool {
        if cfg!(target_os = "linux") {
            matches!(self.backend, wgpu::Backend::Vulkan)
        } else if cfg!(target_os = "macos") {
            matches!(self.backend, wgpu::Backend::Metal)
        } else if cfg!(target_os = "windows") {
            matches!(self.backend, wgpu::Backend::Vulkan) && self.vulkan_external_memory_win32
        } else {
            false
        }
    }

    #[cfg_attr(
        not(feature = "servo-gpu-import"),
        allow(
            dead_code,
            reason = "Servo GPU import backend checks are used only when zero-copy import is enabled"
        )
    )]
    pub(crate) const fn servo_gpu_import_backend_reason(&self) -> Option<&'static str> {
        if cfg!(target_os = "linux") {
            if matches!(self.backend, wgpu::Backend::Vulkan) {
                None
            } else {
                Some("linux servo gpu import requires a Vulkan wgpu backend")
            }
        } else if cfg!(target_os = "macos") {
            if matches!(self.backend, wgpu::Backend::Metal) {
                None
            } else {
                Some("macOS servo gpu import requires a Metal wgpu backend")
            }
        } else if cfg!(target_os = "windows") {
            if !matches!(self.backend, wgpu::Backend::Vulkan) {
                Some("Windows servo gpu import requires a Vulkan wgpu backend")
            } else if !self.vulkan_external_memory_win32 {
                Some("Windows servo gpu import requires VULKAN_EXTERNAL_MEMORY_WIN32")
            } else {
                None
            }
        } else {
            Some("servo gpu import is only available on linux, macOS, and Windows")
        }
    }

    pub(crate) const fn linux_servo_gpu_import_backend_compatible(&self) -> bool {
        cfg!(target_os = "linux") && matches!(self.backend, wgpu::Backend::Vulkan)
    }

    pub(crate) const fn linux_servo_gpu_import_backend_reason(&self) -> Option<&'static str> {
        if !cfg!(target_os = "linux") {
            Some("linux servo gpu import is only available on linux")
        } else if !matches!(self.backend, wgpu::Backend::Vulkan) {
            Some("linux servo gpu import requires a Vulkan wgpu backend")
        } else {
            None
        }
    }
}

pub(crate) fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Noop => "noop",
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "browser_webgpu",
    }
}

pub(crate) fn texture_format_name(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => "rgba8_unorm",
        wgpu::TextureFormat::Rgba8UnormSrgb => "rgba8_unorm_srgb",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_backend_names_to_stable_status_strings() {
        assert_eq!(backend_name(wgpu::Backend::Noop), "noop");
        assert_eq!(backend_name(wgpu::Backend::Vulkan), "vulkan");
        assert_eq!(backend_name(wgpu::Backend::Metal), "metal");
        assert_eq!(backend_name(wgpu::Backend::Dx12), "dx12");
        assert_eq!(backend_name(wgpu::Backend::Gl), "gl");
        assert_eq!(backend_name(wgpu::Backend::BrowserWebGpu), "browser_webgpu");
    }

    #[test]
    fn maps_texture_formats_to_stable_status_strings() {
        assert_eq!(
            texture_format_name(wgpu::TextureFormat::Rgba8Unorm),
            "rgba8_unorm"
        );
        assert_eq!(
            texture_format_name(wgpu::TextureFormat::Rgba8UnormSrgb),
            "rgba8_unorm_srgb"
        );
        assert_eq!(
            texture_format_name(wgpu::TextureFormat::Bgra8Unorm),
            "other"
        );
    }

    #[test]
    fn reports_linux_servo_import_backend_support_from_platform_and_backend() {
        let info = GpuRenderDeviceInfo {
            adapter_name: "test".to_owned(),
            backend: wgpu::Backend::Vulkan,
            vulkan_external_memory_win32: true,
            max_texture_dimension_2d: 16_384,
            max_storage_textures_per_shader_stage: 8,
        };

        assert_eq!(
            info.linux_servo_gpu_import_backend_compatible(),
            cfg!(target_os = "linux")
        );
        assert_eq!(
            info.linux_servo_gpu_import_backend_reason().is_none(),
            cfg!(target_os = "linux")
        );

        let non_vulkan = GpuRenderDeviceInfo {
            backend: wgpu::Backend::Gl,
            ..info
        };
        assert!(!non_vulkan.linux_servo_gpu_import_backend_compatible());
        assert!(non_vulkan.linux_servo_gpu_import_backend_reason().is_some());
    }

    #[test]
    fn reports_servo_import_backend_support_from_platform_and_backend() {
        let vulkan = GpuRenderDeviceInfo {
            adapter_name: "test".to_owned(),
            backend: wgpu::Backend::Vulkan,
            vulkan_external_memory_win32: true,
            max_texture_dimension_2d: 16_384,
            max_storage_textures_per_shader_stage: 8,
        };
        let metal = GpuRenderDeviceInfo {
            backend: wgpu::Backend::Metal,
            ..vulkan.clone()
        };
        let gl = GpuRenderDeviceInfo {
            backend: wgpu::Backend::Gl,
            ..vulkan.clone()
        };

        assert_eq!(
            vulkan.servo_gpu_import_backend_compatible(),
            cfg!(any(target_os = "linux", target_os = "windows"))
        );
        assert_eq!(
            metal.servo_gpu_import_backend_compatible(),
            cfg!(target_os = "macos")
        );
        assert!(!gl.servo_gpu_import_backend_compatible());
        assert_eq!(
            metal.servo_gpu_import_backend_reason().is_none(),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn windows_servo_import_requires_win32_external_memory() {
        let vulkan_without_win32 = GpuRenderDeviceInfo {
            adapter_name: "test".to_owned(),
            backend: wgpu::Backend::Vulkan,
            vulkan_external_memory_win32: false,
            max_texture_dimension_2d: 16_384,
            max_storage_textures_per_shader_stage: 8,
        };

        assert_eq!(
            vulkan_without_win32.servo_gpu_import_backend_compatible(),
            false
        );
        assert_eq!(
            vulkan_without_win32
                .servo_gpu_import_backend_reason()
                .is_some(),
            cfg!(target_os = "windows")
        );
    }
}
