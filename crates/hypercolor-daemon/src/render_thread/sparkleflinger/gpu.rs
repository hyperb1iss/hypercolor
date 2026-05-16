use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, Canvas, PublishedSurface, PublishedSurfaceStorageIdentity, RenderSurfacePool,
    SurfaceDescriptor,
};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::DisplayFaceBlendMode;
use hypercolor_types::spatial::EdgeBehavior;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, DisplayFinalizeParams,
    PreviewSurfaceRequest,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::gpu_device::{GpuRenderDevice, backend_name, texture_format_name};
use crate::render_thread::producer_queue::{GpuTextureFrame, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSampleSource, GpuSamplingPlan, GpuSamplingPlanKey, GpuSpatialSampler,
    PendingGpuSampleReadback,
};

const COMPOSITOR_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const COMPOSE_WORKGROUP_WIDTH: u32 = 8;
const COMPOSE_WORKGROUP_HEIGHT: u32 = 8;
const COMPOSE_PARAM_BYTES: usize = 48;
const DISPLAY_FINALIZE_PARAM_BYTES: usize = 96;
const PREVIEW_SCALE_PARAM_BYTES: usize = 16;
const MAX_CACHED_PREVIEW_SURFACES: usize = 3;
const MAX_CACHED_PREVIEW_READBACK_POOLS: usize = 3;
const PREVIEW_READBACK_SLOT_COUNT: usize = 2;
const GPU_READBACK_WAIT_TIMEOUT: Duration = Duration::from_millis(8);
static GPU_SOURCE_UPLOAD_SKIPPED_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn gpu_source_upload_skipped_total() -> u64 {
    GPU_SOURCE_UPLOAD_SKIPPED_TOTAL.load(Ordering::Relaxed)
}

fn record_gpu_source_upload_skipped() {
    let _ = GPU_SOURCE_UPLOAD_SKIPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Clone)]
pub(crate) struct GpuCompositorProbe {
    pub(crate) adapter_name: String,
    pub(crate) backend: &'static str,
    pub(crate) texture_format: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
    pub(crate) linux_servo_gpu_import_backend_compatible: bool,
    pub(crate) linux_servo_gpu_import_backend_reason: Option<&'static str>,
}

pub(crate) struct GpuSparkleFlinger {
    _render_device: GpuRenderDevice,
    device: wgpu::Device,
    queue: wgpu::Queue,
    probe: GpuCompositorProbe,
    pipeline: GpuCompositorPipeline,
    spatial_sampler: GpuSpatialSampler,
    surfaces: Option<GpuCompositorSurfaceSet>,
    display_finalize_surfaces: Option<GpuDisplayFinalizeSurfaceSet>,
    preview_surfaces: Option<GpuPreviewSurfaceSet>,
    current_output: Option<GpuCompositorOutputSurface>,
    cached_composition_key: Option<CachedReadbackKey>,
    cached_readback_surface: Option<CachedReadbackSurface>,
    cached_preview_surfaces: Vec<CachedPreviewSurface>,
    pending_output_submission: Option<wgpu::CommandEncoder>,
    pending_preview_readback: Option<PendingPreviewReadback>,
    pending_preview_submission: Option<wgpu::SubmissionIndex>,
    pending_preview_map: Option<PendingPreviewMap>,
    ready_preview_surface: Option<PublishedSurface>,
    output_generation: u64,
    cached_sample_result: Option<CachedSampleResult>,
    #[cfg(test)]
    preview_surface_allocation_count: usize,
    #[cfg(test)]
    defer_preview_resolve_once: bool,
    #[cfg(test)]
    defer_preview_map_resolve_once: bool,
}

struct GpuCompositorPipeline {
    compose_bind_group_layout: wgpu::BindGroupLayout,
    compose_pipeline: wgpu::ComputePipeline,
    params_buffer: wgpu::Buffer,
    display_finalize_bind_group_layout: wgpu::BindGroupLayout,
    display_finalize_pipeline: wgpu::ComputePipeline,
    display_finalize_yuv_pipeline: wgpu::ComputePipeline,
    display_finalize_params_buffer: wgpu::Buffer,
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
    cached_compose_params: Option<[u8; COMPOSE_PARAM_BYTES]>,
    front_contents: Option<CachedSourceUpload>,
    back_contents: Option<CachedSourceUpload>,
    cached_source_upload: Option<CachedSourceUpload>,
    #[cfg(test)]
    front_upload_count: usize,
    #[cfg(test)]
    source_upload_count: usize,
    #[cfg(test)]
    compose_dispatch_count: usize,
    #[cfg(test)]
    compose_param_write_count: usize,
    #[cfg(test)]
    last_readback_bytes: u64,
}

struct GpuDisplayFinalizeSurfaceSet {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    yuv_layout: DisplayYuv420Layout,
    output: GpuCompositorTexture,
    yuv_output: wgpu::Buffer,
    readback: wgpu::Buffer,
    yuv_readback: wgpu::Buffer,
    readback_surfaces: RenderSurfacePool,
    scene_source: Option<GpuDisplaySourceTexture>,
    face_source: Option<GpuDisplaySourceTexture>,
    #[cfg(test)]
    scene_upload_count: usize,
    #[cfg(test)]
    face_upload_count: usize,
    #[cfg(test)]
    last_readback_bytes: u64,
    #[cfg(test)]
    last_yuv_readback_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayYuv420Layout {
    y_stride: u32,
    uv_stride: u32,
    y_plane_len: u32,
    u_plane_len: u32,
    total_len: u32,
    word_len: u32,
}

struct GpuDisplaySourceTexture {
    width: u32,
    height: u32,
    texture: GpuCompositorTexture,
    cached_upload: Option<CachedSourceUpload>,
}

struct GpuPreviewSurfaceSet {
    width: u32,
    height: u32,
    capacity_width: u32,
    capacity_height: u32,
    padded_bytes_per_row: u32,
    output_buffer: wgpu::Buffer,
    readbacks: [wgpu::Buffer; PREVIEW_READBACK_SLOT_COUNT],
    next_readback_slot: usize,
    bind_groups: GpuPreviewScaleBindGroups,
    readback_surfaces: RenderSurfacePool,
    cached_readback_surfaces: Vec<CachedPreviewReadbackSurfaces>,
    cached_scale_params: Option<[u8; PREVIEW_SCALE_PARAM_BYTES]>,
    #[cfg(test)]
    scale_param_write_count: usize,
    #[cfg(test)]
    preview_bind_group_count: usize,
    #[cfg(test)]
    last_readback_bytes: u64,
    #[cfg(test)]
    readback_surface_pool_allocation_count: usize,
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

struct CachedPreviewReadbackSurfaces {
    request: PreviewSurfaceRequest,
    surfaces: RenderSurfacePool,
}

#[derive(Debug, Clone)]
enum PendingPreviewReadback {
    PreviewBuffer {
        request: PreviewSurfaceRequest,
        readback_key: Option<CachedReadbackKey>,
        cache_as_full_size: bool,
        slot: usize,
    },
}

struct PendingPreviewMap {
    readback: PendingPreviewReadback,
    used_bytes: u64,
    receiver: mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

impl PendingPreviewReadback {
    fn matches_request(&self, request: PreviewSurfaceRequest) -> bool {
        matches!(
            self,
            Self::PreviewBuffer {
                request: pending_request,
                ..
            } if *pending_request == request
        )
    }
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

pub(crate) enum GpuZoneSamplingDispatch {
    Unsupported,
    Ready,
    Saturated,
    Pending(PendingGpuZoneSampling),
}

pub(crate) struct PendingGpuZoneSampling {
    output_generation: u64,
    sampling_plan: Option<GpuSamplingPlanKey>,
    pending_readback: PendingGpuSampleReadback,
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
        Self::with_render_device(GpuRenderDevice::new("SparkleFlinger GPU compositor")?)
    }

    pub(crate) fn with_render_device(render_device: GpuRenderDevice) -> Result<Self> {
        let probe = probe_render_device(&render_device)?;
        #[cfg(all(target_os = "linux", feature = "servo-gpu-import"))]
        if probe.linux_servo_gpu_import_backend_compatible
            && let Err(error) = hypercolor_core::effect::install_servo_gpu_import_device(
                render_device.device_handle(),
            )
        {
            tracing::debug!(
                %error,
                "Servo GPU import device was already installed or unavailable"
            );
        }
        let device = render_device.device().clone();
        let queue = render_device.queue().clone();

        let pipeline = GpuCompositorPipeline::new(&device);
        let spatial_sampler = GpuSpatialSampler::new(&device);

        Ok(Self {
            _render_device: render_device,
            device,
            queue,
            probe,
            pipeline,
            spatial_sampler,
            surfaces: None,
            display_finalize_surfaces: None,
            preview_surfaces: None,
            current_output: None,
            cached_composition_key: None,
            cached_readback_surface: None,
            cached_preview_surfaces: Vec::with_capacity(MAX_CACHED_PREVIEW_SURFACES),
            pending_output_submission: None,
            pending_preview_readback: None,
            pending_preview_submission: None,
            pending_preview_map: None,
            ready_preview_surface: None,
            output_generation: 0,
            cached_sample_result: None,
            #[cfg(test)]
            preview_surface_allocation_count: 0,
            #[cfg(test)]
            defer_preview_resolve_once: false,
            #[cfg(test)]
            defer_preview_map_resolve_once: false,
        })
    }

    pub(crate) fn supports_plan(&self, plan: &CompositionPlan) -> bool {
        plan.width > 0 && plan.height > 0 && !plan.layers.is_empty()
    }

    pub(crate) fn can_sample_zone_plan(&self, prepared_zones: &[PreparedZonePlan]) -> bool {
        GpuSamplingPlan::supports_prepared_zones(prepared_zones)
    }

    pub(crate) fn read_back_current_output_surface_for_cpu_sampling(
        &mut self,
    ) -> Result<Option<PublishedSurface>> {
        if self.current_output.is_none() {
            return Ok(None);
        }
        let Some((width, height)) = self
            .surfaces
            .as_ref()
            .map(|surfaces| (surfaces.width, surfaces.height))
        else {
            return Ok(None);
        };
        let pending_output_submission = self.pending_output_submission.take();
        Ok(self
            .read_back_current_output_surface(
                width,
                height,
                self.cached_composition_key.clone(),
                true,
                None,
                pending_output_submission,
            )?
            .sampling_surface)
    }

    pub(crate) fn current_output_frame(&mut self) -> Result<Option<GpuTextureFrame>> {
        self.flush_pending_output_submission()?;
        let Some(current_output) = self.current_output else {
            return Ok(None);
        };
        let Some(surfaces) = self.surfaces.as_ref() else {
            return Ok(None);
        };
        let texture = match current_output {
            GpuCompositorOutputSurface::Front => &surfaces.front,
            GpuCompositorOutputSurface::Back => &surfaces.back,
        };
        Ok(Some(GpuTextureFrame {
            width: surfaces.width,
            height: surfaces.height,
            storage_id: self.output_generation,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
        }))
    }

    fn flush_pending_output_submission(&mut self) -> Result<()> {
        if self.pending_preview_readback.is_some() {
            return self.submit_pending_preview_work();
        }
        if let Some(encoder) = self.pending_output_submission.take() {
            self.queue.submit(Some(encoder.finish()));
        }
        Ok(())
    }

    pub(crate) fn finalize_display_face(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<Option<PublishedSurface>> {
        if params.width == 0
            || params.height == 0
            || scene.width() == 0
            || scene.height() == 0
            || face.width() == 0
            || face.height() == 0
        {
            return Ok(None);
        }

        self.flush_pending_output_submission()?;
        self.ensure_display_finalize_surface_size(params.width, params.height);
        let surfaces = self
            .display_finalize_surfaces
            .as_mut()
            .expect("display finalize surfaces should exist after allocation");

        let scene_gpu = gpu_source_frame(scene);
        if scene_gpu.is_none() {
            surfaces.ensure_scene_source(&self.device, scene.width(), scene.height());
            let source = surfaces
                .scene_source
                .as_mut()
                .expect("scene source texture should exist before upload");
            upload_frame_into_cached_texture(
                &self.queue,
                &source.texture.texture,
                &mut source.cached_upload,
                scene,
                #[cfg(test)]
                &mut surfaces.scene_upload_count,
            );
        }
        let face_gpu = gpu_source_frame(face);
        if face_gpu.is_none() {
            surfaces.ensure_face_source(&self.device, face.width(), face.height());
            let source = surfaces
                .face_source
                .as_mut()
                .expect("face source texture should exist before upload");
            upload_frame_into_cached_texture(
                &self.queue,
                &source.texture.texture,
                &mut source.cached_upload,
                face,
                #[cfg(test)]
                &mut surfaces.face_upload_count,
            );
        }

        let scene_view = scene_gpu
            .as_ref()
            .map(GpuSourceFrame::view)
            .unwrap_or_else(|| {
                &surfaces
                    .scene_source
                    .as_ref()
                    .expect("uploaded scene source should exist")
                    .texture
                    .view
            });
        let face_view = face_gpu
            .as_ref()
            .map(GpuSourceFrame::view)
            .unwrap_or_else(|| {
                &surfaces
                    .face_source
                    .as_ref()
                    .expect("uploaded face source should exist")
                    .texture
                    .view
            });
        let bind_group = create_display_finalize_bind_group(
            &self.device,
            &self.pipeline,
            scene_view,
            face_view,
            &surfaces.output.view,
            &surfaces.yuv_output,
        );
        self.queue.write_buffer(
            &self.pipeline.display_finalize_params_buffer,
            0,
            &encode_display_finalize_params(&params, scene, face),
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU display finalize"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SparkleFlinger GPU display finalize pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline.display_finalize_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                params.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
                params.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
                1,
            );
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &surfaces.output.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &surfaces.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(surfaces.padded_bytes_per_row),
                    rows_per_image: Some(params.height),
                },
            },
            texture_extent(params.width, params.height),
        );

        try_read_back_texture_into_surface(
            &self.device,
            &surfaces.readback,
            u64::from(surfaces.padded_bytes_per_row) * u64::from(params.height),
            params.width,
            params.height,
            surfaces.padded_bytes_per_row,
            self.queue.submit(Some(encoder.finish())),
            &mut surfaces.readback_surfaces,
            #[cfg(test)]
            &mut surfaces.last_readback_bytes,
        )
    }

    pub(crate) fn finalize_display_face_yuv420(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<Option<DisplayYuv420Frame>> {
        if params.width == 0
            || params.height == 0
            || scene.width() == 0
            || scene.height() == 0
            || face.width() == 0
            || face.height() == 0
        {
            return Ok(None);
        }

        self.flush_pending_output_submission()?;
        self.ensure_display_finalize_surface_size(params.width, params.height);
        let surfaces = self
            .display_finalize_surfaces
            .as_mut()
            .expect("display finalize surfaces should exist after allocation");

        let scene_gpu = gpu_source_frame(scene);
        if scene_gpu.is_none() {
            surfaces.ensure_scene_source(&self.device, scene.width(), scene.height());
            let source = surfaces
                .scene_source
                .as_mut()
                .expect("scene source texture should exist before upload");
            upload_frame_into_cached_texture(
                &self.queue,
                &source.texture.texture,
                &mut source.cached_upload,
                scene,
                #[cfg(test)]
                &mut surfaces.scene_upload_count,
            );
        }
        let face_gpu = gpu_source_frame(face);
        if face_gpu.is_none() {
            surfaces.ensure_face_source(&self.device, face.width(), face.height());
            let source = surfaces
                .face_source
                .as_mut()
                .expect("face source texture should exist before upload");
            upload_frame_into_cached_texture(
                &self.queue,
                &source.texture.texture,
                &mut source.cached_upload,
                face,
                #[cfg(test)]
                &mut surfaces.face_upload_count,
            );
        }

        let scene_view = scene_gpu
            .as_ref()
            .map(GpuSourceFrame::view)
            .unwrap_or_else(|| {
                &surfaces
                    .scene_source
                    .as_ref()
                    .expect("uploaded scene source should exist")
                    .texture
                    .view
            });
        let face_view = face_gpu
            .as_ref()
            .map(GpuSourceFrame::view)
            .unwrap_or_else(|| {
                &surfaces
                    .face_source
                    .as_ref()
                    .expect("uploaded face source should exist")
                    .texture
                    .view
            });
        let bind_group = create_display_finalize_bind_group(
            &self.device,
            &self.pipeline,
            scene_view,
            face_view,
            &surfaces.output.view,
            &surfaces.yuv_output,
        );
        self.queue.write_buffer(
            &self.pipeline.display_finalize_params_buffer,
            0,
            &encode_display_finalize_params(&params, scene, face),
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU display finalize YUV420"),
            });
        encoder.clear_buffer(&surfaces.yuv_output, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SparkleFlinger GPU display finalize YUV420 pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline.display_finalize_yuv_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                params.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
                params.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
                1,
            );
        }
        encoder.copy_buffer_to_buffer(
            &surfaces.yuv_output,
            0,
            &surfaces.yuv_readback,
            0,
            u64::from(surfaces.yuv_layout.word_len),
        );

        try_read_back_yuv420_buffer(
            &self.device,
            &surfaces.yuv_readback,
            u64::from(surfaces.yuv_layout.total_len),
            params.width,
            params.height,
            surfaces.yuv_layout,
            self.queue.submit(Some(encoder.finish())),
            #[cfg(test)]
            &mut surfaces.last_yuv_readback_bytes,
        )
    }

    fn ensure_display_finalize_surface_size(&mut self, width: u32, height: u32) {
        if matches!(
            self.display_finalize_surfaces,
            Some(GpuDisplayFinalizeSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return;
        }

        self.display_finalize_surfaces = Some(GpuDisplayFinalizeSurfaceSet::new(
            &self.device,
            width,
            height,
        ));
    }

    fn cached_preview_surface(&self, key: &CachedPreviewSurfaceKey) -> Option<PublishedSurface> {
        self.cached_preview_surfaces
            .iter()
            .find(|cached| &cached.key == key)
            .map(|cached| cached.surface.clone())
    }

    fn store_cached_preview_surface(
        &mut self,
        key: CachedPreviewSurfaceKey,
        surface: PublishedSurface,
    ) {
        if let Some(index) = self
            .cached_preview_surfaces
            .iter()
            .position(|cached| cached.key == key)
        {
            self.cached_preview_surfaces.remove(index);
        }
        self.cached_preview_surfaces
            .insert(0, CachedPreviewSurface { key, surface });
        if self.cached_preview_surfaces.len() > MAX_CACHED_PREVIEW_SURFACES {
            self.cached_preview_surfaces
                .truncate(MAX_CACHED_PREVIEW_SURFACES);
        }
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
            return self.compose_bypass_layer(
                plan,
                readback_key,
                layer,
                requires_cpu_sampling_canvas,
                preview_surface_request,
            );
        }

        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width != plan.width || current_height != plan.height
        ) {
            self.discard_superseded_preview_work();
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
                && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_with_preview_surface(cached));
            }
            if let Some(surface) = self
                .cached_readback_surface
                .as_ref()
                .filter(|cached| {
                    cached.key.as_ref() == Some(key)
                        && preview_request_matches_plan(
                            preview_surface_request,
                            plan.width,
                            plan.height,
                        )
                })
                .map(|cached| cached.surface.clone())
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_from_surface(
                    surface,
                    requires_cpu_sampling_canvas,
                ));
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && self.has_pending_or_ready_preview_for(request)
            {
                return Ok(gpu_composed_without_surfaces());
            }
            let pending_output_submission = self.pending_output_submission.take();
            if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
                self.clear_superseded_preview_outputs();
            } else {
                self.discard_ready_and_pending_preview_surface();
            }
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                Some(key.clone()),
                requires_cpu_sampling_canvas,
                preview_surface_request,
                pending_output_submission,
            );
        }
        if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
            self.clear_superseded_preview_outputs();
        } else {
            self.discard_superseded_preview_work();
        }

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
            copy_frame_into_output_texture(
                &self.queue,
                &surfaces.front.texture,
                &mut surfaces.front_contents,
                &mut encoder,
                &first_layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
        } else {
            let full_range = wgpu::ImageSubresourceRange::default();
            encoder.clear_texture(&surfaces.front.texture, &full_range);
            surfaces.front_contents = None;
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

        let current_output = if use_front_as_current {
            GpuCompositorOutputSurface::Front
        } else {
            GpuCompositorOutputSurface::Back
        };
        self.current_output = Some(current_output);
        self.cached_composition_key.clone_from(&readback_key);
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
        match self.begin_sample_zone_plan_into(prepared_zones, zones)? {
            GpuZoneSamplingDispatch::Unsupported => Ok(false),
            GpuZoneSamplingDispatch::Ready => Ok(true),
            GpuZoneSamplingDispatch::Saturated => Ok(false),
            GpuZoneSamplingDispatch::Pending(pending) => {
                self.finish_pending_zone_sampling(pending, zones)?;
                Ok(true)
            }
        }
    }

    pub(crate) fn begin_sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<GpuZoneSamplingDispatch> {
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
            return Ok(GpuZoneSamplingDispatch::Ready);
        }
        let Some(output) = self.current_output else {
            return Ok(GpuZoneSamplingDispatch::Unsupported);
        };
        let (source, source_view, output_width, output_height) = {
            let Some(surfaces) = self.surfaces.as_ref() else {
                return Ok(GpuZoneSamplingDispatch::Unsupported);
            };
            let (source, source_view) = match output {
                GpuCompositorOutputSurface::Front => {
                    (GpuSampleSource::Front, surfaces.front.view.clone())
                }
                GpuCompositorOutputSurface::Back => {
                    (GpuSampleSource::Back, surfaces.back.view.clone())
                }
            };
            (source, source_view, surfaces.width, surfaces.height)
        };
        let pending_output_submission = self.pending_output_submission.take();
        let pending_preview_readback = self.pending_preview_readback.take();
        let sampling_dispatch = self.spatial_sampler.sample_texture_into(
            &self.device,
            &self.queue,
            source,
            &source_view,
            output_width,
            output_height,
            prepared_zones,
            zones,
            pending_output_submission,
        )?;
        if let Some(pending_preview_readback) = pending_preview_readback {
            if sampling_dispatch.submission_index.is_some() {
                if self.pending_preview_map.is_some() {
                    self.discard_pending_preview_map();
                }
                self.begin_pending_preview_map(pending_preview_readback)?;
                self.pending_preview_submission = None;
            } else {
                self.pending_preview_readback = Some(pending_preview_readback);
            }
        }
        if sampling_dispatch.queue_saturated {
            return Ok(GpuZoneSamplingDispatch::Saturated);
        }
        if let Some(pending_readback) = sampling_dispatch.pending_readback {
            return Ok(GpuZoneSamplingDispatch::Pending(PendingGpuZoneSampling {
                output_generation: self.output_generation,
                sampling_plan,
                pending_readback,
            }));
        }
        if sampling_dispatch.sampled
            && let Some(sampling_plan) = sampling_plan
        {
            let mut cached_zones = self
                .cached_sample_result
                .take()
                .map_or_else(Vec::new, |cached| cached.zones);
            cached_zones.clone_from(zones);
            self.cached_sample_result = Some(CachedSampleResult {
                key: CachedSampleResultKey {
                    output_generation: self.output_generation,
                    sampling_plan,
                },
                zones: cached_zones,
            });
        }
        if sampling_dispatch.sampled {
            Ok(GpuZoneSamplingDispatch::Ready)
        } else {
            Ok(GpuZoneSamplingDispatch::Unsupported)
        }
    }

    pub(crate) fn finish_pending_zone_sampling(
        &mut self,
        mut pending: PendingGpuZoneSampling,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<()> {
        if !self.try_finish_pending_zone_sampling(&mut pending, zones)? {
            self.spatial_sampler.finish_pending_readback(
                &self.device,
                pending.pending_readback,
                zones,
            )?;
            self.cache_finished_zone_sampling(
                pending.output_generation,
                pending.sampling_plan,
                zones.as_slice(),
            );
        }
        Ok(())
    }

    pub(crate) fn try_finish_pending_zone_sampling(
        &mut self,
        pending: &mut PendingGpuZoneSampling,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        if !self.spatial_sampler.try_finish_pending_readback(
            &self.device,
            &mut pending.pending_readback,
            zones,
        )? {
            return Ok(false);
        }
        self.cache_finished_zone_sampling(
            pending.output_generation,
            pending.sampling_plan,
            zones.as_slice(),
        );
        Ok(true)
    }

    pub(crate) fn pending_zone_sampling_matches_current_work(
        &self,
        pending: &PendingGpuZoneSampling,
        prepared_zones: &[PreparedZonePlan],
    ) -> bool {
        pending.output_generation == self.output_generation
            && pending.sampling_plan == GpuSamplingPlan::key(prepared_zones)
    }

    pub(crate) fn take_last_sample_readback_wait_blocked(&mut self) -> bool {
        self.spatial_sampler.take_last_readback_wait_blocked()
    }

    pub(crate) const fn max_pending_zone_sampling(&self) -> usize {
        self.spatial_sampler.max_pending_readbacks()
    }

    pub(crate) fn discard_pending_zone_sampling(&mut self, pending: PendingGpuZoneSampling) {
        self.spatial_sampler
            .discard_pending_readback(pending.pending_readback);
    }

    fn cache_finished_zone_sampling(
        &mut self,
        output_generation: u64,
        sampling_plan: Option<GpuSamplingPlanKey>,
        zones: &[ZoneColors],
    ) {
        if output_generation != self.output_generation {
            return;
        }
        let Some(sampling_plan) = sampling_plan else {
            return;
        };
        let mut cached_zones = self
            .cached_sample_result
            .take()
            .map_or_else(Vec::new, |cached| cached.zones);
        cached_zones.clear();
        cached_zones.extend_from_slice(zones);
        self.cached_sample_result = Some(CachedSampleResult {
            key: CachedSampleResultKey {
                output_generation: self.output_generation,
                sampling_plan,
            },
            zones: cached_zones,
        });
    }

    pub(crate) fn resolve_preview_surface(&mut self) -> Result<Option<PublishedSurface>> {
        self.submit_pending_preview_work()?;

        if self.pending_preview_map.is_some() {
            if let Some(surface) = self.try_finish_pending_preview_map()? {
                return Ok(Some(surface));
            }
            if let Some(submission_index) = self.pending_preview_submission.clone()
                && self.preview_submission_ready(submission_index)?
            {
                self.pending_preview_submission = None;
            }
            return Ok(None);
        }

        if self.pending_preview_submission.is_some() || self.pending_preview_readback.is_some() {
            let Some(submission_index) = self.pending_preview_submission.take() else {
                return Ok(None);
            };
            if !self.preview_submission_ready(submission_index.clone())? {
                self.pending_preview_submission = Some(submission_index);
                return Ok(None);
            }
            let Some(pending_preview_readback) = self.pending_preview_readback.take() else {
                return Ok(None);
            };
            if self.pending_preview_map.is_some() {
                self.discard_pending_preview_map();
            }
            self.begin_pending_preview_map(pending_preview_readback)?;
            return self.try_finish_pending_preview_map();
        }

        if let Some(surface) = self.ready_preview_surface.take() {
            return Ok(Some(surface));
        }
        self.try_finish_pending_preview_map()
    }

    pub(crate) fn submit_pending_preview_work(&mut self) -> Result<()> {
        if self.pending_preview_submission.is_some() || self.pending_preview_readback.is_none() {
            return Ok(());
        }
        let Some(encoder) = self.pending_output_submission.take() else {
            return Ok(());
        };
        let submission_index = self.queue.submit(Some(encoder.finish()));
        if self.pending_preview_map.is_some() {
            self.pending_preview_submission = Some(submission_index);
            return Ok(());
        }
        let pending_preview_readback = self
            .pending_preview_readback
            .take()
            .expect("pending preview readback should exist before GPU preview submit");
        self.begin_pending_preview_map(pending_preview_readback)?;
        self.pending_preview_submission = None;
        Ok(())
    }

    fn discard_ready_and_pending_preview_surface(&mut self) {
        self.pending_preview_readback = None;
        self.pending_preview_submission = None;
        self.discard_pending_preview_map();
        self.ready_preview_surface = None;
    }

    fn clear_superseded_preview_outputs(&mut self) {
        self.pending_output_submission = None;
        self.pending_preview_readback = None;
        self.pending_preview_submission = None;
        self.ready_preview_surface = None;
    }

    fn discard_superseded_preview_work(&mut self) {
        self.clear_superseded_preview_outputs();
        self.discard_pending_preview_map();
    }

    pub(crate) fn discard_preview_work(&mut self) {
        self.discard_superseded_preview_work();
    }

    fn preview_submission_ready(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
    ) -> Result<bool> {
        #[cfg(test)]
        if std::mem::take(&mut self.defer_preview_resolve_once) {
            return Ok(false);
        }

        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: Some(Duration::ZERO),
        }) {
            Ok(_) => Ok(true),
            Err(wgpu::PollError::Timeout) => Ok(false),
            Err(error) => Err(error).context("GPU preview readiness poll failed"),
        }
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

        self.discard_pending_preview_map();
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
        self.cached_preview_surfaces.clear();
        self.pending_output_submission = None;
        self.pending_preview_readback = None;
        self.pending_preview_submission = None;
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
    }

    pub(crate) fn surface_snapshot(&self) -> Option<GpuCompositorSurfaceSnapshot> {
        self.surfaces
            .as_ref()
            .map(GpuCompositorSurfaceSet::snapshot)
    }

    #[cfg(test)]
    fn defer_next_preview_map_resolve(&mut self) {
        self.defer_preview_map_resolve_once = true;
    }

    fn begin_pending_preview_map(
        &mut self,
        pending_preview_readback: PendingPreviewReadback,
    ) -> Result<()> {
        let PendingPreviewReadback::PreviewBuffer { request, slot, .. } = &pending_preview_readback;
        let preview_surfaces = self
            .preview_surfaces
            .as_ref()
            .context("GPU scaled preview map requested before preview surfaces existed")?;
        let used_bytes =
            u64::from(preview_surfaces.padded_bytes_per_row) * u64::from(request.height);
        let slice = preview_surfaces.readback(*slot).slice(..used_bytes);
        let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.pending_preview_map = Some(PendingPreviewMap {
            readback: pending_preview_readback,
            used_bytes,
            receiver,
        });
        Ok(())
    }

    fn try_finish_pending_preview_map(&mut self) -> Result<Option<PublishedSurface>> {
        let Some(pending_preview_map) = self.pending_preview_map.as_ref() else {
            return Ok(None);
        };

        self.device
            .poll(wgpu::PollType::Poll)
            .context("GPU preview map poll failed")?;

        #[cfg(test)]
        if std::mem::take(&mut self.defer_preview_map_resolve_once) {
            return Ok(None);
        }

        match pending_preview_map.receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.pending_preview_map = None;
                return Err(error).context("GPU preview buffer mapping failed");
            }
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.pending_preview_map = None;
                anyhow::bail!("GPU preview channel closed before map completion");
            }
        }

        let pending_preview_map = self
            .pending_preview_map
            .take()
            .expect("GPU preview map should remain pending until completion");
        self.finish_mapped_preview_surface(
            pending_preview_map.readback,
            pending_preview_map.used_bytes,
        )
        .map(Some)
    }

    fn has_pending_or_ready_preview_for(&self, request: PreviewSurfaceRequest) -> bool {
        self.ready_preview_surface.as_ref().is_some_and(|surface| {
            surface.width() == request.width && surface.height() == request.height
        }) || self
            .pending_preview_readback
            .as_ref()
            .is_some_and(|pending| pending.matches_request(request))
            || self
                .pending_preview_map
                .as_ref()
                .is_some_and(|pending| pending.readback.matches_request(request))
    }

    fn discard_pending_preview_map(&mut self) {
        let Some(pending_preview_map) = self.pending_preview_map.take() else {
            return;
        };

        let PendingPreviewReadback::PreviewBuffer { slot, .. } = pending_preview_map.readback;
        if let Some(preview_surfaces) = self.preview_surfaces.as_ref() {
            preview_surfaces.readback(slot).unmap();
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "sibling compose_* methods return Result; keeping this one wrapped preserves call-site uniformity"
    )]
    fn compose_bypass_layer(
        &mut self,
        plan: &CompositionPlan,
        readback_key: Option<CachedReadbackKey>,
        layer: &CompositionLayer,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
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
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => false,
            ProducerFrame::GpuTexture(_) => false,
        };
        let same_output = readback_key.as_ref().is_some_and(|key| {
            self.current_output == Some(GpuCompositorOutputSurface::Front)
                && self.cached_composition_key.as_ref() == Some(key)
        }) || same_surface_canvas;
        if same_output {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_bypassed_without_surfaces());
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(key) = readback_key.as_ref()
                && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_with_preview_surface(cached));
            }
            if let Some(surface) = self
                .cached_readback_surface
                .as_ref()
                .filter(|_| {
                    preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
                })
                .map(|cached| cached.surface.clone())
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_bypassed_surface_frame(
                    &surface,
                    requires_cpu_sampling_canvas,
                    requires_preview_surface,
                ));
            }
        }

        self.discard_superseded_preview_work();
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
        self.cached_composition_key.clone_from(&readback_key);
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
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => {
                unreachable!("GPU producer frames are composed instead of bypassed")
            }
            ProducerFrame::GpuTexture(_) => {
                unreachable!("GPU producer frames are composed instead of bypassed")
            }
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
        self.ready_preview_surface = None;
        Ok(composed)
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
        if !requires_cpu_sampling_canvas && let Some(request) = preview_surface_request {
            return self.stage_preview_surface_readback(
                current_output,
                width,
                height,
                readback_key,
                request,
                preview_request_matches_plan(Some(request), width, height),
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
            anyhow::bail!("GPU preview readback requires an explicit preview surface request");
        }

        let readback_buffer = &surfaces.readback;
        let readback_surfaces = &mut surfaces.readback_surfaces;
        let Some(sampling_surface) = try_read_back_texture_into_surface(
            &self.device,
            readback_buffer,
            u64::from(surfaces.padded_bytes_per_row) * u64::from(height),
            width,
            height,
            surfaces.padded_bytes_per_row,
            self.queue.submit(Some(encoder.finish())),
            readback_surfaces,
            #[cfg(test)]
            &mut surfaces.last_readback_bytes,
        )?
        else {
            tracing::trace!(
                width,
                height,
                "GPU output readback was not ready without blocking"
            );
            return Ok(gpu_composed_without_surfaces());
        };
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
        if self
            .preview_surfaces
            .as_ref()
            .is_some_and(|preview_surfaces| {
                preview_surfaces.width == width && preview_surfaces.height == height
            })
        {
            return Ok(());
        }
        if self.preview_surfaces.is_some() {
            self.discard_pending_preview_map();
        }
        if let Some(preview_surfaces) = self.preview_surfaces.as_mut()
            && preview_surfaces.fits_request(width, height)
        {
            preview_surfaces.reconfigure(width, height);
            return Ok(());
        }

        let (front_view, back_view) = {
            let surfaces = self.surfaces.as_ref().context(
                "GPU preview surfaces requested before compositor surfaces were allocated",
            )?;
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
        #[cfg(test)]
        {
            self.preview_surface_allocation_count =
                self.preview_surface_allocation_count.saturating_add(1);
        }
        Ok(())
    }

    fn stage_preview_surface_readback(
        &mut self,
        current_output: GpuCompositorOutputSurface,
        source_width: u32,
        source_height: u32,
        readback_key: Option<CachedReadbackKey>,
        request: PreviewSurfaceRequest,
        cache_as_full_size: bool,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<ComposedFrameSet> {
        if !cache_as_full_size
            && let Some(key) = readback_key.as_ref()
            && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                composition: key.clone(),
                request,
            })
        {
            self.pending_output_submission = None;
            return Ok(gpu_composed_with_preview_surface(cached));
        }

        self.ensure_preview_surface_size(request.width, request.height)?;
        let mapped_readback_slot = self
            .pending_preview_map
            .as_ref()
            .map(|pending| match &pending.readback {
                PendingPreviewReadback::PreviewBuffer { slot, .. } => *slot,
            });
        let preview_surfaces = self
            .preview_surfaces
            .as_mut()
            .expect("preview surfaces should exist after allocation");
        let readback_slot = preview_surfaces.select_readback_slot(mapped_readback_slot);
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
        encoder.copy_buffer_to_buffer(
            &preview_surfaces.output_buffer,
            0,
            preview_surfaces.readback(readback_slot),
            0,
            u64::from(preview_surfaces.padded_bytes_per_row) * u64::from(request.height),
        );
        self.pending_output_submission = Some(encoder);
        self.pending_preview_readback = Some(PendingPreviewReadback::PreviewBuffer {
            request,
            readback_key,
            cache_as_full_size,
            slot: readback_slot,
        });
        self.pending_preview_submission = None;
        Ok(gpu_composed_without_surfaces())
    }

    fn finish_mapped_preview_surface(
        &mut self,
        pending_preview_readback: PendingPreviewReadback,
        used_bytes: u64,
    ) -> Result<PublishedSurface> {
        let PendingPreviewReadback::PreviewBuffer {
            request,
            readback_key,
            cache_as_full_size,
            slot,
        } = pending_preview_readback;
        let preview_surfaces = self
            .preview_surfaces
            .as_mut()
            .context("GPU scaled preview finalize requested before preview surfaces existed")?;
        let readback = preview_surfaces.readback(slot).clone();
        let preview_surface = copy_mapped_readback_buffer_into_surface(
            &readback,
            used_bytes,
            request.width,
            request.height,
            preview_surfaces.padded_bytes_per_row,
            &mut preview_surfaces.readback_surfaces,
            #[cfg(test)]
            &mut preview_surfaces.last_readback_bytes,
        )?;
        if let Some(key) = readback_key {
            if cache_as_full_size {
                self.cached_readback_surface = Some(CachedReadbackSurface {
                    key: Some(key),
                    surface: preview_surface.clone(),
                });
            } else {
                self.store_cached_preview_surface(
                    CachedPreviewSurfaceKey {
                        composition: key,
                        request,
                    },
                    preview_surface.clone(),
                );
            }
        }
        Ok(preview_surface)
    }
}

pub(crate) fn probe_render_device(render_device: &GpuRenderDevice) -> Result<GpuCompositorProbe> {
    render_device.require_texture_usage(
        COMPOSITOR_TEXTURE_FORMAT,
        wgpu::TextureUsages::STORAGE_BINDING,
    )?;

    let info = render_device.info();
    let linux_servo_gpu_import_backend_compatible =
        info.linux_servo_gpu_import_backend_compatible();
    let linux_servo_gpu_import_backend_reason = info.linux_servo_gpu_import_backend_reason();
    Ok(GpuCompositorProbe {
        adapter_name: info.adapter_name,
        backend: backend_name(info.backend),
        texture_format: texture_format_name(COMPOSITOR_TEXTURE_FORMAT),
        max_texture_dimension_2d: info.max_texture_dimension_2d,
        max_storage_textures_per_shader_stage: info.max_storage_textures_per_shader_stage,
        linux_servo_gpu_import_backend_compatible,
        linux_servo_gpu_import_backend_reason,
    })
}

#[allow(
    clippy::missing_fields_in_debug,
    reason = "compositor owns non-Debug GPU handles; surfacing probe + snapshot is sufficient for tracing"
)]
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
            source: wgpu::ShaderSource::Wgsl(include_str!("display_finalize.wgsl").into()),
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
            cached_compose_params: None,
            front_contents: None,
            back_contents: None,
            cached_source_upload: None,
            #[cfg(test)]
            front_upload_count: 0,
            #[cfg(test)]
            source_upload_count: 0,
            #[cfg(test)]
            compose_dispatch_count: 0,
            #[cfg(test)]
            compose_param_write_count: 0,
            #[cfg(test)]
            last_readback_bytes: 0,
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

impl DisplayYuv420Layout {
    fn new(width: u32, height: u32) -> Self {
        let y_stride = width;
        let uv_stride = width.div_ceil(2);
        let uv_height = height.div_ceil(2);
        let y_plane_len = y_stride
            .checked_mul(height)
            .expect("display Y plane size should fit in u32");
        let u_plane_len = uv_stride
            .checked_mul(uv_height)
            .expect("display U/V plane size should fit in u32");
        let total_len = y_plane_len
            .checked_add(
                u_plane_len
                    .checked_mul(2)
                    .expect("display chroma plane size should fit in u32"),
            )
            .expect("display YUV buffer size should fit in u32");
        let word_len = total_len
            .div_ceil(4)
            .checked_mul(4)
            .expect("display YUV word-aligned buffer size should fit in u32");

        Self {
            y_stride,
            uv_stride,
            y_plane_len,
            u_plane_len,
            total_len,
            word_len,
        }
    }
}

impl GpuDisplayFinalizeSurfaceSet {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let yuv_layout = DisplayYuv420Layout::new(width, height);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU display finalize readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let yuv_output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU display finalize YUV output"),
            size: u64::from(yuv_layout.word_len),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let yuv_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU display finalize YUV readback"),
            size: u64::from(yuv_layout.word_len),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            width,
            height,
            padded_bytes_per_row,
            yuv_layout,
            output: GpuCompositorTexture::new(
                device,
                width,
                height,
                "SparkleFlinger Display Finalize Output",
            ),
            yuv_output,
            readback,
            yuv_readback,
            readback_surfaces: RenderSurfacePool::with_slot_count(
                SurfaceDescriptor::rgba8888(width, height),
                3,
            ),
            scene_source: None,
            face_source: None,
            #[cfg(test)]
            scene_upload_count: 0,
            #[cfg(test)]
            face_upload_count: 0,
            #[cfg(test)]
            last_readback_bytes: 0,
            #[cfg(test)]
            last_yuv_readback_bytes: 0,
        }
    }

    fn ensure_scene_source(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        ensure_display_source_texture(
            device,
            &mut self.scene_source,
            width,
            height,
            "SparkleFlinger Display Finalize Scene Source",
        );
    }

    fn ensure_face_source(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        ensure_display_source_texture(
            device,
            &mut self.face_source,
            width,
            height,
            "SparkleFlinger Display Finalize Face Source",
        );
    }
}

impl GpuDisplaySourceTexture {
    fn new(device: &wgpu::Device, width: u32, height: u32, label: &'static str) -> Self {
        Self {
            width,
            height,
            texture: GpuCompositorTexture::new(device, width, height, label),
            cached_upload: None,
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
        let padded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU preview output"),
            size: u64::from(width)
                .saturating_mul(u64::from(height))
                .saturating_mul(BYTES_PER_PIXEL as u64),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = std::array::from_fn(|slot| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(match slot {
                    0 => "SparkleFlinger GPU preview readback A",
                    1 => "SparkleFlinger GPU preview readback B",
                    _ => "SparkleFlinger GPU preview readback",
                }),
                size: u64::from(width)
                    .saturating_mul(u64::from(height))
                    .saturating_mul(BYTES_PER_PIXEL as u64),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        Self {
            width,
            height,
            capacity_width: width,
            capacity_height: height,
            padded_bytes_per_row,
            readbacks,
            next_readback_slot: 0,
            bind_groups: GpuPreviewScaleBindGroups::new(
                device,
                pipeline,
                front_view,
                back_view,
                &output_buffer,
            ),
            output_buffer,
            readback_surfaces: RenderSurfacePool::new(SurfaceDescriptor::rgba8888(width, height)),
            cached_readback_surfaces: Vec::with_capacity(MAX_CACHED_PREVIEW_READBACK_POOLS),
            cached_scale_params: None,
            #[cfg(test)]
            scale_param_write_count: 0,
            #[cfg(test)]
            preview_bind_group_count: 2,
            #[cfg(test)]
            last_readback_bytes: 0,
            #[cfg(test)]
            readback_surface_pool_allocation_count: 1,
        }
    }

    fn fits_request(&self, width: u32, height: u32) -> bool {
        width <= self.capacity_width && height <= self.capacity_height
    }

    fn reconfigure(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        let next_request = PreviewSurfaceRequest { width, height };
        let next_surfaces = self
            .take_cached_readback_surfaces(next_request)
            .unwrap_or_else(|| {
                #[cfg(test)]
                {
                    self.readback_surface_pool_allocation_count = self
                        .readback_surface_pool_allocation_count
                        .saturating_add(1);
                }
                RenderSurfacePool::new(SurfaceDescriptor::rgba8888(width, height))
            });
        let current_request = PreviewSurfaceRequest {
            width: self.width,
            height: self.height,
        };
        let current_surfaces = std::mem::replace(&mut self.readback_surfaces, next_surfaces);
        self.store_cached_readback_surfaces(current_request, current_surfaces);
        self.width = width;
        self.height = height;
        self.padded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
    }

    fn select_readback_slot(&mut self, mapped_slot: Option<usize>) -> usize {
        for offset in 0..PREVIEW_READBACK_SLOT_COUNT {
            let slot = (self.next_readback_slot + offset) % PREVIEW_READBACK_SLOT_COUNT;
            if Some(slot) != mapped_slot {
                self.next_readback_slot = (slot + 1) % PREVIEW_READBACK_SLOT_COUNT;
                return slot;
            }
        }
        mapped_slot
            .map(|slot| (slot + 1) % PREVIEW_READBACK_SLOT_COUNT)
            .unwrap_or(0)
    }

    fn readback(&self, slot: usize) -> &wgpu::Buffer {
        &self.readbacks[slot]
    }

    fn take_cached_readback_surfaces(
        &mut self,
        request: PreviewSurfaceRequest,
    ) -> Option<RenderSurfacePool> {
        self.cached_readback_surfaces
            .iter()
            .position(|cached| cached.request == request)
            .map(|index| self.cached_readback_surfaces.remove(index).surfaces)
    }

    fn store_cached_readback_surfaces(
        &mut self,
        request: PreviewSurfaceRequest,
        surfaces: RenderSurfacePool,
    ) {
        if let Some(index) = self
            .cached_readback_surfaces
            .iter()
            .position(|cached| cached.request == request)
        {
            self.cached_readback_surfaces.remove(index);
        }
        self.cached_readback_surfaces
            .insert(0, CachedPreviewReadbackSurfaces { request, surfaces });
        if self.cached_readback_surfaces.len() > MAX_CACHED_PREVIEW_READBACK_POOLS {
            self.cached_readback_surfaces
                .truncate(MAX_CACHED_PREVIEW_READBACK_POOLS);
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
        preview_buffer: &wgpu::Buffer,
    ) -> Self {
        Self {
            front_to_preview: create_preview_scale_bind_group(
                device,
                pipeline,
                front_view,
                preview_buffer,
                "SparkleFlinger GPU preview scale bind group front->preview",
            ),
            back_to_preview: create_preview_scale_bind_group(
                device,
                pipeline,
                back_view,
                preview_buffer,
                "SparkleFlinger GPU preview scale bind group back->preview",
            ),
        }
    }
}

fn ensure_display_source_texture(
    device: &wgpu::Device,
    source: &mut Option<GpuDisplaySourceTexture>,
    width: u32,
    height: u32,
    label: &'static str,
) {
    if source
        .as_ref()
        .is_some_and(|texture| texture.width == width && texture.height == height)
    {
        return;
    }
    *source = Some(GpuDisplaySourceTexture::new(device, width, height, label));
}

fn compose_layer_into_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    layer: &CompositionLayer,
    use_front_as_current: bool,
) {
    #[cfg(not(feature = "servo-gpu-import"))]
    let _ = device;

    let shader_mode = if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        ComposeShaderMode::Replace
    } else {
        match layer.mode {
            CompositionMode::Replace | CompositionMode::Alpha => ComposeShaderMode::Alpha,
            CompositionMode::Add => ComposeShaderMode::Add,
            CompositionMode::Screen => ComposeShaderMode::Screen,
        }
    };
    let output_surface = if use_front_as_current {
        GpuCompositorOutputSurface::Back
    } else {
        GpuCompositorOutputSurface::Front
    };

    if let Some(frame) = gpu_source_frame(&layer.frame)
        && shader_mode == ComposeShaderMode::Replace
    {
        record_gpu_source_upload_skipped();
        let output_texture = if use_front_as_current {
            &surfaces.back.texture
        } else {
            &surfaces.front.texture
        };
        copy_gpu_source_frame_into_texture(encoder, &frame, output_texture);
        set_texture_contents(surfaces, output_surface, None);
        return;
    }

    let gpu_frame = gpu_source_frame(&layer.frame);

    if gpu_frame.is_none() {
        upload_frame_into_source_texture(queue, surfaces, &layer.frame);
        if shader_mode == ComposeShaderMode::Replace {
            let output_texture = if use_front_as_current {
                &surfaces.back.texture
            } else {
                &surfaces.front.texture
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &surfaces.source.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: output_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                texture_extent(surfaces.width, surfaces.height),
            );
            set_texture_contents(surfaces, output_surface, cached_source_upload(&layer.frame));
            return;
        }
    }

    let params = encode_compose_params(surfaces.width, surfaces.height, shader_mode, layer.opacity);
    if surfaces.cached_compose_params != Some(params) {
        queue.write_buffer(&pipeline.params_buffer, 0, &params);
        surfaces.cached_compose_params = Some(params);
        #[cfg(test)]
        {
            surfaces.compose_param_write_count =
                surfaces.compose_param_write_count.saturating_add(1);
        }
    }
    #[cfg(test)]
    {
        surfaces.compose_dispatch_count = surfaces.compose_dispatch_count.saturating_add(1);
    }
    if let Some(frame) = gpu_frame {
        record_gpu_source_upload_skipped();
        let bind_group = {
            let (current_view, output_view) = if use_front_as_current {
                (&surfaces.front.view, &surfaces.back.view)
            } else {
                (&surfaces.back.view, &surfaces.front.view)
            };
            create_compose_bind_group(
                device,
                pipeline,
                current_view,
                frame.view(),
                output_view,
                "SparkleFlinger GPU imported producer bind group",
            )
        };
        dispatch_compose_pass(
            encoder,
            pipeline,
            &bind_group,
            surfaces.width,
            surfaces.height,
        );
        set_texture_contents(surfaces, output_surface, None);
        return;
    }

    let bind_group = if use_front_as_current {
        &surfaces.bind_groups.front_to_back
    } else {
        &surfaces.bind_groups.back_to_front
    };
    dispatch_compose_pass(
        encoder,
        pipeline,
        bind_group,
        surfaces.width,
        surfaces.height,
    );
    set_texture_contents(surfaces, output_surface, None);
}

fn dispatch_compose_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &GpuCompositorPipeline,
    bind_group: &wgpu::BindGroup,
    width: u32,
    height: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU compose pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.compose_pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(
        width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
        height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
        1,
    );
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

enum GpuSourceFrame<'a> {
    #[cfg(feature = "servo-gpu-import")]
    Imported(&'a hypercolor_core::effect::ImportedEffectFrame),
    Texture(&'a GpuTextureFrame),
}

impl GpuSourceFrame<'_> {
    const fn width(&self) -> u32 {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.width,
            Self::Texture(frame) => frame.width,
        }
    }

    const fn height(&self) -> u32 {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.height,
            Self::Texture(frame) => frame.height,
        }
    }

    fn texture(&self) -> &wgpu::Texture {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.texture.as_ref(),
            Self::Texture(frame) => &frame.texture,
        }
    }

    fn view(&self) -> &wgpu::TextureView {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.view.as_ref(),
            Self::Texture(frame) => &frame.view,
        }
    }
}

fn gpu_source_frame(frame: &ProducerFrame) -> Option<GpuSourceFrame<'_>> {
    match frame {
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(frame) => Some(GpuSourceFrame::Imported(frame)),
        ProducerFrame::GpuTexture(frame) => Some(GpuSourceFrame::Texture(frame)),
        ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => None,
    }
}

fn copy_frame_into_output_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    cached_upload: &mut Option<CachedSourceUpload>,
    encoder: &mut wgpu::CommandEncoder,
    frame: &ProducerFrame,
    #[cfg(test)] upload_count: &mut usize,
) {
    if let Some(frame) = gpu_source_frame(frame) {
        record_gpu_source_upload_skipped();
        copy_gpu_source_frame_into_texture(encoder, &frame, texture);
        *cached_upload = None;
        return;
    }

    upload_frame_into_cached_texture(
        queue,
        texture,
        cached_upload,
        frame,
        #[cfg(test)]
        upload_count,
    );
}

fn copy_gpu_source_frame_into_texture(
    encoder: &mut wgpu::CommandEncoder,
    frame: &GpuSourceFrame<'_>,
    output: &wgpu::Texture,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: frame.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: output,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        texture_extent(frame.width(), frame.height()),
    );
}

fn upload_frame_into_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, frame: &ProducerFrame) {
    let Some(rgba_bytes) = frame.cpu_rgba_bytes() else {
        return;
    };
    let bytes_per_row = frame.width() * BYTES_PER_PIXEL as u32;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba_bytes,
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
        ProducerFrame::Surface(surface) => Some(CachedSourceUpload {
            storage: surface.storage_identity(),
            generation: surface.generation(),
            width: surface.width(),
            height: surface.height(),
        }),
        ProducerFrame::Canvas(canvas) if canvas.is_shared() => Some(CachedSourceUpload {
            storage: canvas.storage_identity(),
            generation: 0,
            width: canvas.width(),
            height: canvas.height(),
        }),
        ProducerFrame::Canvas(_) => None,
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
        ProducerFrame::GpuTexture(_) => None,
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
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
        ProducerFrame::GpuTexture(_) => None,
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

fn try_read_back_texture_into_surface(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    submission_index: wgpu::SubmissionIndex,
    surfaces: &mut RenderSurfacePool,
    #[cfg(test)] last_readback_bytes: &mut u64,
) -> Result<Option<PublishedSurface>> {
    #[cfg(test)]
    {
        *last_readback_bytes = used_bytes;
    }
    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission_index.clone()),
        timeout: Some(GPU_READBACK_WAIT_TIMEOUT),
    }) {
        Ok(_) => {}
        Err(wgpu::PollError::Timeout) => return Ok(None),
        Err(error) => return Err(error).context("GPU readback readiness poll failed"),
    }

    let slice = buffer.slice(..used_bytes);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: Some(GPU_READBACK_WAIT_TIMEOUT),
        })
        .context("GPU readback map poll failed")?;
    match receiver.try_recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error).context("GPU readback buffer mapping failed"),
        Err(TryRecvError::Disconnected) => {
            anyhow::bail!("GPU readback channel closed before map completion");
        }
        Err(TryRecvError::Empty) => {
            buffer.unmap();
            return Ok(None);
        }
    }

    copy_mapped_readback_buffer_into_surface(
        buffer,
        used_bytes,
        width,
        height,
        padded_bytes_per_row,
        surfaces,
        #[cfg(test)]
        last_readback_bytes,
    )
    .map(Some)
}

fn try_read_back_yuv420_buffer(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    width: u32,
    height: u32,
    layout: DisplayYuv420Layout,
    submission_index: wgpu::SubmissionIndex,
    #[cfg(test)] last_readback_bytes: &mut u64,
) -> Result<Option<DisplayYuv420Frame>> {
    #[cfg(test)]
    {
        *last_readback_bytes = used_bytes;
    }
    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission_index.clone()),
        timeout: Some(GPU_READBACK_WAIT_TIMEOUT),
    }) {
        Ok(_) => {}
        Err(wgpu::PollError::Timeout) => return Ok(None),
        Err(error) => return Err(error).context("GPU YUV readback readiness poll failed"),
    }

    let mapped_bytes = u64::from(layout.word_len);
    let slice = buffer.slice(..mapped_bytes);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: Some(GPU_READBACK_WAIT_TIMEOUT),
        })
        .context("GPU YUV readback map poll failed")?;
    match receiver.try_recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error).context("GPU YUV readback buffer mapping failed"),
        Err(TryRecvError::Disconnected) => {
            anyhow::bail!("GPU YUV readback channel closed before map completion");
        }
        Err(TryRecvError::Empty) => {
            buffer.unmap();
            return Ok(None);
        }
    }

    let mapped = slice.get_mapped_range();
    let used_len = usize::try_from(used_bytes).expect("YUV readback should fit usize");
    let mut data = Vec::with_capacity(used_len);
    data.extend_from_slice(&mapped[..used_len]);
    drop(mapped);
    buffer.unmap();

    Ok(Some(DisplayYuv420Frame::from_vec(
        data,
        width,
        height,
        layout.y_stride,
        layout.uv_stride,
        usize::try_from(layout.y_plane_len).expect("Y plane length should fit usize"),
        usize::try_from(layout.u_plane_len).expect("U plane length should fit usize"),
        0,
        0,
    )))
}

fn copy_mapped_readback_buffer_into_surface(
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    surfaces: &mut RenderSurfacePool,
    #[cfg(test)] last_readback_bytes: &mut u64,
) -> Result<PublishedSurface> {
    #[cfg(test)]
    {
        *last_readback_bytes = used_bytes;
    }
    let slice = buffer.slice(..used_bytes);
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

fn create_display_finalize_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    scene: &wgpu::TextureView,
    face: &wgpu::TextureView,
    output: &wgpu::TextureView,
    output_yuv: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("SparkleFlinger GPU display finalize bind group"),
        layout: &pipeline.display_finalize_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(face),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.display_finalize_params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: output_yuv.as_entire_binding(),
            },
        ],
    })
}

fn create_preview_scale_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    source: &wgpu::TextureView,
    output: &wgpu::Buffer,
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
                resource: output.as_entire_binding(),
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

fn encode_display_finalize_params(
    params: &DisplayFinalizeParams,
    scene: &ProducerFrame,
    face: &ProducerFrame,
) -> [u8; DISPLAY_FINALIZE_PARAM_BYTES] {
    let mut bytes = [0u8; DISPLAY_FINALIZE_PARAM_BYTES];
    let circular = u32::from(params.circular);
    let yuv_layout = DisplayYuv420Layout::new(params.width, params.height);
    bytes[0..4].copy_from_slice(&params.width.to_le_bytes());
    bytes[4..8].copy_from_slice(&params.height.to_le_bytes());
    bytes[8..12].copy_from_slice(&circular.to_le_bytes());
    bytes[12..16].copy_from_slice(&(display_finalize_mode(params.blend_mode) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&scene.width().to_le_bytes());
    bytes[20..24].copy_from_slice(&scene.height().to_le_bytes());
    bytes[24..28].copy_from_slice(&face.width().to_le_bytes());
    bytes[28..32].copy_from_slice(&face.height().to_le_bytes());
    bytes[32..36].copy_from_slice(&display_brightness_factor(params.brightness).to_le_bytes());
    bytes[36..40]
        .copy_from_slice(&display_edge_behavior(params.viewport_edge_behavior).to_le_bytes());
    bytes[48..52].copy_from_slice(&params.viewport_position.x.to_le_bytes());
    bytes[52..56].copy_from_slice(&params.viewport_position.y.to_le_bytes());
    bytes[56..60].copy_from_slice(&params.viewport_size.x.to_le_bytes());
    bytes[60..64].copy_from_slice(&params.viewport_size.y.to_le_bytes());
    bytes[64..68].copy_from_slice(&params.viewport_rotation.cos().to_le_bytes());
    bytes[68..72].copy_from_slice(&params.viewport_rotation.sin().to_le_bytes());
    bytes[72..76].copy_from_slice(&params.viewport_scale.to_le_bytes());
    bytes[76..80].copy_from_slice(&params.opacity.clamp(0.0, 1.0).to_le_bytes());
    bytes[80..84].copy_from_slice(&yuv_layout.y_stride.to_le_bytes());
    bytes[84..88].copy_from_slice(&yuv_layout.uv_stride.to_le_bytes());
    bytes[88..92].copy_from_slice(&yuv_layout.y_plane_len.to_le_bytes());
    bytes[92..96].copy_from_slice(&yuv_layout.u_plane_len.to_le_bytes());
    bytes[40..44]
        .copy_from_slice(&display_fade_falloff(params.viewport_edge_behavior).to_le_bytes());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum DisplayFinalizeShaderMode {
    Replace = 0,
    Alpha = 1,
    Tint = 2,
    LumaReveal = 3,
    Add = 4,
    Screen = 5,
    Multiply = 6,
    Overlay = 7,
    SoftLight = 8,
    ColorDodge = 9,
    Difference = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeShaderMode {
    Replace = 0,
    Alpha = 1,
    Add = 2,
    Screen = 3,
}

fn display_finalize_mode(mode: DisplayFaceBlendMode) -> DisplayFinalizeShaderMode {
    match mode {
        DisplayFaceBlendMode::Replace => DisplayFinalizeShaderMode::Replace,
        DisplayFaceBlendMode::Alpha => DisplayFinalizeShaderMode::Alpha,
        DisplayFaceBlendMode::Tint => DisplayFinalizeShaderMode::Tint,
        DisplayFaceBlendMode::LumaReveal => DisplayFinalizeShaderMode::LumaReveal,
        DisplayFaceBlendMode::Add => DisplayFinalizeShaderMode::Add,
        DisplayFaceBlendMode::Screen => DisplayFinalizeShaderMode::Screen,
        DisplayFaceBlendMode::Multiply => DisplayFinalizeShaderMode::Multiply,
        DisplayFaceBlendMode::Overlay => DisplayFinalizeShaderMode::Overlay,
        DisplayFaceBlendMode::SoftLight => DisplayFinalizeShaderMode::SoftLight,
        DisplayFaceBlendMode::ColorDodge => DisplayFinalizeShaderMode::ColorDodge,
        DisplayFaceBlendMode::Difference => DisplayFinalizeShaderMode::Difference,
    }
}

fn display_edge_behavior(edge_behavior: EdgeBehavior) -> u32 {
    match edge_behavior {
        EdgeBehavior::Clamp => 0,
        EdgeBehavior::Wrap => 1,
        EdgeBehavior::Mirror => 2,
        EdgeBehavior::FadeToBlack { .. } => 3,
    }
}

fn display_fade_falloff(edge_behavior: EdgeBehavior) -> f32 {
    match edge_behavior {
        EdgeBehavior::FadeToBlack { falloff } => falloff,
        EdgeBehavior::Clamp | EdgeBehavior::Wrap | EdgeBehavior::Mirror => 0.0,
    }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the helper mirrors display byte brightness policy before encoding GPU uniforms"
)]
fn display_brightness_factor(brightness: f32) -> u32 {
    let value = brightness.clamp(0.0, 1.0);
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= 1.0 {
        return u32::from(u8::MAX);
    }
    (value.mul_add(f32::from(u8::MAX), 0.5)) as u32
}

#[cfg(test)]
#[allow(clippy::manual_let_else)]
mod tests {
    use std::sync::mpsc;

    use hypercolor_core::blend_math::encode_srgb_channel;
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_core::types::canvas::{
        Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
    };
    use hypercolor_types::event::ZoneColors;
    use hypercolor_types::scene::DisplayFaceBlendMode;
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection,
    };

    use super::{
        GpuSparkleFlinger, GpuZoneSamplingDispatch, PendingPreviewMap, PendingPreviewReadback,
    };
    use crate::render_thread::producer_queue::ProducerFrame;
    use crate::render_thread::sparkleflinger::gpu_sampling::GpuSamplingPlan;
    use crate::render_thread::sparkleflinger::{
        CompositionLayer, CompositionPlan, DisplayFinalizeParams, PreviewSurfaceRequest,
        cpu::CpuSparkleFlinger,
    };

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(color);
        canvas
    }

    fn solid_canvas_with_size(width: u32, height: u32, color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(width, height);
        canvas.fill(color);
        canvas
    }

    fn display_finalize_params(
        width: u32,
        height: u32,
        blend_mode: DisplayFaceBlendMode,
    ) -> DisplayFinalizeParams {
        DisplayFinalizeParams {
            width,
            height,
            circular: false,
            brightness: 1.0,
            viewport_position: NormalizedPosition::new(0.5, 0.5),
            viewport_size: NormalizedPosition::new(1.0, 1.0),
            viewport_rotation: 0.0,
            viewport_scale: 1.0,
            viewport_edge_behavior: EdgeBehavior::Clamp,
            blend_mode,
            opacity: 1.0,
        }
    }

    fn patterned_canvas(seed: u8) -> Canvas {
        patterned_canvas_with_size(4, 4, seed)
    }

    fn patterned_canvas_with_size(width: u32, height: u32, seed: u8) -> Canvas {
        let mut canvas = Canvas::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let base = seed.wrapping_add(u8::try_from(x * 31 + y * 17).unwrap_or_default());
                canvas.set_pixel(
                    x,
                    y,
                    Rgba::new(base, base.wrapping_add(53), base.wrapping_add(101), 255),
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

    #[allow(
        clippy::unnecessary_wraps,
        reason = "test helper mirrors the Option<PreviewSurfaceRequest> shape accepted by compositor entry points"
    )]
    fn full_preview_request(plan: &CompositionPlan) -> Option<PreviewSurfaceRequest> {
        Some(PreviewSurfaceRequest {
            width: plan.width,
            height: plan.height,
        })
    }

    fn assert_rgba_bytes_within(actual: &[u8], expected: &[u8], tolerance: u8) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                actual.abs_diff(*expected) <= tolerance,
                "rgba byte {index}: actual {actual}, expected {expected}, tolerance {tolerance}"
            );
        }
    }

    fn assert_zone_colors_within(actual: &[ZoneColors], expected: &[ZoneColors], tolerance: u8) {
        assert_eq!(actual.len(), expected.len());
        for (zone_index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(actual.zone_id, expected.zone_id);
            assert_eq!(actual.colors.len(), expected.colors.len());
            for (color_index, (actual, expected)) in
                actual.colors.iter().zip(&expected.colors).enumerate()
            {
                for channel in 0..3 {
                    assert!(
                        actual[channel].abs_diff(expected[channel]) <= tolerance,
                        "zone {zone_index} color {color_index} channel {channel}: actual {}, expected {}, tolerance {tolerance}",
                        actual[channel],
                        expected[channel],
                    );
                }
            }
        }
    }

    fn resolve_preview_surface_blocking(compositor: &mut GpuSparkleFlinger) -> PublishedSurface {
        loop {
            if let Some(surface) = compositor
                .resolve_preview_surface()
                .expect("GPU preview finalize should succeed")
            {
                return surface;
            }

            if let Some(submission_index) = compositor.pending_preview_submission.clone() {
                compositor
                    .device
                    .poll(wgpu::PollType::Wait {
                        submission_index: Some(submission_index),
                        timeout: None,
                    })
                    .expect("GPU preview wait should succeed");
            } else {
                assert!(
                    compositor.pending_preview_map.is_some(),
                    "pending preview work should remain available",
                );
                compositor
                    .device
                    .poll(wgpu::PollType::Poll)
                    .expect("GPU preview map poll should succeed");
            }
        }
    }

    fn defer_pending_preview_map(compositor: &mut GpuSparkleFlinger) {
        compositor.defer_next_preview_map_resolve();
        assert!(
            compositor
                .resolve_preview_surface()
                .expect("deferred preview finalize should not fail")
                .is_none()
        );

        if let Some(submission_index) = compositor.pending_preview_submission.clone() {
            compositor
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission_index),
                    timeout: None,
                })
                .expect("GPU preview wait should succeed");
            compositor.defer_next_preview_map_resolve();
            assert!(
                compositor
                    .resolve_preview_surface()
                    .expect("deferred preview map finalize should not fail")
                    .is_none()
            );
        }

        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_map.is_some());
    }

    fn sampling_layout(mode: SamplingMode) -> SpatialLayout {
        sampling_layout_with_led_count(mode, 4)
    }

    fn sampling_layout_with_led_count(mode: SamplingMode, led_count: u32) -> SpatialLayout {
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
                    count: led_count,
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
                brightness: None,
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
                brightness: None,
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
            Ok(compositor) => compositor.probe.clone(),
            Err(_) => return,
        };

        assert!(!probe.adapter_name.is_empty());
        assert!(!probe.texture_format.is_empty());
    }

    #[test]
    fn gpu_display_finalize_applies_replace_brightness_and_circular_mask() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let scene = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 0, 255, 255)));
        let face = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(255, 0, 0, 255)));
        let mut params = display_finalize_params(4, 4, DisplayFaceBlendMode::Replace);
        params.circular = true;
        params.brightness = 0.5;

        let surface = compositor
            .finalize_display_face(&scene, &face, params)
            .expect("display finalize should not fail")
            .expect("display finalize should produce a surface");
        let rgba = surface.rgba_bytes();

        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
        assert_eq!(
            &rgba[((2 * 4 + 2) * 4)..((2 * 4 + 2) * 4 + 4)],
            &[128, 0, 0, 255]
        );
    }

    #[test]
    fn gpu_display_finalize_alpha_blends_in_linear_light() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
        let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
        let mut params = display_finalize_params(2, 2, DisplayFaceBlendMode::Alpha);
        params.opacity = 0.5;

        let surface = compositor
            .finalize_display_face(&scene, &face, params)
            .expect("display finalize should not fail")
            .expect("display finalize should produce a surface");

        assert_eq!(
            &surface.rgba_bytes()[0..4],
            &[encode_srgb_channel(0.5), 0, 0, 255],
        );
    }

    #[test]
    fn gpu_display_finalize_yuv420_reads_back_luma_and_chroma_planes() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
        let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
        let params = display_finalize_params(2, 2, DisplayFaceBlendMode::Replace);

        let frame = compositor
            .finalize_display_face_yuv420(&scene, &face, params)
            .expect("display yuv finalize should not fail")
            .expect("display yuv finalize should produce a frame");

        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 2);
        assert_eq!(frame.y_stride, 2);
        assert_eq!(frame.uv_stride, 1);
        assert_eq!(frame.y_plane(), &[76, 76, 76, 76]);
        assert_eq!(frame.u_plane(), &[85]);
        assert_eq!(frame.v_plane(), &[255]);
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
    fn gpu_resize_clears_ready_preview_surface() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };

        compositor.ready_preview_surface = Some(PublishedSurface::from_owned_canvas(
            solid_canvas_with_size(4, 4, Rgba::new(12, 34, 56, 255)),
            0,
            0,
        ));

        compositor.ensure_surface_size(8, 8);

        assert!(compositor.ready_preview_surface.is_none());
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
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("GPU composition should succeed for replace + alpha plans");

        assert_rgba_bytes_within(
            composed
                .sampling_canvas
                .as_ref()
                .expect("GPU alpha compose should materialize a canvas")
                .as_rgba_bytes(),
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU alpha compose should materialize a canvas")
                .as_rgba_bytes(),
            1,
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
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let composed = compositor
            .compose(&plan, true, full_preview_request(&plan))
            .expect("GPU composition should succeed for add plans");

        assert_rgba_bytes_within(
            composed
                .sampling_canvas
                .as_ref()
                .expect("GPU add compose should materialize a canvas")
                .as_rgba_bytes(),
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU add compose should materialize a canvas")
                .as_rgba_bytes(),
            1,
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
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
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
        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
    }

    #[test]
    fn gpu_full_size_preview_uses_preview_buffer_path() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(slot_surface(Rgba::new(
                    255, 32, 0, 255,
                )))),
                CompositionLayer::alpha(
                    ProducerFrame::Surface(slot_surface(Rgba::new(32, 64, 255, 255))),
                    0.35,
                ),
            ],
        );
        let request = PreviewSurfaceRequest {
            width: 4,
            height: 4,
        };

        let composed = compositor
            .compose(&plan, false, Some(request))
            .expect("GPU composition should stage a full-size preview surface");

        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());
        assert!(composed.preview_surface.is_none());
        assert!(compositor.preview_surfaces.is_some());
        assert!(matches!(
            compositor.pending_preview_readback,
            Some(PendingPreviewReadback::PreviewBuffer {
                request: pending_request,
                cache_as_full_size: true,
                ..
            }) if pending_request == request
        ));

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 4);
        assert_eq!(preview_surface.height(), 4);
        assert!(compositor.cached_readback_surface.is_some());
        assert!(compositor.cached_preview_surfaces.is_empty());
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
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(33))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(144)), 0.35),
            ],
        );

        compositor
            .compose(&first_plan, false, Some(request))
            .expect("first scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);
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
        let _ = resolve_preview_surface_blocking(&mut compositor);

        let preview_surfaces = compositor
            .preview_surfaces
            .as_ref()
            .expect("preview surfaces should stay allocated across same-size requests");
        assert_eq!(preview_surfaces.scale_param_write_count, 1);
        assert_eq!(preview_surfaces.preview_bind_group_count, 2);
    }

    #[test]
    fn gpu_scaled_preview_reuses_buffers_across_smaller_requests() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(slot_surface(Rgba::new(
                    255, 32, 0, 255,
                )))),
                CompositionLayer::alpha(
                    ProducerFrame::Surface(slot_surface(Rgba::new(32, 64, 255, 255))),
                    0.35,
                ),
            ],
        );
        let large_request = PreviewSurfaceRequest {
            width: 3,
            height: 3,
        };
        let small_request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&plan, false, Some(large_request))
            .expect("large scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(compositor.preview_surface_allocation_count, 1);

        compositor
            .compose(&plan, false, Some(small_request))
            .expect("small scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);

        let preview_surfaces = compositor
            .preview_surfaces
            .as_ref()
            .expect("scaled preview should keep preview surfaces allocated");
        assert_eq!(preview_surfaces.width, 2);
        assert_eq!(preview_surfaces.height, 2);
        assert_eq!(preview_surfaces.capacity_width, 3);
        assert_eq!(preview_surfaces.capacity_height, 3);
        assert_eq!(preview_surfaces.preview_bind_group_count, 2);
        assert_eq!(preview_surfaces.last_readback_bytes, 16);
        assert_eq!(compositor.preview_surface_allocation_count, 1);

        let composed = compositor
            .compose(&plan, false, Some(large_request))
            .expect("restored scaled preview compose should succeed");
        let _ = composed
            .preview_surface
            .unwrap_or_else(|| resolve_preview_surface_blocking(&mut compositor));
        assert_eq!(compositor.preview_surface_allocation_count, 1);
    }

    #[test]
    fn gpu_scaled_preview_reuses_readback_surface_pools_across_size_flips() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let first_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(24))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(144)), 0.35),
            ],
        );
        let third_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(48))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(192)), 0.35),
            ],
        );
        let large_request = PreviewSurfaceRequest {
            width: 3,
            height: 3,
        };
        let small_request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&first_plan, false, Some(large_request))
            .expect("first scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);

        compositor
            .compose(&second_plan, false, Some(small_request))
            .expect("second scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);

        compositor
            .compose(&third_plan, false, Some(large_request))
            .expect("third scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);

        let preview_surfaces = compositor
            .preview_surfaces
            .as_ref()
            .expect("scaled preview should keep preview surfaces allocated");
        assert_eq!(preview_surfaces.readback_surface_pool_allocation_count, 2);
    }

    #[test]
    fn gpu_scaled_preview_reuses_cached_surface_across_size_flips() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(slot_surface(Rgba::new(
                    255, 32, 0, 255,
                )))),
                CompositionLayer::alpha(
                    ProducerFrame::Surface(slot_surface(Rgba::new(32, 64, 255, 255))),
                    0.35,
                ),
            ],
        );
        let large_request = PreviewSurfaceRequest {
            width: 3,
            height: 3,
        };
        let small_request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&plan, false, Some(large_request))
            .expect("large scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);

        compositor
            .compose(&plan, false, Some(small_request))
            .expect("small scaled preview compose should succeed");
        let _ = resolve_preview_surface_blocking(&mut compositor);

        let composed = compositor
            .compose(&plan, false, Some(large_request))
            .expect("restored scaled preview compose should succeed");
        let preview_surface = composed
            .preview_surface
            .expect("cached large scaled preview should be returned immediately");
        assert_eq!(preview_surface.width(), 3);
        assert_eq!(preview_surface.height(), 3);
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_output_submission.is_none());
        assert!(compositor.cached_preview_surfaces.len() >= 2);
    }

    #[test]
    fn gpu_preview_work_can_submit_before_finalize() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
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
        assert!(compositor.pending_preview_submission.is_none());

        compositor
            .submit_pending_preview_work()
            .expect("GPU preview submit should succeed");
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_map.is_some());
        assert!(compositor.pending_output_submission.is_none());

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
        assert!(compositor.pending_preview_submission.is_none());
    }

    #[test]
    fn gpu_active_preview_map_is_reused_on_identical_compose() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let base = slot_surface(Rgba::new(24, 96, 160, 255));
        let overlay = slot_surface(Rgba::new(200, 48, 96, 255));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(base.clone())),
                CompositionLayer::alpha(ProducerFrame::Surface(overlay.clone()), 0.35),
            ],
        );
        let request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&plan, false, Some(request))
            .expect("first compose should stage a scaled preview surface");
        compositor
            .submit_pending_preview_work()
            .expect("GPU preview submit should succeed");

        let composed = compositor
            .compose(&plan, false, Some(request))
            .expect("identical compose should reuse the pending preview map");
        assert!(composed.preview_surface.is_none());
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_map.is_some());
        assert!(compositor.pending_output_submission.is_none());

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
    }

    #[test]
    fn gpu_preview_finalize_can_defer_without_blocking() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );

        compositor
            .compose(
                &plan,
                false,
                Some(PreviewSurfaceRequest {
                    width: 2,
                    height: 2,
                }),
            )
            .expect("GPU composition should stage a scaled preview surface");
        compositor
            .submit_pending_preview_work()
            .expect("GPU preview submit should succeed");
        defer_pending_preview_map(&mut compositor);

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_map.is_none());
    }

    #[test]
    fn gpu_matching_pending_preview_map_is_reused_on_identical_compose() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let base = slot_surface(Rgba::new(24, 96, 160, 255));
        let overlay = slot_surface(Rgba::new(200, 48, 96, 255));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Surface(base.clone())),
                CompositionLayer::alpha(ProducerFrame::Surface(overlay.clone()), 0.35),
            ],
        );
        let request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&plan, false, Some(request))
            .expect("first compose should stage a scaled preview surface");
        compositor
            .submit_pending_preview_work()
            .expect("GPU preview submit should succeed");
        defer_pending_preview_map(&mut compositor);

        let composed = compositor
            .compose(&plan, false, Some(request))
            .expect("identical compose should reuse the pending preview map");
        assert!(composed.preview_surface.is_none());
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_map.is_some());
        assert!(compositor.pending_output_submission.is_none());

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
    }

    #[test]
    fn gpu_deferred_preview_queues_next_compose_after_pending_map() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let first_plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                255, 32, 0, 255,
            )))),
        );
        let second_plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                32, 64, 255, 255,
            )))),
        );
        let request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&first_plan, false, Some(request))
            .expect("first compose should stage a preview surface");
        compositor
            .submit_pending_preview_work()
            .expect("first preview submit should succeed");
        defer_pending_preview_map(&mut compositor);

        compositor
            .compose(&second_plan, false, Some(request))
            .expect("second compose should queue behind the first deferred preview");
        assert!(compositor.ready_preview_surface.is_none());
        assert!(compositor.pending_preview_readback.is_some());

        let first_preview = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(&first_preview.rgba_bytes()[0..4], &[255, 32, 0, 255]);

        let second_preview = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(&second_preview.rgba_bytes()[0..4], &[32, 64, 255, 255]);
        assert!(
            compositor
                .resolve_preview_surface()
                .expect("queued preview resolve should not fail")
                .is_none()
        );
    }

    #[test]
    fn gpu_fresh_preview_restage_uses_alternate_readback_slot() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let first_plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                255, 32, 0, 255,
            )))),
        );
        let second_plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                32, 64, 255, 255,
            )))),
        );
        let request = PreviewSurfaceRequest {
            width: 2,
            height: 2,
        };

        compositor
            .compose(&first_plan, false, Some(request))
            .expect("first compose should stage a preview surface");
        compositor
            .submit_pending_preview_work()
            .expect("first preview submit should succeed");
        defer_pending_preview_map(&mut compositor);

        let first_slot = match compositor.pending_preview_map.as_ref() {
            Some(PendingPreviewMap {
                readback: PendingPreviewReadback::PreviewBuffer { slot, .. },
                ..
            }) => *slot,
            _ => panic!("first preview should be waiting on a preview-buffer map"),
        };

        compositor
            .compose(&second_plan, false, Some(request))
            .expect("second compose should stage a newer preview surface");
        let second_slot = match compositor.pending_preview_readback.as_ref() {
            Some(PendingPreviewReadback::PreviewBuffer { slot, .. }) => *slot,
            _ => panic!("second preview should keep a staged preview-buffer readback"),
        };
        assert_ne!(first_slot, second_slot);

        compositor
            .submit_pending_preview_work()
            .expect("second preview submit should succeed");
        assert!(compositor.pending_preview_submission.is_some());
        assert!(compositor.pending_preview_readback.is_some());

        let mapped_slot = match compositor.pending_preview_map.as_ref() {
            Some(PendingPreviewMap {
                readback: PendingPreviewReadback::PreviewBuffer { slot, .. },
                ..
            }) => *slot,
            _ => panic!("first preview should remain mapped while the newer preview is queued"),
        };
        assert_eq!(mapped_slot, first_slot);

        let first_preview = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(&first_preview.rgba_bytes()[0..4], &[255, 32, 0, 255]);

        let second_preview = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(&second_preview.rgba_bytes()[0..4], &[32, 64, 255, 255]);
        assert!(compositor.pending_preview_map.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_submission.is_none());
    }

    #[test]
    fn gpu_deferred_preview_is_superseded_by_non_bypass_resize_compose() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let first_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas_with_size(
                    4,
                    4,
                    Rgba::new(255, 32, 0, 255),
                ))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(solid_canvas_with_size(
                        4,
                        4,
                        Rgba::new(32, 64, 255, 255),
                    )),
                    0.35,
                ),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas_with_size(
                    2,
                    2,
                    Rgba::new(16, 220, 32, 255),
                ))),
                CompositionLayer::alpha(
                    ProducerFrame::Canvas(solid_canvas_with_size(
                        2,
                        2,
                        Rgba::new(255, 255, 255, 255),
                    )),
                    0.25,
                ),
            ],
        );

        compositor
            .compose(&first_plan, false, full_preview_request(&first_plan))
            .expect("first compose should stage a full-size preview");
        compositor
            .submit_pending_preview_work()
            .expect("first preview submit should succeed");
        defer_pending_preview_map(&mut compositor);

        compositor
            .compose(&second_plan, false, full_preview_request(&second_plan))
            .expect("resize compose should supersede the older deferred preview");

        let preview = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview.width(), 2);
        assert_eq!(preview.height(), 2);
        assert!(
            compositor
                .resolve_preview_surface()
                .expect("superseded resize preview resolve should not fail")
                .is_none()
        );
    }

    #[test]
    fn gpu_discard_superseded_preview_work_clears_preview_state() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };

        compositor.pending_output_submission = Some(compositor.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("stale cached preview test"),
            },
        ));
        compositor.pending_preview_readback = Some(PendingPreviewReadback::PreviewBuffer {
            request: PreviewSurfaceRequest {
                width: 2,
                height: 2,
            },
            readback_key: None,
            cache_as_full_size: false,
            slot: 0,
        });
        let (_sender, receiver) =
            mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        compositor.pending_preview_map = Some(PendingPreviewMap {
            readback: PendingPreviewReadback::PreviewBuffer {
                request: PreviewSurfaceRequest {
                    width: 2,
                    height: 2,
                },
                readback_key: None,
                cache_as_full_size: false,
                slot: 1,
            },
            used_bytes: 16,
            receiver,
        });
        compositor.ready_preview_surface = Some(PublishedSurface::from_owned_canvas(
            solid_canvas(Rgba::new(8, 16, 24, 255)),
            0,
            0,
        ));

        compositor.discard_superseded_preview_work();

        assert!(compositor.pending_output_submission.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_map.is_none());
        assert!(compositor.ready_preview_surface.is_none());
    }

    #[test]
    fn gpu_sampler_arms_preview_map_after_sampling_completion() {
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
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
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
        assert!(compositor.ready_preview_surface.is_none());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_map.is_some());

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 2);
        assert_eq!(preview_surface.height(), 2);
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_map.is_none());
    }

    #[test]
    fn gpu_zero_sample_plan_keeps_pending_preview_work() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 0));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
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
                .expect("GPU zone sampling should succeed for empty plans")
        );
        assert!(sampled.is_empty());
        assert!(compositor.pending_preview_readback.is_none());
        assert!(compositor.pending_preview_submission.is_none());
        assert!(compositor.pending_preview_map.is_some());

        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
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

        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("cached no-readback composition should remain sampleable")
        );
        assert_zone_colors_within(
            &sampled,
            &engine.sample(
                expected
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU compose should materialize a canvas"),
            ),
            1,
        );
    }

    #[test]
    fn gpu_compositor_reuses_compose_params_for_same_alpha_shape() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let first_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(180)), 0.35),
            ],
        );

        compositor
            .compose(&first_plan, false, None)
            .expect("first GPU composition should succeed");
        assert_eq!(
            compositor
                .surfaces
                .as_ref()
                .expect("surface allocation should exist after composition")
                .compose_param_write_count,
            1
        );

        compositor
            .compose(&second_plan, false, None)
            .expect("second GPU composition should succeed");
        assert_eq!(
            compositor
                .surfaces
                .as_ref()
                .expect("surface allocation should persist across compositions")
                .compose_param_write_count,
            1
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
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
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

        assert_zone_colors_within(&sampled, &expected_zones, 1);
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
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
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

        assert_zone_colors_within(&sampled, &expected_zones, 1);
    }

    #[test]
    fn gpu_sampling_matches_cpu_after_canvas_resize() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let plan = CompositionPlan::single(
            8,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas_with_size(8, 4, 21))),
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
        let expected_zones = engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize resized canvas"),
        );

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before resized sampling");
        let mut sampled = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
                .expect("GPU sampling should succeed for resized canvas")
        );

        assert_zone_colors_within(&sampled, &expected_zones, 1);
    }

    #[test]
    fn gpu_sampler_rejects_gaussian_plans_without_dispatch() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::GaussianArea {
            sigma: 1.0,
            radius: 2,
        }));
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(21))),
        );

        assert!(!compositor.can_sample_zone_plan(engine.sampling_plan().as_ref()));
        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should still succeed before Gaussian fallback");
        assert!(matches!(
            compositor
                .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
                .expect("unsupported GPU sampling mode should be non-fatal"),
            GpuZoneSamplingDispatch::Unsupported
        ));
        assert_eq!(compositor.spatial_sampler.sample_dispatch_count(), 0);
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
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
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
    fn gpu_cached_sample_hit_preserves_retained_preview_submission() {
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

        let composed = compositor
            .compose(&plan, false, full_preview_request(&plan))
            .expect("retained GPU composition should stage a preview surface");
        assert!(composed.preview_surface.is_none());
        assert!(compositor.pending_output_submission.is_some());
        assert!(compositor.pending_preview_readback.is_some());

        let mut cached_sample = Vec::new();
        match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut cached_sample)
            .expect("cached GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Ready => {}
            _ => panic!("cached GPU sample should reuse the retained result"),
        }

        assert_eq!(cached_sample, first_sample);
        assert!(compositor.pending_output_submission.is_some());
        assert!(compositor.pending_preview_readback.is_some());

        compositor
            .submit_pending_preview_work()
            .expect("preview submit should still succeed after cached sample reuse");
        let preview_surface = resolve_preview_surface_blocking(&mut compositor);
        assert_eq!(preview_surface.width(), 4);
        assert_eq!(preview_surface.height(), 4);
    }

    #[test]
    fn gpu_sampler_caches_bind_groups_by_output_surface() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let back_output_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(7))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(41)), 0.35),
            ],
        );
        let front_output_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(11))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(53)), 0.35),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(97)), 0.2),
            ],
        );

        compositor
            .compose(&back_output_plan, false, None)
            .expect("back-output GPU composition should succeed");
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
                .expect("back-output GPU sample should succeed")
        );
        assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 1);

        compositor
            .compose(&front_output_plan, false, None)
            .expect("front-output GPU composition should succeed");
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
                .expect("front-output GPU sample should succeed")
        );
        assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 2);

        compositor
            .compose(&back_output_plan, false, None)
            .expect("second back-output GPU composition should succeed");
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
                .expect("second back-output GPU sample should succeed")
        );
        assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 2);
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
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(180)), 0.35),
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
    fn gpu_sampler_copies_only_live_sample_bytes_after_capacity_growth() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let large_engine =
            SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 16));
        let small_engine =
            SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 4));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before readback sizing tests");

        let mut large_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(large_engine.sampling_plan().as_ref(), &mut large_sample)
                .expect("large GPU sample should succeed")
        );
        assert_eq!(compositor.spatial_sampler.last_readback_copy_bytes(), 64);

        let mut small_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(small_engine.sampling_plan().as_ref(), &mut small_sample)
                .expect("smaller GPU sample should succeed after capacity growth")
        );
        assert_eq!(compositor.spatial_sampler.last_readback_copy_bytes(), 16);
        assert_eq!(
            small_sample,
            small_engine.sample(
                expected
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU compose should materialize a canvas"),
            )
        );
    }

    #[test]
    fn gpu_sampler_rotates_readback_slots_for_overlapped_dispatches() {
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
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before overlapped sample dispatch testing");

        let mut first_sample = Vec::new();
        let first_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
            .expect("first GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("first GPU sample dispatch should defer readback completion"),
        };

        let mut second_sample = Vec::new();
        let second_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut second_sample)
            .expect("second GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("second GPU sample dispatch should defer readback completion"),
        };

        let mut third_sample = Vec::new();
        let third_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut third_sample)
            .expect("third GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("third GPU sample dispatch should defer readback completion"),
        };

        assert_ne!(
            first_pending.pending_readback.readback_slot(),
            second_pending.pending_readback.readback_slot()
        );
        assert_ne!(
            first_pending.pending_readback.readback_slot(),
            third_pending.pending_readback.readback_slot()
        );
        assert_ne!(
            second_pending.pending_readback.readback_slot(),
            third_pending.pending_readback.readback_slot()
        );

        compositor
            .finish_pending_zone_sampling(first_pending, &mut first_sample)
            .expect("first pending sample finalize should succeed");
        compositor
            .finish_pending_zone_sampling(second_pending, &mut second_sample)
            .expect("second pending sample finalize should succeed");
        compositor
            .finish_pending_zone_sampling(third_pending, &mut third_sample)
            .expect("third pending sample finalize should succeed");

        assert_eq!(
            first_sample,
            engine.sample(
                expected
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU compose should materialize a canvas"),
            )
        );
        assert_eq!(second_sample, first_sample);
        assert_eq!(third_sample, first_sample);
    }

    #[test]
    fn gpu_sampler_refuses_a_fourth_overlapped_readback_until_a_slot_is_released() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let alternate_engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
            radius_x: 1.0,
            radius_y: 1.0,
        }));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before saturation testing");

        let first_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("first GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("first GPU sample dispatch should defer readback completion"),
        };
        let second_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("second GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("second GPU sample dispatch should defer readback completion"),
        };
        let third_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("third GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("third GPU sample dispatch should defer readback completion"),
        };

        assert!(
            matches!(
                compositor
                    .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
                    .expect("fourth GPU sample dispatch should stay non-fatal"),
                GpuZoneSamplingDispatch::Saturated
            ),
            "fourth overlapped GPU sample should refuse to reuse an in-flight readback slot"
        );

        compositor
            .finish_pending_zone_sampling(first_pending, &mut Vec::new())
            .expect("first pending sample finalize should succeed");

        assert!(
            matches!(
                compositor
                    .begin_sample_zone_plan_into(
                        alternate_engine.sampling_plan().as_ref(),
                        &mut Vec::new()
                    )
                    .expect("dispatch after releasing a slot should succeed"),
                GpuZoneSamplingDispatch::Pending(_)
            ),
            "freeing one readback slot should allow the next overlapped GPU sample dispatch"
        );

        compositor
            .finish_pending_zone_sampling(second_pending, &mut Vec::new())
            .expect("second pending sample finalize should succeed");
        compositor
            .finish_pending_zone_sampling(third_pending, &mut Vec::new())
            .expect("third pending sample finalize should succeed");
    }

    #[test]
    fn gpu_sampler_discard_releases_overlapped_readback_slot() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let alternate_engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
            radius_x: 1.0,
            radius_y: 1.0,
        }));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before discard testing");

        let first_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("first GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("first GPU sample dispatch should defer readback completion"),
        };
        let second_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("second GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("second GPU sample dispatch should defer readback completion"),
        };
        let third_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("third GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("third GPU sample dispatch should defer readback completion"),
        };

        compositor.discard_pending_zone_sampling(first_pending);

        assert!(
            matches!(
                compositor
                    .begin_sample_zone_plan_into(
                        alternate_engine.sampling_plan().as_ref(),
                        &mut Vec::new()
                    )
                    .expect("dispatch after discarding a slot should succeed"),
                GpuZoneSamplingDispatch::Pending(_)
            ),
            "discarding an unfinished GPU sample should free one readback slot for new work"
        );

        compositor
            .finish_pending_zone_sampling(second_pending, &mut Vec::new())
            .expect("second pending sample finalize should succeed");
        compositor
            .finish_pending_zone_sampling(third_pending, &mut Vec::new())
            .expect("third pending sample finalize should succeed");
    }

    #[test]
    fn gpu_sampler_decodes_overlapped_readbacks_with_distinct_sampling_plans() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let large_engine =
            SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Nearest, 16));
        let small_engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before distinct overlapped sample dispatches");

        let mut large_sample = Vec::new();
        let large_pending = match compositor
            .begin_sample_zone_plan_into(large_engine.sampling_plan().as_ref(), &mut large_sample)
            .expect("large GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("large GPU sample dispatch should defer readback completion"),
        };

        let mut small_sample = Vec::new();
        let small_pending = match compositor
            .begin_sample_zone_plan_into(small_engine.sampling_plan().as_ref(), &mut small_sample)
            .expect("small GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("small GPU sample dispatch should defer readback completion"),
        };

        compositor
            .finish_pending_zone_sampling(large_pending, &mut large_sample)
            .expect("large pending sample finalize should succeed");
        compositor
            .finish_pending_zone_sampling(small_pending, &mut small_sample)
            .expect("small pending sample finalize should succeed");

        let expected_canvas = expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas");
        assert_eq!(large_sample, large_engine.sample(expected_canvas));
        assert_eq!(small_sample, small_engine.sample(expected_canvas));
    }

    #[test]
    fn gpu_pending_sample_try_finish_can_prime_cache_without_blocking() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before nonblocking sample finalize");

        let mut sampled = Vec::new();
        let mut pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("GPU sample dispatch should defer readback completion"),
        };
        compositor
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(pending.pending_readback.submission_index()),
                timeout: None,
            })
            .expect("GPU sample submission should become ready");

        let dispatch_count_before = compositor.spatial_sampler.sample_dispatch_count();
        let mut deferred_sample = Vec::new();
        assert!(
            compositor
                .try_finish_pending_zone_sampling(&mut pending, &mut deferred_sample)
                .expect("nonblocking GPU sample finalize should succeed when ready")
        );
        assert!(!compositor.take_last_sample_readback_wait_blocked());
        assert_eq!(
            deferred_sample,
            engine.sample(
                expected
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU compose should materialize a canvas"),
            )
        );

        let mut cached_sample = Vec::new();
        match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut cached_sample)
            .expect("cached GPU sample should succeed after nonblocking finalize")
        {
            GpuZoneSamplingDispatch::Ready => {}
            _ => panic!("ready nonblocking finalize should prime the cached sample result"),
        }
        assert_eq!(cached_sample, deferred_sample);
        assert_eq!(
            compositor.spatial_sampler.sample_dispatch_count(),
            dispatch_count_before
        );
    }

    #[test]
    fn gpu_stale_pending_sample_finalize_does_not_poison_new_output_cache() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
        let first_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let second_plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(180)), 0.5),
            ],
        );
        let first_expected = CpuSparkleFlinger::new().compose(
            first_plan.clone(),
            true,
            full_preview_request(&first_plan),
        );
        let second_expected = CpuSparkleFlinger::new().compose(
            second_plan.clone(),
            true,
            full_preview_request(&second_plan),
        );

        compositor
            .compose(&first_plan, false, None)
            .expect("first GPU composition should succeed");
        let mut stale_sample = Vec::new();
        let stale_pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut stale_sample)
            .expect("first GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("first GPU sample dispatch should defer readback completion"),
        };

        compositor
            .compose(&second_plan, false, None)
            .expect("second GPU composition should succeed");

        compositor
            .finish_pending_zone_sampling(stale_pending, &mut stale_sample)
            .expect("stale pending sample finalize should still decode successfully");
        assert_eq!(
            stale_sample,
            engine.sample(
                first_expected
                    .sampling_canvas
                    .as_ref()
                    .expect("first CPU compose should materialize a canvas"),
            )
        );

        let dispatch_count_before = compositor.spatial_sampler.sample_dispatch_count();
        let mut current_sample = Vec::new();
        assert!(
            compositor
                .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut current_sample)
                .expect("current GPU sample should succeed")
        );
        assert_eq!(
            current_sample,
            engine.sample(
                second_expected
                    .sampling_canvas
                    .as_ref()
                    .expect("second CPU compose should materialize a canvas"),
            )
        );
        assert_eq!(
            compositor.spatial_sampler.sample_dispatch_count(),
            dispatch_count_before.saturating_add(1)
        );
    }

    #[test]
    fn gpu_pending_sample_stops_matching_after_layout_generation_changes() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let mut engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
        );

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before pending sample dispatch");
        let pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("GPU sample dispatch should defer readback completion"),
        };

        engine.update_layout(sampling_layout(SamplingMode::Nearest));

        assert!(
            !compositor.pending_zone_sampling_matches_current_work(
                &pending,
                engine.sampling_plan().as_ref()
            )
        );
        compositor.discard_pending_zone_sampling(pending);
    }

    #[test]
    fn gpu_can_late_read_back_surface_for_cpu_sampling_fallback() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, None);

        let composed = compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before late CPU sampling fallback");
        assert!(composed.sampling_canvas.is_none());
        assert!(composed.sampling_surface.is_none());

        let sampling_surface = compositor
            .read_back_current_output_surface_for_cpu_sampling()
            .expect("late CPU sampling readback should succeed")
            .expect("late CPU sampling readback should materialize a surface");
        let expected_canvas = expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas");

        assert_eq!(sampling_surface.width(), expected_canvas.width());
        assert_eq!(sampling_surface.height(), expected_canvas.height());
        for (actual, expected) in sampling_surface
            .rgba_bytes()
            .iter()
            .zip(expected_canvas.as_rgba_bytes())
        {
            assert!(actual.abs_diff(*expected) <= 1);
        }
    }

    #[test]
    fn gpu_sampler_skips_blocking_wait_when_readback_is_already_mapped() {
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
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before pending sample readback testing");

        let mut sampled = Vec::new();
        let pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("GPU sample dispatch should defer readback completion"),
        };
        compositor
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(pending.pending_readback.submission_index()),
                timeout: None,
            })
            .expect("GPU sample submission should become ready");

        compositor
            .finish_pending_zone_sampling(pending, &mut sampled)
            .expect("GPU pending sample finalize should succeed");

        assert_eq!(compositor.spatial_sampler.sample_readback_wait_count(), 0);
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
    fn gpu_sampler_nonblocking_finalize_eventually_completes_without_explicit_wait() {
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
                CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("GPU composition should succeed before nonblocking sample finalize");

        let mut sampled = Vec::new();
        let mut pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("GPU sample dispatch should defer readback completion"),
        };

        let start = std::time::Instant::now();
        loop {
            if compositor
                .try_finish_pending_zone_sampling(&mut pending, &mut sampled)
                .expect("nonblocking GPU sample finalize should not fail while pending")
            {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(2),
                "expected nonblocking GPU sample finalize to complete within 2 seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert!(!compositor.take_last_sample_readback_wait_blocked());
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
    fn gpu_pending_sample_matches_and_finishes_across_retained_bypass_frames() {
        let mut compositor = match GpuSparkleFlinger::new() {
            Ok(compositor) => compositor,
            Err(_) => return,
        };
        let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
        let source = PublishedSurface::from_owned_canvas(patterned_canvas(12), 1, 1);
        let plan = CompositionPlan::single(
            4,
            4,
            CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

        compositor
            .compose(&plan, false, None)
            .expect("initial retained GPU composition should succeed");

        let mut sampled = Vec::new();
        let mut pending = match compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sample dispatch should succeed")
        {
            GpuZoneSamplingDispatch::Pending(pending) => pending,
            _ => panic!("GPU sample dispatch should defer readback completion"),
        };

        let start = std::time::Instant::now();
        loop {
            compositor
                .compose(&plan, false, None)
                .expect("retained GPU composition should keep succeeding");
            assert!(
                compositor.pending_zone_sampling_matches_current_work(
                    &pending,
                    engine.sampling_plan().as_ref()
                ),
                "retained bypass should preserve pending GPU sample identity: pending_output_generation={} current_output_generation={} pending_sampling_plan={:?} current_sampling_plan={:?} current_output={:?} cached_key_present={}",
                pending.output_generation,
                compositor.output_generation,
                pending.sampling_plan,
                GpuSamplingPlan::key(engine.sampling_plan().as_ref()),
                compositor.current_output,
                compositor.cached_composition_key.is_some()
            );
            if compositor
                .try_finish_pending_zone_sampling(&mut pending, &mut sampled)
                .expect("nonblocking GPU sample finalize should not fail while pending")
            {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(2),
                "expected retained pending GPU sample to complete within 2 seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

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
                CompositionLayer::screen(ProducerFrame::Canvas(patterned_canvas(96)), 0.6),
            ],
        );
        let expected =
            CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
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
        assert_zone_colors_within(&sampled, &expected_zones, 1);
    }
}
