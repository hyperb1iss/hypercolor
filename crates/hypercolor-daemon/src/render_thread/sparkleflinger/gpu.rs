#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use super::MediaTextureSourceKey;
use super::{ComposedFrameSet, DisplayFinalizeCacheKey, SparkleFlingerSurfacePoolCounts};
use crate::render_thread::gpu_device::{GpuRenderDevice, texture_format_name};
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameLease, GpuTextureFrameOrigin,
};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use crate::render_thread::producer_queue::{MacosScreenTextureLease, SubmissionRetirementQueue};
use crate::render_thread::sparkleflinger::gpu_sampling::GpuSpatialSampler;
use anyhow::{Context, Result};
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use hypercolor_core::input::screen::ScreenNativeExecutionTarget;
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, PublishedSurface, SurfaceStateCounts};
use hypercolor_types::scene::ZoneId;

mod canvas;
mod compositor;
mod display_finalize;
mod frame_set;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
mod macos_screen;
mod media_upload;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
mod native_screen;
mod pipeline;
mod preview;
mod probe;
mod projected;
mod readback;
mod runtime;
mod sampler;
mod screen_upload;
mod snapshot;
mod source;
mod submission;
mod telemetry;
#[cfg(target_os = "windows")]
mod windows_screen;

#[cfg(test)]
use canvas::GpuCanvasFallbackReason;
pub(crate) use canvas::GpuCanvasPreparation;
use canvas::{GpuCanvasAdmission, gpu_canvas_admission};
#[cfg(feature = "allocation-contract-tests")]
pub(crate) use compositor::ProjectedLookupAllocationFixture as GpuProjectedLookupAllocationFixture;
use compositor::{
    ComposeSourceBindGroupCache, SamplingReadbackBuffers, SamplingReadbackLatch,
    create_compose_bind_group,
};
#[cfg(test)]
use display_finalize::DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
pub(crate) use display_finalize::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame, PendingGpuDisplayFinalize,
};
use display_finalize::{GpuDisplayFinalizeSurfaceSet, GpuDisplaySourceTexture};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use macos_screen::MacosScreenBridge;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use macos_screen::MacosScreenGpuRecoveryState;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
pub(crate) use macos_screen::{MacosScreenCopyOutcome, PreparedMacosScreenTarget};
#[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
use macos_screen::{
    prepared_macos_screen_target_exclusive_bytes, prepared_macos_screen_target_retention,
};
#[cfg(test)]
use media_upload::{MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES, MEDIA_UPLOAD_TEXTURE_RING_LEN};
use media_upload::{MediaUploadTextureKey, MediaUploadTexturePool};
use pipeline::GpuCompositorPipeline;
use preview::{
    CachedPreviewSurface, GpuPreviewSurfaceSet, PendingPreviewMap, PendingPreviewReadback,
};
pub(crate) use probe::{GpuCompositorProbe, probe_render_device};
#[allow(unused_imports)]
pub(crate) use projected::{GpuProjectedScenePreparation, GpuProjectedSceneResourceError};
use readback::{CachedReadbackKey, CachedReadbackSurface};
use sampler::CachedSampleResult;
pub(crate) use sampler::{GpuZoneSamplingDispatch, PendingGpuZoneSampling};
use screen_upload::{
    ScreenPublicationUploadPool, ScreenUploadContentKey, ScreenUploadResidencyPolicy,
};
use source::{CachedGpuSourceCopy, CachedSourceUpload, SourceCopyBindGroupCache};
use submission::FrameInFlight;
pub(crate) use telemetry::{GpuSparkleFlingerTelemetrySnapshot, record_gpu_display_finalize_latch};
#[cfg(target_os = "windows")]
use windows_screen::WindowsScreenBridge;
#[cfg(all(test, target_os = "windows"))]
use windows_screen::{
    NativeScreenCopyFailurePolicy, native_screen_copy_failure_policy,
    screen_storage_requires_cache_turnover, validate_windows_plan_generation,
};
#[cfg(target_os = "windows")]
pub(crate) use windows_screen::{
    is_retryable_native_screen_copy_error, native_screen_copy_error_invalidates_frame,
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
// A render owns the previous retained generation until the replacement frame
// is snapshotted, then retention atomically drops it. Scene GPU frames are not
// published outside that serial frame executor, so exactly two generations
// cover every in-engine lease overlap without render-path allocation.
const IMMUTABLE_SCENE_GENERATIONS_IN_FLIGHT: usize = 2;
static NEXT_GPU_TEXTURE_STORAGE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_GPU_SURFACE_SET_GENERATION: AtomicU64 = AtomicU64::new(1);
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
static NEXT_SCREEN_TARGET_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) enum GpuComposeOutcome {
    Produced(ComposedFrameSet),
    Retained(ComposedFrameSet),
    Failed(anyhow::Error),
}

pub(crate) struct GpuCanvasGeneration {
    surfaces: GpuCompositorSurfaceSet,
    preview_surfaces: Option<GpuPreviewSurfaceSet>,
    sampling_readback_buffers: Option<SamplingReadbackBuffers>,
}

fn ensure_readback_buffer_capacity(
    max_buffer_size: u64,
    width: u32,
    height: u32,
    align_rows: bool,
) -> Result<u64> {
    let row_bytes = width
        .checked_mul(BYTES_PER_PIXEL as u32)
        .context("GPU readback row byte size overflowed")?;
    let bytes_per_row = if align_rows {
        row_bytes
            .checked_add(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT - 1)
            .map(|value| {
                value / wgpu::COPY_BYTES_PER_ROW_ALIGNMENT * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
            })
            .context("GPU readback aligned row byte size overflowed")?
    } else {
        row_bytes
    };
    let required_bytes = u64::from(bytes_per_row)
        .checked_mul(u64::from(height))
        .context("GPU readback buffer byte size overflowed")?;
    anyhow::ensure!(
        required_bytes <= max_buffer_size,
        "GPU readback for {width}x{height} requires {required_bytes} bytes but the device supports {max_buffer_size} bytes per buffer"
    );
    Ok(required_bytes)
}

fn ensure_storage_buffer_capacity(
    max_storage_buffer_binding_size: u64,
    width: u32,
    height: u32,
) -> Result<u64> {
    let required_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL as u64))
        .context("GPU storage buffer byte size overflowed")?;
    anyhow::ensure!(
        required_bytes <= max_storage_buffer_binding_size,
        "GPU storage buffer for {width}x{height} requires {required_bytes} bytes but the device supports {max_storage_buffer_binding_size} bytes per storage binding"
    );
    Ok(required_bytes)
}

fn try_create_gpu_resources<T>(
    device: &wgpu::Device,
    context: &'static str,
    create: impl FnOnce() -> T,
) -> Result<T> {
    let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let resources = create();
    let validation_error = pollster::block_on(validation_scope.pop());
    let internal_error = pollster::block_on(internal_scope.pop());
    let out_of_memory_error = pollster::block_on(out_of_memory_scope.pop());
    if let Some(error) = validation_error.or(internal_error).or(out_of_memory_error) {
        anyhow::bail!("{context}: {error}");
    }
    Ok(resources)
}

fn reject_injected_gpu_preparation(fail_after_prepare: bool, resource: &'static str) -> Result<()> {
    anyhow::ensure!(
        !fail_after_prepare,
        "injected {resource} preparation failure"
    );
    Ok(())
}

pub(crate) struct GpuSparkleFlinger {
    _render_device: GpuRenderDevice,
    device: wgpu::Device,
    queue: wgpu::Queue,
    probe: GpuCompositorProbe,
    max_buffer_size: u64,
    max_storage_buffer_binding_size: u64,
    canvas_gpu_admitted: bool,
    pipeline: GpuCompositorPipeline,
    spatial_sampler: GpuSpatialSampler,
    opaque_black_texture: Option<GpuCompositorTexture>,
    surfaces: Option<GpuCompositorSurfaceSet>,
    compositor_surface_cache: HashMap<(u32, u32), Option<GpuCompositorSurfaceSet>>,
    display_finalize_surfaces: HashMap<DisplayFinalizeCacheKey, GpuDisplayFinalizeSurfaceSet>,
    display_finalize_generation: u64,
    preview_surfaces: Option<GpuPreviewSurfaceSet>,
    media_texture_pools: HashMap<MediaUploadTextureKey, MediaUploadTexturePool>,
    media_texture_epoch: u64,
    projected_group_snapshots: HashMap<ZoneId, Option<GpuProjectionSnapshot>>,
    immutable_scene_snapshots: Vec<GpuImmutableSceneSnapshot>,
    current_output: Option<GpuCompositorOutputSurface>,
    cached_composition_key: Option<CachedReadbackKey>,
    cached_readback_surface: Option<CachedReadbackSurface>,
    cached_preview_surfaces: Vec<CachedPreviewSurface>,
    frame_in_flight: Option<FrameInFlight>,
    pending_preview_map: Option<PendingPreviewMap>,
    ready_preview_surface: Option<PublishedSurface>,
    sampling_latch: SamplingReadbackLatch,
    output_generation: u64,
    producer_content_generation: u64,
    cached_sample_result: Option<CachedSampleResult>,
    #[cfg(target_os = "windows")]
    screen_bridge: Option<Arc<WindowsScreenBridge>>,
    #[cfg(target_os = "windows")]
    screen_target: Option<ScreenNativeExecutionTarget>,
    #[cfg(target_os = "windows")]
    screen_storage_id: Option<u64>,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    screen_bridge: Option<Arc<MacosScreenBridge>>,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    screen_target: Option<ScreenNativeExecutionTarget>,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    macos_screen_recovery: MacosScreenGpuRecoveryState,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    metal4_capable: bool,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    native_screen_lease_retirements:
        SubmissionRetirementQueue<wgpu::SubmissionIndex, MacosScreenTextureLease>,
    #[cfg(test)]
    superseded_frame_count: usize,
    #[cfg(test)]
    preview_surface_allocation_count: usize,
    #[cfg(test)]
    defer_preview_resolve_once: bool,
    #[cfg(test)]
    defer_preview_map_resolve_once: bool,
    #[cfg(test)]
    fail_next_sampling_readback_preparation: bool,
    #[cfg(test)]
    fail_next_preview_scale_output_preparation: bool,
    #[cfg(test)]
    fail_next_screen_upload_pool_saturation: bool,
    #[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
    fail_next_macos_screen_rebuild: bool,
    #[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
    fail_next_macos_screen_import: bool,
    #[cfg(test)]
    snapshot_texture_allocation_count: Cell<usize>,
    #[cfg(test)]
    compositor_surface_allocation_count: Cell<usize>,
    #[cfg(test)]
    projected_bind_group_creation_count: Cell<usize>,
    #[cfg(test)]
    fail_next_projected_scene_preparation: Cell<bool>,
}

pub(crate) struct GpuCompositorSurfaceSet {
    generation: u64,
    width: u32,
    height: u32,
    front: GpuCompositorTexture,
    back: GpuCompositorTexture,
    source: GpuCompositorTexture,
    bind_groups: GpuCompositorBindGroups,
    compose_source_bind_groups: ComposeSourceBindGroupCache,
    source_copy_bind_groups: SourceCopyBindGroupCache,
    cached_compose_params: Option<[u8; COMPOSE_PARAM_BYTES]>,
    cached_compose_params_offset: Option<u32>,
    pending_upload_buffers: PendingUploadBuffers,
    screen_upload_pool: ScreenPublicationUploadPool,
    uploaded_screen_frame_scratch: Vec<Option<GpuTextureFrame>>,
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
    screen_layer_host_allocation_count: usize,
}

/// One-shot staging buffers that must stay alive until the encoder that
/// references them is submitted.
#[derive(Default)]
struct PendingUploadBuffers {
    buffers: Vec<wgpu::Buffer>,
    #[cfg(test)]
    creation_count: usize,
}

impl PendingUploadBuffers {
    fn push(&mut self, buffer: wgpu::Buffer) {
        #[cfg(test)]
        {
            self.creation_count = self.creation_count.saturating_add(1);
        }
        self.buffers.push(buffer);
    }

    fn clear(&mut self) {
        self.buffers.clear();
    }
}

pub(crate) struct GpuCompositorTexture {
    storage_id: u64,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub(crate) struct GpuProjectionSnapshot {
    width: u32,
    height: u32,
    texture: GpuCompositorTexture,
    content_generation: u64,
    lease: Arc<GpuTextureFrameLease>,
}

pub(crate) struct GpuImmutableSceneSnapshot {
    width: u32,
    height: u32,
    texture: GpuCompositorTexture,
    content_generation: u64,
    lease: Arc<GpuTextureFrameLease>,
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
    pub(crate) fn surface_pool_counts(&mut self) -> SparkleFlingerSurfacePoolCounts {
        let preview = self.preview_surfaces.as_mut().map_or_else(
            SurfaceStateCounts::default,
            GpuPreviewSurfaceSet::surface_pool_counts,
        );
        let mut compositor = self.sampling_latch.surface_pool_counts();
        for surfaces in self.display_finalize_surfaces.values_mut() {
            compositor = merge_surface_state_counts(compositor, surfaces.surface_pool_counts());
        }
        SparkleFlingerSurfacePoolCounts {
            preview,
            compositor,
        }
    }
}

fn merge_surface_state_counts(
    mut total: SurfaceStateCounts,
    counts: SurfaceStateCounts,
) -> SurfaceStateCounts {
    total.free = total.free.saturating_add(counts.free);
    total.dequeued = total.dequeued.saturating_add(counts.dequeued);
    total.published = total.published.saturating_add(counts.published);
    total
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

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
impl Drop for GpuSparkleFlinger {
    fn drop(&mut self) {
        self.wait_for_native_screen_lease_retirements();
    }
}

impl GpuCompositorSurfaceSet {
    fn finish_pending_uploads(&mut self, submission_index: wgpu::SubmissionIndex) {
        self.pending_upload_buffers.clear();
        self.screen_upload_pool.mark_submitted(submission_index);
    }

    fn discard_pending_uploads(&mut self) {
        self.pending_upload_buffers.clear();
        self.screen_upload_pool.discard_encoding();
    }

    fn try_new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let generation = NEXT_GPU_SURFACE_SET_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("GPU compositor surface identity space is exhausted"))?;
        let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let front = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Front");
        let back = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Back");
        let source = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Source");

        let surfaces = Self {
            generation,
            width,
            height,
            bind_groups: GpuCompositorBindGroups::new(device, pipeline, &front, &back, &source),
            compose_source_bind_groups: ComposeSourceBindGroupCache::default(),
            source_copy_bind_groups: SourceCopyBindGroupCache::default(),
            front,
            back,
            source,
            cached_compose_params: None,
            cached_compose_params_offset: None,
            pending_upload_buffers: PendingUploadBuffers::default(),
            screen_upload_pool: ScreenPublicationUploadPool::new(
                ScreenUploadResidencyPolicy::compositor_pipeline(),
            ),
            uploaded_screen_frame_scratch: Vec::new(),
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
            screen_layer_host_allocation_count: 0,
        };
        let validation_error = pollster::block_on(validation_scope.pop());
        let internal_error = pollster::block_on(internal_scope.pop());
        let out_of_memory_error = pollster::block_on(out_of_memory_scope.pop());
        if let Some(error) = validation_error.or(internal_error).or(out_of_memory_error) {
            anyhow::bail!("GPU compositor surface allocation failed: {error}");
        }
        Ok(surfaces)
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
    fn try_new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        label: &'static str,
    ) -> Result<Self> {
        let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = Self::new(device, width, height, label);
        let validation_error = pollster::block_on(validation_scope.pop());
        let internal_error = pollster::block_on(internal_scope.pop());
        let out_of_memory_error = pollster::block_on(out_of_memory_scope.pop());
        if let Some(error) = validation_error.or(internal_error).or(out_of_memory_error) {
            anyhow::bail!("GPU texture allocation failed for {label}: {error}");
        }
        Ok(texture)
    }

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
        Self {
            storage_id: NEXT_GPU_TEXTURE_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            texture,
            view,
        }
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
