use std::collections::HashMap;
use std::fmt;

use anyhow::Result;
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, Canvas, PublishedSurface};

use super::{CompositionPlan, DisplayFinalizeCacheKey, MediaTextureSourceKey};
use crate::render_thread::gpu_device::{GpuRenderDevice, texture_format_name};
use crate::render_thread::producer_queue::{GpuTextureFrame, GpuTextureFrameOrigin};
use crate::render_thread::sparkleflinger::gpu_sampling::{GpuSamplingPlan, GpuSpatialSampler};

mod compositor;
mod display_finalize;
mod frame_set;
mod media_upload;
mod pipeline;
mod preview;
mod probe;
mod readback;
mod sampler;
mod source;
mod telemetry;

use compositor::create_compose_bind_group;
#[cfg(test)]
use display_finalize::DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
pub(crate) use display_finalize::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame, PendingGpuDisplayFinalize,
};
use display_finalize::{GpuDisplayFinalizeSurfaceSet, GpuDisplaySourceTexture};
#[cfg(test)]
use media_upload::MEDIA_UPLOAD_TEXTURE_RING_LEN;
use media_upload::{
    MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES, MediaUploadTextureKey, MediaUploadTexturePool,
};
use pipeline::GpuCompositorPipeline;
use preview::{
    CachedPreviewSurface, GpuPreviewSurfaceSet, PendingPreviewMap, PendingPreviewReadback,
};
use probe::servo_import_backend_preference;
pub(crate) use probe::{GpuCompositorProbe, probe_render_device};
use readback::{CachedReadbackKey, CachedReadbackSurface};
use sampler::CachedSampleResult;
pub(crate) use sampler::{GpuZoneSamplingDispatch, PendingGpuZoneSampling};
use source::{CachedGpuSourceCopy, CachedSourceUpload, gpu_source_frame, write_rgba_texture};
use telemetry::record_gpu_media_texture_upload;
pub(crate) use telemetry::{GpuSparkleFlingerTelemetrySnapshot, record_gpu_display_finalize_latch};

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
    pending_upload_buffers: Vec<wgpu::Buffer>,
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
            self.clear_pending_upload_buffers();
        }
        Ok(())
    }

    pub(super) fn clear_pending_upload_buffers(&mut self) {
        if let Some(surfaces) = self.surfaces.as_mut() {
            surfaces.pending_upload_buffers.clear();
        }
    }

    pub(crate) fn surface_snapshot(&self) -> Option<GpuCompositorSurfaceSnapshot> {
        self.surfaces
            .as_ref()
            .map(GpuCompositorSurfaceSet::snapshot)
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
            pending_upload_buffers: Vec::new(),
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

#[cfg(test)]
#[allow(clippy::manual_let_else)]
mod tests;
