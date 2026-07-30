use std::collections::HashMap;
use std::fmt;
#[cfg(target_os = "windows")]
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Arc, Weak};

use anyhow::{Context, Result};
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
#[cfg(target_os = "windows")]
use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureDynamicRange, CapturePixelFormat, CaptureTransferFunction,
    PlatformGpuApi, ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor,
    ScreenBranchPayload, ScreenBranchPublication, ScreenCaptureBackend, ScreenCursorPolicy,
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativePreparationPayload,
    ScreenNativeTargetPreparation, ScreenNativeTargetPreparer, ScreenPhysicalGpuDeviceIdentity,
    ScreenPlanGeneration, ScreenPublicationKind, ScreenReductionFilter, ScreenResourceApi,
};
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, Canvas, PublishedSurface, SurfaceStateCounts,
};
#[cfg(target_os = "windows")]
use hypercolor_windows_capture::{
    GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy, GpuSurfaceFilter,
    GpuSurfaceFormat, GpuSurfacePublication, GpuSurfaceSourceColorSpace,
    GpuSurfaceTargetPreparation,
};
#[cfg(target_os = "windows")]
use hypercolor_windows_gpu_interop::{
    D3d11On12ScreenBridge, D3d11On12ScreenInteropError, PreparedScreenCopyTarget,
};

use super::{
    CompositionPlan, DisplayFinalizeCacheKey, MediaTextureSourceKey,
    SparkleFlingerSurfacePoolCounts,
};
use crate::render_thread::gpu_device::{GpuRenderDevice, texture_format_name};
#[cfg(target_os = "windows")]
use crate::render_thread::producer_queue::WindowsScreenTextureLease;
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
mod screen_upload;
mod source;
mod telemetry;

use compositor::{ComposeSourceBindGroupCache, SamplingReadbackLatch, create_compose_bind_group};
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
use screen_upload::{
    ScreenPublicationUploadPool, ScreenUploadContentKey, ScreenUploadResidencyPolicy,
};
use source::{
    CachedGpuSourceCopy, CachedSourceUpload, SourceCopyBindGroupCache, gpu_source_frame,
    write_rgba_texture,
};
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
static NEXT_GPU_TEXTURE_STORAGE_ID: AtomicU64 = AtomicU64::new(1);
#[cfg(target_os = "windows")]
static NEXT_SCREEN_TARGET_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuCanvasFallbackReason {
    InvalidExtent,
    TextureDimension,
    ResourceAllocation,
}

impl GpuCanvasFallbackReason {
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidExtent => "canvas extent is empty or not representable",
            Self::TextureDimension => "canvas extent exceeds the GPU texture dimension limit",
            Self::ResourceAllocation => "GPU canvas resources could not be admitted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuCanvasAdmission {
    Gpu,
    CpuFallback(GpuCanvasFallbackReason),
}

pub(crate) enum GpuCanvasPreparation {
    Gpu(GpuCompositorSurfaceSet),
    CpuFallback,
}

impl GpuCanvasPreparation {
    pub(super) const fn is_admitted(&self) -> bool {
        matches!(self, Self::Gpu(_))
    }

    pub(super) const fn cpu_fallback() -> Self {
        Self::CpuFallback
    }
}

fn gpu_canvas_admission(
    max_texture_dimension_2d: u32,
    width: u32,
    height: u32,
) -> GpuCanvasAdmission {
    if width == 0 || height == 0 {
        return GpuCanvasAdmission::CpuFallback(GpuCanvasFallbackReason::InvalidExtent);
    }
    if width > max_texture_dimension_2d || height > max_texture_dimension_2d {
        return GpuCanvasAdmission::CpuFallback(GpuCanvasFallbackReason::TextureDimension);
    }
    GpuCanvasAdmission::Gpu
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

#[cfg(target_os = "windows")]
struct WindowsScreenBridge {
    interop: D3d11On12ScreenBridge,
}

#[cfg(target_os = "windows")]
struct WindowsScreenTargetPreparer {
    bridge: Weak<WindowsScreenBridge>,
}

#[cfg(target_os = "windows")]
struct PreparedWindowsScreenTarget {
    interop: PreparedScreenCopyTarget,
    storage_id: u64,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeScreenCopyFailurePolicy {
    Retain,
    Reprepare,
    InvalidateFrameAndReprepare,
}

#[cfg(target_os = "windows")]
fn native_screen_copy_failure_policy(
    error: &D3d11On12ScreenInteropError,
) -> NativeScreenCopyFailurePolicy {
    match error {
        D3d11On12ScreenInteropError::KeyedMutexTimeout
        | D3d11On12ScreenInteropError::Capture(
            hypercolor_windows_capture::CaptureError::GpuSurfaceUseUnavailable { .. },
        ) => NativeScreenCopyFailurePolicy::Retain,
        D3d11On12ScreenInteropError::TargetContentUncertain { .. } => {
            NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare
        }
        _ => NativeScreenCopyFailurePolicy::Reprepare,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn native_screen_copy_error_invalidates_frame(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<D3d11On12ScreenInteropError>()
        .is_some_and(|error| {
            native_screen_copy_failure_policy(error)
                == NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare
        })
}

#[cfg(target_os = "windows")]
fn screen_storage_requires_cache_turnover(current: Option<u64>, next: u64) -> bool {
    current != Some(next)
}

#[cfg(target_os = "windows")]
fn validate_windows_plan_generation(core: u64, native: u64) -> Result<()> {
    anyhow::ensure!(
        core == native,
        "Windows target manifest plan generation does not match the candidate"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn is_retryable_native_screen_copy_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<D3d11On12ScreenInteropError>()
        .is_some_and(|error| {
            native_screen_copy_failure_policy(error) == NativeScreenCopyFailurePolicy::Retain
        })
}

#[cfg(target_os = "windows")]
impl ScreenNativeTargetPreparer for WindowsScreenTargetPreparer {
    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<ScreenNativeTargetPreparation> {
        let manifest = platform
            .downcast::<GpuSurfaceTargetPreparation>()
            .context("Windows screen target received an unknown preparation manifest")?;
        validate_windows_target_manifest(descriptor, platform.plan_generation(), &manifest)?;
        let bridge = self
            .bridge
            .upgrade()
            .context("Windows screen renderer was retired during target preparation")?;
        let interop = bridge
            .interop
            .prepare_target(&manifest)
            .context("failed to prepare the renderer screen-copy target")?;
        let storage_id = NEXT_GPU_TEXTURE_STORAGE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("GPU texture storage identity space is exhausted"))?;
        let retained_bytes = interop.retained_bytes();
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(PreparedWindowsScreenTarget {
                    interop,
                    storage_id,
                }),
            ),
            retained_bytes,
        ))
    }
}

#[cfg(target_os = "windows")]
fn validate_windows_target_manifest(
    descriptor: &ResolvedScreenPublicationDescriptor,
    plan_generation: ScreenPlanGeneration,
    manifest: &GpuSurfaceTargetPreparation,
) -> Result<()> {
    anyhow::ensure!(
        descriptor.kind() == ScreenPublicationKind::Surface,
        "Windows native target requires a Surface descriptor"
    );
    let physical = descriptor.physical();
    let native = manifest.descriptor();
    validate_windows_plan_generation(plan_generation.get(), manifest.plan_generation().get())?;
    let region = physical.source_region();
    let integer = |value: hypercolor_core::input::screen::ScreenRational| {
        (value.denominator().get() == 1)
            .then(|| u32::try_from(value.numerator()).ok())
            .flatten()
    };
    let native_region = native.source_region();
    anyhow::ensure!(
        integer(region.x()) == Some(native_region.origin_x())
            && integer(region.y()) == Some(native_region.origin_y())
            && integer(region.width()) == Some(native_region.width())
            && integer(region.height()) == Some(native_region.height()),
        "Windows target manifest source region does not match the resolved descriptor"
    );
    let reduction_extent = physical.reduction_extent();
    anyhow::ensure!(
        native.output_extent().width() == reduction_extent.width()
            && native.output_extent().height() == reduction_extent.height(),
        "Windows target manifest extent does not match the resolved descriptor"
    );
    anyhow::ensure!(
        native.coordinate_space() == GpuSurfaceCoordinateSpace::LogicalDisplay
            && native.filter() == GpuSurfaceFilter::Nearest
            && native.format() == GpuSurfaceFormat::Rgba8Unorm
            && native.color_pipeline() == GpuSurfaceColorPipeline::PreserveEncoded,
        "Windows target manifest execution contract is not exact"
    );
    anyhow::ensure!(
        physical.reduction_filter() == ScreenReductionFilter::Nearest
            && physical.target_pixel_format() == CapturePixelFormat::Rgba8
            && physical.color_pipeline().transform()
                == ResolvedScreenColorTransform::PreserveEncodedSamples
            && native.algorithm_revision() == physical.algorithm_revision(),
        "Windows target manifest processing contract does not match the resolved descriptor"
    );
    let cursor_matches = matches!(
        (physical.cursor(), native.cursor()),
        (ScreenCursorPolicy::Exclude, GpuSurfaceCursorPolicy::Exclude)
            | (ScreenCursorPolicy::Include, GpuSurfaceCursorPolicy::Include)
    );
    anyhow::ensure!(
        cursor_matches,
        "Windows target manifest cursor contract does not match the resolved descriptor"
    );
    let source = descriptor.source();
    anyhow::ensure!(
        source.resources().backend() == &ScreenCaptureBackend::WindowsDesktopDuplication
            && source.resources().api()
                == &ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        "Windows target manifest was paired with a non-D3D11 source"
    );
    let adapter = manifest.adapter_luid();
    anyhow::ensure!(
        matches!(
            source.resources().physical_gpu_device(),
            Some(ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
                low_part,
                high_part,
            }) if *low_part == adapter.low_part() && *high_part == adapter.high_part()
        ),
        "Windows target manifest adapter does not match the resolved source"
    );
    let source_id = descriptor
        .source_epoch()
        .source_id
        .as_str()
        .strip_prefix("windows:")
        .context("Windows source identity is not canonical")?;
    anyhow::ensure!(
        manifest.source_id() == source_id
            && manifest.topology_generation() == descriptor.source_epoch().topology_generation
            && manifest.duplication_generation() == source.resources().resource_generation(),
        "Windows target manifest source generation does not match the resolved source"
    );
    anyhow::ensure!(
        source_color_space_matches(source.colorimetry(), native.source_color_space()),
        "Windows target manifest color space does not match the resolved source"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
fn source_color_space_matches(
    core: hypercolor_core::input::screen::CaptureColorimetry,
    native: GpuSurfaceSourceColorSpace,
) -> bool {
    match native {
        GpuSurfaceSourceColorSpace::RgbFullG22P709 => {
            core.color_space() == CaptureColorSpace::Srgb
                && core.transfer_function() == CaptureTransferFunction::Srgb
                && core.dynamic_range() == Some(CaptureDynamicRange::Standard)
        }
        GpuSurfaceSourceColorSpace::RgbFullLinearP709 => {
            core.color_space() == CaptureColorSpace::Srgb
                && core.transfer_function() == CaptureTransferFunction::Linear
                && core.dynamic_range() == Some(CaptureDynamicRange::Standard)
        }
        GpuSurfaceSourceColorSpace::RgbFullPqP2020 => {
            core.color_space() == CaptureColorSpace::Rec2020
                && core.transfer_function() == CaptureTransferFunction::Pq
                && core.dynamic_range() == Some(CaptureDynamicRange::High)
        }
        GpuSurfaceSourceColorSpace::Unknown => {
            core.color_space() == CaptureColorSpace::Unknown
                && core.transfer_function() == CaptureTransferFunction::Unknown
                && core.dynamic_range().is_none()
        }
    }
}

#[cfg(target_os = "windows")]
fn create_screen_bridge(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    max_texture_dimension: u32,
) -> (
    Option<Arc<WindowsScreenBridge>>,
    Option<ScreenNativeExecutionTarget>,
) {
    let interop = match D3d11On12ScreenBridge::new(device.clone(), queue.clone()) {
        Ok(bridge) => bridge,
        Err(error) => {
            tracing::debug!(%error, "renderer does not expose a DX12 screen-copy target");
            return (None, None);
        }
    };
    let bridge = Arc::new(WindowsScreenBridge { interop });
    let target = create_screen_target(&bridge, max_texture_dimension);
    (Some(bridge), target)
}

#[cfg(target_os = "windows")]
fn create_screen_target(
    bridge: &Arc<WindowsScreenBridge>,
    max_texture_dimension: u32,
) -> Option<ScreenNativeExecutionTarget> {
    let Ok(target_id) =
        NEXT_SCREEN_TARGET_ID.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
    else {
        tracing::warn!("screen target identity space is exhausted");
        return None;
    };
    let target_id = ScreenNativeExecutionTargetId::new(
        NonZeroU64::new(target_id).expect("screen target identities start at one"),
    );
    let adapter_luid = bridge.interop.adapter_luid();
    let target = ScreenNativeExecutionTarget::new(
        target_id,
        PlatformGpuApi::Direct3d11,
        ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
            low_part: adapter_luid.low_part(),
            high_part: adapter_luid.high_part(),
        },
        NonZeroU32::new(max_texture_dimension)
            .expect("wgpu devices expose a non-zero texture dimension limit"),
        Arc::new(WindowsScreenTargetPreparer {
            bridge: Arc::downgrade(bridge),
        }),
    );
    Some(target)
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
    #[cfg(test)]
    superseded_frame_count: usize,
    #[cfg(test)]
    preview_surface_allocation_count: usize,
    #[cfg(test)]
    defer_preview_resolve_once: bool,
    #[cfg(test)]
    defer_preview_map_resolve_once: bool,
}

struct FrameInFlight {
    generation: u64,
    encoder: EncoderStage,
    readbacks: Vec<StagedReadback>,
}

enum EncoderStage {
    Building(Option<wgpu::CommandEncoder>),
    Submitted(wgpu::SubmissionIndex),
    Superseded,
}

enum StagedReadback {
    Preview {
        readback: PendingPreviewReadback,
        stage: ReadbackStage,
    },
}

enum ReadbackStage {
    Encoded,
    Submitted(wgpu::SubmissionIndex),
}

impl FrameInFlight {
    fn building(
        generation: u64,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
    ) -> Self {
        let readbacks = preview_readback.map_or_else(Vec::new, |readback| {
            vec![StagedReadback::Preview {
                readback,
                stage: ReadbackStage::Encoded,
            }]
        });
        Self {
            generation,
            encoder: EncoderStage::Building(Some(encoder)),
            readbacks,
        }
    }

    fn submitted(
        generation: u64,
        submission_index: wgpu::SubmissionIndex,
        preview_readback: PendingPreviewReadback,
    ) -> Self {
        Self {
            generation,
            encoder: EncoderStage::Submitted(submission_index.clone()),
            readbacks: vec![StagedReadback::Preview {
                readback: preview_readback,
                stage: ReadbackStage::Submitted(submission_index),
            }],
        }
    }

    fn preview_readback(&self) -> Option<&PendingPreviewReadback> {
        self.readbacks.first().map(|readback| match readback {
            StagedReadback::Preview { readback, .. } => readback,
        })
    }

    fn preview_submission_index(&self) -> Option<wgpu::SubmissionIndex> {
        self.readbacks.iter().find_map(|readback| match readback {
            StagedReadback::Preview {
                stage: ReadbackStage::Submitted(submission_index),
                ..
            } => Some(submission_index.clone()),
            StagedReadback::Preview {
                stage: ReadbackStage::Encoded,
                ..
            } => None,
        })
    }

    fn take_preview_readback(&mut self) -> Option<PendingPreviewReadback> {
        let index = self
            .readbacks
            .iter()
            .position(|readback| matches!(readback, StagedReadback::Preview { .. }))?;
        match self.readbacks.remove(index) {
            StagedReadback::Preview { readback, .. } => Some(readback),
        }
    }

    fn submission_index(&self) -> Option<wgpu::SubmissionIndex> {
        match &self.encoder {
            EncoderStage::Submitted(submission_index) => Some(submission_index.clone()),
            EncoderStage::Building(_) | EncoderStage::Superseded => None,
        }
    }

    fn is_building(&self) -> bool {
        matches!(self.encoder, EncoderStage::Building(_))
    }

    fn take_encoder_for_chaining(&mut self) -> Option<wgpu::CommandEncoder> {
        match &mut self.encoder {
            EncoderStage::Building(encoder) => encoder.take(),
            EncoderStage::Submitted(_) | EncoderStage::Superseded => None,
        }
    }

    fn mark_submitted(&mut self, submission_index: wgpu::SubmissionIndex) {
        debug_assert!(
            matches!(self.encoder, EncoderStage::Building(None)),
            "only a consumed building encoder can advance to submitted"
        );
        self.encoder = EncoderStage::Submitted(submission_index.clone());
        for readback in &mut self.readbacks {
            match readback {
                StagedReadback::Preview { stage, .. } => {
                    *stage = ReadbackStage::Submitted(submission_index.clone());
                }
            }
        }
    }

    fn submit(&mut self, queue: &wgpu::Queue) -> Option<wgpu::SubmissionIndex> {
        if let Some(submission_index) = self.submission_index() {
            return Some(submission_index);
        }
        let encoder = self.take_encoder_for_chaining()?;
        let submission_index = queue.submit(Some(encoder.finish()));
        self.mark_submitted(submission_index.clone());
        Some(submission_index)
    }

    fn supersede(mut self, reason: &'static str) -> Option<wgpu::CommandEncoder> {
        let encoder = self.take_encoder_for_chaining();
        self.encoder = EncoderStage::Superseded;
        self.readbacks.clear();
        tracing::trace!(
            generation = self.generation,
            reason,
            "superseding deferred GPU frame"
        );
        encoder
    }

    #[cfg(test)]
    fn encoded_preview_for_test() -> Self {
        Self {
            generation: 7,
            encoder: EncoderStage::Building(None),
            readbacks: vec![StagedReadback::Preview {
                readback: PendingPreviewReadback::PreviewBuffer {
                    request: super::PreviewSurfaceRequest {
                        width: 2,
                        height: 2,
                    },
                    readback_key: None,
                    cache_as_full_size: false,
                    slot: 0,
                },
                stage: ReadbackStage::Encoded,
            }],
        }
    }
}

impl Drop for FrameInFlight {
    fn drop(&mut self) {
        if cfg!(debug_assertions) && !std::thread::panicking() {
            debug_assert!(
                !self.is_building() || self.readbacks.is_empty(),
                "generation {} dropped with encoded GPU readbacks before submit or supersede",
                self.generation
            );
        }
    }
}

pub(crate) struct GpuCompositorSurfaceSet {
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

struct GpuCompositorTexture {
    storage_id: u64,
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
        let max_buffer_size = device.limits().max_buffer_size;
        let max_storage_buffer_binding_size = device.limits().max_storage_buffer_binding_size;

        let pipeline = GpuCompositorPipeline::new(&device);
        let spatial_sampler = GpuSpatialSampler::new(&device);
        #[cfg(target_os = "windows")]
        let (screen_bridge, screen_target) =
            create_screen_bridge(&device, &queue, probe.max_texture_dimension_2d);

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
            #[cfg(test)]
            superseded_frame_count: 0,
            #[cfg(test)]
            preview_surface_allocation_count: 0,
            #[cfg(test)]
            defer_preview_resolve_once: false,
            #[cfg(test)]
            defer_preview_map_resolve_once: false,
        })
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
    }

    pub(super) const fn canvas_gpu_admitted(&self) -> bool {
        self.canvas_gpu_admitted
    }

    #[cfg(test)]
    pub(super) const fn max_texture_dimension_2d(&self) -> u32 {
        self.probe.max_texture_dimension_2d
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn screen_native_execution_target(&self) -> Option<&ScreenNativeExecutionTarget> {
        if !self.canvas_gpu_admitted {
            return None;
        }
        self.screen_target.as_ref()
    }

    pub(super) fn prepare_canvas_resize(&self, width: u32, height: u32) -> GpuCanvasPreparation {
        let admission = gpu_canvas_admission(self.probe.max_texture_dimension_2d, width, height);
        match admission {
            GpuCanvasAdmission::Gpu => {}
            GpuCanvasAdmission::CpuFallback(reason) => {
                tracing::info!(
                    width,
                    height,
                    reason = reason.message(),
                    "using CPU compositor for canvas extent"
                );
                return GpuCanvasPreparation::CpuFallback;
            }
        }
        match GpuCompositorSurfaceSet::try_new(&self.device, &self.pipeline, width, height) {
            Ok(surfaces) => GpuCanvasPreparation::Gpu(surfaces),
            Err(error) => {
                tracing::warn!(
                    %error,
                    width,
                    height,
                    reason = GpuCanvasFallbackReason::ResourceAllocation.message(),
                    "using CPU compositor after GPU canvas admission failed"
                );
                GpuCanvasPreparation::CpuFallback
            }
        }
    }

    pub(super) fn apply_canvas_resize(&mut self, preparation: GpuCanvasPreparation) {
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("canvas resize committed"));
        self.surfaces = match preparation {
            GpuCanvasPreparation::Gpu(surfaces) => {
                self.canvas_gpu_admitted = true;
                Some(surfaces)
            }
            GpuCanvasPreparation::CpuFallback => {
                self.canvas_gpu_admitted = false;
                None
            }
        };
        self.preview_surfaces = None;
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surfaces.clear();
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
        #[cfg(target_os = "windows")]
        self.release_native_screen_caches();
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn release_native_screen_caches(&mut self) {
        if let Some(surfaces) = &mut self.surfaces {
            surfaces
                .compose_source_bind_groups
                .release_native_screen_entries();
            surfaces
                .source_copy_bind_groups
                .release_native_screen_entries();
        }
        self.screen_storage_id = None;
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
            windows_screen_lease: Some(WindowsScreenTextureLease::new(
                copy,
                target_lifetime,
                capture_lifetime,
            )),
        }))
    }

    pub(crate) fn can_sample_zone_plan(&self, prepared_zones: &[PreparedZonePlan]) -> bool {
        GpuSamplingPlan::supports_prepared_zones(prepared_zones)
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
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
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
        self.producer_content_generation = self.producer_content_generation.saturating_add(1);
        Some(GpuTextureFrame {
            width: canvas.width(),
            height: canvas.height(),
            storage_id: texture.storage_id,
            content_generation: self.producer_content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
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
                self.finish_pending_uploads(submission_index);
            }
            self.release_retired_uniform_slots();
        }
        Ok(())
    }

    pub(super) fn supersede_frame_in_flight(
        &mut self,
        reason: &'static str,
    ) -> Option<wgpu::CommandEncoder> {
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
        let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let front = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Front");
        let back = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Back");
        let source = GpuCompositorTexture::new(device, width, height, "SparkleFlinger Source");

        let surfaces = Self {
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
