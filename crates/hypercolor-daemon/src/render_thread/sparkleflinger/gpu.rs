use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use anyhow::{Context, Result};
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, Canvas, PublishedSurface};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::ZoneId;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, DisplayFinalizeCacheKey,
    DisplayFinalizeParams, MediaTextureSourceKey, PreviewSurfaceRequest,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::gpu_device::{
    GpuBackendPreference, GpuRenderDevice, backend_name, device_type_name, texture_format_name,
};
use crate::render_thread::producer_queue::{GpuTextureFrame, GpuTextureFrameOrigin, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSampleSource, GpuSamplingPlan, GpuSamplingPlanKey, GpuSpatialSampler,
    PendingGpuSampleReadback,
};

mod display_finalize;
mod media_upload;
mod pipeline;
mod preview;
mod readback;
mod source;
mod telemetry;

#[cfg(test)]
use display_finalize::{DISPLAY_FINALIZE_READBACK_SLOT_COUNT, wait_for_display_finalize_readback};
pub(crate) use display_finalize::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame, PendingGpuDisplayFinalize,
};
use display_finalize::{
    GpuDisplayFinalizeFormat, GpuDisplayFinalizeSurfaceSet, GpuDisplaySourceTexture,
    begin_display_finalize_readback, create_display_finalize_bind_group,
    encode_display_finalize_params, finish_yuv420_display_readback,
    poll_display_finalize_readback_ready,
};
#[cfg(test)]
use media_upload::MEDIA_UPLOAD_TEXTURE_RING_LEN;
use media_upload::{
    MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES, MediaUploadTextureKey, MediaUploadTexturePool,
};
use pipeline::GpuCompositorPipeline;
use preview::{
    CachedPreviewSurface, CachedPreviewSurfaceKey, GpuPreviewSurfaceSet, PendingPreviewMap,
    PendingPreviewReadback, bypass_preview_surface, encode_preview_scale_params,
    preview_request_matches_plan,
};
use readback::{
    CachedReadbackKey, CachedReadbackSurface, copy_mapped_readback_buffer_into_surface,
};
use source::{
    CachedGpuSourceCopy, CachedSourceUpload, GpuSourceFrame, cached_readback_key,
    cached_source_upload, copy_frame_into_output_texture, copy_gpu_source_frame_into_texture,
    gpu_source_frame, prepare_display_source_texture, upload_frame_into_cached_texture,
    upload_frame_into_source_texture, write_rgba_texture,
};
#[cfg(test)]
use telemetry::record_gpu_display_finalize_blocking_wait;
pub(crate) use telemetry::{GpuSparkleFlingerTelemetrySnapshot, record_gpu_display_finalize_latch};
use telemetry::{
    record_gpu_display_finalize_attempt, record_gpu_display_finalize_result,
    record_gpu_display_finalize_surface_realloc, record_gpu_media_texture_upload,
    record_gpu_source_upload_skipped,
};

pub(crate) fn gpu_sparkleflinger_telemetry_snapshot() -> GpuSparkleFlingerTelemetrySnapshot {
    telemetry::gpu_sparkleflinger_telemetry_snapshot()
}

const COMPOSITOR_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const COMPOSE_WORKGROUP_WIDTH: u32 = 8;
const COMPOSE_WORKGROUP_HEIGHT: u32 = 8;
const COMPOSE_PARAM_BYTES: usize = 96;
const SOURCE_COPY_PARAM_BYTES: usize = 16;
const DISPLAY_FINALIZE_PARAM_BYTES: usize = 96;
const PREVIEW_SCALE_PARAM_BYTES: usize = 16;
const MAX_CACHED_PREVIEW_SURFACES: usize = 3;

#[derive(Debug, Clone)]
pub(crate) struct GpuCompositorProbe {
    pub(crate) adapter_name: String,
    pub(crate) adapter_device_type: &'static str,
    pub(crate) backend: &'static str,
    pub(crate) texture_format: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
    pub(crate) software_adapter_reason: Option<&'static str>,
    pub(crate) servo_gpu_import_backend_compatible: bool,
    pub(crate) servo_gpu_import_backend_reason: Option<&'static str>,
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
    display_finalize_surfaces: HashMap<DisplayFinalizeCacheKey, GpuDisplayFinalizeSurfaceSet>,
    display_finalize_generation: u64,
    preview_surfaces: Option<GpuPreviewSurfaceSet>,
    media_texture_pools: HashMap<MediaUploadTextureKey, MediaUploadTexturePool>,
    media_texture_epoch: u64,
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
    producer_texture_generation: u64,
    cached_sample_result: Option<CachedSampleResult>,
    #[cfg(test)]
    preview_surface_allocation_count: usize,
    #[cfg(test)]
    defer_preview_resolve_once: bool,
    #[cfg(test)]
    defer_preview_map_resolve_once: bool,
}

struct GpuCompositorSurfaceSet {
    width: u32,
    height: u32,
    front: GpuCompositorTexture,
    back: GpuCompositorTexture,
    source: GpuCompositorTexture,
    bind_groups: GpuCompositorBindGroups,
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
}

struct GpuCompositorTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct GpuCompositorBindGroups {
    front_to_back: wgpu::BindGroup,
    back_to_front: wgpu::BindGroup,
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
        Self::with_render_device(GpuRenderDevice::new_with_backend_preference(
            "SparkleFlinger GPU compositor",
            servo_import_backend_preference(),
        )?)
    }

    pub(crate) fn with_render_device(render_device: GpuRenderDevice) -> Result<Self> {
        let probe = probe_render_device(&render_device)?;
        #[cfg(all(
            any(target_os = "linux", target_os = "macos", target_os = "windows"),
            feature = "servo-gpu-import"
        ))]
        {
            let info = render_device.info();
            #[cfg(target_os = "windows")]
            let servo_adapter_info = Some(hypercolor_core::effect::ServoGpuImportAdapterInfo {
                vendor_id: info.adapter_vendor_id,
                device_id: info.adapter_device_id,
            });
            #[cfg(not(target_os = "windows"))]
            let servo_adapter_info = None;
            if info.servo_gpu_import_backend_compatible()
                && let Err(error) = hypercolor_core::effect::install_servo_gpu_import_device(
                    render_device.device_handle(),
                    servo_adapter_info,
                )
            {
                tracing::debug!(
                    %error,
                    "Servo GPU import device was already installed or unavailable"
                );
            } else if let Some(reason) = info.servo_gpu_import_backend_reason() {
                tracing::debug!(reason, "Servo GPU import device was not installed");
            }
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
            display_finalize_surfaces: HashMap::new(),
            display_finalize_generation: 0,
            preview_surfaces: None,
            media_texture_pools: HashMap::new(),
            media_texture_epoch: 0,
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
            producer_texture_generation: 0,
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
        plan.width > 0
            && plan.height > 0
            && !plan.layers.is_empty()
            && plan.layers.iter().all(|layer| {
                gpu_source_frame(&layer.frame).is_some()
                    || layer.frame_matches_size(plan.width, plan.height)
            })
    }

    pub(crate) fn can_sample_zone_plan(&self, prepared_zones: &[PreparedZonePlan]) -> bool {
        GpuSamplingPlan::supports_prepared_zones(prepared_zones)
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
            origin: GpuTextureFrameOrigin::CompositorOutput,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
        }))
    }

    #[cfg(test)]
    pub(crate) fn upload_canvas_frame(&mut self, canvas: &Canvas) -> Option<GpuTextureFrame> {
        self.upload_media_canvas_frame(MediaTextureSourceKey::for_test(0), canvas)
    }

    pub(crate) fn begin_media_upload_frame(&mut self) {
        self.media_texture_epoch = self.media_texture_epoch.saturating_add(1);
        self.prune_idle_media_texture_pools();
    }

    fn prune_idle_media_texture_pools(&mut self) {
        let current_epoch = self.media_texture_epoch;
        self.media_texture_pools.retain(|_, pool| {
            current_epoch.saturating_sub(pool.last_used_epoch)
                <= MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES
        });
    }

    pub(crate) fn upload_media_canvas_frame(
        &mut self,
        source: MediaTextureSourceKey,
        canvas: &Canvas,
    ) -> Option<GpuTextureFrame> {
        let max_texture_dimension = self.probe.max_texture_dimension_2d;
        if canvas.width() == 0
            || canvas.height() == 0
            || canvas.width() > max_texture_dimension
            || canvas.height() > max_texture_dimension
        {
            tracing::warn!(
                width = canvas.width(),
                height = canvas.height(),
                max_texture_dimension,
                "skipping GPU canvas upload for media frame with unsupported dimensions"
            );
            return None;
        }
        let key = MediaUploadTextureKey {
            source,
            width: canvas.width(),
            height: canvas.height(),
        };
        let pool = self
            .media_texture_pools
            .entry(key)
            .or_insert_with(MediaUploadTexturePool::new);
        let texture = pool.next_texture(&self.device, key, self.media_texture_epoch);
        record_gpu_media_texture_upload(canvas.width(), canvas.height());
        write_rgba_texture(
            &self.queue,
            &texture.texture,
            canvas.width(),
            canvas.height(),
            canvas.as_rgba_bytes(),
        );
        self.producer_texture_generation = self.producer_texture_generation.saturating_add(1);
        Some(GpuTextureFrame {
            width: canvas.width(),
            height: canvas.height(),
            storage_id: self.producer_texture_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
        })
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

    #[cfg(test)]
    pub(crate) fn finalize_display_face(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<Option<PublishedSurface>> {
        let pending = match self.begin_finalize_display_face(scene, face, params)? {
            GpuDisplayFinalizeDispatch::Pending(pending) => pending,
            GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
                return Ok(None);
            }
        };
        match self.finish_pending_display_finalization_blocking(pending)? {
            Some(GpuDisplayFinalizeFrame::Rgba(surface)) => Ok(Some(surface)),
            Some(GpuDisplayFinalizeFrame::Yuv420(_)) | None => Ok(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn finalize_display_face_yuv420(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<Option<DisplayYuv420Frame>> {
        let pending = match self.begin_finalize_display_face_yuv420(scene, face, params)? {
            GpuDisplayFinalizeDispatch::Pending(pending) => pending,
            GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
                return Ok(None);
            }
        };
        match self.finish_pending_display_finalization_blocking(pending)? {
            Some(GpuDisplayFinalizeFrame::Yuv420(frame)) => Ok(Some(frame)),
            Some(GpuDisplayFinalizeFrame::Rgba(_)) | None => Ok(None),
        }
    }

    pub(crate) fn begin_finalize_display_face(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<GpuDisplayFinalizeDispatch> {
        self.begin_display_finalize(scene, face, params, GpuDisplayFinalizeFormat::Rgba)
    }

    pub(crate) fn begin_finalize_display_face_yuv420(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<GpuDisplayFinalizeDispatch> {
        self.begin_display_finalize(scene, face, params, GpuDisplayFinalizeFormat::Yuv420)
    }

    fn begin_display_finalize(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
        format: GpuDisplayFinalizeFormat,
    ) -> Result<GpuDisplayFinalizeDispatch> {
        if params.width == 0
            || params.height == 0
            || scene.width() == 0
            || scene.height() == 0
            || face.width() == 0
            || face.height() == 0
        {
            return Ok(GpuDisplayFinalizeDispatch::Unsupported);
        }

        record_gpu_display_finalize_attempt(format == GpuDisplayFinalizeFormat::Yuv420);
        self.flush_pending_output_submission()?;
        self.retain_current_display_finalize_route(params.cache_key);
        self.ensure_display_finalize_surfaces(params.cache_key);
        let device = &self.device;
        let queue = &self.queue;
        let pipeline = &self.pipeline;
        let surfaces = self
            .display_finalize_surfaces
            .get_mut(&params.cache_key)
            .expect("display finalize surfaces should exist after allocation");
        let surface_generation = surfaces.generation;
        let Some((readback_slot, readback_buffer)) = surfaces.next_readback_buffer(format) else {
            record_gpu_display_finalize_result(false);
            return Ok(GpuDisplayFinalizeDispatch::Saturated);
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SparkleFlinger GPU display finalize"),
        });

        let scene_gpu = gpu_source_frame(scene);
        prepare_display_source_texture(
            device,
            queue,
            pipeline,
            &mut encoder,
            &mut surfaces.scene_source,
            scene,
            scene_gpu.as_ref(),
            "SparkleFlinger Display Scene Source",
            #[cfg(test)]
            &mut surfaces.scene_upload_count,
        );
        let face_gpu = gpu_source_frame(face);
        prepare_display_source_texture(
            device,
            queue,
            pipeline,
            &mut encoder,
            &mut surfaces.face_source,
            face,
            face_gpu.as_ref(),
            "SparkleFlinger Display Face Source",
            #[cfg(test)]
            &mut surfaces.face_upload_count,
        );

        let scene_view = scene_gpu
            .as_ref()
            .filter(|frame| !frame.needs_display_source_copy())
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
            .filter(|frame| !frame.needs_display_source_copy())
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
            device,
            pipeline,
            scene_view,
            face_view,
            &surfaces.output.view,
            &surfaces.yuv_output,
        );
        queue.write_buffer(
            &pipeline.display_finalize_params_buffer,
            0,
            &encode_display_finalize_params(&params, scene, face),
        );

        let (used_bytes, mapped_bytes) = match format {
            GpuDisplayFinalizeFormat::Rgba => {
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("SparkleFlinger GPU display finalize pass"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline.display_finalize_pipeline);
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
                        buffer: &readback_buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(surfaces.padded_bytes_per_row),
                            rows_per_image: Some(params.height),
                        },
                    },
                    texture_extent(params.width, params.height),
                );
                let bytes = u64::from(surfaces.padded_bytes_per_row) * u64::from(params.height);
                #[cfg(test)]
                {
                    surfaces.last_readback_bytes = bytes;
                }
                (bytes, bytes)
            }
            GpuDisplayFinalizeFormat::Yuv420 => {
                encoder.clear_buffer(&surfaces.yuv_output, 0, None);
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("SparkleFlinger GPU display finalize YUV420 pass"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline.display_finalize_yuv_pipeline);
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
                    &readback_buffer,
                    0,
                    u64::from(surfaces.yuv_layout.word_len),
                );
                #[cfg(test)]
                {
                    surfaces.last_yuv_readback_bytes = u64::from(surfaces.yuv_layout.total_len);
                }
                (
                    u64::from(surfaces.yuv_layout.total_len),
                    u64::from(surfaces.yuv_layout.word_len),
                )
            }
        };
        let submission_index = queue.submit(Some(encoder.finish()));
        let pending = begin_display_finalize_readback(PendingGpuDisplayFinalize::new(
            params.cache_key,
            surface_generation,
            format,
            params.width,
            params.height,
            surfaces.padded_bytes_per_row,
            surfaces.yuv_layout,
            used_bytes,
            mapped_bytes,
            submission_index,
            readback_buffer,
            readback_slot,
        ));
        Ok(GpuDisplayFinalizeDispatch::Pending(pending))
    }

    pub(crate) fn try_finish_pending_display_finalization(
        &mut self,
        pending: &mut PendingGpuDisplayFinalize,
    ) -> Result<Option<GpuDisplayFinalizeFrame>> {
        if let Err(error) = poll_display_finalize_readback_ready(&self.device, pending) {
            self.discard_pending_display_finalization_slot(pending);
            return Err(error);
        }
        if !pending.map_ready() {
            return Ok(None);
        }
        let frame = match self.finish_display_finalize_readback(pending) {
            Ok(frame) => frame,
            Err(error) => {
                pending.buffer.unmap();
                self.release_display_finalize_slot(
                    pending.cache_key,
                    pending.surface_generation,
                    pending.slot,
                );
                return Err(error);
            }
        };
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
        record_gpu_display_finalize_result(true);
        Ok(Some(frame))
    }

    pub(crate) fn discard_pending_display_finalization(
        &mut self,
        pending: PendingGpuDisplayFinalize,
    ) {
        pending.buffer.unmap();
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
    }

    fn discard_pending_display_finalization_slot(
        &mut self,
        pending: &mut PendingGpuDisplayFinalize,
    ) {
        pending.unmap_after_failed_map();
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
    }

    #[cfg(test)]
    fn finish_pending_display_finalization_blocking(
        &mut self,
        mut pending: PendingGpuDisplayFinalize,
    ) -> Result<Option<GpuDisplayFinalizeFrame>> {
        let wait_start = Instant::now();
        let ready = match wait_for_display_finalize_readback(&self.device, &mut pending) {
            Ok(ready) => ready,
            Err(error) => {
                self.discard_pending_display_finalization_slot(&mut pending);
                return Err(error);
            }
        };
        record_gpu_display_finalize_blocking_wait(wait_start.elapsed());
        if !ready {
            self.discard_pending_display_finalization(pending);
            record_gpu_display_finalize_result(false);
            return Ok(None);
        }
        let frame = match self.finish_display_finalize_readback(&pending) {
            Ok(frame) => frame,
            Err(error) => {
                pending.buffer.unmap();
                self.release_display_finalize_slot(
                    pending.cache_key,
                    pending.surface_generation,
                    pending.slot,
                );
                return Err(error);
            }
        };
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
        record_gpu_display_finalize_result(true);
        Ok(Some(frame))
    }

    fn finish_display_finalize_readback(
        &mut self,
        pending: &PendingGpuDisplayFinalize,
    ) -> Result<GpuDisplayFinalizeFrame> {
        match pending.format {
            GpuDisplayFinalizeFormat::Rgba => {
                let surfaces = self
                    .display_finalize_surfaces
                    .get_mut(&pending.cache_key)
                    .filter(|surfaces| surfaces.generation == pending.surface_generation)
                    .context("GPU display finalize surfaces changed before RGBA readback")?;
                copy_mapped_readback_buffer_into_surface(
                    &pending.buffer,
                    pending.used_bytes,
                    pending.width,
                    pending.height,
                    pending.padded_bytes_per_row,
                    &mut surfaces.readback_surfaces,
                    #[cfg(test)]
                    &mut surfaces.last_readback_bytes,
                )
                .map(GpuDisplayFinalizeFrame::Rgba)
            }
            GpuDisplayFinalizeFormat::Yuv420 => Ok(GpuDisplayFinalizeFrame::Yuv420(
                finish_yuv420_display_readback(pending),
            )),
        }
    }

    fn ensure_display_finalize_surfaces(&mut self, key: DisplayFinalizeCacheKey) {
        if !self.display_finalize_surfaces.contains_key(&key) {
            record_gpu_display_finalize_surface_realloc();
            self.display_finalize_generation = self.display_finalize_generation.saturating_add(1);
            self.display_finalize_surfaces.insert(
                key,
                GpuDisplayFinalizeSurfaceSet::new(
                    &self.device,
                    self.display_finalize_generation,
                    key.width,
                    key.height,
                ),
            );
        }
    }

    fn retain_current_display_finalize_route(&mut self, key: DisplayFinalizeCacheKey) {
        self.display_finalize_surfaces
            .retain(|cached_key, _| cached_key.group_id != key.group_id || *cached_key == key);
    }

    pub(crate) fn retain_display_finalize_groups(&mut self, active_group_ids: &[ZoneId]) {
        self.display_finalize_surfaces
            .retain(|key, _| active_group_ids.contains(&key.group_id));
    }

    fn release_display_finalize_slot(
        &mut self,
        key: DisplayFinalizeCacheKey,
        surface_generation: u64,
        slot: usize,
    ) {
        if let Some(surfaces) = self.display_finalize_surfaces.get_mut(&key)
            && surfaces.generation == surface_generation
        {
            surfaces.release_readback_slot(slot);
        }
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
            && self.layer_reuses_current_output_texture(layer, plan.width, plan.height)
        {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_composed_without_surfaces());
            }
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                readback_key,
                requires_cpu_sampling_canvas,
                preview_surface_request,
                None,
            );
        }
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

        if first_layer.can_bypass_for_size(plan.width, plan.height) {
            copy_frame_into_output_texture(
                &self.device,
                &self.queue,
                &self.pipeline,
                &surfaces.front,
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
        self.spatial_sampler.clear_bind_groups();
    }

    fn layer_reuses_current_output_texture(
        &self,
        layer: &CompositionLayer,
        width: u32,
        height: u32,
    ) -> bool {
        self.current_output.is_some()
            && layer.mode == CompositionMode::Replace
            && layer.opacity >= 1.0
            && layer.transform.is_none()
            && layer.adjust.is_none()
            && matches!(
                &layer.frame,
                ProducerFrame::GpuTexture(frame)
                    if frame.origin == GpuTextureFrameOrigin::CompositorOutput
                        && frame.storage_id == self.output_generation
                        && frame.width == width
                        && frame.height == height
            )
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
                self.discard_pending_preview_map();
                return Err(error).context("GPU preview buffer mapping failed");
            }
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.discard_pending_preview_map();
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
        if requires_cpu_sampling_canvas {
            if let Some(encoder) = encoder {
                self.pending_output_submission = Some(encoder);
            }
            return Ok(gpu_composed_without_surfaces());
        }
        let Some(current_output) = self.current_output else {
            anyhow::bail!("GPU readback requested without a composed output surface");
        };
        if let Some(request) = preview_surface_request {
            let cache_as_full_size = preview_request_matches_plan(Some(request), width, height);
            return self.stage_preview_surface_readback(
                current_output,
                width,
                height,
                readback_key,
                request,
                cache_as_full_size,
                encoder,
            );
        }
        if let Some(encoder) = encoder {
            self.pending_output_submission = Some(encoder);
        }
        Ok(gpu_composed_without_surfaces())
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

        let request_bytes_per_row = request.width.saturating_mul(BYTES_PER_PIXEL as u32);
        let direct_source_texture = if request.width == source_width
            && request.height == source_height
            && request_bytes_per_row.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        {
            let surfaces = self
                .surfaces
                .as_ref()
                .context("GPU preview readback requested before compositor surfaces existed")?;
            Some(match current_output {
                GpuCompositorOutputSurface::Front => surfaces.front.texture.clone(),
                GpuCompositorOutputSurface::Back => surfaces.back.texture.clone(),
            })
        } else {
            None
        };

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
        let mut encoder = encoder.unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU preview scale"),
                })
        });
        if let Some(source_texture) = direct_source_texture {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: preview_surfaces.readback(readback_slot),
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(preview_surfaces.padded_bytes_per_row),
                        rows_per_image: Some(request.height),
                    },
                },
                texture_extent(request.width, request.height),
            );
        } else {
            let bind_group = match current_output {
                GpuCompositorOutputSurface::Front => &preview_surfaces.bind_groups.front_to_preview,
                GpuCompositorOutputSurface::Back => &preview_surfaces.bind_groups.back_to_preview,
            };
            let params = encode_preview_scale_params(
                source_width,
                source_height,
                request.width,
                request.height,
            );
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
        }
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
    let servo_gpu_import_backend_compatible = info.servo_gpu_import_backend_compatible();
    let servo_gpu_import_backend_reason = info.servo_gpu_import_backend_reason();
    let linux_servo_gpu_import_backend_compatible =
        info.linux_servo_gpu_import_backend_compatible();
    let linux_servo_gpu_import_backend_reason = info.linux_servo_gpu_import_backend_reason();
    let software_adapter_reason = info.software_adapter_reason();
    Ok(GpuCompositorProbe {
        adapter_name: info.adapter_name,
        adapter_device_type: device_type_name(info.adapter_device_type),
        backend: backend_name(info.backend),
        texture_format: texture_format_name(COMPOSITOR_TEXTURE_FORMAT),
        max_texture_dimension_2d: info.max_texture_dimension_2d,
        max_storage_textures_per_shader_stage: info.max_storage_textures_per_shader_stage,
        software_adapter_reason,
        servo_gpu_import_backend_compatible,
        servo_gpu_import_backend_reason,
        linux_servo_gpu_import_backend_compatible,
        linux_servo_gpu_import_backend_reason,
    })
}

fn servo_import_backend_preference() -> GpuBackendPreference {
    #[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
    {
        if matches!(
            hypercolor_core::effect::servo_gpu_import_mode(),
            hypercolor_types::config::ServoGpuImportMode::On
        ) {
            return GpuBackendPreference::VulkanRequiredForServoImport;
        }
    }

    GpuBackendPreference::Default
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

impl GpuCompositorSurfaceSet {
    fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        width: u32,
        height: u32,
    ) -> Self {
        let front = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Front");
        let back = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Back");
        let source = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Source");

        Self {
            width,
            height,
            bind_groups: GpuCompositorBindGroups::new(device, pipeline, &front, &back, &source),
            front,
            back,
            source,
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

fn compose_layer_into_gpu(
    device: &wgpu::Device,
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
            CompositionMode::Multiply => ComposeShaderMode::Multiply,
            CompositionMode::Overlay => ComposeShaderMode::Overlay,
            CompositionMode::SoftLight => ComposeShaderMode::SoftLight,
            CompositionMode::ColorDodge => ComposeShaderMode::ColorDodge,
            CompositionMode::Difference => ComposeShaderMode::Difference,
            CompositionMode::Tint => ComposeShaderMode::Tint,
            CompositionMode::LumaReveal => ComposeShaderMode::LumaReveal,
        }
    };
    let output_surface = if use_front_as_current {
        GpuCompositorOutputSurface::Back
    } else {
        GpuCompositorOutputSurface::Front
    };

    if let Some(frame) = gpu_source_frame(&layer.frame)
        && shader_mode == ComposeShaderMode::Replace
        && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
    {
        record_gpu_source_upload_skipped();
        let output = if use_front_as_current {
            &surfaces.back
        } else {
            &surfaces.front
        };
        copy_gpu_source_frame_into_texture(device, queue, pipeline, encoder, &frame, output);
        set_texture_contents(surfaces, output_surface, None);
        return;
    }

    let gpu_frame = gpu_source_frame(&layer.frame);

    if let Some(frame) = gpu_frame.as_ref()
        && frame.needs_shader_copy()
    {
        record_gpu_source_upload_skipped();
        copy_gpu_source_frame_into_texture(
            device,
            queue,
            pipeline,
            encoder,
            frame,
            &surfaces.source,
        );
        surfaces.cached_source_upload = None;
    } else if gpu_frame.is_none() {
        upload_frame_into_source_texture(queue, surfaces, &layer.frame);
        if shader_mode == ComposeShaderMode::Replace
            && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
        {
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

    let params = encode_compose_params(surfaces.width, surfaces.height, shader_mode, layer);
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
            let source_view = if frame.needs_shader_copy() {
                &surfaces.source.view
            } else {
                frame.view()
            };
            create_compose_bind_group(
                device,
                pipeline,
                current_view,
                source_view,
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

fn gpu_composed_without_surfaces() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

fn gpu_composed_with_preview_surface(preview_surface: PublishedSurface) -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: Some(preview_surface),
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
}

fn gpu_bypassed_without_surfaces() -> ComposedFrameSet {
    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: None,
        bypassed: true,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
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
            gpu_readback_failed: false,
            compositor_acceleration_downgraded: false,
        };
    }

    ComposedFrameSet {
        sampling_canvas: None,
        sampling_surface: None,
        preview_surface: Some(sampling_surface),
        bypassed: false,
        backend: CompositorBackendKind::Gpu,
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
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
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
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
        gpu_readback_failed: false,
        compositor_acceleration_downgraded: false,
    }
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
    layer: &CompositionLayer,
) -> [u8; COMPOSE_PARAM_BYTES] {
    let mut bytes = [0u8; COMPOSE_PARAM_BYTES];
    let transform = layer.transform.unwrap_or_default();
    let adjust = layer.adjust.unwrap_or_default();
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&(mode as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(fit_mode(transform.fit) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&layer.frame.width().to_le_bytes());
    bytes[20..24].copy_from_slice(&layer.frame.height().to_le_bytes());
    let processing = if layer.needs_processing_for_size(width, height) {
        1_u32
    } else {
        0_u32
    };
    bytes[24..28].copy_from_slice(&processing.to_le_bytes());
    bytes[32..36].copy_from_slice(&layer.opacity.to_le_bytes());
    bytes[36..40].copy_from_slice(&transform.anchor.x.to_le_bytes());
    bytes[40..44].copy_from_slice(&transform.anchor.y.to_le_bytes());
    bytes[44..48].copy_from_slice(&transform.scale[0].to_le_bytes());
    bytes[48..52].copy_from_slice(&transform.scale[1].to_le_bytes());
    bytes[52..56].copy_from_slice(&transform.rotation.cos().to_le_bytes());
    bytes[56..60].copy_from_slice(&transform.rotation.sin().to_le_bytes());
    bytes[64..68].copy_from_slice(&adjust.brightness.to_le_bytes());
    bytes[68..72].copy_from_slice(&adjust.saturation.to_le_bytes());
    bytes[72..76].copy_from_slice(&adjust.hue_shift.to_le_bytes());
    let tint_strength = (adjust.tint_strength * adjust.tint[3].clamp(0.0, 1.0)).clamp(0.0, 1.0);
    bytes[76..80].copy_from_slice(&tint_strength.to_le_bytes());
    bytes[80..84].copy_from_slice(&adjust.tint[0].to_le_bytes());
    bytes[84..88].copy_from_slice(&adjust.tint[1].to_le_bytes());
    bytes[88..92].copy_from_slice(&adjust.tint[2].to_le_bytes());
    bytes[92..96].copy_from_slice(&adjust.contrast.to_le_bytes());
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeShaderMode {
    Replace = 0,
    Alpha = 1,
    Add = 2,
    Screen = 3,
    Multiply = 4,
    Overlay = 5,
    SoftLight = 6,
    ColorDodge = 7,
    Difference = 8,
    Tint = 9,
    LumaReveal = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeFitMode {
    Contain = 0,
    Cover = 1,
    Stretch = 2,
    Tile = 3,
    Mirror = 4,
}

fn fit_mode(mode: hypercolor_types::viewport::FitMode) -> ComposeFitMode {
    match mode {
        hypercolor_types::viewport::FitMode::Contain => ComposeFitMode::Contain,
        hypercolor_types::viewport::FitMode::Cover => ComposeFitMode::Cover,
        hypercolor_types::viewport::FitMode::Stretch => ComposeFitMode::Stretch,
        hypercolor_types::viewport::FitMode::Tile => ComposeFitMode::Tile,
        hypercolor_types::viewport::FitMode::Mirror => ComposeFitMode::Mirror,
    }
}

#[cfg(test)]
#[allow(clippy::manual_let_else)]
mod tests;
