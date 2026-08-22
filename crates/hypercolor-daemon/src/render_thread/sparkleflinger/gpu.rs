#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use hypercolor_core::input::screen::ScreenNativeExecutionTarget;
#[cfg(target_os = "windows")]
use hypercolor_core::input::screen::{ScreenBranchPayload, ScreenBranchPublication};
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, Canvas, PublishedSurface, SurfaceStateCounts,
};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use hypercolor_macos_gpu_interop::probe_macos_metal4_capabilities;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::ZoneId;
#[cfg(target_os = "windows")]
use hypercolor_windows_capture::GpuSurfacePublication;

use super::{
    ComposedFrameSet, CompositionPlan, DisplayFinalizeCacheKey, MediaTextureSourceKey,
    ProjectedGroupTextureRequirement, SparkleFlingerSurfacePoolCounts,
};
use crate::render_thread::gpu_device::{
    GpuBackendPreference, GpuRenderDevice, texture_format_name,
};
#[cfg(target_os = "windows")]
use crate::render_thread::producer_queue::WindowsScreenTextureLease;
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameLease, GpuTextureFrameOrigin, ProducerFrame,
};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use crate::render_thread::producer_queue::{MacosScreenTextureLease, SubmissionRetirementQueue};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use crate::render_thread::sparkleflinger::gpu_sampling::GpuSampleSource;
use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSamplingPlan, GpuSamplingPreparation, GpuSpatialSampler,
};

mod canvas;
mod compositor;
mod display_finalize;
mod frame_set;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
mod macos_screen;
mod media_upload;
mod pipeline;
mod preview;
mod probe;
mod readback;
mod sampler;
mod screen_upload;
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
    ComposeSourceBindGroupCache, PreparedProjectedComposeBindGroups, SamplingReadbackBuffers,
    SamplingReadbackLatch, create_compose_bind_group,
};
#[cfg(test)]
use display_finalize::DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
pub(crate) use display_finalize::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame, PendingGpuDisplayFinalize,
};
use display_finalize::{GpuDisplayFinalizeSurfaceSet, GpuDisplaySourceTexture};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use macos_screen::MacosScreenGpuRecoveryState;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use macos_screen::{MacosScreenBridge, create_screen_bridge};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
pub(crate) use macos_screen::{MacosScreenCopyOutcome, PreparedMacosScreenTarget};
#[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
use macos_screen::{
    prepared_macos_screen_target_exclusive_bytes, prepared_macos_screen_target_retention,
};
#[cfg(test)]
use media_upload::MEDIA_UPLOAD_TEXTURE_RING_LEN;
use media_upload::{
    MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES, MediaUploadTextureKey, MediaUploadTexturePool,
};
use pipeline::GpuCompositorPipeline;
use preview::{
    CachedPreviewSurface, GpuPreviewSurfaceSet, PendingPreviewMap, PendingPreviewReadback,
};
pub(crate) use probe::{GpuCompositorProbe, probe_render_device};
use readback::{CachedReadbackKey, CachedReadbackSurface};
use sampler::CachedSampleResult;
pub(crate) use sampler::{GpuZoneSamplingDispatch, PendingGpuZoneSampling};
use screen_upload::{
    ScreenPublicationUploadPool, ScreenUploadContentKey, ScreenUploadResidencyPolicy,
};
use source::{
    CachedGpuSourceCopy, CachedSourceUpload, SourceCopyBindGroupCache, gpu_source_frame,
    write_rgba_texture,
};
use submission::{FrameInFlight, StashedFrame};
use telemetry::record_gpu_media_texture_upload;
pub(crate) use telemetry::{GpuSparkleFlingerTelemetrySnapshot, record_gpu_display_finalize_latch};
#[cfg(target_os = "windows")]
use windows_screen::{
    NativeScreenCopyFailurePolicy, PreparedWindowsScreenTarget, WindowsScreenBridge,
    create_screen_bridge, create_screen_target, native_screen_copy_failure_policy,
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

pub(crate) enum GpuProjectedScenePreparation {
    Disabled {
        projected_bind_groups: Option<PreparedProjectedComposeBindGroups>,
    },
    Admitted {
        snapshots: HashMap<ZoneId, Option<GpuProjectionSnapshot>>,
        compositor_surfaces: HashMap<(u32, u32), Option<GpuCompositorSurfaceSet>>,
        projected_bind_groups: PreparedProjectedComposeBindGroups,
        scene_extent: (u32, u32),
    },
    ResourceFallback {
        error: GpuProjectedSceneResourceError,
        projected_bind_groups: Option<PreparedProjectedComposeBindGroups>,
    },
}

impl GpuProjectedScenePreparation {
    pub(super) const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted { .. })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GpuProjectedSceneResourceError {
    #[error("GPU projection snapshot metadata allocation failed")]
    Metadata(#[source] std::collections::TryReserveError),
    #[error("GPU projection snapshot allocation failed for {width}x{height}")]
    Snapshot {
        width: u32,
        height: u32,
        #[source]
        source: anyhow::Error,
    },
    #[error("GPU compositor surface metadata allocation failed")]
    CompositorMetadata(#[source] std::collections::TryReserveError),
    #[error("GPU compositor surface allocation failed for {width}x{height}")]
    CompositorSurface {
        width: u32,
        height: u32,
        #[source]
        source: anyhow::Error,
    },
    #[error("GPU projected bind-group metadata allocation failed")]
    BindGroupMetadata(#[source] std::collections::TryReserveError),
    #[error("GPU projected compositor surface {width}x{height} was not admitted")]
    MissingCompositorSurface { width: u32, height: u32 },
    #[cfg(test)]
    #[error("GPU projection snapshot allocation failure injected by test")]
    Injected,
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

    pub(crate) fn new() -> Result<Self> {
        Self::new_with_backend_preference(GpuBackendPreference::Default)
    }

    pub(crate) fn new_with_backend_preference(
        backend_preference: GpuBackendPreference,
    ) -> Result<Self> {
        Self::with_render_device(GpuRenderDevice::new_with_backend_preference(
            "SparkleFlinger GPU compositor",
            backend_preference,
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
        let max_buffer_size = device.limits().max_buffer_size;
        let max_storage_buffer_binding_size = device.limits().max_storage_buffer_binding_size;

        let pipeline = GpuCompositorPipeline::new(&device);
        let spatial_sampler = GpuSpatialSampler::new(&device);
        #[cfg(target_os = "windows")]
        let (screen_bridge, screen_target) =
            create_screen_bridge(&device, &queue, probe.max_texture_dimension_2d);
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        let (screen_bridge, screen_target, macos_screen_recovery) = match create_screen_bridge(
            &device,
            probe.max_texture_dimension_2d,
        ) {
            Ok((bridge, target)) => {
                let recovery = MacosScreenGpuRecoveryState::ready(target.id());
                (Some(bridge), Some(target), recovery)
            }
            Err(error) => {
                tracing::debug!(%error, "renderer does not expose native Metal screen execution");
                (None, None, MacosScreenGpuRecoveryState::unavailable(&error))
            }
        };
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        let metal4_capable = probe_macos_metal4_capabilities(&device)?.all_required_facilities();

        Ok(Self {
            _render_device: render_device,
            device,
            queue,
            probe,
            max_buffer_size,
            max_storage_buffer_binding_size,
            canvas_gpu_admitted: true,
            pipeline,
            spatial_sampler,
            opaque_black_texture: None,
            surfaces: None,
            compositor_surface_cache: HashMap::new(),
            display_finalize_surfaces: HashMap::new(),
            display_finalize_generation: 0,
            preview_surfaces: None,
            media_texture_pools: HashMap::new(),
            media_texture_epoch: 0,
            projected_group_snapshots: HashMap::new(),
            immutable_scene_snapshots: Vec::new(),
            current_output: None,
            cached_composition_key: None,
            cached_readback_surface: None,
            cached_preview_surfaces: Vec::with_capacity(MAX_CACHED_PREVIEW_SURFACES),
            frame_in_flight: None,
            pending_preview_map: None,
            ready_preview_surface: None,
            sampling_latch: SamplingReadbackLatch::default(),
            output_generation: 0,
            producer_content_generation: 0,
            cached_sample_result: None,
            #[cfg(target_os = "windows")]
            screen_bridge,
            #[cfg(target_os = "windows")]
            screen_target,
            #[cfg(target_os = "windows")]
            screen_storage_id: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            screen_bridge,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            screen_target,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_recovery,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            metal4_capable,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_lease_retirements: SubmissionRetirementQueue::default(),
            #[cfg(test)]
            superseded_frame_count: 0,
            #[cfg(test)]
            preview_surface_allocation_count: 0,
            #[cfg(test)]
            defer_preview_resolve_once: false,
            #[cfg(test)]
            defer_preview_map_resolve_once: false,
            #[cfg(test)]
            fail_next_sampling_readback_preparation: false,
            #[cfg(test)]
            fail_next_preview_scale_output_preparation: false,
            #[cfg(test)]
            fail_next_screen_upload_pool_saturation: false,
            #[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
            fail_next_macos_screen_rebuild: false,
            #[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
            fail_next_macos_screen_import: false,
            #[cfg(test)]
            snapshot_texture_allocation_count: Cell::new(0),
            #[cfg(test)]
            compositor_surface_allocation_count: Cell::new(0),
            #[cfg(test)]
            projected_bind_group_creation_count: Cell::new(0),
            #[cfg(test)]
            fail_next_projected_scene_preparation: Cell::new(false),
        })
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(crate) const fn macos_metal4_capability(&self) -> bool {
        self.metal4_capable
    }

    fn take_sampling_readback_failure_injection(&mut self) -> bool {
        #[cfg(test)]
        {
            std::mem::take(&mut self.fail_next_sampling_readback_preparation)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    fn take_preview_scale_output_failure_injection(&mut self) -> bool {
        #[cfg(test)]
        {
            std::mem::take(&mut self.fail_next_preview_scale_output_preparation)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    #[cfg(test)]
    fn take_screen_upload_pool_saturation_injection(&mut self) -> bool {
        std::mem::take(&mut self.fail_next_screen_upload_pool_saturation)
    }

    #[cfg(test)]
    pub(super) fn fail_next_sampling_readback_preparation(&mut self) {
        self.fail_next_sampling_readback_preparation = true;
    }

    #[cfg(test)]
    pub(super) fn fail_next_preview_scale_output_preparation(&mut self) {
        self.fail_next_preview_scale_output_preparation = true;
    }

    #[cfg(test)]
    pub(super) fn fail_next_screen_upload_pool_saturation(&mut self) {
        self.fail_next_screen_upload_pool_saturation = true;
    }

    pub(crate) fn supports_plan(&self, plan: &CompositionPlan) -> bool {
        self.canvas_gpu_admitted
            && matches!(
                gpu_canvas_admission(self.probe.max_texture_dimension_2d, plan.width, plan.height,),
                GpuCanvasAdmission::Gpu
            )
            && !plan.layers.is_empty()
            && plan.layers.iter().all(|layer| {
                gpu_source_frame(&layer.frame).is_some()
                    || layer.frame_matches_size(plan.width, plan.height)
            })
            && !self.plan_samples_compositor_storage(plan)
    }

    fn plan_samples_compositor_storage(&self, plan: &CompositionPlan) -> bool {
        plan.layers.iter().any(|layer| {
            let ProducerFrame::GpuTexture(frame) = &layer.frame else {
                return false;
            };
            if plan.layers.len() == 1
                && self.layer_reuses_current_output_texture(layer, plan.width, plan.height)
            {
                return false;
            }
            self.surfaces
                .iter()
                .chain(
                    self.compositor_surface_cache
                        .values()
                        .filter_map(Option::as_ref),
                )
                .any(|surfaces| {
                    frame.storage_id == surfaces.front.storage_id
                        || frame.storage_id == surfaces.back.storage_id
                        || frame.storage_id == surfaces.source.storage_id
                })
        })
    }

    pub(super) const fn canvas_gpu_admitted(&self) -> bool {
        self.canvas_gpu_admitted
    }

    #[cfg(test)]
    pub(super) const fn max_texture_dimension_2d(&self) -> u32 {
        self.probe.max_texture_dimension_2d
    }

    #[cfg(test)]
    pub(super) fn backend_name(&self) -> &str {
        &self.probe.backend
    }

    #[cfg(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    ))]
    pub(crate) fn screen_native_execution_target(
        &mut self,
    ) -> Option<&ScreenNativeExecutionTarget> {
        if !self.canvas_gpu_admitted {
            return None;
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        self.retry_macos_screen_execution();
        self.screen_target.as_ref()
    }

    #[cfg(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    ))]
    pub(crate) fn release_native_screen_caches(&mut self) {
        if let Some(surfaces) = &mut self.surfaces {
            surfaces
                .compose_source_bind_groups
                .release_native_screen_entries();
            surfaces
                .source_copy_bind_groups
                .release_native_screen_entries();
        }
        for surfaces in self.compositor_surface_cache.values_mut().flatten() {
            surfaces
                .compose_source_bind_groups
                .release_native_screen_entries();
            surfaces
                .source_copy_bind_groups
                .release_native_screen_entries();
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        {
            if let Some(bridge) = &self.screen_bridge {
                bridge.clear_capture_caches();
            }
        }
        #[cfg(target_os = "windows")]
        {
            self.screen_storage_id = None;
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        self.release_completed_native_screen_leases();
    }

    #[cfg(target_os = "windows")]
    fn reprepare_native_screen_target(&mut self) {
        self.release_native_screen_caches();
        self.screen_target = self
            .screen_bridge
            .as_ref()
            .and_then(|bridge| create_screen_target(bridge, self.probe.max_texture_dimension_2d));
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn copy_screen_publication(
        &mut self,
        publication: &Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        let Some(bridge) = self.screen_bridge.clone() else {
            return Ok(None);
        };
        let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
            return Ok(None);
        };
        let Some(native) = payload.surface().owner::<GpuSurfacePublication>() else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has an unknown platform owner");
        };
        let Some(prepared) = payload
            .surface()
            .retained_owner::<PreparedWindowsScreenTarget>()
        else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has no prepared renderer target");
        };
        let Some(target_lifetime) = payload.surface().resource_lifetime().cloned() else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has no renderer allocation lifetime");
        };
        let Some(capture_lifetime) = payload.surface().capture_resource_lifetime().cloned() else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has no capture allocation lifetime");
        };
        let copy = match bridge.interop.copy_publication(&prepared.interop, &native) {
            Ok(copy) => copy,
            Err(error) => {
                return match native_screen_copy_failure_policy(&error) {
                    NativeScreenCopyFailurePolicy::Retain => {
                        Err(error).context("native screen publication is not ready")
                    }
                    NativeScreenCopyFailurePolicy::Reprepare => {
                        self.reprepare_native_screen_target();
                        Err(error).context("failed to copy the native screen publication")
                    }
                    NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare => {
                        self.reprepare_native_screen_target();
                        Err(error).context("native screen target contents became uncertain")
                    }
                };
            }
        };
        if screen_storage_requires_cache_turnover(self.screen_storage_id, prepared.storage_id) {
            self.release_native_screen_caches();
            self.screen_storage_id = Some(prepared.storage_id);
        }
        let width = copy.width;
        let height = copy.height;
        let content_generation = copy.content_generation;
        let texture = copy.texture.as_ref().clone();
        let view = copy.view.as_ref().clone();
        Ok(Some(GpuTextureFrame {
            width,
            height,
            storage_id: prepared.storage_id,
            content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture,
            view,
            immutable_lease: None,
            windows_screen_lease: Some(WindowsScreenTextureLease::new(
                copy,
                target_lifetime,
                capture_lifetime,
            )),
        }))
    }

    pub(crate) fn can_sample_zone_plan(&mut self, prepared_zones: &[PreparedZonePlan]) -> bool {
        let dimensions = self
            .surfaces
            .as_ref()
            .map(|surfaces| (surfaces.width, surfaces.height))
            .or_else(|| {
                prepared_zones
                    .first()
                    .map(|zone| (zone.prepared_canvas_width, zone.prepared_canvas_height))
            });
        let Some((width, height)) = dimensions else {
            return GpuSamplingPlan::supports_prepared_zones(prepared_zones);
        };
        self.spatial_sampler
            .can_sample_plan(&self.device, width, height, prepared_zones)
    }

    pub(super) fn prepare_zone_sampling_plan(
        &mut self,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
    ) -> GpuSamplingPreparation {
        self.spatial_sampler
            .prepare_plan(&self.device, width, height, prepared_zones)
    }

    pub(super) fn apply_zone_sampling_plan(&mut self, preparation: GpuSamplingPreparation) {
        self.spatial_sampler.apply_preparation(preparation);
        self.cached_sample_result = None;
    }

    #[cfg(test)]
    pub(super) fn fail_next_sampling_preparation(&mut self) {
        self.spatial_sampler.fail_next_plan_preparation();
    }

    pub(crate) fn current_output_frame(&mut self) -> Result<Option<GpuTextureFrame>> {
        self.flush_pending_output_submission()?;
        let Some(surfaces) = self.surfaces.as_ref() else {
            return Ok(None);
        };
        let Some(texture) = self.current_output.map(|output| match output {
            GpuCompositorOutputSurface::Front => &surfaces.front,
            GpuCompositorOutputSurface::Back => &surfaces.back,
        }) else {
            return Ok(None);
        };
        Ok(Some(GpuTextureFrame {
            width: surfaces.width,
            height: surfaces.height,
            storage_id: texture.storage_id,
            content_generation: self.output_generation,
            origin: GpuTextureFrameOrigin::CompositorOutput,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            immutable_lease: None,
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        }))
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(crate) fn sample_texture_zone_plan(
        &mut self,
        frame: &GpuTextureFrame,
        prepared_zones: &[PreparedZonePlan],
    ) -> Result<Option<Vec<ZoneColors>>> {
        self.spatial_sampler.clear_bind_groups();
        let result = (|| {
            let mut zones = Vec::new();
            let dispatch = self.spatial_sampler.sample_texture_into(
                &self.device,
                &self.queue,
                GpuSampleSource::Diagnostic,
                &frame.view,
                frame.width,
                frame.height,
                prepared_zones,
                &mut zones,
                None,
            )?;
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            if let Some(submission_index) = dispatch.submission_index.clone() {
                self.retire_native_screen_leases(
                    submission_index,
                    frame.macos_screen_lease.clone().into_iter().collect(),
                );
            }
            if dispatch.queue_saturated || !dispatch.sampled {
                if let Some(pending) = dispatch.pending_readback {
                    self.spatial_sampler.discard_pending_readback(pending);
                }
                return Ok(None);
            }
            if let Some(pending) = dispatch.pending_readback {
                self.spatial_sampler
                    .finish_pending_readback(&self.device, pending, &mut zones)?;
            }
            Ok(Some(zones))
        })();
        self.spatial_sampler.clear_bind_groups();
        result
    }

    fn prepare_empty_projected_bind_groups(
        &self,
        canvas_preparation: Option<&GpuCanvasPreparation>,
    ) -> Option<PreparedProjectedComposeBindGroups> {
        let surfaces = match canvas_preparation {
            Some(preparation) => preparation.compositor_surfaces(),
            None => self.surfaces.as_ref(),
        };
        surfaces.map(|surfaces| PreparedProjectedComposeBindGroups::empty(surfaces.generation))
    }

    fn projected_scene_resource_fallback(
        &self,
        error: GpuProjectedSceneResourceError,
        canvas_preparation: Option<&GpuCanvasPreparation>,
    ) -> GpuProjectedScenePreparation {
        GpuProjectedScenePreparation::ResourceFallback {
            error,
            projected_bind_groups: self.prepare_empty_projected_bind_groups(canvas_preparation),
        }
    }

    pub(crate) fn prepare_projected_scene_resources(
        &self,
        requirements: &[ProjectedGroupTextureRequirement],
        gpu_projection_admitted: bool,
        scene_width: u32,
        scene_height: u32,
        canvas_preparation: Option<&GpuCanvasPreparation>,
    ) -> GpuProjectedScenePreparation {
        if !gpu_projection_admitted || requirements.is_empty() {
            return GpuProjectedScenePreparation::Disabled {
                projected_bind_groups: self.prepare_empty_projected_bind_groups(canvas_preparation),
            };
        }
        #[cfg(test)]
        if self.fail_next_projected_scene_preparation.replace(false) {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::Injected,
                canvas_preparation,
            );
        }
        let mut snapshots = HashMap::new();
        if let Err(error) = snapshots.try_reserve(requirements.len()) {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::Metadata(error),
                canvas_preparation,
            );
        }
        for requirement in requirements {
            let reusable = self
                .projected_group_snapshots
                .get(&requirement.group_id)
                .and_then(Option::as_ref)
                .is_some_and(|snapshot| {
                    snapshot.width == requirement.width && snapshot.height == requirement.height
                });
            let snapshot = if reusable {
                None
            } else {
                let allocation = GpuProjectionSnapshot::try_new(
                    &self.device,
                    requirement.width,
                    requirement.height,
                )
                .inspect(|_| {
                    #[cfg(test)]
                    self.snapshot_texture_allocation_count.set(
                        self.snapshot_texture_allocation_count
                            .get()
                            .saturating_add(1),
                    );
                });
                let snapshot = match allocation {
                    Ok(snapshot) => snapshot,
                    Err(source) => {
                        return self.projected_scene_resource_fallback(
                            GpuProjectedSceneResourceError::Snapshot {
                                width: requirement.width,
                                height: requirement.height,
                                source,
                            },
                            canvas_preparation,
                        );
                    }
                };
                Some(snapshot)
            };
            snapshots.insert(requirement.group_id, snapshot);
        }
        let mut compositor_surfaces = HashMap::new();
        if let Err(error) = compositor_surfaces.try_reserve(requirements.len().saturating_add(1)) {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::CompositorMetadata(error),
                canvas_preparation,
            );
        }
        compositor_surfaces.insert((scene_width, scene_height), None);
        for requirement in requirements {
            compositor_surfaces
                .entry((requirement.width, requirement.height))
                .or_insert(None);
        }
        for (&(width, height), surface) in &mut compositor_surfaces {
            let supplied_by_resize = canvas_preparation
                .and_then(GpuCanvasPreparation::compositor_surfaces)
                .is_some_and(|surfaces| (surfaces.width, surfaces.height) == (width, height));
            let active_reusable = canvas_preparation
                .and_then(GpuCanvasPreparation::compositor_surfaces)
                .is_none()
                && self
                    .surfaces
                    .as_ref()
                    .is_some_and(|current| current.width == width && current.height == height);
            let cached_reusable = self
                .compositor_surface_cache
                .get(&(width, height))
                .is_some_and(Option::is_some);
            if supplied_by_resize || active_reusable || cached_reusable {
                continue;
            }
            let replacement = match self.try_create_compositor_surface_set(width, height) {
                Ok(replacement) => replacement,
                Err(source) => {
                    return self.projected_scene_resource_fallback(
                        GpuProjectedSceneResourceError::CompositorSurface {
                            width,
                            height,
                            source,
                        },
                        canvas_preparation,
                    );
                }
            };
            *surface = Some(replacement);
        }
        let scene_extent = (scene_width, scene_height);
        let scene_surface = compositor_surfaces
            .get(&scene_extent)
            .and_then(Option::as_ref)
            .or_else(|| {
                canvas_preparation
                    .and_then(GpuCanvasPreparation::compositor_surfaces)
                    .filter(|surface| (surface.width, surface.height) == scene_extent)
            })
            .or_else(|| {
                self.surfaces
                    .as_ref()
                    .filter(|surface| (surface.width, surface.height) == scene_extent)
            })
            .or_else(|| {
                self.compositor_surface_cache
                    .get(&scene_extent)
                    .and_then(Option::as_ref)
            });
        let Some(scene_surface) = scene_surface else {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::MissingCompositorSurface {
                    width: scene_width,
                    height: scene_height,
                },
                canvas_preparation,
            );
        };
        let sources = requirements.iter().map(|requirement| {
            let snapshot = snapshots
                .get(&requirement.group_id)
                .and_then(Option::as_ref)
                .or_else(|| {
                    self.projected_group_snapshots
                        .get(&requirement.group_id)
                        .and_then(Option::as_ref)
                })
                .expect("projected source snapshot must be admitted before bind groups");
            (
                snapshot.texture.storage_id,
                snapshot.texture.view.clone(),
                Arc::downgrade(&snapshot.lease),
            )
        });
        let (projected_bind_groups, created_bind_groups) =
            match scene_surface.compose_source_bind_groups.prepare_projected(
                &self.device,
                &self.pipeline,
                scene_surface.generation,
                &scene_surface.front.view,
                &scene_surface.back.view,
                requirements.len(),
                sources,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return self.projected_scene_resource_fallback(
                        GpuProjectedSceneResourceError::BindGroupMetadata(error),
                        canvas_preparation,
                    );
                }
            };
        #[cfg(test)]
        self.projected_bind_group_creation_count.set(
            self.projected_bind_group_creation_count
                .get()
                .saturating_add(created_bind_groups),
        );
        #[cfg(not(test))]
        let _ = created_bind_groups;
        GpuProjectedScenePreparation::Admitted {
            snapshots,
            compositor_surfaces,
            projected_bind_groups,
            scene_extent,
        }
    }

    pub(crate) fn apply_projected_scene_resources(
        &mut self,
        preparation: GpuProjectedScenePreparation,
    ) {
        let clear_non_admitted =
            |gpu: &mut Self, projected_bind_groups: Option<PreparedProjectedComposeBindGroups>| {
                if let Some(surfaces) = &mut gpu.surfaces {
                    if let Some(projected_bind_groups) = projected_bind_groups {
                        surfaces
                            .compose_source_bind_groups
                            .install_projected(projected_bind_groups, surfaces.generation);
                    } else {
                        debug_assert!(false, "active surfaces require a prepared empty bind map");
                        surfaces.compose_source_bind_groups.clear_projected();
                    }
                } else {
                    debug_assert!(projected_bind_groups.is_none());
                }
                gpu.projected_group_snapshots.clear();
                gpu.compositor_surface_cache.clear();
            };
        let GpuProjectedScenePreparation::Admitted {
            mut snapshots,
            mut compositor_surfaces,
            projected_bind_groups,
            scene_extent,
        } = preparation
        else {
            match preparation {
                GpuProjectedScenePreparation::Disabled {
                    projected_bind_groups,
                } => clear_non_admitted(self, projected_bind_groups),
                GpuProjectedScenePreparation::ResourceFallback {
                    error,
                    projected_bind_groups,
                } => {
                    clear_non_admitted(self, projected_bind_groups);
                    tracing::warn!(
                        %error,
                        "using CPU scene projection after GPU snapshot admission failed"
                    );
                }
                GpuProjectedScenePreparation::Admitted { .. } => unreachable!(),
            }
            return;
        };
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("projected scene resources committed"));
        self.discard_pending_uploads();
        let mut installed_surfaces = std::mem::take(&mut self.compositor_surface_cache);
        let mut active_surface = self.surfaces.take();
        for (&extent, surface) in &mut compositor_surfaces {
            if surface.is_some() {
                continue;
            }
            if active_surface
                .as_ref()
                .is_some_and(|active| (active.width, active.height) == extent)
            {
                *surface = active_surface.take();
            } else {
                *surface = installed_surfaces.remove(&extent).flatten();
            }
            debug_assert!(surface.is_some());
        }
        let mut scene_surface = compositor_surfaces
            .remove(&scene_extent)
            .flatten()
            .or(active_surface);
        debug_assert!(
            scene_surface
                .as_ref()
                .is_some_and(|surfaces| { (surfaces.width, surfaces.height) == scene_extent })
        );
        if let Some(surface) = &mut scene_surface {
            surface
                .compose_source_bind_groups
                .install_projected(projected_bind_groups, surface.generation);
        }
        self.surfaces = scene_surface;
        self.compositor_surface_cache = compositor_surfaces;
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.preview_surfaces = None;
        self.cached_preview_surfaces.clear();
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
        let mut installed = std::mem::take(&mut self.projected_group_snapshots);
        for (group_id, snapshot) in &mut snapshots {
            if snapshot.is_none() {
                *snapshot = installed.remove(group_id).flatten();
            }
            debug_assert!(snapshot.is_some());
        }
        self.projected_group_snapshots = snapshots;
    }

    pub(crate) fn has_projected_group_resource(
        &self,
        group_id: ZoneId,
        width: u32,
        height: u32,
    ) -> bool {
        self.projected_group_snapshots
            .get(&group_id)
            .and_then(Option::as_ref)
            .is_some_and(|snapshot| snapshot.width == width && snapshot.height == height)
    }

    #[cfg(test)]
    pub(crate) fn snapshot_texture_allocation_count(&self) -> usize {
        self.snapshot_texture_allocation_count.get()
    }

    #[cfg(test)]
    pub(crate) fn compositor_surface_allocation_count(&self) -> usize {
        self.compositor_surface_allocation_count.get()
    }

    #[cfg(test)]
    pub(crate) fn projected_bind_group_creation_count(&self) -> usize {
        self.projected_bind_group_creation_count.get()
    }

    #[cfg(test)]
    pub(crate) fn projected_bind_group_entry_count(&self) -> usize {
        self.surfaces.as_ref().map_or(0, |surfaces| {
            surfaces.compose_source_bind_groups.projected_entry_count()
        })
    }

    #[cfg(test)]
    pub(crate) fn projected_bind_group_source_storage_ids(&self) -> Vec<u64> {
        self.surfaces.as_ref().map_or_else(Vec::new, |surfaces| {
            surfaces
                .compose_source_bind_groups
                .projected_source_storage_ids()
        })
    }

    #[cfg(test)]
    pub(crate) fn retired_projected_bind_group_entry_count(&self) -> usize {
        self.surfaces.as_ref().map_or(0, |surfaces| {
            surfaces
                .compose_source_bind_groups
                .retired_projected_entry_count()
        })
    }

    #[cfg(test)]
    pub(crate) fn projected_snapshot_retained_bytes(&self) -> u64 {
        self.projected_group_snapshots
            .values()
            .filter_map(Option::as_ref)
            .fold(0_u64, |total, snapshot| {
                total.saturating_add(
                    u64::from(snapshot.width)
                        .saturating_mul(u64::from(snapshot.height))
                        .saturating_mul(BYTES_PER_PIXEL as u64),
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn compositor_surface_cache_entry_count(&self) -> usize {
        self.compositor_surface_cache.len()
    }

    #[cfg(test)]
    pub(crate) fn screen_layer_host_allocation_count(&self) -> usize {
        self.surfaces
            .iter()
            .chain(
                self.compositor_surface_cache
                    .values()
                    .filter_map(Option::as_ref),
            )
            .fold(0_usize, |total, surfaces| {
                total.saturating_add(surfaces.screen_layer_host_allocation_count)
            })
    }

    #[cfg(test)]
    pub(crate) fn active_surface_generation(&self) -> Option<u64> {
        self.surfaces.as_ref().map(|surfaces| surfaces.generation)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_projected_scene_preparation(&self) {
        self.fail_next_projected_scene_preparation.set(true);
    }

    pub(crate) fn snapshot_projected_group_frame(
        &mut self,
        group_id: ZoneId,
        frame: GpuTextureFrame,
    ) -> Result<GpuTextureFrame> {
        debug_assert_eq!(frame.origin, GpuTextureFrameOrigin::CompositorOutput);
        self.flush_pending_output_submission()?;
        let snapshot = self
            .projected_group_snapshots
            .get_mut(&group_id)
            .and_then(Option::as_mut)
            .context("projected group GPU snapshot was not admitted before rendering")?;
        anyhow::ensure!(
            snapshot.width == frame.width && snapshot.height == frame.height,
            "projected group GPU snapshot dimensions do not match the rendered frame"
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger projected group snapshot"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &snapshot.texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(frame.width, frame.height),
        );
        let _ = self.queue.submit(Some(encoder.finish()));
        snapshot.content_generation = snapshot.content_generation.saturating_add(1);
        Ok(GpuTextureFrame {
            width: snapshot.width,
            height: snapshot.height,
            storage_id: snapshot.texture.storage_id,
            content_generation: snapshot.content_generation,
            origin: GpuTextureFrameOrigin::ProjectionSnapshot,
            texture: snapshot.texture.texture.clone(),
            view: snapshot.texture.view.clone(),
            immutable_lease: Some(Arc::clone(&snapshot.lease)),
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    pub(crate) fn snapshot_current_output_frame(&mut self) -> Result<Option<GpuTextureFrame>> {
        let Some(frame) = self.current_output_frame()? else {
            return Ok(None);
        };
        self.snapshot_scene_frame(frame).map(Some)
    }

    pub(crate) fn opaque_black_frame(&self) -> Option<GpuTextureFrame> {
        let texture = self.opaque_black_texture.as_ref()?;
        Some(GpuTextureFrame {
            width: 1,
            height: 1,
            storage_id: texture.storage_id,
            content_generation: 1,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            immutable_lease: None,
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    pub(crate) fn snapshot_scene_frame(
        &mut self,
        frame: GpuTextureFrame,
    ) -> Result<GpuTextureFrame> {
        if frame.origin == GpuTextureFrameOrigin::ImmutableSnapshot {
            return Ok(frame);
        }
        self.flush_pending_output_submission()?;
        let snapshot = self
            .immutable_scene_snapshots
            .iter_mut()
            .find(|snapshot| {
                snapshot.width == frame.width
                    && snapshot.height == frame.height
                    && Arc::strong_count(&snapshot.lease) == 1
            })
            .context("all pre-admitted immutable GPU scene snapshots are still leased")?;
        anyhow::ensure!(
            snapshot.texture.storage_id != frame.storage_id,
            "immutable GPU scene snapshot cannot alias its source texture"
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger immutable scene snapshot"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &snapshot.texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(frame.width, frame.height),
        );
        let _ = self.queue.submit(Some(encoder.finish()));
        snapshot.content_generation = snapshot.content_generation.saturating_add(1);
        Ok(GpuTextureFrame {
            width: snapshot.width,
            height: snapshot.height,
            storage_id: snapshot.texture.storage_id,
            content_generation: snapshot.content_generation,
            origin: GpuTextureFrameOrigin::ImmutableSnapshot,
            texture: snapshot.texture.texture.clone(),
            view: snapshot.texture.view.clone(),
            immutable_lease: Some(Arc::clone(&snapshot.lease)),
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    pub(crate) fn restore_scene_frame(&mut self, frame: &GpuTextureFrame) -> Result<()> {
        self.flush_pending_output_submission()?;
        let surfaces = self
            .surfaces
            .as_mut()
            .filter(|surfaces| surfaces.width == frame.width && surfaces.height == frame.height)
            .context("retained GPU scene dimensions do not match admitted compositor surfaces")?;
        anyhow::ensure!(
            surfaces.front.storage_id != frame.storage_id,
            "retained GPU scene cannot alias its restore destination"
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger retained scene restore"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &surfaces.front.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(frame.width, frame.height),
        );
        let _ = self.queue.submit(Some(encoder.finish()));
        surfaces.front_contents = None;
        surfaces.back_contents = None;
        self.current_output = Some(GpuCompositorOutputSurface::Front);
        self.output_generation = self.output_generation.saturating_add(1);
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_sample_result = None;
        Ok(())
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
        self.producer_content_generation = self.producer_content_generation.saturating_add(1);
        Some(GpuTextureFrame {
            width: canvas.width(),
            height: canvas.height(),
            storage_id: texture.storage_id,
            content_generation: self.producer_content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            immutable_lease: None,
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    fn flush_pending_output_submission(&mut self) -> Result<()> {
        if self.pending_preview_readback().is_some() {
            return self.submit_pending_preview_work();
        }
        if let Some(mut frame) = self.frame_in_flight.take() {
            debug_assert_eq!(frame.generation, self.output_generation);
            let submission_index = frame.submit(&self.queue);
            debug_assert!(submission_index.is_some());
            if let Some(submission_index) = submission_index {
                self.finish_pending_uploads(submission_index.clone());
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                self.retire_native_screen_leases(
                    submission_index,
                    frame.take_native_screen_leases(),
                );
            }
            self.release_retired_uniform_slots();
        }
        Ok(())
    }

    pub(super) fn supersede_frame_in_flight(
        &mut self,
        reason: &'static str,
    ) -> Option<StashedFrame> {
        let frame = self.frame_in_flight.take()?;
        let encoder = frame.supersede(reason);
        #[cfg(test)]
        {
            self.superseded_frame_count = self.superseded_frame_count.saturating_add(1);
        }
        encoder
    }

    fn stage_frame_in_flight(
        &mut self,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
    ) {
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        self.stage_frame_in_flight_with_native_screen_leases(encoder, preview_readback, Vec::new());
        #[cfg(not(all(target_os = "macos", feature = "screen-capture")))]
        {
            debug_assert!(
                self.frame_in_flight.is_none(),
                "deferred GPU frame must be submitted or superseded before replacement"
            );
            self.frame_in_flight = Some(FrameInFlight::building(
                self.output_generation,
                encoder,
                preview_readback,
            ));
        }
    }

    fn pending_preview_readback(&self) -> Option<&PendingPreviewReadback> {
        self.frame_in_flight
            .as_ref()
            .and_then(FrameInFlight::preview_readback)
    }

    pub(super) fn pending_preview_submission(&self) -> Option<wgpu::SubmissionIndex> {
        self.frame_in_flight
            .as_ref()
            .and_then(FrameInFlight::preview_submission_index)
    }

    pub(super) fn has_pending_output_submission(&self) -> bool {
        self.frame_in_flight
            .as_ref()
            .is_some_and(FrameInFlight::is_building)
    }

    pub(super) fn finish_pending_uploads(&mut self, submission_index: wgpu::SubmissionIndex) {
        if let Some(surfaces) = self.surfaces.as_mut() {
            surfaces.finish_pending_uploads(submission_index);
        }
    }

    pub(super) fn discard_pending_uploads(&mut self) {
        if let Some(surfaces) = self.surfaces.as_mut() {
            surfaces.discard_pending_uploads();
        }
    }

    /// Advances the uniform ring watermarks so retired slots can be reused.
    ///
    /// Invariant: a ring slot must never be rewritten while a not-yet-
    /// submitted encoder references it. Call sites guarantee no local encoder
    /// is being built; the guard covers the stashed compositor encoder.
    pub(super) fn release_retired_uniform_slots(&mut self) {
        if !self.has_pending_output_submission() {
            self.pipeline.release_retired_uniform_slots();
        }
    }

    pub(crate) fn surface_snapshot(&self) -> Option<GpuCompositorSurfaceSnapshot> {
        self.surfaces
            .as_ref()
            .map(GpuCompositorSurfaceSet::snapshot)
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

impl GpuProjectionSnapshot {
    fn try_new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        let texture = GpuCompositorTexture::try_new(
            device,
            width,
            height,
            "SparkleFlinger Projected Group Snapshot",
        )?;
        Ok(Self {
            width,
            height,
            texture,
            content_generation: 0,
            lease: Arc::new(GpuTextureFrameLease),
        })
    }
}

impl GpuImmutableSceneSnapshot {
    fn try_new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        let texture = GpuCompositorTexture::try_new(
            device,
            width,
            height,
            "SparkleFlinger Immutable Scene Snapshot",
        )?;
        Ok(Self {
            width,
            height,
            texture,
            content_generation: 0,
            lease: Arc::new(GpuTextureFrameLease),
        })
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
