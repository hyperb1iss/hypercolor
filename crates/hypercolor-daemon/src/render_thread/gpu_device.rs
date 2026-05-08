use std::sync::Arc;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub(crate) struct GpuRenderDevice {
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
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
}

impl GpuRenderDevice {
    pub(crate) fn new(label: &'static str) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .with_context(|| format!("no compatible wgpu adapter was available for {label}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .with_context(|| format!("failed to create a {label} wgpu device"))?;

        let adapter_info = adapter.get_info();
        let limits = device.limits();
        let info = GpuRenderDeviceInfo {
            adapter_name: adapter_info.name,
            backend: adapter_info.backend,
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

    #[cfg(feature = "servo-gpu-import")]
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
}
