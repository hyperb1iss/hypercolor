#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use std::alloc::Layout;
#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use std::sync::Mutex;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use std::sync::Weak;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use std::time::Instant;

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
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use hypercolor_core::input::screen::{
    CapturePixelFormat, CaptureRotation, CaptureTransferFunction, LED_TONE_MAP_ALGORITHM_REVISION,
    MacosNativeTargetManifest, PlatformGpuApi, PreparedLedToneMap, ResolvedScreenColorTransform,
    ResolvedScreenPublicationDescriptor, ScreenBranchPayload, ScreenBranchPublication,
    ScreenCaptureBackend, ScreenColorTransformCapabilities, ScreenLetterboxFill,
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativePreparationPayload,
    ScreenNativeRetentionQuote, ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
    ScreenPhysicalGpuDeviceIdentity, ScreenPhysicalReductionDescriptor, ScreenPlanGeneration,
    ScreenPublicationKind, ScreenReductionFilter, ScreenResourceApi, ScreenSourceReflection,
};
use hypercolor_core::spatial::PreparedZonePlan;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use hypercolor_macos_capture::MacosCaptureFrame;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use hypercolor_macos_gpu_interop::{
    ImportedMacosScreenFrame, MacosNativeColorTransform, MacosNativeLetterboxFill,
    MacosNativeOutputTransfer, MacosNativeReducer, MacosNativeReductionDescriptor,
    MacosNativeReductionFilter, MacosNativeReductionTarget, MacosNativeTargetFormat,
    MacosScreenBridge as MacosInteropScreenBridge, MacosScreenStorageIdentity,
    probe_macos_metal4_capabilities,
};
use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, PublishedSurface, SurfaceStateCounts};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::ZoneId;
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
    Gpu {
        generation: GpuCanvasGeneration,
        immutable_scene_snapshots: Vec<GpuImmutableSceneSnapshot>,
        opaque_black_texture: GpuCompositorTexture,
    },
    CpuFallback,
}

pub(crate) struct GpuCanvasGeneration {
    surfaces: GpuCompositorSurfaceSet,
    preview_surfaces: Option<GpuPreviewSurfaceSet>,
    sampling_readback_buffers: Option<SamplingReadbackBuffers>,
}

pub(crate) enum GpuComposeOutcome {
    Produced(ComposedFrameSet),
    Retained(ComposedFrameSet),
    Failed(anyhow::Error),
}

impl GpuCanvasPreparation {
    pub(super) const fn is_admitted(&self) -> bool {
        matches!(self, Self::Gpu { .. })
    }

    pub(super) const fn cpu_fallback() -> Self {
        Self::CpuFallback
    }

    fn compositor_surfaces(&self) -> Option<&GpuCompositorSurfaceSet> {
        match self {
            Self::Gpu { generation, .. } => Some(&generation.surfaces),
            Self::CpuFallback => None,
        }
    }
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

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
struct MacosScreenBridge {
    device: wgpu::Device,
    interop: MacosInteropScreenBridge,
    reducer: MacosNativeReducer,
    storage_ids: Mutex<HashMap<MacosScreenStorageIdentity, u64>>,
    physical_targets: Mutex<
        Vec<(
            ScreenPlanGeneration,
            ScreenPhysicalReductionDescriptor,
            Weak<PreparedMacosPhysicalTarget>,
        )>,
    >,
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
struct MacosScreenTargetPreparer {
    bridge: Weak<MacosScreenBridge>,
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
#[derive(Debug)]
struct PreparedMacosPhysicalTarget {
    target: MacosNativeReductionTarget,
    storage_id: u64,
    content_sequence: Mutex<Option<u64>>,
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
#[derive(Debug)]
pub(crate) struct PreparedMacosScreenTarget {
    resource_generation: u64,
    descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    physical: Option<Arc<PreparedMacosPhysicalTarget>>,
    logical_target: Option<MacosNativeReductionTarget>,
    logical_storage_id: Option<u64>,
    logical_content_sequence: Mutex<Option<u64>>,
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
impl Clone for PreparedMacosScreenTarget {
    fn clone(&self) -> Self {
        Self {
            resource_generation: self.resource_generation,
            descriptor: Arc::clone(&self.descriptor),
            physical: self.physical.clone(),
            logical_target: self.logical_target.clone(),
            logical_storage_id: self.logical_storage_id,
            logical_content_sequence: Mutex::new(
                *self
                    .logical_content_sequence
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
        }
    }
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
impl MacosScreenBridge {
    fn import_frame(
        &self,
        device: &wgpu::Device,
        resource_generation: u64,
        frame: Arc<MacosCaptureFrame>,
    ) -> Result<(ImportedMacosScreenFrame, u64)> {
        let imported = self
            .interop
            .import_frame(device, resource_generation, frame)
            .context("failed to import the native macOS screen publication")?;
        let identity = imported.storage_identity();
        let mut storage_ids = self
            .storage_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let storage_id = match storage_ids.entry(identity) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let storage_id = next_gpu_texture_storage_id()?;
                entry.insert(storage_id);
                storage_id
            }
        };
        Ok((imported, storage_id))
    }

    fn prepare_target(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        plan_generation: ScreenPlanGeneration,
    ) -> Result<PreparedMacosScreenTarget> {
        macos_native_color_transform(descriptor)?;
        let physical = if macos_descriptor_requires_native_work(descriptor) {
            let mut targets = self
                .physical_targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            targets.retain(|(_, _, target)| target.strong_count() > 0);
            if let Some(target) = targets.iter().find_map(|(plan, candidate, target)| {
                (*plan == plan_generation && candidate == descriptor.physical())
                    .then(|| target.upgrade())
                    .flatten()
            }) {
                Some(target)
            } else {
                let extent = descriptor.physical().reduction_extent();
                let format =
                    macos_native_target_format(descriptor.physical().target_pixel_format())?;
                let target = Arc::new(PreparedMacosPhysicalTarget {
                    target: self.reducer.create_target(
                        self.interop_device(),
                        extent.width(),
                        extent.height(),
                        format,
                    )?,
                    storage_id: next_gpu_texture_storage_id()?,
                    content_sequence: Mutex::new(None),
                });
                targets.push((
                    plan_generation,
                    descriptor.physical().clone(),
                    Arc::downgrade(&target),
                ));
                Some(target)
            }
        } else {
            None
        };
        let geometry = descriptor.geometry();
        let needs_materialization = physical.is_some() && !geometry.content_fills_output();
        if needs_materialization {
            macos_native_letterbox_fill(descriptor)?;
        }
        let logical_target = if needs_materialization {
            let extent = geometry.output_extent();
            Some(self.reducer.create_target(
                self.interop_device(),
                extent.width(),
                extent.height(),
                macos_native_target_format(descriptor.physical().target_pixel_format())?,
            )?)
        } else {
            None
        };
        let logical_storage_id = logical_target
            .as_ref()
            .map(|_| next_gpu_texture_storage_id())
            .transpose()?;
        Ok(PreparedMacosScreenTarget {
            resource_generation: descriptor.source().resources().resource_generation(),
            descriptor: Arc::new(descriptor.clone()),
            physical,
            logical_target,
            logical_storage_id,
            logical_content_sequence: Mutex::new(None),
        })
    }

    fn interop_device(&self) -> &wgpu::Device {
        &self.device
    }

    fn clear_capture_caches(&self) {
        self.interop.clear_capture_caches();
        self.storage_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn next_gpu_texture_storage_id() -> Result<u64> {
    NEXT_GPU_TEXTURE_STORAGE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| anyhow::anyhow!("GPU texture storage identity space is exhausted"))
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn prepared_macos_screen_target_metadata_bytes() -> Result<u64> {
    checked_macos_arc_allocation_bytes::<PreparedMacosScreenTarget>()?
        .checked_add(checked_macos_arc_allocation_bytes::<
            ResolvedScreenPublicationDescriptor,
        >()?)
        .context("macOS prepared target metadata accounting overflow")
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn prepared_macos_screen_target_exclusive_bytes(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<u64> {
    let mut bytes = prepared_macos_screen_target_metadata_bytes()?;
    if !macos_descriptor_requires_native_work(descriptor) {
        return Ok(bytes);
    }
    if !descriptor.geometry().content_fills_output() {
        let logical_texture_bytes =
            macos_target_texture_bytes(descriptor.geometry().output_extent())
                .context("macOS logical target texture accounting overflow")?;
        bytes = bytes
            .checked_add(logical_texture_bytes)
            .context("macOS logical target accounting overflow")?;
    }
    Ok(bytes)
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn prepared_macos_screen_target_shared_bytes(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<u64> {
    if !macos_descriptor_requires_native_work(descriptor) {
        return Ok(0);
    }
    let physical_texture_bytes =
        macos_target_texture_bytes(descriptor.physical().reduction_extent())
            .context("macOS physical target texture accounting overflow")?;
    checked_macos_arc_allocation_bytes::<PreparedMacosPhysicalTarget>()?
        .checked_add(physical_texture_bytes)
        .context("macOS physical target accounting overflow")
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn prepared_macos_screen_target_retention(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<ScreenNativeRetentionQuote> {
    Ok(ScreenNativeRetentionQuote::split(
        prepared_macos_screen_target_exclusive_bytes(descriptor)?,
        prepared_macos_screen_target_shared_bytes(descriptor)?,
    ))
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn macos_target_texture_bytes(extent: hypercolor_core::input::screen::PixelExtent) -> Option<u64> {
    u64::from(extent.width())
        .checked_mul(u64::from(extent.height()))
        .and_then(|pixels| pixels.checked_mul(4))
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn macos_descriptor_requires_native_work(descriptor: &ResolvedScreenPublicationDescriptor) -> bool {
    let source = descriptor.source();
    descriptor.source_pixel_format() != CapturePixelFormat::Bgra8
        || source.geometry().crop().is_some()
        || descriptor.geometry().output_extent() != source.geometry().storage_extent()
        || descriptor.physical().reduction_extent() != source.geometry().storage_extent()
        || descriptor.physical().target_pixel_format() != descriptor.source_pixel_format()
        || !matches!(
            descriptor.physical().color_pipeline().transform(),
            ResolvedScreenColorTransform::PreserveEncodedSamples
        )
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
#[error("unsupported macOS native reduction target format: {0:?}")]
struct UnsupportedMacosNativeTargetFormat(CapturePixelFormat);

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn macos_native_target_format(
    format: CapturePixelFormat,
) -> std::result::Result<MacosNativeTargetFormat, UnsupportedMacosNativeTargetFormat> {
    match format {
        CapturePixelFormat::Rgba8 => Ok(MacosNativeTargetFormat::Rgba8),
        CapturePixelFormat::Bgra8 => Ok(MacosNativeTargetFormat::Bgra8),
        unsupported => Err(UnsupportedMacosNativeTargetFormat(unsupported)),
    }
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn macos_reduction_descriptor(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<MacosNativeReductionDescriptor> {
    let source = descriptor.source();
    let geometry = source.geometry();
    anyhow::ensure!(
        geometry.rotation() == CaptureRotation::Identity
            && source.reflection() == ScreenSourceReflection::None
            && geometry.native_extent() == geometry.storage_extent()
            && geometry.source_scale().numerator() == geometry.source_scale().denominator(),
        "macOS native reduction received unsupported pending source geometry"
    );
    let crop = geometry.crop();
    let crop_x = crop.map_or(0, hypercolor_core::input::screen::PixelRect::x);
    let crop_y = crop.map_or(0, hypercolor_core::input::screen::PixelRect::y);
    let region = descriptor.physical().source_region();
    let rational = |value: hypercolor_core::input::screen::ScreenRational| {
        value.numerator() as f32 / value.denominator().get() as f32
    };
    let source_rect = [
        crop_x as f32 + rational(region.x()),
        crop_y as f32 + rational(region.y()),
        rational(region.width()),
        rational(region.height()),
    ];
    let output = descriptor.physical().reduction_extent();
    let filter = match descriptor.physical().reduction_filter() {
        ScreenReductionFilter::Nearest => MacosNativeReductionFilter::Nearest,
        ScreenReductionFilter::Bilinear => MacosNativeReductionFilter::Bilinear,
        ScreenReductionFilter::Area => MacosNativeReductionFilter::Area,
    };
    MacosNativeReductionDescriptor::new(
        [output.width(), output.height()],
        [0, 0, output.width(), output.height()],
        source_rect,
        filter,
        macos_native_color_transform(descriptor)?,
    )
    .map_err(anyhow::Error::from)
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn macos_native_color_transform(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<Option<(MacosNativeOutputTransfer, MacosNativeColorTransform)>> {
    let pipeline = descriptor.physical().color_pipeline();
    if pipeline.transform() == ResolvedScreenColorTransform::PreserveEncodedSamples {
        return Ok(None);
    }
    let source = pipeline
        .effective_source()
        .context("managed macOS native reduction has no effective source colorimetry")?;
    let output = pipeline
        .output()
        .try_known()
        .context("managed macOS native reduction has no known output colorimetry")?;
    let calibration = pipeline
        .calibration()
        .context("managed macOS native reduction has no calibration")?;
    let prepared = PreparedLedToneMap::prepare(source, output, calibration)
        .context("failed to prepare shared macOS native color constants")?;
    let output_transfer = match output.transfer_function() {
        CaptureTransferFunction::Srgb => MacosNativeOutputTransfer::Srgb,
        CaptureTransferFunction::Linear => MacosNativeOutputTransfer::Linear,
        CaptureTransferFunction::Rec709 => MacosNativeOutputTransfer::Rec709,
        CaptureTransferFunction::Rec2020 => MacosNativeOutputTransfer::Rec2020,
        unsupported => {
            anyhow::bail!("unsupported macOS native output transfer function: {unsupported:?}")
        }
    };
    let constants = prepared.constants();
    Ok(Some((
        output_transfer,
        MacosNativeColorTransform::new(
            constants.source_to_target,
            constants.source_luminance_and_exposure,
            constants.curve,
        ),
    )))
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn macos_native_letterbox_fill(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<MacosNativeLetterboxFill> {
    match descriptor.processing_profile().letterbox_fill() {
        ScreenLetterboxFill::Transparent => Ok(MacosNativeLetterboxFill::Transparent),
        ScreenLetterboxFill::Solid(color) => Ok(MacosNativeLetterboxFill::Solid(
            color.map(|channel| f32::from(channel) / f32::from(u8::MAX)),
        )),
        ScreenLetterboxFill::EdgeExtend => {
            anyhow::bail!("macOS native reduction does not support edge-extended letterbox fill")
        }
    }
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn checked_macos_arc_allocation_bytes<T>() -> Result<u64> {
    let (layout, _) = Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .context("macOS Arc allocation layout overflow")?;
    u64::try_from(layout.pad_to_align().size()).context("macOS Arc allocation exceeds u64")
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
impl ScreenNativeTargetPreparer for MacosScreenTargetPreparer {
    fn quote_retained_bytes(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<u64> {
        let manifest = platform
            .downcast_ref::<MacosNativeTargetManifest>()
            .context("macOS screen target received an unknown preparation manifest")?;
        validate_macos_target_manifest(descriptor, manifest)?;
        self.bridge
            .upgrade()
            .context("macOS screen renderer was retired during target admission")?;
        prepared_macos_screen_target_exclusive_bytes(descriptor)
    }

    fn quote_retention(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<ScreenNativeRetentionQuote> {
        self.quote_retained_bytes(descriptor, platform)?;
        prepared_macos_screen_target_retention(descriptor)
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<ScreenNativeTargetPreparation> {
        let manifest = platform
            .downcast_ref::<MacosNativeTargetManifest>()
            .context("macOS screen target received an unknown preparation manifest")?;
        validate_macos_target_manifest(descriptor, manifest)?;
        let bridge = self
            .bridge
            .upgrade()
            .context("macOS screen renderer was retired during target preparation")?;
        let prepared = bridge.prepare_target(descriptor, platform.plan_generation())?;
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(prepared),
            ),
            prepared_macos_screen_target_retention(descriptor)?,
        ))
    }
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn validate_macos_target_manifest(
    descriptor: &ResolvedScreenPublicationDescriptor,
    manifest: &MacosNativeTargetManifest,
) -> Result<()> {
    anyhow::ensure!(
        descriptor.kind() == ScreenPublicationKind::Surface,
        "macOS native target requires a Surface descriptor"
    );
    let source = descriptor.source();
    let resources = source.resources();
    anyhow::ensure!(
        resources.backend() == &ScreenCaptureBackend::MacosScreenCaptureKit
            && resources.api() == &ScreenResourceApi::PlatformGpu(PlatformGpuApi::Metal),
        "macOS target manifest was paired with a non-Metal source"
    );
    anyhow::ensure!(
        matches!(
            resources.physical_gpu_device(),
            Some(ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(registry_id))
                if *registry_id == manifest.metal_registry_id()
        ),
        "macOS target manifest Metal device does not match the resolved source"
    );
    anyhow::ensure!(
        descriptor.source_epoch().session_generation == manifest.capture_session_generation()
            && resources.device_generation() == manifest.capture_session_generation(),
        "macOS target manifest capture session does not match the resolved source"
    );
    anyhow::ensure!(
        resources.resource_generation() == manifest.resource_generation(),
        "macOS target manifest resource generation does not match the resolved source"
    );
    Ok(())
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

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
pub(crate) const fn native_screen_copy_error_invalidates_frame(_error: &anyhow::Error) -> bool {
    true
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
fn prepared_windows_screen_target_metadata_bytes() -> Result<u64> {
    checked_arc_allocation_bytes::<PreparedWindowsScreenTarget>()?
        .checked_add(checked_arc_allocation_bytes::<
            ResolvedScreenPublicationDescriptor,
        >()?)
        .context("Windows prepared target metadata accounting overflow")
}

#[cfg(target_os = "windows")]
fn checked_arc_allocation_bytes<T>() -> Result<u64> {
    let (layout, _) = Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .context("Windows Arc allocation layout overflow")?;
    u64::try_from(layout.pad_to_align().size()).context("Windows Arc allocation exceeds u64")
}

#[cfg(target_os = "windows")]
pub(crate) fn is_retryable_native_screen_copy_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<D3d11On12ScreenInteropError>()
        .is_some_and(|error| {
            native_screen_copy_failure_policy(error) == NativeScreenCopyFailurePolicy::Retain
        })
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
pub(crate) const fn is_retryable_native_screen_copy_error(_error: &anyhow::Error) -> bool {
    false
}

#[cfg(target_os = "windows")]
impl ScreenNativeTargetPreparer for WindowsScreenTargetPreparer {
    fn quote_retained_bytes(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<u64> {
        let manifest = platform
            .downcast_ref::<GpuSurfaceTargetPreparation>()
            .context("Windows screen target received an unknown preparation manifest")?;
        validate_windows_target_manifest(descriptor, platform.plan_generation(), manifest)?;
        let bridge = self
            .bridge
            .upgrade()
            .context("Windows screen renderer was retired during target admission")?;
        let interop_bytes = bridge
            .interop
            .quote_target_retained_bytes(manifest)
            .context("failed to quote the renderer screen-copy target")?;
        interop_bytes
            .checked_add(prepared_windows_screen_target_metadata_bytes()?)
            .context("Windows prepared target retained-byte quote overflow")
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<ScreenNativeTargetPreparation> {
        let manifest = platform
            .downcast_ref::<GpuSurfaceTargetPreparation>()
            .context("Windows screen target received an unknown preparation manifest")?;
        validate_windows_target_manifest(descriptor, platform.plan_generation(), manifest)?;
        let bridge = self
            .bridge
            .upgrade()
            .context("Windows screen renderer was retired during target preparation")?;
        let interop = bridge
            .interop
            .prepare_target(manifest)
            .context("failed to prepare the renderer screen-copy target")?;
        let storage_id = NEXT_GPU_TEXTURE_STORAGE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("GPU texture storage identity space is exhausted"))?;
        let retained_bytes = interop
            .total_retained_bytes()
            .checked_add(prepared_windows_screen_target_metadata_bytes()?)
            .context("Windows prepared target retained-byte accounting overflow")?;
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

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn create_screen_bridge(
    device: &wgpu::Device,
    max_texture_dimension: u32,
) -> (
    Option<Arc<MacosScreenBridge>>,
    Option<ScreenNativeExecutionTarget>,
) {
    let interop = match MacosInteropScreenBridge::new(device) {
        Ok(bridge) => bridge,
        Err(error) => {
            tracing::debug!(%error, "renderer does not expose a Metal screen-import target");
            return (None, None);
        }
    };
    let reducer = match MacosNativeReducer::new(device) {
        Ok(reducer) => reducer,
        Err(error) => {
            tracing::debug!(%error, "renderer does not expose a native Metal screen reducer");
            return (None, None);
        }
    };
    let bridge = Arc::new(MacosScreenBridge {
        device: device.clone(),
        interop,
        reducer,
        storage_ids: Mutex::new(HashMap::new()),
        physical_targets: Mutex::new(Vec::new()),
    });
    let target = create_screen_target(&bridge, max_texture_dimension);
    (Some(bridge), target)
}

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
fn create_screen_target(
    bridge: &Arc<MacosScreenBridge>,
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
    Some(
        ScreenNativeExecutionTarget::new(
            ScreenNativeExecutionTargetId::new(
                NonZeroU64::new(target_id).expect("screen target identities start at one"),
            ),
            PlatformGpuApi::Metal,
            ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(bridge.interop.metal_registry_id()),
            NonZeroU32::new(max_texture_dimension)
                .expect("wgpu devices expose a non-zero texture dimension limit"),
            Arc::new(MacosScreenTargetPreparer {
                bridge: Arc::downgrade(bridge),
            }),
        )
        .with_color_capabilities(ScreenColorTransformCapabilities::new(
            true,
            true,
            true,
            LED_TONE_MAP_ALGORITHM_REVISION,
        )),
    )
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
    #[cfg(test)]
    snapshot_texture_allocation_count: Cell<usize>,
    #[cfg(test)]
    compositor_surface_allocation_count: Cell<usize>,
    #[cfg(test)]
    projected_bind_group_creation_count: Cell<usize>,
    #[cfg(test)]
    fail_next_projected_scene_preparation: Cell<bool>,
}

struct FrameInFlight {
    generation: u64,
    encoder: EncoderStage,
    readbacks: Vec<StagedReadback>,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    native_screen_leases: Vec<MacosScreenTextureLease>,
}

pub(super) struct StashedFrame {
    pub(super) encoder: wgpu::CommandEncoder,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(super) native_screen_leases: Vec<MacosScreenTextureLease>,
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
        #[cfg(all(target_os = "macos", feature = "screen-capture"))] native_screen_leases: Vec<
            MacosScreenTextureLease,
        >,
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases,
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases: Vec::new(),
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

    fn supersede(mut self, reason: &'static str) -> Option<StashedFrame> {
        let encoder = self.take_encoder_for_chaining();
        self.encoder = EncoderStage::Superseded;
        self.readbacks.clear();
        tracing::trace!(
            generation = self.generation,
            reason,
            "superseding deferred GPU frame"
        );
        encoder.map(|encoder| StashedFrame {
            encoder,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases: std::mem::take(&mut self.native_screen_leases),
        })
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    fn take_native_screen_leases(&mut self) -> Vec<MacosScreenTextureLease> {
        std::mem::take(&mut self.native_screen_leases)
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases: Vec::new(),
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
        let (screen_bridge, screen_target) =
            create_screen_bridge(&device, probe.max_texture_dimension_2d);
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
    pub(crate) fn screen_native_execution_target(&self) -> Option<&ScreenNativeExecutionTarget> {
        if !self.canvas_gpu_admitted {
            return None;
        }
        self.screen_target.as_ref()
    }

    pub(super) fn prepare_canvas_resize(
        &mut self,
        width: u32,
        height: u32,
        active_preview_request: Option<super::PreviewSurfaceRequest>,
    ) -> GpuCanvasPreparation {
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
        let preparation = (|| {
            let generation =
                self.prepare_gpu_canvas_generation(width, height, active_preview_request)?;
            let immutable_scene_snapshots = (0..IMMUTABLE_SCENE_GENERATIONS_IN_FLIGHT)
                .map(|_| GpuImmutableSceneSnapshot::try_new(&self.device, width, height))
                .collect::<Result<Vec<_>>>()?;
            let opaque_black_texture =
                GpuCompositorTexture::try_new(&self.device, 1, 1, "SparkleFlinger Opaque Black")
                    .context("GPU opaque-black base texture allocation failed")?;
            source::write_rgba_texture(
                &self.queue,
                &opaque_black_texture.texture,
                1,
                1,
                &[0, 0, 0, 255],
            );
            #[cfg(test)]
            self.snapshot_texture_allocation_count.set(
                self.snapshot_texture_allocation_count
                    .get()
                    .saturating_add(immutable_scene_snapshots.len()),
            );
            Ok::<_, anyhow::Error>(GpuCanvasPreparation::Gpu {
                generation,
                immutable_scene_snapshots,
                opaque_black_texture,
            })
        })();
        match preparation {
            Ok(preparation) => preparation,
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

    fn prepare_gpu_canvas_generation(
        &mut self,
        width: u32,
        height: u32,
        active_preview_request: Option<super::PreviewSurfaceRequest>,
    ) -> Result<GpuCanvasGeneration> {
        let surfaces = self
            .compositor_surface_cache
            .remove(&(width, height))
            .flatten()
            .map_or_else(|| self.try_create_compositor_surface_set(width, height), Ok)?;
        let sampling_readback_buffers =
            self.prepare_sampling_readback_buffers_for_canvas_resize(width, height)?;
        let preview_surfaces = active_preview_request
            .map(|request| {
                let scale_views = preview::preview_requires_scale(request, width, height)
                    .then_some((&surfaces.front.view, &surfaces.back.view));
                self.prepare_preview_surfaces_for_canvas_resize(width, height, request, scale_views)
            })
            .transpose()?;
        Ok(GpuCanvasGeneration {
            surfaces,
            preview_surfaces,
            sampling_readback_buffers,
        })
    }

    fn try_create_compositor_surface_set(
        &self,
        width: u32,
        height: u32,
    ) -> Result<GpuCompositorSurfaceSet> {
        let surfaces =
            GpuCompositorSurfaceSet::try_new(&self.device, &self.pipeline, width, height)?;
        #[cfg(test)]
        self.compositor_surface_allocation_count.set(
            self.compositor_surface_allocation_count
                .get()
                .saturating_add(1),
        );
        Ok(surfaces)
    }

    pub(super) fn apply_canvas_resize(&mut self, preparation: GpuCanvasPreparation) {
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("canvas resize committed"));
        self.discard_pending_uploads();
        let (surfaces, preview_surfaces, sampling_readback_buffers) = match preparation {
            GpuCanvasPreparation::Gpu {
                mut generation,
                immutable_scene_snapshots,
                opaque_black_texture,
            } => {
                if let Some(current) = self.surfaces.as_mut() {
                    std::mem::swap(
                        &mut current.screen_upload_pool,
                        &mut generation.surfaces.screen_upload_pool,
                    );
                }
                self.canvas_gpu_admitted = true;
                self.immutable_scene_snapshots = immutable_scene_snapshots;
                self.opaque_black_texture = Some(opaque_black_texture);
                (
                    Some(generation.surfaces),
                    generation.preview_surfaces,
                    generation.sampling_readback_buffers,
                )
            }
            GpuCanvasPreparation::CpuFallback => {
                self.canvas_gpu_admitted = false;
                self.immutable_scene_snapshots.clear();
                self.opaque_black_texture = None;
                (None, None, None)
            }
        };
        self.surfaces = surfaces;
        self.preview_surfaces = preview_surfaces;
        self.sampling_latch
            .install_buffers(sampling_readback_buffers);
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surfaces.clear();
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
        #[cfg(any(
            target_os = "windows",
            all(target_os = "macos", feature = "screen-capture")
        ))]
        self.release_native_screen_caches();
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

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(crate) fn copy_screen_publication(
        &mut self,
        publication: &Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        let Some(bridge) = self.screen_bridge.clone() else {
            return Ok(None);
        };
        let (surface, requires_work) = match publication.payload() {
            ScreenBranchPayload::GpuSurface(payload) => (payload.surface(), false),
            ScreenBranchPayload::NativeWork(payload) => (payload.source(), true),
            ScreenBranchPayload::Surface(_) | ScreenBranchPayload::Zones(_) => return Ok(None),
        };
        let capture_owner = surface
            .owner::<MacosCaptureFrame>()
            .context("native macOS screen publication has an unknown capture owner")?;
        let target_owner = surface
            .retained_owner::<PreparedMacosScreenTarget>()
            .context("native macOS screen publication has no prepared renderer target")?;
        let target_lifetime = surface
            .resource_lifetime()
            .cloned()
            .context("native macOS screen publication has no renderer allocation lifetime")?;
        let shared_target_lifetime = surface.shared_resource_lifetime().cloned();
        let capture_lifetime = surface
            .capture_resource_lifetime()
            .cloned()
            .context("native macOS screen publication has no capture allocation lifetime")?;
        let capture = capture_owner
            .downgrade()
            .upgrade()
            .context("native macOS capture owner retired before import")?;
        let import_started = Instant::now();
        let imported = bridge.import_frame(&self.device, target_owner.resource_generation, capture);
        if let Some(timing_sink) = surface.timing_sink() {
            timing_sink.record_import(import_started.elapsed());
        }
        let (imported, storage_id) = match imported {
            Ok(imported) => imported,
            Err(error) => {
                self.release_native_screen_caches();
                return Err(error);
            }
        };
        anyhow::ensure!(
            imported.capture().storage_extent.width == surface.extent().width()
                && imported.capture().storage_extent.height == surface.extent().height(),
            "native macOS imported extent does not match the published surface"
        );
        let content_generation = imported.content_sequence();
        let descriptor = &target_owner.descriptor;
        let native_screen_submission_lease = MacosScreenTextureLease::new(
            imported.clone(),
            capture_owner.clone(),
            target_owner.clone(),
            target_lifetime.clone(),
            shared_target_lifetime.clone(),
            capture_lifetime.clone(),
        );
        let (width, height, storage_id, texture, view) = if requires_work {
            self.flush_pending_output_submission()?;
            let reduction_started = Instant::now();
            let mut submitted_native_reduction = false;
            let physical = target_owner
                .physical
                .as_ref()
                .context("native macOS work has no prepared physical target")?;
            let mut physical_sequence = physical
                .content_sequence
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *physical_sequence != Some(content_generation) {
                let mut encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("SparkleFlinger macOS native screen reduction"),
                        });
                let reduction = bridge.reducer.encode(
                    &imported,
                    &physical.target,
                    macos_reduction_descriptor(descriptor)?,
                    &mut encoder,
                );
                if let Err(error) = reduction {
                    self.release_native_screen_caches();
                    return Err(error.into());
                }
                let submission_index = self.queue.submit(Some(encoder.finish()));
                self.retire_native_screen_leases(
                    submission_index,
                    vec![native_screen_submission_lease.clone()],
                );
                submitted_native_reduction = true;
                *physical_sequence = Some(content_generation);
            }
            drop(physical_sequence);

            let target = if let Some(logical_target) = target_owner.logical_target.as_ref() {
                let mut logical_sequence = target_owner
                    .logical_content_sequence
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *logical_sequence != Some(content_generation) {
                    let geometry = descriptor.geometry();
                    let mut encoder =
                        self.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("SparkleFlinger macOS native screen materialization"),
                            });
                    let materialization = bridge.reducer.encode_materialization(
                        &physical.target,
                        logical_target,
                        [
                            geometry.content_x(),
                            geometry.content_y(),
                            geometry.content_extent().width(),
                            geometry.content_extent().height(),
                        ],
                        macos_native_letterbox_fill(descriptor)?,
                        &mut encoder,
                    );
                    if let Err(error) = materialization {
                        self.release_native_screen_caches();
                        return Err(error.into());
                    }
                    let submission_index = self.queue.submit(Some(encoder.finish()));
                    self.retire_native_screen_leases(
                        submission_index,
                        vec![native_screen_submission_lease.clone()],
                    );
                    submitted_native_reduction = true;
                    *logical_sequence = Some(content_generation);
                }
                (
                    logical_target.width(),
                    logical_target.height(),
                    target_owner
                        .logical_storage_id
                        .context("logical macOS target has no storage identity")?,
                    logical_target.texture().clone(),
                    logical_target.view().clone(),
                )
            } else {
                (
                    physical.target.width(),
                    physical.target.height(),
                    physical.storage_id,
                    physical.target.texture().clone(),
                    physical.target.view().clone(),
                )
            };
            if submitted_native_reduction && let Some(timing_sink) = surface.timing_sink() {
                timing_sink.record_native_reduction_submission(reduction_started.elapsed());
            }
            target
        } else {
            let extent = descriptor.geometry().output_extent();
            anyhow::ensure!(
                surface.extent() == extent,
                "native macOS identity surface extent does not match its target"
            );
            (
                extent.width(),
                extent.height(),
                storage_id,
                imported
                    .texture()
                    .context("native macOS identity publication has no wgpu texture")?
                    .as_ref()
                    .clone(),
                imported
                    .view()
                    .context("native macOS identity publication has no wgpu texture view")?
                    .as_ref()
                    .clone(),
            )
        };
        Ok(Some(GpuTextureFrame {
            width,
            height,
            storage_id,
            content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture,
            view,
            immutable_lease: None,
            macos_screen_lease: Some(MacosScreenTextureLease::new(
                imported,
                capture_owner,
                target_owner,
                target_lifetime,
                shared_target_lifetime,
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

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    fn stage_frame_in_flight_with_native_screen_leases(
        &mut self,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
        native_screen_leases: Vec<MacosScreenTextureLease>,
    ) {
        debug_assert!(
            self.frame_in_flight.is_none(),
            "deferred GPU frame must be submitted or superseded before replacement"
        );
        self.frame_in_flight = Some(FrameInFlight::building(
            self.output_generation,
            encoder,
            preview_readback,
            native_screen_leases,
        ));
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    fn retire_native_screen_leases(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
        leases: Vec<MacosScreenTextureLease>,
    ) {
        self.native_screen_lease_retirements
            .retire(submission_index, leases);
        self.release_completed_native_screen_leases();
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    fn release_completed_native_screen_leases(&mut self) {
        let device = &self.device;
        self.native_screen_lease_retirements
            .release_completed(|submission_index| {
                match device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission_index.clone()),
                    timeout: Some(std::time::Duration::ZERO),
                }) {
                    Ok(_) => true,
                    Err(wgpu::PollError::Timeout) => false,
                    Err(error) => {
                        tracing::debug!(%error, "GPU native screen lease retirement poll failed");
                        false
                    }
                }
            });
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    fn wait_for_native_screen_lease_retirements(&mut self) {
        while let Some(submission_index) = self
            .native_screen_lease_retirements
            .front_submission()
            .cloned()
        {
            match self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(submission_index),
                timeout: None,
            }) {
                Ok(_) => self.native_screen_lease_retirements.release_front(),
                Err(error) => {
                    tracing::debug!(%error, "GPU stopped before native screen lease retirement");
                    self.native_screen_lease_retirements.release_front();
                }
            }
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
