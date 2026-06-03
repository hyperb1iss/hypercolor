use super::{
    COMPOSE_PARAM_BYTES, COMPOSITOR_TEXTURE_FORMAT, DISPLAY_FINALIZE_PARAM_BYTES,
    PREVIEW_SCALE_PARAM_BYTES, SOURCE_COPY_PARAM_BYTES,
};

pub(super) struct GpuCompositorPipeline {
    pub(super) compose_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) compose_pipeline: wgpu::ComputePipeline,
    pub(super) params_buffer: wgpu::Buffer,
    pub(super) source_copy_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) source_copy_pipeline: wgpu::ComputePipeline,
    pub(super) source_copy_params_buffer: wgpu::Buffer,
    pub(super) display_finalize_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) display_finalize_pipeline: wgpu::ComputePipeline,
    pub(super) display_finalize_yuv_pipeline: wgpu::ComputePipeline,
    pub(super) display_finalize_params_buffer: wgpu::Buffer,
    pub(super) preview_scale_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) preview_scale_pipeline: wgpu::ComputePipeline,
    pub(super) preview_scale_params_buffer: wgpu::Buffer,
}

impl GpuCompositorPipeline {
    pub(super) fn new(device: &wgpu::Device) -> Self {
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../blend.wgsl").into()),
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
        let source_copy_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SparkleFlinger GPU source copy bind group layout"),
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
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: COMPOSITOR_TEXTURE_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: Some(
                                wgpu::BufferSize::new(SOURCE_COPY_PARAM_BYTES as u64)
                                    .expect("source copy uniform buffer size should be non-zero"),
                            ),
                        },
                        count: None,
                    },
                ],
            });
        let source_copy_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("SparkleFlinger GPU source copy pipeline layout"),
                bind_group_layouts: &[Some(&source_copy_bind_group_layout)],
                immediate_size: 0,
            });
        let source_copy_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU source copy shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../source_copy.wgsl").into()),
        });
        let source_copy_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("SparkleFlinger GPU source copy pipeline"),
                layout: Some(&source_copy_pipeline_layout),
                module: &source_copy_shader,
                entry_point: Some("copy_source"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let source_copy_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU source copy params"),
            size: SOURCE_COPY_PARAM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let display_finalize_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SparkleFlinger GPU display finalize bind group layout"),
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
                                wgpu::BufferSize::new(DISPLAY_FINALIZE_PARAM_BYTES as u64).expect(
                                    "display finalize uniform buffer size should be non-zero",
                                ),
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let display_finalize_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("SparkleFlinger GPU display finalize pipeline layout"),
                bind_group_layouts: &[Some(&display_finalize_bind_group_layout)],
                immediate_size: 0,
            });
        let display_finalize_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU display finalize shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../display_finalize.wgsl").into()),
        });
        let display_finalize_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("SparkleFlinger GPU display finalize pipeline"),
                layout: Some(&display_finalize_pipeline_layout),
                module: &display_finalize_shader,
                entry_point: Some("finalize_display"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let display_finalize_yuv_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("SparkleFlinger GPU display finalize YUV pipeline"),
                layout: Some(&display_finalize_pipeline_layout),
                module: &display_finalize_shader,
                entry_point: Some("finalize_display_yuv420"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let display_finalize_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU display finalize params"),
            size: DISPLAY_FINALIZE_PARAM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let preview_scale_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SparkleFlinger GPU preview scale bind group layout"),
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
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: Some(
                                wgpu::BufferSize::new(PREVIEW_SCALE_PARAM_BYTES as u64)
                                    .expect("preview scale uniform buffer size should be non-zero"),
                            ),
                        },
                        count: None,
                    },
                ],
            });
        let preview_scale_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("SparkleFlinger GPU preview scale pipeline layout"),
                bind_group_layouts: &[Some(&preview_scale_bind_group_layout)],
                immediate_size: 0,
            });
        let preview_scale_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU preview scale shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../preview_scale.wgsl").into()),
        });
        let preview_scale_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("SparkleFlinger GPU preview scale pipeline"),
                layout: Some(&preview_scale_pipeline_layout),
                module: &preview_scale_shader,
                entry_point: Some("scale_preview"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let preview_scale_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU preview scale params"),
            size: PREVIEW_SCALE_PARAM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            compose_bind_group_layout,
            compose_pipeline,
            params_buffer,
            source_copy_bind_group_layout,
            source_copy_pipeline,
            source_copy_params_buffer,
            display_finalize_bind_group_layout,
            display_finalize_pipeline,
            display_finalize_yuv_pipeline,
            display_finalize_params_buffer,
            preview_scale_bind_group_layout,
            preview_scale_pipeline,
            preview_scale_params_buffer,
        }
    }
}
