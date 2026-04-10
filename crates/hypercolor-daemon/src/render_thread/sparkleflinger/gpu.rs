use std::fmt;
use std::sync::mpsc;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, Canvas};

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, publish_composed_frame,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::ProducerFrame;

const COMPOSITOR_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const COMPOSE_WORKGROUP_WIDTH: u32 = 8;
const COMPOSE_WORKGROUP_HEIGHT: u32 = 8;
const COMPOSE_PARAM_BYTES: usize = 48;

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
    queue: wgpu::Queue,
    probe: GpuCompositorProbe,
    pipeline: GpuCompositorPipeline,
    surfaces: Option<GpuCompositorSurfaceSet>,
}

struct GpuCompositorPipeline {
    compose_bind_group_layout: wgpu::BindGroupLayout,
    compose_pipeline: wgpu::ComputePipeline,
    params_buffer: wgpu::Buffer,
}

struct GpuCompositorSurfaceSet {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    front: GpuCompositorTexture,
    back: GpuCompositorTexture,
    source: GpuCompositorTexture,
    readback: wgpu::Buffer,
}

struct GpuCompositorTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuCompositorSurfaceSnapshot {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) texture_format: &'static str,
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

        let format_features = adapter.get_texture_format_features(COMPOSITOR_TEXTURE_FORMAT);
        if !format_features
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING)
        {
            anyhow::bail!(
                "adapter does not support storage textures for {}",
                texture_format_name(COMPOSITOR_TEXTURE_FORMAT)
            );
        }

        let info = adapter.get_info();
        let limits = device.limits();
        let probe = GpuCompositorProbe {
            adapter_name: info.name,
            backend: backend_name(info.backend),
            texture_format: texture_format_name(COMPOSITOR_TEXTURE_FORMAT),
            max_texture_dimension_2d: limits.max_texture_dimension_2d,
            max_storage_textures_per_shader_stage: limits.max_storage_textures_per_shader_stage,
        };

        let pipeline = GpuCompositorPipeline::new(&device);

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            probe,
            pipeline,
            surfaces: None,
        })
    }

    pub(crate) fn describe(&self) -> GpuCompositorProbe {
        self.probe.clone()
    }

    pub(crate) fn supports_plan(&self, plan: &CompositionPlan) -> bool {
        plan.width > 0 && plan.height > 0 && !plan.layers.is_empty()
    }

    pub(crate) fn compose(&mut self, plan: CompositionPlan) -> Result<ComposedFrameSet> {
        let CompositionPlan {
            width,
            height,
            mut layers,
        } = plan;

        if layers.len() == 1
            && let Some(layer) = layers.pop()
            && layer.is_bypass_candidate()
        {
            let mut composed = publish_composed_frame(layer.frame.into_render_frame(), true);
            composed.backend = CompositorBackendKind::Gpu;
            return Ok(composed);
        }

        self.ensure_surface_size(width, height);
        let surfaces = self
            .surfaces
            .as_ref()
            .expect("surface allocation should succeed before composition");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU compose"),
            });

        let mut use_front_as_current = true;
        let mut layers = layers.into_iter();
        let first_layer = layers
            .next()
            .context("GPU composition requires at least one layer")?;

        if first_layer.mode == CompositionMode::Replace && first_layer.opacity >= 1.0 {
            upload_frame_into_texture(&self.queue, &surfaces.front.texture, first_layer.frame);
        } else {
            let full_range = wgpu::ImageSubresourceRange::default();
            encoder.clear_texture(&surfaces.front.texture, &full_range);
            compose_layer_into_gpu(
                &self.device,
                &self.queue,
                &self.pipeline,
                surfaces,
                &mut encoder,
                first_layer,
                true,
            );
            use_front_as_current = false;
        }

        for layer in layers {
            compose_layer_into_gpu(
                &self.device,
                &self.queue,
                &self.pipeline,
                surfaces,
                &mut encoder,
                layer,
                use_front_as_current,
            );
            use_front_as_current = !use_front_as_current;
        }

        let current_texture = if use_front_as_current {
            &surfaces.front.texture
        } else {
            &surfaces.back.texture
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: current_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &surfaces.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(surfaces.padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            texture_extent(width, height),
        );

        let bytes = read_back_texture(
            &self.device,
            &surfaces.readback,
            width,
            height,
            surfaces.padded_bytes_per_row,
            self.queue.submit(Some(encoder.finish())),
        )?;
        let canvas = Canvas::from_vec(bytes, width, height);
        let mut composed = publish_composed_frame((canvas, None), false);
        composed.backend = CompositorBackendKind::Gpu;
        Ok(composed)
    }

    pub(crate) fn ensure_surface_size(&mut self, width: u32, height: u32) {
        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return;
        }

        self.surfaces = Some(GpuCompositorSurfaceSet::new(&self.device, width, height));
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

impl GpuCompositorPipeline {
    fn new(device: &wgpu::Device) -> Self {
        let compose_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SparkleFlinger GPU compose bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: COMPOSITOR_TEXTURE_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: Some(
                                wgpu::BufferSize::new(COMPOSE_PARAM_BYTES as u64)
                                    .expect("uniform buffer size should be non-zero"),
                            ),
                        },
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SparkleFlinger GPU compose pipeline layout"),
            bind_group_layouts: &[Some(&compose_bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU compose shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blend.wgsl").into()),
        });
        let compose_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SparkleFlinger GPU compose pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU compose params"),
            size: COMPOSE_PARAM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            compose_bind_group_layout,
            compose_pipeline,
            params_buffer,
        }
    }
}

impl GpuCompositorSurfaceSet {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            width,
            height,
            padded_bytes_per_row,
            front: GpuCompositorTexture::new(device, width, height, "SparkleFlinger Front"),
            back: GpuCompositorTexture::new(device, width, height, "SparkleFlinger Back"),
            source: GpuCompositorTexture::new(device, width, height, "SparkleFlinger Source"),
            readback,
        }
    }

    fn snapshot(&self) -> GpuCompositorSurfaceSnapshot {
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
            size: texture_extent(width, height),
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
        Self { texture, view }
    }
}

fn compose_layer_into_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &GpuCompositorPipeline,
    surfaces: &GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    layer: CompositionLayer,
    use_front_as_current: bool,
) {
    let current = if use_front_as_current {
        &surfaces.front
    } else {
        &surfaces.back
    };
    let output = if use_front_as_current {
        &surfaces.back
    } else {
        &surfaces.front
    };

    let shader_mode = if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        ComposeShaderMode::Replace
    } else {
        match layer.mode {
            CompositionMode::Replace | CompositionMode::Alpha => ComposeShaderMode::Alpha,
            CompositionMode::Add => ComposeShaderMode::Add,
            CompositionMode::Screen => ComposeShaderMode::Screen,
        }
    };
    upload_frame_into_texture(queue, &surfaces.source.texture, layer.frame);
    if shader_mode == ComposeShaderMode::Replace {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &surfaces.source.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &output.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(surfaces.width, surfaces.height),
        );
        return;
    }

    queue.write_buffer(
        &pipeline.params_buffer,
        0,
        &encode_compose_params(surfaces.width, surfaces.height, shader_mode, layer.opacity),
    );
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("SparkleFlinger GPU compose bind group"),
        layout: &pipeline.compose_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&current.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&surfaces.source.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&output.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.params_buffer.as_entire_binding(),
            },
        ],
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU compose pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.compose_pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.dispatch_workgroups(
        surfaces.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
        surfaces.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
        1,
    );
    drop(pass);
}

fn upload_frame_into_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, frame: ProducerFrame) {
    let (canvas, _) = frame.into_render_frame();
    let bytes_per_row = canvas.width() * BYTES_PER_PIXEL as u32;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        canvas.as_rgba_bytes(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(canvas.height()),
        },
        texture_extent(canvas.width(), canvas.height()),
    );
}

fn read_back_texture(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    submission_index: wgpu::SubmissionIndex,
) -> Result<Vec<u8>> {
    let slice = buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: None,
        })
        .context("GPU readback poll failed")?;
    receiver
        .recv()
        .context("GPU readback channel closed before map completion")?
        .context("GPU readback buffer mapping failed")?;

    let mapped = slice.get_mapped_range();
    let unpadded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
    let mut bytes = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
    for row in mapped
        .chunks(usize::try_from(padded_bytes_per_row).expect("row pitch should fit in usize"))
        .take(height as usize)
    {
        bytes.extend_from_slice(
            &row[..usize::try_from(unpadded_bytes_per_row).expect("row width should fit in usize")],
        );
    }
    drop(mapped);
    buffer.unmap();

    Ok(bytes)
}

fn texture_extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * BYTES_PER_PIXEL as u32;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(alignment) * alignment
}

fn encode_compose_params(
    width: u32,
    height: u32,
    mode: ComposeShaderMode,
    opacity: f32,
) -> [u8; COMPOSE_PARAM_BYTES] {
    let mut bytes = [0u8; COMPOSE_PARAM_BYTES];
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&(mode as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&opacity.to_le_bytes());
    bytes
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeShaderMode {
    Replace = 0,
    Alpha = 1,
    Add = 2,
    Screen = 3,
}

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{Canvas, PublishedSurface, Rgba};

    use super::GpuSparkleFlinger;
    use crate::render_thread::producer_queue::ProducerFrame;
    use crate::render_thread::sparkleflinger::{
        CompositionLayer, CompositionPlan, cpu::CpuSparkleFlinger,
    };

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(color);
        canvas
    }

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

        compositor.ensure_surface_size(640, 480);
        let first = compositor
            .surface_snapshot()
            .expect("surface allocation should publish a snapshot");
        compositor.ensure_surface_size(640, 480);
        let second = compositor
            .surface_snapshot()
            .expect("surface snapshot should remain available");

        assert_eq!(first, second);
    }

    #[test]
    fn gpu_compositor_matches_cpu_alpha_composition() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };

        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                    255, 32, 0, 255,
                )))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(32, 64, 255, 255))),
                    0.35,
                ),
            ],
        );
        let expected = CpuSparkleFlinger::new().compose(plan.clone());
        let composed = compositor
            .compose(plan)
            .expect("GPU composition should succeed for replace + alpha plans");

        assert_eq!(
            composed.sampling_canvas.as_rgba_bytes(),
            expected.sampling_canvas.as_rgba_bytes()
        );
    }

    #[test]
    fn gpu_compositor_matches_cpu_add_composition() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };

        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                    32, 12, 96, 255,
                )))),
                CompositionLayer::add(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(96, 64, 48, 255))),
                    0.4,
                ),
            ],
        );
        let expected = CpuSparkleFlinger::new().compose(plan.clone());
        let composed = compositor
            .compose(plan)
            .expect("GPU composition should succeed for add plans");

        assert_eq!(
            composed.sampling_canvas.as_rgba_bytes(),
            expected.sampling_canvas.as_rgba_bytes()
        );
    }

    #[test]
    fn gpu_compositor_matches_cpu_screen_composition() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };

        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                    12, 120, 48, 255,
                )))),
                CompositionLayer::screen(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(200, 32, 64, 255))),
                    0.6,
                ),
            ],
        );
        let expected = CpuSparkleFlinger::new().compose(plan.clone());
        let composed = compositor
            .compose(plan)
            .expect("GPU composition should succeed for screen plans");

        assert_eq!(
            composed.sampling_canvas.as_rgba_bytes(),
            expected.sampling_canvas.as_rgba_bytes()
        );
    }

    #[test]
    fn gpu_compositor_bypasses_single_replace_surfaces() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let source =
            PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(12, 34, 56, 255)), 1, 2);
        let composed = compositor
            .compose(CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
            ))
            .expect("single replace surface should bypass GPU composition");

        let surface = composed
            .sampling_surface
            .expect("bypass path should preserve the source surface");
        assert_eq!(surface.rgba_bytes().as_ptr(), source.rgba_bytes().as_ptr());
    }
}
