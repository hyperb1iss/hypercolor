use wgpu::util::DeviceExt;

use super::{
    COMPOSE_PARAM_BYTES, COMPOSITOR_TEXTURE_FORMAT, DISPLAY_FINALIZE_PARAM_BYTES,
    PREVIEW_SCALE_PARAM_BYTES, PendingUploadBuffers, SOURCE_COPY_PARAM_BYTES,
};

const COMPOSE_PARAM_RING_SLOTS: u64 = 64;
const SOURCE_COPY_PARAM_RING_SLOTS: u64 = 32;
const DISPLAY_FINALIZE_PARAM_RING_SLOTS: u64 = 32;
const PREVIEW_SCALE_PARAM_RING_SLOTS: u64 = 16;

/// A persistent uniform buffer organized as a ring of aligned slots.
///
/// Compositor command encoders can stay unsubmitted across calls
/// (`pending_output_submission`), so a uniform slot must never be rewritten
/// while any not-yet-submitted encoder still references it. Every write goes
/// to a fresh slot via `queue.write_buffer`, and dispatches bind the slot
/// through a dynamic offset. `watermark` marks the oldest write that may
/// still be referenced by an unsubmitted encoder; it only advances at points
/// where no unsubmitted encoder exists (see
/// `GpuSparkleFlinger::release_retired_uniform_slots`). If allocating the
/// next slot would clobber a write at or after the watermark, the ring falls
/// back to the pre-ring staging path: a one-shot `create_buffer_init` buffer
/// copied into a dedicated overflow slot inside the current encoder. Those
/// copies travel with their encoder, so each dispatch reads the value its own
/// encoder wrote regardless of submission order.
pub(super) struct UniformParamsRing {
    buffer: wgpu::Buffer,
    label: &'static str,
    param_bytes: u32,
    slot_stride: u32,
    slot_count: u64,
    overflow_offset: u32,
    cursor: u64,
    watermark: u64,
    #[cfg(test)]
    pub(super) ring_write_count: usize,
    #[cfg(test)]
    pub(super) fallback_write_count: usize,
}

/// Result of a params upload: the dynamic offset to bind for the dispatch,
/// and whether the offset may be reused by a later byte-identical dispatch.
pub(super) struct UniformParamsWrite {
    pub(super) offset: u32,
    pub(super) reusable: bool,
}

impl UniformParamsRing {
    fn new(device: &wgpu::Device, label: &'static str, param_bytes: u32, slot_count: u64) -> Self {
        let alignment = device.limits().min_uniform_buffer_offset_alignment.max(1);
        let slot_stride = param_bytes.div_ceil(alignment) * alignment;
        let overflow_offset = u32::try_from(slot_count)
            .expect("uniform ring slot count should fit in u32")
            * slot_stride;
        // One extra slot holds overflow writes staged through encoder copies.
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: u64::from(slot_stride) * (slot_count + 1),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            label,
            param_bytes,
            slot_stride,
            slot_count,
            overflow_offset,
            cursor: 0,
            watermark: 0,
            #[cfg(test)]
            ring_write_count: 0,
            #[cfg(test)]
            fallback_write_count: 0,
        }
    }

    pub(super) fn binding(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: 0,
            size: Some(
                wgpu::BufferSize::new(u64::from(self.param_bytes))
                    .expect("uniform params size should be non-zero"),
            ),
        })
    }

    pub(super) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pending_upload_buffers: &mut PendingUploadBuffers,
        params: &[u8],
    ) -> UniformParamsWrite {
        debug_assert_eq!(params.len(), self.param_bytes as usize);
        if self.cursor.saturating_sub(self.watermark) >= self.slot_count {
            // Overflow: the next ring slot may still be referenced by an
            // unsubmitted encoder, so stage the write through an encoder
            // copy into the dedicated overflow slot instead.
            let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(self.label),
                contents: params,
                usage: wgpu::BufferUsages::COPY_SRC,
            });
            encoder.copy_buffer_to_buffer(
                &upload,
                0,
                &self.buffer,
                u64::from(self.overflow_offset),
                u64::from(self.param_bytes),
            );
            pending_upload_buffers.push(upload);
            #[cfg(test)]
            {
                self.fallback_write_count = self.fallback_write_count.saturating_add(1);
            }
            return UniformParamsWrite {
                offset: self.overflow_offset,
                reusable: false,
            };
        }
        let offset = self.slot_offset(self.cursor);
        queue.write_buffer(&self.buffer, u64::from(offset), params);
        self.cursor += 1;
        #[cfg(test)]
        {
            self.ring_write_count = self.ring_write_count.saturating_add(1);
        }
        UniformParamsWrite {
            offset,
            reusable: true,
        }
    }

    /// Pins the most recent ring write so its slot cannot be reused while a
    /// dispatch that re-binds it (params dedup) is still unsubmitted. Without
    /// this, a retired slot re-bound by a fresh dispatch could be rewritten
    /// before that dispatch's encoder is submitted.
    pub(super) fn pin_last_slot(&mut self) {
        if let Some(last) = self.cursor.checked_sub(1) {
            self.watermark = self.watermark.min(last);
        }
    }

    /// Marks every written slot as reusable. Callers must guarantee that no
    /// unsubmitted encoder references the ring when this is called.
    fn release_retired_slots(&mut self) {
        self.watermark = self.cursor;
    }

    fn slot_offset(&self, cursor: u64) -> u32 {
        u32::try_from(cursor % self.slot_count).expect("ring slot index should fit in u32")
            * self.slot_stride
    }

    #[cfg(test)]
    pub(super) fn set_slot_count_for_test(&mut self, slot_count: u64) {
        assert!(slot_count >= 1 && slot_count <= self.slot_count);
        self.slot_count = slot_count;
    }
}

pub(super) struct GpuCompositorPipeline {
    pub(super) compose_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) compose_pipeline: wgpu::ComputePipeline,
    pub(super) compose_params: UniformParamsRing,
    pub(super) source_copy_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) source_copy_pipeline: wgpu::ComputePipeline,
    pub(super) source_copy_params: UniformParamsRing,
    pub(super) display_finalize_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) display_finalize_pipeline: wgpu::ComputePipeline,
    pub(super) display_finalize_yuv_pipeline: wgpu::ComputePipeline,
    pub(super) display_finalize_params: UniformParamsRing,
    pub(super) preview_scale_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) preview_scale_pipeline: wgpu::ComputePipeline,
    pub(super) preview_scale_params: UniformParamsRing,
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
                            has_dynamic_offset: true,
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
        let compose_params = UniformParamsRing::new(
            device,
            "SparkleFlinger GPU compose params",
            COMPOSE_PARAM_BYTES as u32,
            COMPOSE_PARAM_RING_SLOTS,
        );
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
                            has_dynamic_offset: true,
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
        let source_copy_params = UniformParamsRing::new(
            device,
            "SparkleFlinger GPU source copy params",
            SOURCE_COPY_PARAM_BYTES as u32,
            SOURCE_COPY_PARAM_RING_SLOTS,
        );
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
                            has_dynamic_offset: true,
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
        let display_finalize_params = UniformParamsRing::new(
            device,
            "SparkleFlinger GPU display finalize params",
            DISPLAY_FINALIZE_PARAM_BYTES as u32,
            DISPLAY_FINALIZE_PARAM_RING_SLOTS,
        );
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
                            has_dynamic_offset: true,
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
        let preview_scale_params = UniformParamsRing::new(
            device,
            "SparkleFlinger GPU preview scale params",
            PREVIEW_SCALE_PARAM_BYTES as u32,
            PREVIEW_SCALE_PARAM_RING_SLOTS,
        );

        Self {
            compose_bind_group_layout,
            compose_pipeline,
            compose_params,
            source_copy_bind_group_layout,
            source_copy_pipeline,
            source_copy_params,
            display_finalize_bind_group_layout,
            display_finalize_pipeline,
            display_finalize_yuv_pipeline,
            display_finalize_params,
            preview_scale_bind_group_layout,
            preview_scale_pipeline,
            preview_scale_params,
        }
    }

    /// Marks every written uniform ring slot as reusable. Callers must
    /// guarantee that no unsubmitted command encoder references the rings.
    pub(super) fn release_retired_uniform_slots(&mut self) {
        self.compose_params.release_retired_slots();
        self.source_copy_params.release_retired_slots();
        self.display_finalize_params.release_retired_slots();
        self.preview_scale_params.release_retired_slots();
    }
}
