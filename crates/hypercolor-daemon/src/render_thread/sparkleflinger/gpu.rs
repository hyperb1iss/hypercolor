use std::collections::HashMap;
use std::fmt;

use anyhow::{Context, Result};
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, Canvas, PublishedSurface};
use hypercolor_types::event::ZoneColors;

use super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, DisplayFinalizeCacheKey,
    MediaTextureSourceKey, PreviewSurfaceRequest,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::gpu_device::{GpuRenderDevice, texture_format_name};
use crate::render_thread::producer_queue::{GpuTextureFrame, GpuTextureFrameOrigin, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSampleSource, GpuSamplingPlan, GpuSamplingPlanKey, GpuSpatialSampler,
    PendingGpuSampleReadback,
};

mod display_finalize;
mod frame_set;
mod media_upload;
mod pipeline;
mod preview;
mod probe;
mod readback;
mod source;
mod telemetry;

#[cfg(test)]
use display_finalize::DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
pub(crate) use display_finalize::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame, PendingGpuDisplayFinalize,
};
use display_finalize::{GpuDisplayFinalizeSurfaceSet, GpuDisplaySourceTexture};
use frame_set::{
    gpu_bypassed_canvas_frame, gpu_bypassed_surface_frame, gpu_bypassed_without_surfaces,
    gpu_composed_from_surface, gpu_composed_with_preview_surface, gpu_composed_without_surfaces,
};
#[cfg(test)]
use media_upload::MEDIA_UPLOAD_TEXTURE_RING_LEN;
use media_upload::{
    MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES, MediaUploadTextureKey, MediaUploadTexturePool,
};
use pipeline::GpuCompositorPipeline;
use preview::{
    CachedPreviewSurface, CachedPreviewSurfaceKey, GpuPreviewSurfaceSet, PendingPreviewMap,
    PendingPreviewReadback, bypass_preview_surface, preview_request_matches_plan,
};
use probe::servo_import_backend_preference;
pub(crate) use probe::{GpuCompositorProbe, probe_render_device};
use readback::{CachedReadbackKey, CachedReadbackSurface};
use source::{
    CachedGpuSourceCopy, CachedSourceUpload, cached_readback_key, cached_source_upload,
    copy_frame_into_output_texture, copy_gpu_source_frame_into_texture, gpu_source_frame,
    upload_frame_into_cached_texture, upload_frame_into_source_texture, write_rgba_texture,
};
pub(crate) use telemetry::{GpuSparkleFlingerTelemetrySnapshot, record_gpu_display_finalize_latch};
use telemetry::{record_gpu_media_texture_upload, record_gpu_source_upload_skipped};

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
