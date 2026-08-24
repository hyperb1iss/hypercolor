#[cfg(test)]
mod tests;

use std::alloc::Layout;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use anyhow::{Context, Result};
use hypercolor_core::input::screen::implementer::{
    CaptureColorSpace, CaptureDynamicRange, CapturePixelFormat, CaptureTransferFunction,
    PlatformGpuApi,
};
use hypercolor_core::input::screen::planner::{
    ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor, ScreenCaptureBackend,
    ScreenCursorPolicy, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
    ScreenNativePreparationPayload, ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanGeneration, ScreenPublicationKind,
    ScreenReductionFilter, ScreenResourceApi,
};
use hypercolor_windows_capture::{
    GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy, GpuSurfaceFilter,
    GpuSurfaceFormat, GpuSurfaceSourceColorSpace, GpuSurfaceTargetPreparation,
};
use hypercolor_windows_gpu_interop::{
    D3d11On12ScreenBridge, D3d11On12ScreenInteropError, PreparedScreenCopyTarget,
};

use hypercolor_core::input::screen::implementer::{ScreenBranchPayload, ScreenBranchPublication};
use hypercolor_windows_capture::GpuSurfacePublication;

use super::super::{GpuSparkleFlinger, NEXT_GPU_TEXTURE_STORAGE_ID, NEXT_SCREEN_TARGET_ID};
use super::{InstalledNativeScreen, NativeScreenBridge, NativeScreenCopyOutcome};
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameOrigin, NativeScreenCacheRetention, NativeScreenTextureLease,
};

pub(super) struct WindowsScreenBridge {
    interop: D3d11On12ScreenBridge,
}

struct WindowsScreenTargetPreparer {
    bridge: Weak<WindowsScreenBridge>,
}

pub(super) struct PreparedWindowsScreenTarget {
    pub(super) interop: PreparedScreenCopyTarget,
    pub(super) storage_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NativeScreenCopyFailurePolicy {
    Retain,
    Reprepare,
    InvalidateFrameAndReprepare,
}

pub(super) fn native_screen_copy_failure_policy(
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

fn native_screen_copy_error_invalidates_frame(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<D3d11On12ScreenInteropError>()
        .is_some_and(|error| {
            native_screen_copy_failure_policy(error)
                == NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare
        })
}

pub(super) fn screen_storage_requires_cache_turnover(current: Option<u64>, next: u64) -> bool {
    current != Some(next)
}

pub(super) fn validate_windows_plan_generation(core: u64, native: u64) -> Result<()> {
    anyhow::ensure!(
        core == native,
        "Windows target manifest plan generation does not match the candidate"
    );
    Ok(())
}

fn prepared_windows_screen_target_metadata_bytes() -> Result<u64> {
    checked_arc_allocation_bytes::<PreparedWindowsScreenTarget>()?
        .checked_add(checked_arc_allocation_bytes::<
            ResolvedScreenPublicationDescriptor,
        >()?)
        .context("Windows prepared target metadata accounting overflow")
}

fn checked_arc_allocation_bytes<T>() -> Result<u64> {
    let (layout, _) = Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .context("Windows Arc allocation layout overflow")?;
    u64::try_from(layout.pad_to_align().size()).context("Windows Arc allocation exceeds u64")
}

fn is_retryable_native_screen_copy_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<D3d11On12ScreenInteropError>()
        .is_some_and(|error| {
            native_screen_copy_failure_policy(error) == NativeScreenCopyFailurePolicy::Retain
        })
}

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
        source.resources().backend() == &ScreenCaptureBackend::DesktopDuplication
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

/// DXGI desktop duplication copies into a daemon-owned D3D11on12 texture.
pub(super) struct WindowsScreenHost {
    bridge: Arc<WindowsScreenBridge>,
    /// Storage identity of the last prepared target, so cache turnover
    /// happens exactly when the renderer target changes.
    storage_id: Option<u64>,
}

pub(super) fn install(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    max_texture_dimension: u32,
) -> InstalledNativeScreen {
    let interop = match D3d11On12ScreenBridge::new(device.clone(), queue.clone()) {
        Ok(bridge) => bridge,
        Err(error) => {
            tracing::debug!(%error, "renderer does not expose a DX12 screen-copy target");
            return InstalledNativeScreen::none();
        }
    };
    let bridge = Arc::new(WindowsScreenBridge { interop });
    let target = create_screen_target(&bridge, max_texture_dimension);
    InstalledNativeScreen {
        bridge: Some(Box::new(WindowsScreenHost {
            bridge,
            storage_id: None,
        })),
        target,
    }
}

impl WindowsScreenHost {
    fn reprepare_target(&mut self, gpu: &mut GpuSparkleFlinger) {
        self.storage_id = None;
        gpu.release_native_screen_bind_groups();
        gpu.release_completed_native_screen_leases();
        gpu.screen_target = create_screen_target(&self.bridge, gpu.probe.max_texture_dimension_2d);
    }

    fn try_copy(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        publication: &Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
            return Ok(None);
        };
        let Some(native) = payload.surface().owner::<GpuSurfacePublication>() else {
            self.reprepare_target(gpu);
            anyhow::bail!("native screen publication has an unknown platform owner");
        };
        let Some(prepared) = payload
            .surface()
            .retained_owner::<PreparedWindowsScreenTarget>()
        else {
            self.reprepare_target(gpu);
            anyhow::bail!("native screen publication has no prepared renderer target");
        };
        let Some(target_lifetime) = payload.surface().resource_lifetime().cloned() else {
            self.reprepare_target(gpu);
            anyhow::bail!("native screen publication has no renderer allocation lifetime");
        };
        let Some(capture_lifetime) = payload.surface().capture_resource_lifetime().cloned() else {
            self.reprepare_target(gpu);
            anyhow::bail!("native screen publication has no capture allocation lifetime");
        };
        let copy = match self
            .bridge
            .interop
            .copy_publication(&prepared.interop, &native)
        {
            Ok(copy) => copy,
            Err(error) => {
                return match native_screen_copy_failure_policy(&error) {
                    NativeScreenCopyFailurePolicy::Retain => {
                        Err(error).context("native screen publication is not ready")
                    }
                    NativeScreenCopyFailurePolicy::Reprepare => {
                        self.reprepare_target(gpu);
                        Err(error).context("failed to copy the native screen publication")
                    }
                    NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare => {
                        self.reprepare_target(gpu);
                        Err(error).context("native screen target contents became uncertain")
                    }
                };
            }
        };
        if screen_storage_requires_cache_turnover(self.storage_id, prepared.storage_id) {
            self.storage_id = None;
            gpu.release_native_screen_bind_groups();
            gpu.release_completed_native_screen_leases();
            self.storage_id = Some(prepared.storage_id);
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
            // The DXGI copy lands in a daemon-owned texture, so cached bind
            // groups only need the target allocation to stay alive.
            native_screen_lease: Some(NativeScreenTextureLease::new(
                (copy, capture_lifetime),
                target_lifetime,
                NativeScreenCacheRetention::TargetLifetime,
            )),
        }))
    }
}

impl NativeScreenBridge for WindowsScreenHost {
    fn copy_screen_publication(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        publication: &Arc<ScreenBranchPublication>,
    ) -> NativeScreenCopyOutcome {
        match self.try_copy(gpu, publication) {
            Ok(Some(frame)) => NativeScreenCopyOutcome::Copied(frame),
            Ok(None) => NativeScreenCopyOutcome::Ignored,
            Err(error) if is_retryable_native_screen_copy_error(&error) => {
                NativeScreenCopyOutcome::Deferred(error)
            }
            Err(error) if native_screen_copy_error_invalidates_frame(&error) => {
                NativeScreenCopyOutcome::Invalidated(error)
            }
            Err(error) => NativeScreenCopyOutcome::Failed(error),
        }
    }

    fn release_caches(&mut self) {
        self.storage_id = None;
    }

    #[cfg(test)]
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

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
