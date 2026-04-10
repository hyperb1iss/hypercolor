use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub(crate) struct GpuCompositorProbe {
    pub(crate) adapter_name: String,
    pub(crate) backend: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
}

pub(crate) fn probe_gpu_compositor() -> Result<GpuCompositorProbe> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .context("no compatible wgpu adapter was available for SparkleFlinger")?;
    let info = adapter.get_info();
    let limits = adapter.limits();

    Ok(GpuCompositorProbe {
        adapter_name: info.name,
        backend: backend_name(info.backend),
        max_texture_dimension_2d: limits.max_texture_dimension_2d,
        max_storage_textures_per_shader_stage: limits.max_storage_textures_per_shader_stage,
    })
}

fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Noop => "noop",
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "browser_webgpu",
    }
}
