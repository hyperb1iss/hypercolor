use std::fmt;
use std::sync::mpsc;

use anyhow::{Context, Result};
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, Canvas, PublishedSurface, PublishedSurfaceStorageIdentity, RenderSurfacePool,
    SurfaceDescriptor,
};
use hypercolor_types::event::ZoneColors;

use super::{ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::ProducerFrame;
use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSamplingPlan, GpuSamplingPlanKey, GpuSpatialSampler,
};

const COMPOSITOR_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const COMPOSE_WORKGROUP_WIDTH: u32 = 8;
const COMPOSE_WORKGROUP_HEIGHT: u32 = 8;
const COMPOSE_PARAM_BYTES: usize = 48;
const PREVIEW_SCALE_PARAM_BYTES: usize = 16;

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
    spatial_sampler: GpuSpatialSampler,
    surfaces: Option<GpuCompositorSurfaceSet>,
    preview_surfaces: Option<GpuPreviewSurfaceSet>,
    current_output: Option<GpuCompositorOutputSurface>,
    cached_composition_key: Option<CachedReadbackKey>,
    cached_readback_surface: Option<CachedReadbackSurface>,
    cached_preview_surface: Option<CachedPreviewSurface>,
    pending_output_submission: Option<wgpu::CommandEncoder>,
    pending_preview_readback: Option<PendingPreviewReadback>,
    ready_preview_surface: Option<PublishedSurface>,
    output_generation: u64,
    cached_sample_result: Option<CachedSampleResult>,
}

struct GpuCompositorPipeline {
    compose_bind_group_layout: wgpu::BindGroupLayout,
    compose_pipeline: wgpu::ComputePipeline,
    params_buffer: wgpu::Buffer,
    preview_scale_bind_group_layout: wgpu::BindGroupLayout,
    preview_scale_pipeline: wgpu::ComputePipeline,
    preview_scale_params_buffer: wgpu::Buffer,
}

struct GpuCompositorSurfaceSet {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    front: GpuCompositorTexture,
    back: GpuCompositorTexture,
    source: GpuCompositorTexture,
    bind_groups: GpuCompositorBindGroups,
    readback: wgpu::Buffer,
    readback_surfaces: RenderSurfacePool,
    front_contents: Option<CachedSourceUpload>,
    back_contents: Option<CachedSourceUpload>,
    cached_source_upload: Option<CachedSourceUpload>,
    #[cfg(test)]
    front_upload_count: usize,
    #[cfg(test)]
    source_upload_count: usize,
    #[cfg(test)]
    compose_dispatch_count: usize,
}

struct GpuPreviewSurfaceSet {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    texture: GpuCompositorTexture,
    bind_groups: GpuPreviewScaleBindGroups,
    readback: wgpu::Buffer,
    readback_surfaces: RenderSurfacePool,
    cached_scale_params: Option<[u8; PREVIEW_SCALE_PARAM_BYTES]>,
    #[cfg(test)]
    scale_param_write_count: usize,
    #[cfg(test)]
    preview_bind_group_count: usize,
}

struct GpuCompositorTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct GpuCompositorBindGroups {
    front_to_back: wgpu::BindGroup,
    back_to_front: wgpu::BindGroup,
}

struct GpuPreviewScaleBindGroups {
    front_to_preview: wgpu::BindGroup,
    back_to_preview: wgpu::BindGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CachedSourceUpload {
    storage: PublishedSurfaceStorageIdentity,
    generation: u64,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedReadbackKey {
    width: u32,
    height: u32,
    layers: Vec<CachedReadbackLayer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CachedReadbackLayer {
    source: CachedSourceUpload,
    mode: CompositionMode,
    opacity_bits: u32,
}

#[derive(Debug, Clone)]
struct CachedReadbackSurface {
    key: Option<CachedReadbackKey>,
    surface: PublishedSurface,
}

#[derive(Debug, Clone)]
struct CachedPreviewSurface {
    key: CachedPreviewSurfaceKey,
    surface: PublishedSurface,
}

#[derive(Debug, Clone)]
enum PendingPreviewReadback {
    FullSize {
        width: u32,
        height: u32,
        readback_key: Option<CachedReadbackKey>,
    },
    Scaled {
        request: PreviewSurfaceRequest,
        readback_key: Option<CachedReadbackKey>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedPreviewSurfaceKey {
    composition: CachedReadbackKey,
    request: PreviewSurfaceRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CachedSampleResultKey {
    output_generation: u64,
    sampling_plan: GpuSamplingPlanKey,
}

#[derive(Debug, Clone)]
struct CachedSampleResult {
    key: CachedSampleResultKey,
    zones: Vec<ZoneColors>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuCompositorSurfaceSnapshot {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) texture_format: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuCompositorOutputSurface {
    Front,
    Back,
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
        let spatial_sampler = GpuSpatialSampler::new(&device);

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            probe,
            pipeline,
            spatial_sampler,
            surfaces: None,
            preview_surfaces: None,
            current_output: None,
            cached_composition_key: None,
            cached_readback_surface: None,
            cached_preview_surface: None,
            pending_output_submission: None,
            pending_preview_readback: None,
            ready_preview_surface: None,
            output_generation: 0,
            cached_sample_result: None,
        })
    }

    pub(crate) fn describe(&self) -> GpuCompositorProbe {
        self.probe.clone()
    }

    pub(crate) fn supports_plan(&self, plan: &CompositionPlan) -> bool {
        plan.width > 0 && plan.height > 0 && !plan.layers.is_empty()
    }

    pub(crate) fn can_sample_zone_plan(&self, prepared_zones: &[PreparedZonePlan]) -> bool {
        GpuSamplingPlan::supports_prepared_zones(prepared_zones)
    }

    pub(crate) fn compose(
        &mut self,
        plan: &CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
        let requires_preview_surface = preview_surface_request.is_some();
        let readback_key = cached_readback_key(plan);
        if plan.layers.len() == 1
            && let Some(layer) = plan.layers.first()
            && layer.is_bypass_candidate()
            && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
        {
            self.pending_output_submission = None;
            self.pending_preview_readback = None;
            self.ready_preview_surface = None;
            return Ok(self.compose_bypass_layer(
                plan,
                readback_key,
                layer,
                requires_cpu_sampling_canvas,
                preview_surface_request,
            ));
        }

        self.ensure_surface_size(plan.width, plan.height);
        if let Some(key) = readback_key.as_ref()
            && self.current_output.is_some()
            && self.cached_composition_key.as_ref() == Some(key)
        {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_composed_without_surfaces());
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(cached) = self.cached_preview_surface.as_ref()
                && cached.key
                    == (CachedPreviewSurfaceKey {
                        composition: key.clone(),
                        request,
                    })
            {
                self.pending_output_submission = None;
                return Ok(gpu_composed_with_preview_surface(cached.surface.clone()));
            }
            if let Some(cached) = self.cached_readback_surface.as_ref()
                && cached.key.as_ref() == Some(key)
                && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
            {
                self.pending_output_submission = None;
                return Ok(gpu_composed_from_surface(
                    cached.surface.clone(),
                    requires_cpu_sampling_canvas,
                ));
            }
            let pending_output_submission = self.pending_output_submission.take();
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                Some(key.clone()),
                requires_cpu_sampling_canvas,
                preview_surface_request,
                pending_output_submission,
            );
        }
        self.pending_output_submission = None;
        self.pending_preview_readback = None;
        self.ready_preview_surface = None;

        let surfaces = self
            .surfaces
            .as_mut()
            .expect("surface allocation should succeed before composition");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU compose"),
            });

        let mut use_front_as_current = true;
        let mut layers = plan.layers.iter();
        let first_layer = layers
            .next()
            .context("GPU composition requires at least one layer")?;

        if first_layer.mode == CompositionMode::Replace && first_layer.opacity >= 1.0 {
            upload_frame_into_cached_texture(
                &self.queue,
                &surfaces.front.texture,
                &mut surfaces.front_contents,
                &first_layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
        } else {
            let full_range = wgpu::ImageSubresourceRange::default();
            encoder.clear_texture(&surfaces.front.texture, &full_range);
            surfaces.front_contents = None;
            compose_layer_into_gpu(
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
                &self.queue,
                &self.pipeline,
                surfaces,
                &mut encoder,
                layer,
                use_front_as_current,
            );
            use_front_as_current = !use_front_as_current;
        }

        let current_output = if use_front_as_current {
            GpuCompositorOutputSurface::Front
        } else {
            GpuCompositorOutputSurface::Back
        };
        self.current_output = Some(current_output);
        self.cached_composition_key = readback_key.clone();
        self.output_generation = self.output_generation.saturating_add(1);
        self.cached_sample_result = None;
        if !requires_cpu_sampling_canvas && !requires_preview_surface {
            self.pending_output_submission = Some(encoder);
            return Ok(gpu_composed_without_surfaces());
        }

        if let Some(key) = readback_key.as_ref()
            && let Some(cached) = self.cached_readback_surface.as_ref()
            && cached.key.as_ref() == Some(key)
            && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
        {
            self.queue.submit(Some(encoder.finish()));
            return Ok(gpu_composed_from_surface(
                cached.surface.clone(),
                requires_cpu_sampling_canvas,
            ));
        }

        self.read_back_current_output_surface(
            plan.width,
            plan.height,
            readback_key,
            requires_cpu_sampling_canvas,
            preview_surface_request,
            Some(encoder),
        )
    }

    pub(crate) fn sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        let pending_output_submission = self.pending_output_submission.take();
        let sampling_plan = GpuSamplingPlan::key(prepared_zones);
        if let Some(sampling_plan) = sampling_plan
            && let Some(cached) = self.cached_sample_result.as_ref()
            && cached.key
                == (CachedSampleResultKey {
                    output_generation: self.output_generation,
                    sampling_plan,
                })
        {
            zones.clone_from(&cached.zones);
            return Ok(true);
        }
        let Some(output) = self.current_output else {
            return Ok(false);
        };
        let Some(surfaces) = self.surfaces.as_ref() else {
            return Ok(false);
        };
        let source_view = match output {
            GpuCompositorOutputSurface::Front => &surfaces.front.view,
            GpuCompositorOutputSurface::Back => &surfaces.back.view,
        };
        let (sampled, submission_index) = self.spatial_sampler.sample_texture_into(
            &self.device,
            &self.queue,
            source_view,
            surfaces.width,
            surfaces.height,
            prepared_zones,
            zones,
            pending_output_submission,
        )?;
        if let Some(submission_index) = submission_index {
            self.resolve_pending_preview_surface_for_submission(submission_index)?;
        }
        if sampled && let Some(sampling_plan) = sampling_plan {
            self.cached_sample_result = Some(CachedSampleResult {
                key: CachedSampleResultKey {
                    output_generation: self.output_generation,
                    sampling_plan,
                },
                zones: zones.clone(),
            });
        }
        Ok(sampled)
    }

    pub(crate) fn resolve_preview_surface(&mut self) -> Result<Option<PublishedSurface>> {
        if let Some(surface) = self.ready_preview_surface.take() {
            return Ok(Some(surface));
        }

        let Some(pending_preview_readback) = self.pending_preview_readback.take() else {
            return Ok(None);
        };
        let encoder = self.pending_output_submission.take().unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU preview finalize"),
                })
        });
        let submission_index = self.queue.submit(Some(encoder.finish()));
        self.finish_pending_preview_readback(pending_preview_readback, submission_index)
            .map(Some)
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

        self.surfaces = Some(GpuCompositorSurfaceSet::new(
            &self.device,
            &self.pipeline,
            width,
            height,
        ));
        self.preview_surfaces = None;
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surface = None;
        self.pending_output_submission = None;
        self.pending_preview_readback = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
    }

    pub(crate) fn surface_snapshot(&self) -> Option<GpuCompositorSurfaceSnapshot> {
        self.surfaces
            .as_ref()
            .map(GpuCompositorSurfaceSet::snapshot)
    }

    fn compose_bypass_layer(
        &mut self,
        plan: &CompositionPlan,
        readback_key: Option<CachedReadbackKey>,
        layer: &CompositionLayer,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> ComposedFrameSet {
        let requires_preview_surface = preview_surface_request.is_some();
        let same_surface_canvas = match &layer.frame {
            ProducerFrame::Canvas(canvas) => {
                self.current_output == Some(GpuCompositorOutputSurface::Front)
                    && self.cached_readback_surface.as_ref().is_some_and(|cached| {
                        cached.surface.width() == plan.width
                            && cached.surface.height() == plan.height
                            && cached.surface.storage_identity() == canvas.storage_identity()
                    })
            }
            ProducerFrame::Surface(_) => false,
        };
        let same_output = readback_key.as_ref().is_some_and(|key| {
            self.current_output == Some(GpuCompositorOutputSurface::Front)
                && self.cached_composition_key.as_ref() == Some(key)
        }) || same_surface_canvas;
        if same_output {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return gpu_bypassed_without_surfaces();
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(key) = readback_key.as_ref()
                && let Some(cached) = self.cached_preview_surface.as_ref()
                && cached.key
                    == (CachedPreviewSurfaceKey {
                        composition: key.clone(),
                        request,
                    })
            {
                return gpu_composed_with_preview_surface(cached.surface.clone());
            }
            if let Some(cached) = self.cached_readback_surface.as_ref()
                && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
            {
                return gpu_bypassed_surface_frame(
                    &cached.surface,
                    requires_cpu_sampling_canvas,
                    requires_preview_surface,
                );
            }
        }

        self.ensure_surface_size(plan.width, plan.height);
        if let Some(surfaces) = self.surfaces.as_mut() {
            upload_frame_into_cached_texture(
                &self.queue,
                &surfaces.front.texture,
                &mut surfaces.front_contents,
                &layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
            surfaces.back_contents = None;
        }
        self.current_output = Some(GpuCompositorOutputSurface::Front);
        self.cached_composition_key = readback_key.clone();
        if !same_output {
            self.output_generation = self.output_generation.saturating_add(1);
            self.cached_sample_result = None;
        }

        let mut composed = match &layer.frame {
            ProducerFrame::Surface(surface) => gpu_bypassed_surface_frame(
                surface,
                requires_cpu_sampling_canvas,
                requires_preview_surface,
            ),
            ProducerFrame::Canvas(canvas) => gpu_bypassed_canvas_frame(
                canvas,
                requires_cpu_sampling_canvas,
                requires_preview_surface,
            ),
        };
        let cached_surface = composed
            .preview_surface
            .as_ref()
            .or(composed.sampling_surface.as_ref())
            .cloned()
            .or_else(|| bypass_preview_surface(&layer.frame));
        self.cached_readback_surface = cached_surface.map(|surface| CachedReadbackSurface {
            key: readback_key,
            surface,
        });
        composed.backend = CompositorBackendKind::Gpu;
        composed
    }

    fn read_back_current_output_surface(
        &mut self,
        width: u32,
        height: u32,
        readback_key: Option<CachedReadbackKey>,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<ComposedFrameSet> {
        let Some(current_output) = self.current_output else {
            anyhow::bail!("GPU readback requested without a composed output surface");
        };
        if !requires_cpu_sampling_canvas
            && let Some(request) = preview_surface_request
            && !preview_request_matches_plan(Some(request), width, height)
        {
            return self.stage_scaled_preview_surface_readback(
                current_output,
                width,
                height,
                readback_key,
                request,
                encoder,
            );
        }
        let surfaces = self
            .surfaces
            .as_mut()
            .context("GPU readback requested before compositor surfaces were allocated")?;
        let current_texture = match current_output {
            GpuCompositorOutputSurface::Front => &surfaces.front.texture,
            GpuCompositorOutputSurface::Back => &surfaces.back.texture,
        };
        let mut encoder = encoder.unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU cached readback"),
                })
        });
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
        if !requires_cpu_sampling_canvas {
            self.pending_output_submission = Some(encoder);
            self.pending_preview_readback = Some(PendingPreviewReadback::FullSize {
                width,
                height,
                readback_key,
            });
            self.ready_preview_surface = None;
            return Ok(gpu_composed_without_surfaces());
        }

        let readback_buffer = &surfaces.readback;
        let readback_surfaces = &mut surfaces.readback_surfaces;
        let sampling_surface = read_back_texture_into_surface(
            &self.device,
            readback_buffer,
            width,
            height,
            surfaces.padded_bytes_per_row,
            self.queue.submit(Some(encoder.finish())),
            readback_surfaces,
        )?;
        if let Some(key) = readback_key {
            self.cached_readback_surface = Some(CachedReadbackSurface {
                key: Some(key),
                surface: sampling_surface.clone(),
            });
        }
        Ok(gpu_composed_from_surface(
            sampling_surface,
            requires_cpu_sampling_canvas,
        ))
    }

    fn ensure_preview_surface_size(&mut self, width: u32, height: u32) -> Result<()> {
        if matches!(
            self.preview_surfaces,
            Some(GpuPreviewSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return Ok(());
        }

        let (front_view, back_view) = {
            let surfaces = self
                .surfaces
                .as_ref()
                .context("GPU preview surfaces requested before compositor surfaces were allocated")?;
            (surfaces.front.view.clone(), surfaces.back.view.clone())
        };

        self.preview_surfaces = Some(GpuPreviewSurfaceSet::new(
            &self.device,
            &self.pipeline,
            &front_view,
            &back_view,
            width,
            height,
        ));
        self.cached_preview_surface = None;
        Ok(())
    }

    fn stage_scaled_preview_surface_readback(
        &mut self,
        current_output: GpuCompositorOutputSurface,
        source_width: u32,
        source_height: u32,
        readback_key: Option<CachedReadbackKey>,
        request: PreviewSurfaceRequest,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<ComposedFrameSet> {
        if let Some(key) = readback_key.as_ref()
            && let Some(cached) = self.cached_preview_surface.as_ref()
            && cached.key
                == (CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
        {
            self.pending_output_submission = None;
            return Ok(gpu_composed_with_preview_surface(cached.surface.clone()));
        }

        self.ensure_preview_surface_size(request.width, request.height)?;
        let preview_surfaces = self
            .preview_surfaces
            .as_mut()
            .expect("preview surfaces should exist after allocation");
        let bind_group = match current_output {
            GpuCompositorOutputSurface::Front => &preview_surfaces.bind_groups.front_to_preview,
            GpuCompositorOutputSurface::Back => &preview_surfaces.bind_groups.back_to_preview,
        };
        let params =
            encode_preview_scale_params(source_width, source_height, request.width, request.height);
        if preview_surfaces.cached_scale_params != Some(params) {
            self.queue
                .write_buffer(&self.pipeline.preview_scale_params_buffer, 0, &params);
            preview_surfaces.cached_scale_params = Some(params);
            #[cfg(test)]
            {
                preview_surfaces.scale_param_write_count =
                    preview_surfaces.scale_param_write_count.saturating_add(1);
            }
        }
        let mut encoder = encoder.unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU preview scale"),
                })
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SparkleFlinger GPU preview scale pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline.preview_scale_pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(
            request.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
            request.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
            1,
        );
        drop(pass);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &preview_surfaces.texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &preview_surfaces.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(preview_surfaces.padded_bytes_per_row),
                    rows_per_image: Some(request.height),
                },
            },
            texture_extent(request.width, request.height),
        );
        self.pending_output_submission = Some(encoder);
        self.pending_preview_readback = Some(PendingPreviewReadback::Scaled {
            request,
            readback_key,
        });
        self.ready_preview_surface = None;
        Ok(gpu_composed_without_surfaces())
    }

    fn resolve_pending_preview_surface_for_submission(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
    ) -> Result<()> {
        let Some(pending_preview_readback) = self.pending_preview_readback.take() else {
            return Ok(());
        };
        let surface =
            self.finish_pending_preview_readback(pending_preview_readback, submission_index)?;
        self.ready_preview_surface = Some(surface);
        Ok(())
    }

    fn finish_pending_preview_readback(
        &mut self,
        pending_preview_readback: PendingPreviewReadback,
        submission_index: wgpu::SubmissionIndex,
    ) -> Result<PublishedSurface> {
        match pending_preview_readback {
            PendingPreviewReadback::FullSize {
                width,
                height,
                readback_key,
            } => {
                let surfaces = self
                    .surfaces
                    .as_mut()
                    .context("GPU preview finalize requested before compositor surfaces existed")?;
                let preview_surface = read_back_texture_into_surface(
                    &self.device,
                    &surfaces.readback,
                    width,
                    height,
                    surfaces.padded_bytes_per_row,
                    submission_index,
                    &mut surfaces.readback_surfaces,
                )?;
                if let Some(key) = readback_key {
                    self.cached_readback_surface = Some(CachedReadbackSurface {
                        key: Some(key),
                        surface: preview_surface.clone(),
                    });
                }
                Ok(preview_surface)
            }
            PendingPreviewReadback::Scaled {
                request,
                readback_key,
            } => {
                let preview_surfaces = self
                    .preview_surfaces
                    .as_mut()
                    .context("GPU scaled preview finalize requested before preview surfaces existed")?;
                let preview_surface = read_back_texture_into_surface(
                    &self.device,
                    &preview_surfaces.readback,
                    request.width,
                    request.height,
                    preview_surfaces.padded_bytes_per_row,
                    submission_index,
                    &mut preview_surfaces.readback_surfaces,
                )?;
                if let Some(key) = readback_key {
                    self.cached_preview_surface = Some(CachedPreviewSurface {
                        key: CachedPreviewSurfaceKey {
                            composition: key,
                            request,
                        },
                        surface: preview_surface.clone(),
                    });
                }
                Ok(preview_surface)
            }
        }
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
            source: wgpu::ShaderSource::Wgsl(include_str!("preview_scale.wgsl").into()),
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
            preview_scale_bind_group_layout,
            preview_scale_pipeline,
            preview_scale_params_buffer,
        }
    }
}

impl GpuCompositorSurfaceSet {
    fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        width: u32,
        height: u32,
    ) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let front = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Front");
        let back = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Back");
        let source = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Source");

        Self {
            width,
            height,
            padded_bytes_per_row,
            bind_groups: GpuCompositorBindGroups::new(device, pipeline, &front, &back, &source),
            front,
            back,
            source,
            readback,
            readback_surfaces: RenderSurfacePool::new(SurfaceDescriptor::rgba8888(width, height)),
            front_contents: None,
            back_contents: None,
            cached_source_upload: None,
            #[cfg(test)]
            front_upload_count: 0,
            #[cfg(test)]
            source_upload_count: 0,
            #[cfg(test)]
            compose_dispatch_count: 0,
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

impl GpuPreviewSurfaceSet {
    fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU preview readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let texture = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Preview");
        Self {
            width,
            height,
            padded_bytes_per_row,
            bind_groups: GpuPreviewScaleBindGroups::new(
                device,
                pipeline,
                front_view,
                back_view,
                &texture.view,
            ),
            texture,
            readback,
            readback_surfaces: RenderSurfacePool::new(SurfaceDescriptor::rgba8888(width, height)),
            cached_scale_params: None,
            #[cfg(test)]
            scale_param_write_count: 0,
            #[cfg(test)]
            preview_bind_group_count: 2,
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

impl GpuCompositorBindGroups {
    fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front: &GpuCompositorTexture,
        back: &GpuCompositorTexture,
        source: &GpuCompositorTexture,
    ) -> Self {
        Self {
            front_to_back: create_compose_bind_group(
                device,
                pipeline,
                &front.view,
                &source.view,
                &back.view,
                "SparkleFlinger GPU bind group front->back",
            ),
            back_to_front: create_compose_bind_group(
                device,
                pipeline,
                &back.view,
                &source.view,
                &front.view,
                "SparkleFlinger GPU bind group back->front",
            ),
        }
    }
}

impl GpuPreviewScaleBindGroups {
    fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        preview_view: &wgpu::TextureView,
    ) -> Self {
        Self {
            front_to_preview: create_preview_scale_bind_group(
                device,
                pipeline,
                front_view,
                preview_view,
                "SparkleFlinger GPU preview scale bind group front->preview",
            ),
            back_to_preview: create_preview_scale_bind_group(
                device,
                pipeline,
                back_view,
                preview_view,
                "SparkleFlinger GPU preview scale bind group back->preview",
            ),
        }
    }
}

fn compose_layer_into_gpu(
    queue: &wgpu::Queue,
    pipeline: &GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    layer: &CompositionLayer,
    use_front_as_current: bool,
) {
    let shader_mode = if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        ComposeShaderMode::Replace
    } else {
        match layer.mode {
            CompositionMode::Replace | CompositionMode::Alpha => ComposeShaderMode::Alpha,
            CompositionMode::Add => ComposeShaderMode::Add,
            CompositionMode::Screen => ComposeShaderMode::Screen,
        }
    };
    upload_frame_into_source_texture(queue, surfaces, &layer.frame);
    let output_surface = if use_front_as_current {
        GpuCompositorOutputSurface::Back
    } else {
        GpuCompositorOutputSurface::Front
    };
    let output = if use_front_as_current {
        &surfaces.back
    } else {
        &surfaces.front
    };
    let bind_group = if use_front_as_current {
        &surfaces.bind_groups.front_to_back
    } else {
        &surfaces.bind_groups.back_to_front
    };
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
        set_texture_contents(surfaces, output_surface, cached_source_upload(&layer.frame));
        return;
    }

    queue.write_buffer(
        &pipeline.params_buffer,
        0,
        &encode_compose_params(surfaces.width, surfaces.height, shader_mode, layer.opacity),
    );
    #[cfg(test)]
    {
        surfaces.compose_dispatch_count = surfaces.compose_dispatch_count.saturating_add(1);
    }
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU compose pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.compose_pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(
        surfaces.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
        surfaces.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
        1,
    );
    drop(pass);
    set_texture_contents(surfaces, output_surface, None);
}

fn upload_frame_into_source_texture(
    queue: &wgpu::Queue,
    surfaces: &mut GpuCompositorSurfaceSet,
    frame: &ProducerFrame,
) {
    upload_frame_into_cached_texture(
        queue,
        &surfaces.source.texture,
        &mut surfaces.cached_source_upload,
        frame,
        #[cfg(test)]
        &mut surfaces.source_upload_count,
    );
}

fn upload_frame_into_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, frame: &ProducerFrame) {
    let bytes_per_row = frame.width() * BYTES_PER_PIXEL as u32;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        frame.rgba_bytes(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(frame.height()),
        },
        texture_extent(frame.width(), frame.height()),
    );
}

fn upload_frame_into_cached_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    cached_upload: &mut Option<CachedSourceUpload>,
    frame: &ProducerFrame,
    #[cfg(test)] upload_count: &mut usize,
) {
    let next_upload = cached_source_upload(frame);
    if next_upload.is_some() && *cached_upload == next_upload {
        return;
    }

    upload_frame_into_texture(queue, texture, frame);
    *cached_upload = next_upload;
    #[cfg(test)]
    {
        *upload_count = upload_count.saturating_add(1);
    }
}

fn cached_source_upload(frame: &ProducerFrame) -> Option<CachedSourceUpload> {
    match frame {
        ProducerFrame::Surface(surface) => {
            if surface.generation() == 0 {
                return None;
            }

            Some(CachedSourceUpload {
                storage: surface.storage_identity(),
                generation: surface.generation(),
                width: surface.width(),
                height: surface.height(),
            })
        }
        ProducerFrame::Canvas(canvas) if canvas.is_shared() => Some(CachedSourceUpload {
            storage: canvas.storage_identity(),
            generation: 0,
            width: canvas.width(),
            height: canvas.height(),
        }),
        ProducerFrame::Canvas(_) => None,
    }
}

fn cached_readback_key(plan: &CompositionPlan) -> Option<CachedReadbackKey> {
    let mut layers = Vec::with_capacity(plan.layers.len());
    for layer in &plan.layers {
        layers.push(CachedReadbackLayer {
            source: cached_source_upload(&layer.frame)?,
            mode: layer.mode,
            opacity_bits: layer.opacity.to_bits(),
        });
    }
    Some(CachedReadbackKey {
        width: plan.width,
        height: plan.height,
        layers,
    })
}

fn gpu_composed_without_surfaces() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
    }
}

fn gpu_composed_with_preview_surface(preview_surface: PublishedSurface) -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: Some(preview_surface),
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
    }
}

fn gpu_bypassed_without_surfaces() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
    }
}

fn gpu_composed_from_surface(
    sampling_surface: PublishedSurface,
    requires_cpu_sampling_canvas: bool,
) -> ComposedFrameSet {
    if requires_cpu_sampling_canvas {
        let sampling_canvas = Canvas::from_published_surface(&sampling_surface);
        return ComposedFrameSet {
            sampling_canvas: Some(sampling_canvas),
            sampling_surface: Some(sampling_surface),
            preview_surface: None,
            bypassed: false,
            backend: CompositorBackendKind::Gpu,
        };
    }

    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: Some(sampling_surface),
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
    }
}

fn gpu_bypassed_surface_frame(
    surface: &PublishedSurface,
    requires_cpu_sampling_canvas: bool,
    requires_preview_surface: bool,
) -> ComposedFrameSet {
    let preview_surface =
        (!requires_cpu_sampling_canvas && requires_preview_surface).then(|| surface.clone());
    let (sampling_canvas, sampling_surface) = if requires_cpu_sampling_canvas {
        (
            Some(Canvas::from_published_surface(surface)),
            Some(surface.clone()),
        )
    } else {
        (None, None)
    };
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
    }
}

fn gpu_bypassed_canvas_frame(
    canvas: &Canvas,
    requires_cpu_sampling_canvas: bool,
    requires_preview_surface: bool,
) -> ComposedFrameSet {
    let published_surface = (requires_cpu_sampling_canvas || requires_preview_surface)
        .then(|| PublishedSurface::from_owned_canvas(canvas.clone(), 0, 0));
    let preview_surface = (!requires_cpu_sampling_canvas && requires_preview_surface).then(|| {
        published_surface
            .as_ref()
            .expect("preview bypass should allocate a published surface")
            .clone()
    });
    let (sampling_canvas, sampling_surface) = if requires_cpu_sampling_canvas {
        let sampling_surface =
            published_surface.expect("CPU sampling bypass should allocate a published surface");
        (
            Some(Canvas::from_published_surface(&sampling_surface)),
            Some(sampling_surface),
        )
    } else {
        (None, None)
    };
    ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
    }
}

fn bypass_preview_surface(frame: &ProducerFrame) -> Option<PublishedSurface> {
    match frame {
        ProducerFrame::Surface(surface) => Some(surface.clone()),
        ProducerFrame::Canvas(_) => None,
    }
}

fn preview_request_matches_plan(
    request: Option<PreviewSurfaceRequest>,
    width: u32,
    height: u32,
) -> bool {
    request.is_none_or(|request| request.width == width && request.height == height)
}

fn set_texture_contents(
    surfaces: &mut GpuCompositorSurfaceSet,
    output: GpuCompositorOutputSurface,
    contents: Option<CachedSourceUpload>,
) {
    match output {
        GpuCompositorOutputSurface::Front => surfaces.front_contents = contents,
        GpuCompositorOutputSurface::Back => surfaces.back_contents = contents,
    }
}

fn read_back_texture_into_surface(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    submission_index: wgpu::SubmissionIndex,
    surfaces: &mut RenderSurfacePool,
) -> Result<PublishedSurface> {
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
    let mut lease = surfaces
        .dequeue()
        .context("GPU readback surface pool should provide a reusable slot")?;
    let target = lease.canvas_mut().as_rgba_bytes_mut();
    if padded_bytes_per_row == unpadded_bytes_per_row {
        target.copy_from_slice(
            &mapped[..usize::try_from(unpadded_bytes_per_row)
                .expect("row width should fit in usize")
                .saturating_mul(height as usize)],
        );
    } else {
        let row_width = usize::try_from(unpadded_bytes_per_row).expect("row width should fit");
        let padded_row_width =
            usize::try_from(padded_bytes_per_row).expect("row pitch should fit in usize");
        for (target_row, row) in target.chunks_exact_mut(row_width).zip(
            mapped
                .chunks(
                    usize::try_from(padded_bytes_per_row).expect("row pitch should fit in usize"),
                )
                .take(height as usize),
        ) {
            debug_assert_eq!(row.len(), padded_row_width);
            target_row.copy_from_slice(&row[..row_width]);
        }
    }
    drop(mapped);
    buffer.unmap();

    Ok(lease.submit(0, 0))
}

fn create_compose_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    current: &wgpu::TextureView,
    source: &wgpu::TextureView,
    output: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.compose_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(current),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.params_buffer.as_entire_binding(),
            },
        ],
    })
}

fn create_preview_scale_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    source: &wgpu::TextureView,
    output: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.preview_scale_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pipeline.preview_scale_params_buffer.as_entire_binding(),
            },
        ],
    })
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

fn encode_preview_scale_params(
    source_width: u32,
    source_height: u32,
    preview_width: u32,
    preview_height: u32,
) -> [u8; PREVIEW_SCALE_PARAM_BYTES] {
    let mut bytes = [0u8; PREVIEW_SCALE_PARAM_BYTES];
    bytes[0..4].copy_from_slice(&source_width.to_le_bytes());
    bytes[4..8].copy_from_slice(&source_height.to_le_bytes());
    bytes[8..12].copy_from_slice(&preview_width.to_le_bytes());
    bytes[12..16].copy_from_slice(&preview_height.to_le_bytes());
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
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_core::types::canvas::{
        Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
    };
    use hypercolor_types::event::ZoneColors;
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection,
    };

    use super::GpuSparkleFlinger;
    use crate::render_thread::producer_queue::ProducerFrame;
    use crate::render_thread::sparkleflinger::{
        CompositionLayer, CompositionPlan, PreviewSurfaceRequest, cpu::CpuSparkleFlinger,
    };

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(color);
        canvas
    }

    fn patterned_canvas(seed: u8) -> Canvas {
        let mut canvas = Canvas::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                let base = seed.wrapping_add(u8::try_from(x * 31 + y * 17).unwrap_or_default());
                canvas.set_pixel(
                    x,
                    y,
                    Rgba::new(
                        base,
                        base.wrapping_add(53),
                        base.wrapping_add(101),
                        255,
                    ),
                );
            }
        }
        canvas
    }

    fn slot_surface(color: Rgba) -> PublishedSurface {
        let mut pool = RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 1);
        let mut lease = pool.dequeue().expect("surface slot should be available");
        lease.canvas_mut().fill(color);
        lease.submit(0, 0)
    }

    fn full_preview_request(plan: &CompositionPlan) -> Option<PreviewSurfaceRequest> {
        Some(PreviewSurfaceRequest {
            width: plan.width,
            height: plan.height,
        })
    }

    fn sampling_layout(mode: SamplingMode) -> SpatialLayout {
        SpatialLayout {
            id: "gpu-sampling".into(),
            name: "GPU Sampling".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![DeviceZone {
                id: "zone".into(),
                name: "zone".into(),
                device_id: "device:zone".into(),
                zone_name: None,
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: 4,
                    direction: StripDirection::LeftToRight,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(mode),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: None,
                shape_preset: None,
                display_order: 0,
                attachment: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    fn fade_sampling_layout(mode: SamplingMode) -> SpatialLayout {
        SpatialLayout {
            id: "gpu-sampling-fade".into(),
            name: "GPU Sampling Fade".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![DeviceZone {
                id: "zone".into(),
                name: "zone".into(),
                device_id: "device:zone".into(),
                zone_name: None,
                position: NormalizedPosition::new(1.25, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Point,
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(mode),
                edge_behavior: Some(EdgeBehavior::FadeToBlack { falloff: 8.0 }),
                shape: None,
                shape_preset: None,
                display_order: 0,
                attachment: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
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
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("GPU composition should succeed for replace + alpha plans");

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("GPU alpha compose should materialize a canvas")
                .as_rgba_bytes(),
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU alpha compose should materialize a canvas")
                .as_rgba_bytes()
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
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("GPU composition should succeed for add plans");

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("GPU add compose should materialize a canvas")
                .as_rgba_bytes(),
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU add compose should materialize a canvas")
                .as_rgba_bytes()
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
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("GPU composition should succeed for screen plans");

        assert_eq!(
            composed
                .sampling_canvas
                .as_ref()
                .expect("GPU screen compose should materialize a canvas")
                .as_rgba_bytes(),
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU screen compose should materialize a canvas")
                .as_rgba_bytes()
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
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
        );
        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("single replace surface should bypass GPU composition");

        let surface = composed
            .sampling_surface
            .expect("bypass path should preserve the source surface");
        assert_eq!(surface.rgba_bytes().as_ptr(), source.rgba_bytes().as_ptr());
    }

    #[test]
    fn gpu_compositor_bypass_surfaces_still_support_gpu_zone_sampling() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let source = slot_surface(Rgba::new(24, 88, 160, 255));
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
        );
        let expected = engine.sample(&Canvas::from_published_surface(&source));

        let composed = compositor
            .compose(&plan, false, None)
            .expect("single replace surface should still compose on the GPU");
        assert!(composed.sampling_canvas.is_none());
        assert!(composed.preview_surface.is_none());

        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU sampler should reuse bypassed front textures")
        );
        assert_eq!(sampled, expected);
    }

    #[test]
    fn gpu_compositor_skips_cpu_readback_when_canvas_is_not_required() {
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

        let composed = compositor
            .compose(&plan, false, None)
            .expect("GPU composition should support no-readback mode");

        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
        assert!(!composed.bypassed);
    }

    #[test]
    fn gpu_compositor_scales_preview_surface_to_requested_size() {
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

        let composed = compositor
            .compose(
                &plan,
                false,
                Some(PreviewSurfaceRequest {
                    width: 2,
                    height: 2,
                }),
            )
            .expect("GPU composition should support scaled preview surfaces");

        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
        assert!(composed.preview_surface.is_none());
        let preview_surface = compositor
            .resolve_preview_surface()
            .expect("GPU preview finalize should succeed")
            .expect("scaled preview requests should resolve a preview surface");
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
    }

    #[test]
    fn gpu_scaled_preview_reuses_bind_groups_and_scale_params() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };
        let first_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(patterned_canvas(96)),
                    0.35,
                ),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(33))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(patterned_canvas(144)),
                    0.35,
                ),
            ],
        );

        compositor
            .compose(&first_plan, false, Some(request))
            .expect("first scaled preview compose should succeed");
        compositor
            .resolve_preview_surface()
            .expect("first preview finalize should succeed")
            .expect("first compose should publish a preview surface");
        {
            let preview_surfaces = compositor
                .preview_surfaces
                .as_ref()
                .expect("scaled preview should allocate preview surfaces");
            assert_eq!(preview_surfaces.scale_param_write_count, 1);
            assert_eq!(preview_surfaces.preview_bind_group_count, 2);
        }

        compositor
            .compose(&second_plan, false, Some(request))
            .expect("second scaled preview compose should succeed");
        compositor
            .resolve_preview_surface()
            .expect("second preview finalize should succeed")
            .expect("second compose should publish a preview surface");

        let preview_surfaces = compositor
            .preview_surfaces
            .as_ref()
            .expect("preview surfaces should stay allocated across same-size requests");
        assert_eq!(preview_surfaces.scale_param_write_count, 1);
        assert_eq!(preview_surfaces.preview_bind_group_count, 2);
    }

    #[test]
    fn gpu_sampler_resolves_pending_preview_surface_after_sampling() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(patterned_canvas(96)),
                    0.35,
                ),
            ],
        );

        let composed = compositor
            .compose(
                &plan,
                false,
                Some(PreviewSurfaceRequest {
                    width: 2,
                    height: 2,
                }),
            )
            .expect("GPU composition should stage a scaled preview surface");
        assert!(composed.preview_surface.is_none());

        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU zone sampling should succeed")
        );

        let preview_surface = compositor
            .resolve_preview_surface()
            .expect("GPU preview finalize should succeed")
            .expect("sampling should resolve the staged preview surface");
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
    }

    #[test]
    fn gpu_compositor_bypassed_canvas_shares_sampling_surface_storage() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                24, 88, 160, 255,
            )))),
        );

        let composed = compositor
            .compose(&plan, true, None)
            .expect("single replace canvas should bypass on the GPU");
        let sampling_surface = composed
            .sampling_surface
            .as_ref()
            .expect("bypassed canvas should publish a sampling surface");
        let sampling_canvas = composed
            .sampling_canvas
            .as_ref()
            .expect("bypassed canvas should materialize a canvas view");

        assert_eq!(
            sampling_canvas.as_rgba_bytes().as_ptr(),
            sampling_surface.rgba_bytes().as_ptr()
        );
        assert!(composed.preview_surface.is_none());
        assert!(composed.bypassed);
    }

    #[test]
    fn gpu_compositor_reuses_cached_shared_canvas_bypass_surfaces() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let canvas = solid_canvas(Rgba::new(24, 88, 160, 255));
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(canvas.clone())),
        );

        let first = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("initial shared canvas bypass should succeed");
        let first_surface = first
            .sampling_surface
            .as_ref()
            .expect("bypassed shared canvas should publish a sampling surface");
        let first_ptr = first_surface.rgba_bytes().as_ptr();
        let first_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should exist after bypass")
            .front_upload_count;

        let second = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("cached shared canvas bypass should succeed");
        let second_surface = second
            .sampling_surface
            .as_ref()
            .expect("cached bypass should still publish a sampling surface");
        let second_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should persist across bypasses")
            .front_upload_count;

        assert_eq!(second_surface.rgba_bytes().as_ptr(), first_ptr);
        assert_eq!(second_upload_count, first_upload_count);
        assert!(second.bypassed);
    }

    #[test]
    fn gpu_compositor_reuses_cached_unique_canvas_bypass_surfaces_on_second_frame() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                24, 88, 160, 255,
            )))),
        );

        let first = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("initial unique canvas bypass should succeed");
        let first_surface = first
            .sampling_surface
            .as_ref()
            .expect("bypassed unique canvas should publish a sampling surface");
        let first_ptr = first_surface.rgba_bytes().as_ptr();
        let first_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should exist after bypass")
            .front_upload_count;

        let second = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("second unique canvas bypass should reuse the cached surface");
        let second_surface = second
            .sampling_surface
            .as_ref()
            .expect("cached unique bypass should still publish a sampling surface");
        let second_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should persist across bypasses")
            .front_upload_count;

        assert_eq!(second_surface.rgba_bytes().as_ptr(), first_ptr);
        assert_eq!(second_upload_count, first_upload_count);
        assert!(second.bypassed);
    }

    #[test]
    fn gpu_compositor_reuses_cached_slot_backed_frame_uploads() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let retained_base = slot_surface(Rgba::new(255, 32, 0, 255));
        let retained_overlay = slot_surface(Rgba::new(32, 64, 255, 255));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(retained_base)),
                CompositionLayer::alpha(ProducerFrame::Surface(retained_overlay), 0.35),
            ],
        );

        compositor
            .compose(&plan, false, None)
            .expect("initial GPU composition should succeed");
        let first_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should exist after composition")
            .source_upload_count;
        let first_front_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should exist after composition")
            .front_upload_count;
        let first_compose_dispatch_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should exist after composition")
            .compose_dispatch_count;

        compositor
            .compose(&plan, false, None)
            .expect("cached GPU composition should succeed");
        let second_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should persist across compositions")
            .source_upload_count;
        let second_front_upload_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should persist across compositions")
            .front_upload_count;
        let second_compose_dispatch_count = compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should persist across compositions")
            .compose_dispatch_count;

        assert_eq!(first_upload_count, 1);
        assert_eq!(second_upload_count, first_upload_count);
        assert_eq!(first_front_upload_count, 1);
        assert_eq!(second_front_upload_count, first_front_upload_count);
        assert_eq!(first_compose_dispatch_count, 1);
        assert_eq!(second_compose_dispatch_count, first_compose_dispatch_count);

        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("cached no-readback composition should remain sampleable")
        );
        assert_eq!(
            sampled,
            engine.sample(
                expected
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU compose should materialize a canvas"),
            )
        );
    }

    #[test]
    fn gpu_compositor_readback_surfaces_are_slot_backed() {
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

        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("GPU composition should materialize a CPU readback surface");

        assert!(
            composed
                .sampling_surface
                .expect("readback should publish a surface")
                .generation()
                > 0
        );
    }

    #[test]
    fn gpu_sampler_matches_cpu_spatial_sampling_for_bilinear_plans() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
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
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let expected_zones = engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        );
        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before GPU sampling");
        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU spatial sampling should succeed")
        );

        assert_eq!(sampled, expected_zones);
    }

    #[test]
    fn gpu_sampler_matches_cpu_spatial_sampling_with_fade_edges() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(fade_sampling_layout(SamplingMode::Bilinear));
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
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let expected_zones = engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        );

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before GPU fade sampling");
        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU spatial sampling should support prepared attenuation")
        );

        assert_eq!(sampled, expected_zones);
    }

    #[test]
    fn gpu_sampler_reuses_zone_output_storage() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                    24, 88, 160, 255,
                )))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(220, 48, 24, 255))),
                    0.25,
                ),
            ],
        );
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before output reuse testing");

        let mut sampled = vec![ZoneColors {
            zone_id: "stale".into(),
            colors: vec![[0_u8; 3]; 8],
        }];
        let first_colors_ptr = sampled[0].colors.as_ptr();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU spatial sampling should succeed for bilinear plans")
        );

        assert_eq!(
            sampled,
            engine.sample(
                expected
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU compose should materialize a canvas"),
            )
        );
        assert_eq!(sampled[0].colors.as_ptr(), first_colors_ptr);
    }

    #[test]
    fn gpu_sampler_reuses_cached_zone_results_for_identical_retained_surfaces() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let retained_base = slot_surface(Rgba::new(255, 32, 0, 255));
        let retained_overlay = slot_surface(Rgba::new(32, 64, 255, 255));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(retained_base)),
                CompositionLayer::alpha(ProducerFrame::Surface(retained_overlay), 0.35),
            ],
        );

        compositor
            .compose(&plan, false, None)
            .expect("initial GPU composition should succeed");
        let mut first_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
                .expect("initial GPU sample should succeed")
        );
        let first_dispatch_count = compositor.spatial_sampler.sample_dispatch_count();

        compositor
            .compose(&plan, false, None)
            .expect("cached GPU composition should succeed");
        let mut second_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut second_sample)
                .expect("cached GPU sample should succeed")
        );

        assert_eq!(second_sample, first_sample);
        assert_eq!(
            compositor.spatial_sampler.sample_dispatch_count(),
            first_dispatch_count
        );
    }

    #[test]
    fn gpu_sampler_reuses_sample_params_for_same_output_shape() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let first_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(patterned_canvas(96)),
                    0.35,
                ),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(patterned_canvas(180)),
                    0.35,
                ),
            ],
        );

        compositor
            .compose(&first_plan, false, None)
            .expect("first GPU composition should succeed");
        let mut first_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
                .expect("first GPU sample should succeed")
        );
        assert_eq!(compositor.spatial_sampler.sample_param_write_count(), 1);

        compositor
            .compose(&second_plan, false, None)
            .expect("second GPU composition should succeed");
        let mut second_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut second_sample)
                .expect("second GPU sample should succeed")
        );

        assert_eq!(compositor.spatial_sampler.sample_param_write_count(), 1);
    }

    #[test]
    fn gpu_sampler_matches_cpu_spatial_sampling_for_area_plans() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
            radius_x: 1.0,
            radius_y: 1.0,
        }));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::screen(
                    ProducerFrame::Canvas(patterned_canvas(96)),
                    0.6,
                ),
            ],
        );
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let expected_zones = engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        );
        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before GPU area sampling");
        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU sampler should support area plans")
        );
        assert_eq!(sampled, expected_zones);
    }
}
