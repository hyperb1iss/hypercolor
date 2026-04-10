use std::fmt;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub(crate) struct GpuCompositorProbe {
    pub(crate) adapter_name: String,
    pub(crate) backend: &'static str,
    pub(crate) texture_format: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
}

pub(crate) struct GpuSparkleFlinger {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    _queue: wgpu::Queue,
    probe: GpuCompositorProbe,
    surfaces: Option<GpuCompositorSurfaceSet>,
}

struct GpuCompositorSurfaceSet {
    width: u32,
    height: u32,
    input: GpuCompositorTexture,
    scratch: GpuCompositorTexture,
}

struct GpuCompositorTexture {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
}

impl GpuSparkleFlinger {
    pub(crate) fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .context("no compatible wgpu adapter was available for SparkleFlinger")?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("SparkleFlinger GPU compositor"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .context("failed to create a SparkleFlinger wgpu device")?;

        let info = adapter.get_info();
        let limits = device.limits();
        let probe = GpuCompositorProbe {
            adapter_name: info.name,
            backend: backend_name(info.backend),
            texture_format: texture_format_name(COMPOSITOR_TEXTURE_FORMAT),
            max_texture_dimension_2d: limits.max_texture_dimension_2d,
            max_storage_textures_per_shader_stage: limits.max_storage_textures_per_shader_stage,
        };

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            _queue: queue,
            probe,
            surfaces: None,
        })
    }

    pub(crate) fn describe(&self) -> GpuCompositorProbe {
        self.probe.clone()
    }

    pub(crate) fn ensure_surface_size(&mut self, width: u32, height: u32) -> Result<()> {
        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return Ok(());
        }

        self.surfaces = Some(GpuCompositorSurfaceSet::new(&self.device, width, height));
        Ok(())
    }

    pub(crate) fn surface_snapshot(&self) -> Option<GpuCompositorSurfaceSnapshot> {
        self.surfaces
            .as_ref()
            .map(GpuCompositorSurfaceSet::snapshot)
    }
}

impl fmt::Debug for GpuSparkleFlinger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuSparkleFlinger")
            .field("probe", &self.probe)
            .field("surface_snapshot", &self.surface_snapshot())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuCompositorSurfaceSnapshot {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) texture_format: &'static str,
}

const COMPOSITOR_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl GpuCompositorSurfaceSet {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            input: GpuCompositorTexture::new(device, width, height, "SparkleFlinger Input"),
            scratch: GpuCompositorTexture::new(device, width, height, "SparkleFlinger Scratch"),
        }
    }

    fn snapshot(&self) -> GpuCompositorSurfaceSnapshot {
        let _ = (&self.input, &self.scratch);
        GpuCompositorSurfaceSnapshot {
            width: self.width,
            height: self.height,
            texture_format: texture_format_name(COMPOSITOR_TEXTURE_FORMAT),
        }
    }
}

impl GpuCompositorTexture {
    fn new(device: &wgpu::Device, width: u32, height: u32, label: &'static str) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COMPOSITOR_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            _view: view,
        }
    }
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

fn texture_format_name(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => "rgba8_unorm",
        wgpu::TextureFormat::Rgba8UnormSrgb => "rgba8_unorm_srgb",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::GpuSparkleFlinger;

    #[test]
    fn gpu_compositor_probe_reports_a_texture_format() {
        let probe = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor.describe(),
            Err(_) => return,
        };

        assert!(!probe.adapter_name.is_empty());
        assert!(!probe.texture_format.is_empty());
    }

    #[test]
    fn gpu_compositor_reuses_matching_surface_sizes() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };

        compositor
            .ensure_surface_size(640, 480)
            .expect("first GPU surface allocation should succeed");
        let first = compositor
            .surface_snapshot()
            .expect("surface allocation should publish a snapshot");
        compositor
            .ensure_surface_size(640, 480)
            .expect("same-size GPU surface allocation should reuse existing textures");
        let second = compositor
            .surface_snapshot()
            .expect("surface snapshot should remain available");

        assert_eq!(first, second);
    }
}
