#[cfg(all(feature = "screen-capture", target_os = "macos"))]
use std::num::{NonZeroU32, NonZeroU64};
#[cfg(any(
    all(feature = "servo-gpu-import", target_os = "linux"),
    all(feature = "servo-gpu-import", target_os = "macos"),
    all(feature = "screen-capture", target_os = "macos")
))]
use std::sync::Arc;
use std::sync::mpsc;

use hypercolor_core::blend_math::encode_srgb_channel;
#[cfg(all(feature = "screen-capture", target_os = "macos"))]
use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureEpoch, CaptureGeometry,
    CaptureLuminanceContext, CapturePixelFormat, CapturePositiveScalar, CaptureRotation,
    CaptureSourceId, CaptureTransferFunction, InputPublicationDemandRevision,
    KnownCaptureColorimetry, LedToneMapCalibration, PhysicalOrigin, PixelExtent, PlatformGpuApi,
    PlatformGpuSurface, PreparedLedToneMap, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenByteAdmissionCoordinator, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenExecutorColorCapabilities, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenLetterboxFill, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
    ScreenNativePreparationPayload, ScreenNativeRetentionQuote, ScreenNativeTargetPreparation,
    ScreenNativeTargetPreparer, ScreenPhysicalGpuDeviceIdentity, ScreenPlanBuilder,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenPublicationExecutor,
    ScreenPublicationExecutorRequest, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenPublicationSlotPolicy, ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector,
    ScreenUpscalePolicy, ScreenWorkerExactLedgerBuilder, SourceScale,
};
use hypercolor_core::spatial::SpatialEngine;
#[cfg(all(feature = "screen-capture", target_os = "macos"))]
use hypercolor_macos_capture::{
    MacosCaptureColorimetry, MacosCaptureFrame, MacosCaptureGeometry, MacosCapturePixelFormat,
    MacosCaptureSurface, MacosChromaLocation, MacosColorPrimaries, MacosColorRange,
    MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosScale, MacosTransferFunction,
    MacosYuvMatrix,
};
use hypercolor_types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::{DeviceId, DisplayFrameFormat};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::ZoneId;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use hypercolor_types::viewport::FitMode;
#[cfg(target_os = "windows")]
use hypercolor_windows_gpu_interop::D3d11On12ScreenInteropError;

#[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
use super::CachedGpuSourceCopy;
use super::compositor::{ComposeShaderMode, encode_compose_params};
use super::screen_upload::{
    ScreenPublicationUploadPool, ScreenUploadContentKey, ScreenUploadPoolSaturated,
    ScreenUploadResidencyPolicy, resident_frame_bytes,
};
use super::{
    DISPLAY_FINALIZE_READBACK_SLOT_COUNT, DisplayYuv420Frame, FrameInFlight, GpuCanvasAdmission,
    GpuCanvasFallbackReason, GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFrame,
    GpuSparkleFlinger, GpuZoneSamplingDispatch, MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES,
    MEDIA_UPLOAD_TEXTURE_RING_LEN, MediaTextureSourceKey, MediaUploadTextureKey, PendingPreviewMap,
    PendingPreviewReadback, ensure_readback_buffer_capacity, ensure_storage_buffer_capacity,
    gpu_canvas_admission,
};
#[cfg(all(feature = "screen-capture", target_os = "macos"))]
use super::{
    MacosNativeColorTransform, MacosNativeOutputTransfer, MacosNativeReductionDescriptor,
    MacosNativeReductionFilter, MacosNativeTargetFormat, PreparedMacosScreenTarget,
    UnsupportedMacosNativeTargetFormat, macos_native_target_format,
    native_screen_copy_error_invalidates_frame,
};
#[cfg(target_os = "windows")]
use super::{
    NativeScreenCopyFailurePolicy, native_screen_copy_failure_policy,
    screen_storage_requires_cache_turnover, validate_windows_plan_generation,
};
use crate::performance::CompositorBackendKind;
#[cfg(all(feature = "screen-capture", target_os = "macos"))]
use crate::render_thread::producer_queue::MacosScreenTextureLease;
use crate::render_thread::producer_queue::{GpuTextureFrame, GpuTextureFrameOrigin, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu_sampling::GpuSamplingPlan;
use crate::render_thread::sparkleflinger::{
    CompositionLayer, CompositionPlan, CompositionTransform, DisplayFinalizeCacheKey,
    DisplayFinalizeParams, PreviewSurfaceRequest, SparkleFlinger, SparkleFlingerBackend,
    cpu::CpuSparkleFlinger,
};

#[test]
fn compose_params_encode_target_space_as_float_flag() {
    let layer = CompositionLayer::replace(ProducerFrame::Canvas(Canvas::new(2, 2))).with_transform(
        CompositionTransform {
            anchor: NormalizedPosition::new(0.5, 0.5),
            scale: [0.5, 0.5],
            rotation: 0.0,
            fit: FitMode::Stretch,
            sample_target_space: true,
        },
    );
    let params = encode_compose_params(4, 4, ComposeShaderMode::Replace, &layer, false);
    let flag = f32::from_le_bytes(
        params[60..64]
            .try_into()
            .expect("flag should occupy four bytes"),
    );

    assert_eq!(flag, 1.0);
}

fn solid_canvas(color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(4, 4);
    canvas.fill(color);
    canvas
}

fn required_gpu<T>(result: anyhow::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "required real-GPU test environment is unavailable: {error:#}"
            );
            eprintln!(
                "real-GPU test skipped because no compatible adapter is available: {error:#}"
            );
            None
        }
    }
}

fn gpu_test_compositor() -> Option<GpuSparkleFlinger> {
    required_gpu(GpuSparkleFlinger::new())
}

fn gpu_test_sparkleflinger() -> Option<SparkleFlinger> {
    required_gpu(SparkleFlinger::new(RenderAccelerationMode::Gpu))
}

fn bypass_surface_plan(width: u32, height: u32) -> CompositionPlan {
    CompositionPlan::single(
        width,
        height,
        CompositionLayer::replace(ProducerFrame::Surface(slot_surface_with_size(
            width,
            height,
            Rgba::new(32, 96, 224, 255),
        ))),
    )
}

fn layered_surface_plan(width: u32, height: u32) -> CompositionPlan {
    CompositionPlan::with_layers(
        width,
        height,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface_with_size(
                width,
                height,
                Rgba::new(240, 48, 96, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface_with_size(
                    width,
                    height,
                    Rgba::new(24, 128, 208, 255),
                )),
                0.35,
            ),
        ],
    )
}

fn gpu_backend_mut(sparkleflinger: &mut SparkleFlinger) -> &mut GpuSparkleFlinger {
    match &mut sparkleflinger.backend {
        SparkleFlingerBackend::Gpu { gpu, .. } => gpu,
        SparkleFlingerBackend::Cpu(_) => panic!("test requires the GPU backend"),
    }
}

#[test]
fn gpu_canvas_admission_uses_only_texture_capability() {
    assert!(matches!(
        gpu_canvas_admission(16_384, 3840, 2160),
        GpuCanvasAdmission::Gpu
    ));
    assert_eq!(
        gpu_canvas_admission(8192, 8193, 1),
        GpuCanvasAdmission::CpuFallback(GpuCanvasFallbackReason::TextureDimension)
    );
    assert!(ensure_readback_buffer_capacity(1024, 128, 3, true).is_err());
    assert_eq!(
        ensure_readback_buffer_capacity(1536, 128, 3, true)
            .expect("readback should fit its exact device limit"),
        1536
    );
    assert!(ensure_storage_buffer_capacity(1024, 32, 16).is_err());
    assert_eq!(
        ensure_storage_buffer_capacity(2048, 32, 16)
            .expect("storage output should fit its exact binding limit"),
        2048
    );
}

#[test]
fn frame_boundary_preview_preparation_failure_preserves_active_generation() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let initial = compositor.prepare_canvas_resize(4, 4, None);
    assert!(initial.is_admitted());
    compositor.apply_canvas_resize(initial);
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
    compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("active preview generation should compose");
    assert!(compositor.has_pending_output_submission());

    compositor.fail_next_preview_scale_output_preparation();
    let rejected = compositor.prepare_canvas_resize(
        8,
        8,
        Some(PreviewSurfaceRequest {
            width: 2,
            height: 2,
        }),
    );
    assert!(!rejected.is_admitted());
    assert_eq!(
        compositor
            .surface_snapshot()
            .map(|snapshot| (snapshot.width, snapshot.height)),
        Some((4, 4))
    );
    assert!(compositor.has_pending_output_submission());
    assert!(
        compositor
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == 2 && surfaces.height == 2 && surfaces.has_scale_output()
            })
    );

    let prepared = compositor.prepare_canvas_resize(
        8,
        8,
        Some(PreviewSurfaceRequest {
            width: 2,
            height: 2,
        }),
    );
    assert!(prepared.is_admitted());
    compositor.apply_canvas_resize(prepared);
    assert_eq!(
        compositor
            .surface_snapshot()
            .map(|snapshot| (snapshot.width, snapshot.height)),
        Some((8, 8))
    );
    assert!(
        compositor
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == 2 && surfaces.height == 2 && surfaces.has_scale_output()
            })
    );
    assert!(!compositor.has_pending_output_submission());
}

#[test]
fn frame_boundary_equal_extent_preview_preserves_concrete_request() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let initial = compositor.prepare_canvas_resize(64, 4, None);
    assert!(initial.is_admitted());
    compositor.apply_canvas_resize(initial);
    let fixed_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let plan = CompositionPlan::with_layers(
        64,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface_with_size(
                64,
                4,
                Rgba::new(255, 32, 0, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface_with_size(64, 4, Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    compositor
        .compose(&plan, false, Some(fixed_request))
        .expect("equal-extent preview should compose");
    assert!(
        compositor
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| !surfaces.has_scale_output())
    );

    compositor.fail_next_preview_scale_output_preparation();
    let rejected = compositor.prepare_canvas_resize(128, 8, Some(fixed_request));
    assert!(!rejected.is_admitted());
    assert_eq!(
        compositor
            .surface_snapshot()
            .map(|snapshot| (snapshot.width, snapshot.height)),
        Some((64, 4))
    );

    let prepared = compositor.prepare_canvas_resize(128, 8, Some(fixed_request));
    assert!(prepared.is_admitted());
    compositor.apply_canvas_resize(prepared);
    assert!(
        compositor
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == fixed_request.width
                    && surfaces.height == fixed_request.height
                    && surfaces.has_scale_output()
            })
    );

    compositor.fail_next_preview_scale_output_preparation();
    let resized_plan = CompositionPlan::with_layers(
        128,
        8,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface_with_size(
                128,
                8,
                Rgba::new(24, 72, 160, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface_with_size(128, 8, Rgba::new(210, 48, 96, 255))),
                0.25,
            ),
        ],
    );
    compositor
        .compose(&resized_plan, false, Some(fixed_request))
        .expect("prepared fixed preview should not allocate after resize acceptance");
    assert!(compositor.fail_next_preview_scale_output_preparation);
    compositor.discard_preview_work();
}

#[test]
fn active_preview_request_bypass_is_prepared_before_resize() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let initial = sparkleflinger
        .prepare_canvas_resize(64, 4)
        .expect("initial GPU canvas should prepare");
    assert!(initial.is_admitted());
    sparkleflinger.apply_canvas_resize(initial);
    let fixed_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let composed =
        sparkleflinger.compose_for_outputs(bypass_surface_plan(64, 4), false, Some(fixed_request));
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert_eq!(sparkleflinger.active_preview_request, Some(fixed_request));
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .is_none()
    );

    gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation();
    let rejected = sparkleflinger
        .prepare_canvas_resize(128, 8)
        .expect("CPU fallback should prepare after injected GPU rejection");
    assert!(!rejected.is_admitted());
    assert_eq!(sparkleflinger.active_preview_request, Some(fixed_request));
    assert_eq!(
        gpu_backend_mut(&mut sparkleflinger)
            .surface_snapshot()
            .map(|snapshot| (snapshot.width, snapshot.height)),
        Some((64, 4))
    );

    let prepared = sparkleflinger
        .prepare_canvas_resize(128, 8)
        .expect("retry should prepare the complete GPU generation");
    assert!(prepared.is_admitted());
    sparkleflinger.apply_canvas_resize(prepared);
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == fixed_request.width
                    && surfaces.height == fixed_request.height
                    && surfaces.has_scale_output()
            })
    );

    gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation();
    let composed =
        sparkleflinger.compose_for_outputs(bypass_surface_plan(128, 8), false, Some(fixed_request));
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert!(!composed.gpu_readback_failed);
    assert!(gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation);
    gpu_backend_mut(&mut sparkleflinger).discard_preview_work();
}

#[test]
fn active_preview_request_successful_none_clears() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let initial = sparkleflinger
        .prepare_canvas_resize(64, 4)
        .expect("initial GPU canvas should prepare");
    assert!(initial.is_admitted());
    sparkleflinger.apply_canvas_resize(initial);
    let request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let plan = bypass_surface_plan(64, 4);
    let composed = sparkleflinger.compose_for_outputs(plan.clone(), false, Some(request));
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert_eq!(sparkleflinger.active_preview_request, Some(request));

    let composed = sparkleflinger.compose_for_outputs(plan, false, None);
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert_eq!(sparkleflinger.active_preview_request, None);

    gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation();
    assert!(
        sparkleflinger
            .prepare_canvas_resize(128, 8)
            .expect("GPU resize without preview should prepare")
            .is_admitted()
    );
    assert!(gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation);
}

#[test]
fn active_preview_request_failed_compose_retains_last_good() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let initial = sparkleflinger
        .prepare_canvas_resize(64, 4)
        .expect("initial GPU canvas should prepare");
    assert!(initial.is_admitted());
    sparkleflinger.apply_canvas_resize(initial);
    let retained_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let composed = sparkleflinger.compose_for_outputs(
        bypass_surface_plan(64, 4),
        false,
        Some(retained_request),
    );
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);

    let mut canvas = Canvas::new(64, 4);
    canvas.fill(Rgba::new(80, 160, 224, 255));
    let gpu_frame = sparkleflinger
        .upload_canvas_frame(&canvas)
        .expect("GPU frame should upload");
    gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation();
    let rejected_request = PreviewSurfaceRequest {
        width: 32,
        height: 2,
    };
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(
            64,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::GpuTexture(gpu_frame)),
                CompositionLayer::alpha(
                    ProducerFrame::Surface(slot_surface_with_size(
                        64,
                        4,
                        Rgba::new(224, 48, 112, 255),
                    )),
                    0.25,
                ),
            ],
        ),
        false,
        Some(rejected_request),
    );
    assert_eq!(composed.backend, CompositorBackendKind::GpuFallback);
    assert!(composed.gpu_readback_failed);
    assert_eq!(
        sparkleflinger.active_preview_request,
        Some(retained_request)
    );
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .is_none()
    );
}

#[test]
fn active_preview_request_upload_saturation_rejects_replacement() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let retained_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let composed = sparkleflinger.compose_for_outputs(
        bypass_surface_plan(64, 4),
        false,
        Some(retained_request),
    );
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);

    gpu_backend_mut(&mut sparkleflinger).fail_next_screen_upload_pool_saturation();
    let replacement_request = PreviewSurfaceRequest {
        width: 32,
        height: 2,
    };
    let retained = sparkleflinger.compose_for_outputs(
        layered_surface_plan(64, 4),
        false,
        Some(replacement_request),
    );

    assert_eq!(retained.backend, CompositorBackendKind::Gpu);
    assert_eq!(
        sparkleflinger.active_preview_request,
        Some(retained_request)
    );
}

#[test]
fn active_preview_request_upload_saturation_rejects_clear() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let retained_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let composed = sparkleflinger.compose_for_outputs(
        bypass_surface_plan(64, 4),
        false,
        Some(retained_request),
    );
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);

    gpu_backend_mut(&mut sparkleflinger).fail_next_screen_upload_pool_saturation();
    let retained = sparkleflinger.compose_for_outputs(layered_surface_plan(64, 4), false, None);

    assert_eq!(retained.backend, CompositorBackendKind::Gpu);
    assert_eq!(
        sparkleflinger.active_preview_request,
        Some(retained_request)
    );
}

#[test]
fn active_preview_request_replaces_stale_surface_on_resize() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let initial = sparkleflinger
        .prepare_canvas_resize(64, 4)
        .expect("initial GPU canvas should prepare");
    assert!(initial.is_admitted());
    sparkleflinger.apply_canvas_resize(initial);
    let stale_request = PreviewSurfaceRequest {
        width: 32,
        height: 2,
    };
    let composed =
        sparkleflinger.compose_for_outputs(layered_surface_plan(64, 4), false, Some(stale_request));
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == stale_request.width && surfaces.height == stale_request.height
            })
    );

    let active_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let composed =
        sparkleflinger.compose_for_outputs(bypass_surface_plan(64, 4), false, Some(active_request));
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert_eq!(sparkleflinger.active_preview_request, Some(active_request));
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == stale_request.width && surfaces.height == stale_request.height
            })
    );

    let prepared = sparkleflinger
        .prepare_canvas_resize(128, 8)
        .expect("resize should prepare the authoritative request");
    assert!(prepared.is_admitted());
    sparkleflinger.apply_canvas_resize(prepared);
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == active_request.width
                    && surfaces.height == active_request.height
                    && surfaces.has_scale_output()
            })
    );
}

#[test]
fn active_preview_request_cpu_fallback_change_survives_gpu_readmission() {
    let Some(mut sparkleflinger) = gpu_test_sparkleflinger() else {
        return;
    };
    let initial = sparkleflinger
        .prepare_canvas_resize(64, 4)
        .expect("initial GPU canvas should prepare");
    assert!(initial.is_admitted());
    sparkleflinger.apply_canvas_resize(initial);
    let old_request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };
    let composed =
        sparkleflinger.compose_for_outputs(bypass_surface_plan(64, 4), false, Some(old_request));
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);

    let fallback_width = sparkleflinger
        .max_texture_dimension_2d()
        .expect("GPU backend should expose its texture limit")
        .checked_add(1)
        .expect("GPU texture limit should leave a CPU fallback extent");
    let fallback = sparkleflinger
        .prepare_canvas_resize(fallback_width, 1)
        .expect("CPU fallback canvas should prepare");
    assert!(!fallback.is_admitted());
    sparkleflinger.apply_canvas_resize(fallback);
    let fallback_request = PreviewSurfaceRequest {
        width: 32,
        height: 1,
    };
    let composed = sparkleflinger.compose_for_outputs(
        bypass_surface_plan(fallback_width, 1),
        false,
        Some(fallback_request),
    );
    assert_eq!(composed.backend, CompositorBackendKind::GpuFallback);
    assert!(!composed.gpu_readback_failed);
    assert_eq!(
        sparkleflinger.active_preview_request,
        Some(fallback_request)
    );

    gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation();
    let rejected = sparkleflinger
        .prepare_canvas_resize(128, 8)
        .expect("CPU fallback should prepare after injected GPU rejection");
    assert!(!rejected.is_admitted());

    let prepared = sparkleflinger
        .prepare_canvas_resize(128, 8)
        .expect("GPU re-admission should prepare the fallback request");
    assert!(prepared.is_admitted());
    sparkleflinger.apply_canvas_resize(prepared);
    assert!(
        gpu_backend_mut(&mut sparkleflinger)
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| {
                surfaces.width == fallback_request.width
                    && surfaces.height == fallback_request.height
                    && surfaces.has_scale_output()
            })
    );

    gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation();
    let composed = sparkleflinger.compose_for_outputs(
        bypass_surface_plan(128, 8),
        false,
        Some(fallback_request),
    );
    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert!(!composed.gpu_readback_failed);
    assert!(gpu_backend_mut(&mut sparkleflinger).fail_next_preview_scale_output_preparation);
    gpu_backend_mut(&mut sparkleflinger).discard_preview_work();
}

#[test]
fn frame_boundary_sampling_preparation_failure_preserves_active_generation() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let initial = compositor.prepare_canvas_resize(4, 4, None);
    assert!(initial.is_admitted());
    compositor.apply_canvas_resize(initial);
    let frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(90), &patterned_canvas(37))
        .expect("sampling source should upload");
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(frame)),
    );
    compositor
        .compose(&plan, true, None)
        .expect("active sampling generation should compose");
    assert_eq!(compositor.sampling_latch.buffer_extent(), Some((4, 4)));

    compositor.fail_next_sampling_readback_preparation();
    let rejected = compositor.prepare_canvas_resize(8, 8, None);
    assert!(!rejected.is_admitted());
    assert_eq!(
        compositor
            .surface_snapshot()
            .map(|snapshot| (snapshot.width, snapshot.height)),
        Some((4, 4))
    );
    assert_eq!(compositor.sampling_latch.buffer_extent(), Some((4, 4)));

    let prepared = compositor.prepare_canvas_resize(8, 8, None);
    assert!(prepared.is_admitted());
    compositor.apply_canvas_resize(prepared);
    assert_eq!(
        compositor
            .surface_snapshot()
            .map(|snapshot| (snapshot.width, snapshot.height)),
        Some((8, 8))
    );
    assert_eq!(compositor.sampling_latch.buffer_extent(), Some((8, 8)));
}

#[test]
fn frame_boundary_resize_retains_prepared_screen_upload_descriptors() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let initial = compositor.prepare_canvas_resize(4, 4, None);
    assert!(initial.is_admitted());
    compositor.apply_canvas_resize(initial);
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let pixels = vec![53_u8; 3 * 2 * 4];
    let content_key = screen_upload_key(71, 5, 3, 2);
    let original_storage_id = {
        let pool = &mut compositor
            .surfaces
            .as_mut()
            .expect("initial GPU generation should be installed")
            .screen_upload_pool;
        let (texture, uploaded) = pool
            .upload_rgba(&device, &queue, 3, 2, &pixels, content_key, |_| {})
            .expect("screen descriptor should prepare before resize");
        assert!(uploaded);
        pool.discard_encoding();
        pool.fail_next_allocation();
        texture.storage_id
    };

    let prepared = compositor.prepare_canvas_resize(8, 8, None);
    assert!(prepared.is_admitted());
    compositor.apply_canvas_resize(prepared);
    let pool = &mut compositor
        .surfaces
        .as_mut()
        .expect("resized GPU generation should be installed")
        .screen_upload_pool;
    let (retained, uploaded) = pool
        .upload_rgba(&device, &queue, 3, 2, &pixels, content_key, |_| {})
        .expect("known screen descriptor should be ready without allocation");
    assert!(!uploaded);
    assert_eq!(retained.storage_id, original_storage_id);
    assert!(pool.allocation_failure_is_armed());
    let error = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &pixels,
            screen_upload_key(72, 6, 3, 2),
            |_| {},
        )
        .expect_err("unknown descriptor should consume the armed allocation failure");
    assert!(
        error
            .to_string()
            .contains("injected screen upload texture preparation failure")
    );
    pool.discard_encoding();
}

#[test]
fn texture_composition_survives_scoped_full_frame_readback_rejection() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    compositor.max_buffer_size = 4;
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(PublishedSurface::from_owned_canvas(
                solid_canvas(Rgba::new(20, 60, 100, 255)),
                1,
                1,
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(PublishedSurface::from_owned_canvas(
                    solid_canvas(Rgba::new(220, 40, 80, 128)),
                    2,
                    1,
                )),
                0.5,
            ),
        ],
    );

    assert!(compositor.supports_plan(&plan));
    let composed = compositor
        .compose(&plan, false, None)
        .expect("texture-only composition should not require readback capacity");
    assert_eq!(
        composed.backend,
        crate::performance::CompositorBackendKind::Gpu
    );
    assert!(compositor.canvas_gpu_admitted());
    #[cfg(target_os = "windows")]
    let native_target_available = compositor.screen_native_execution_target().is_some();

    let error = compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 4,
                height: 4,
            }),
        )
        .expect_err("full-size CPU readback should reject its local buffer request");
    assert!(error.to_string().contains("requires 64 bytes"));
    assert!(compositor.canvas_gpu_admitted());
    #[cfg(target_os = "windows")]
    assert_eq!(
        compositor.screen_native_execution_target().is_some(),
        native_target_available
    );
    assert!(
        compositor.has_pending_output_submission(),
        "readback rejection must preserve deferred GPU composition work"
    );
    assert!(
        compositor
            .current_output_frame()
            .expect("retained GPU output should remain queryable")
            .is_some()
    );

    let downscaled = compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 1,
                height: 1,
            }),
        )
        .expect("downscaled preview should fit its own readback request");
    assert_eq!(
        downscaled.backend,
        crate::performance::CompositorBackendKind::Gpu
    );
    assert!(compositor.canvas_gpu_admitted());
    assert!(
        compositor
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| { surfaces.width == 1 && surfaces.height == 1 })
    );
    compositor.discard_preview_work();
}

const fn screen_upload_key(
    descriptor_identity: u64,
    branch_sequence: u64,
    width: u32,
    height: u32,
) -> ScreenUploadContentKey {
    ScreenUploadContentKey::new(1, descriptor_identity, branch_sequence, width, height)
}

#[test]
fn screen_upload_pool_reuses_only_completion_retired_textures() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let mut pool =
        ScreenPublicationUploadPool::new(ScreenUploadResidencyPolicy::compositor_pipeline());
    let pixels = vec![37_u8; 3 * 2 * 4];

    let (first, wrote_first) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &pixels,
            screen_upload_key(1, 1, 3, 2),
            |_| {},
        )
        .expect("unaligned-width screen upload should succeed");
    assert!(wrote_first);
    assert_eq!(pool.state_counts(), (0, 1, 0));
    assert_eq!(pool.allocation_count, 1);

    let submission_index = queue.submit(std::iter::empty());
    pool.mark_submitted(submission_index.clone());
    assert_eq!(pool.state_counts(), (0, 0, 1));
    pool.discard_encoding();
    assert_eq!(pool.state_counts(), (0, 0, 1));

    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: None,
        })
        .expect("screen upload submission should complete");
    let (second, wrote_second) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &pixels,
            screen_upload_key(1, 2, 3, 2),
            |_| {},
        )
        .expect("completed screen upload texture should be reusable");
    assert!(wrote_second);
    assert_eq!(second.storage_id, first.storage_id);
    assert_eq!(pool.state_counts(), (0, 1, 0));
    assert_eq!(pool.allocation_count, 1);
    assert_eq!(pool.reuse_count, 1);
    pool.discard_encoding();
}

#[test]
fn unchanged_screen_publication_does_not_reupload_when_other_layer_recomposes() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let mut pool =
        ScreenPublicationUploadPool::new(ScreenUploadResidencyPolicy::compositor_pipeline());
    let pixels = vec![41_u8; 3 * 2 * 4];
    let content_key = screen_upload_key(1, 7, 3, 2);

    let (first, wrote_first) = pool
        .upload_rgba(&device, &queue, 3, 2, &pixels, content_key, |_| {})
        .expect("initial screen publication should upload");
    assert!(wrote_first);
    let first_submission = queue.submit(std::iter::empty());
    pool.mark_submitted(first_submission);

    let (recomposed, wrote_recomposed) = pool
        .upload_rgba(&device, &queue, 3, 2, &pixels, content_key, |_| {})
        .expect("unchanged screen publication should remain GPU-resident");
    assert!(!wrote_recomposed);
    assert_eq!(recomposed.storage_id, first.storage_id);
    assert_eq!(pool.upload_count, 1);
    assert_eq!(pool.state_counts(), (0, 1, 0));

    let recomposed_submission = queue.submit(std::iter::empty());
    pool.mark_submitted(recomposed_submission);
    assert_eq!(pool.state_counts(), (0, 0, 1));
}

#[test]
fn screen_upload_pool_fences_distinct_branches_with_matching_sequences() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let mut pool =
        ScreenPublicationUploadPool::new(ScreenUploadResidencyPolicy::compositor_pipeline());
    let first_pixels = vec![19_u8; 3 * 2 * 4];
    let second_pixels = vec![91_u8; 3 * 2 * 4];

    let (first, first_wrote) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &first_pixels,
            screen_upload_key(17, 1, 3, 2),
            |_| {},
        )
        .expect("first branch should enter the upload pipeline");
    let (second, second_wrote) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &second_pixels,
            screen_upload_key(23, 1, 3, 2),
            |_| {},
        )
        .expect("second branch should not reuse the first branch texture");

    assert!(first_wrote && second_wrote);
    assert_ne!(first.storage_id, second.storage_id);
    pool.discard_encoding();
}

#[test]
fn screen_upload_pool_charges_aligned_row_residency() {
    assert_eq!(resident_frame_bytes(3, 2).expect("extent is valid"), 512);
    assert_eq!(resident_frame_bytes(64, 2).expect("extent is valid"), 512);
    assert_eq!(resident_frame_bytes(65, 2).expect("extent is valid"), 1024);
}

#[test]
fn screen_upload_pool_evicts_completed_descriptors_within_texture_limit() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let mut pool =
        ScreenPublicationUploadPool::new(ScreenUploadResidencyPolicy::with_max_textures(1));
    let first_pixels = vec![11_u8; 3 * 2 * 4];
    let (first, _) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &first_pixels,
            screen_upload_key(1, 1, 3, 2),
            |_| {},
        )
        .expect("first descriptor should fit the pool fence");
    let second_pixels = vec![23_u8; 65 * 4];
    let error = pool
        .upload_rgba(
            &device,
            &queue,
            65,
            1,
            &second_pixels,
            screen_upload_key(1, 2, 65, 1),
            |_| {},
        )
        .expect_err("encoding textures must not be evicted or reused");
    assert!(error.is::<ScreenUploadPoolSaturated>());
    pool.discard_encoding();

    let mut evicted = Vec::new();
    let (second, _) = pool
        .upload_rgba(
            &device,
            &queue,
            65,
            1,
            &second_pixels,
            screen_upload_key(1, 2, 65, 1),
            |storage_id| evicted.push(storage_id),
        )
        .expect("completed descriptor should be evicted for a new shape");

    assert_ne!(second.storage_id, first.storage_id);
    assert_eq!(evicted, vec![first.storage_id]);
    assert_eq!(pool.state_counts(), (0, 1, 0));
    assert_eq!(pool.allocation_count, 2);
    pool.discard_encoding();
}

#[test]
fn screen_upload_pool_allows_pipeline_depth_then_reports_typed_saturation() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let mut pool =
        ScreenPublicationUploadPool::new(ScreenUploadResidencyPolicy::compositor_pipeline());
    let pixels = vec![47_u8; 3 * 2 * 4];
    let first_key = screen_upload_key(1, 1, 3, 2);
    let second_key = screen_upload_key(1, 2, 3, 2);
    let third_key = screen_upload_key(1, 3, 3, 2);

    pool.preflight_uploads(&device, [first_key, second_key])
        .expect("two changing frames should fit the projected pipeline");
    let projected_error = pool
        .preflight_uploads(&device, [first_key, second_key, third_key])
        .expect_err("three changing frames should exceed the projected pipeline");
    assert!(projected_error.is::<ScreenUploadPoolSaturated>());

    let (first, _) = pool
        .upload_rgba(&device, &queue, 3, 2, &pixels, first_key, |_| {})
        .expect("first changing frame should enter the upload pipeline");
    let (second, _) = pool
        .upload_rgba(&device, &queue, 3, 2, &pixels, second_key, |_| {})
        .expect("second changing frame should enter the upload pipeline");
    let error = pool
        .upload_rgba(&device, &queue, 3, 2, &pixels, third_key, |_| {})
        .expect_err("a third in-flight texture should hit the explicit pipeline bound");

    assert_ne!(first.storage_id, second.storage_id);
    assert!(error.is::<ScreenUploadPoolSaturated>());
    assert_eq!(pool.state_counts(), (0, 2, 0));
    assert_eq!(pool.allocation_count, 2);
    pool.discard_encoding();
}

#[test]
fn screen_upload_pool_reuses_only_matching_free_descriptors() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let device = compositor.device.clone();
    let queue = compositor.queue.clone();
    let mut pool =
        ScreenPublicationUploadPool::new(ScreenUploadResidencyPolicy::compositor_pipeline());
    let narrow_pixels = vec![29_u8; 3 * 2 * 4];
    let wide_pixels = vec![83_u8; 65 * 4];
    let (narrow, _) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &narrow_pixels,
            screen_upload_key(1, 1, 3, 2),
            |_| {},
        )
        .expect("narrow descriptor should upload");
    let (wide, _) = pool
        .upload_rgba(
            &device,
            &queue,
            65,
            1,
            &wide_pixels,
            screen_upload_key(1, 2, 65, 1),
            |_| {},
        )
        .expect("wide descriptor should upload independently");
    pool.discard_encoding();

    let (reused, _) = pool
        .upload_rgba(
            &device,
            &queue,
            3,
            2,
            &narrow_pixels,
            screen_upload_key(1, 3, 3, 2),
            |_| {},
        )
        .expect("matching free descriptor should be reused");

    assert_eq!(reused.storage_id, narrow.storage_id);
    assert_ne!(reused.storage_id, wide.storage_id);
    assert_eq!(pool.allocation_count, 2);
    pool.discard_encoding();
}

fn solid_canvas_with_size(width: u32, height: u32, color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(color);
    canvas
}

fn display_finalize_params(
    width: u32,
    height: u32,
    blend_mode: BlendMode,
) -> DisplayFinalizeParams {
    display_finalize_params_for_format(width, height, blend_mode, DisplayFrameFormat::Rgb)
}

fn display_finalize_params_for_format(
    width: u32,
    height: u32,
    blend_mode: BlendMode,
    frame_format: DisplayFrameFormat,
) -> DisplayFinalizeParams {
    DisplayFinalizeParams {
        cache_key: DisplayFinalizeCacheKey {
            zone_id: ZoneId::new(),
            device_id: DeviceId::new(),
            width,
            height,
            circular: false,
            frame_format,
        },
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

#[test]
fn frame_in_flight_requires_explicit_supersede_for_encoded_readbacks() {
    let frame = FrameInFlight::encoded_preview_for_test();
    assert_eq!(frame.generation, 7);
    assert!(frame.is_building());
    assert!(frame.preview_readback().is_some());

    assert!(frame.supersede("typed-state regression").is_none());
}

#[test]
fn frame_in_flight_rejects_silent_encoded_readback_drop() {
    let result = std::panic::catch_unwind(|| {
        drop(FrameInFlight::encoded_preview_for_test());
    });

    if cfg!(debug_assertions) {
        assert!(result.is_err());
    } else {
        assert!(result.is_ok());
    }
}

fn slot_surface(color: Rgba) -> PublishedSurface {
    let mut pool = RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 1);
    let mut lease = pool.dequeue().expect("surface slot should be available");
    lease.canvas_mut().fill(color);
    lease.submit(0, 0)
}

fn slot_surface_with_size(width: u32, height: u32, color: Rgba) -> PublishedSurface {
    let mut pool =
        RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(width, height), 1);
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

fn assert_gpu_samples_match_cpu(
    compositor: &mut GpuSparkleFlinger,
    plan: &CompositionPlan,
    tolerance: u8,
) {
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );
    let composed = compositor
        .compose(plan, false, None)
        .expect("GPU composition should succeed");
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());

    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampling should succeed")
    );
    assert_zone_colors_within(&sampled, &expected_zones, tolerance);
}

fn resolve_preview_surface_blocking(compositor: &mut GpuSparkleFlinger) -> PublishedSurface {
    loop {
        if let Some(surface) = compositor
            .resolve_preview_surface()
            .expect("GPU preview finalize should succeed")
        {
            return surface;
        }

        if let Some(submission_index) = compositor.pending_preview_submission() {
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

fn finalize_display_face_blocking(
    compositor: &mut GpuSparkleFlinger,
    scene: &ProducerFrame,
    face: &ProducerFrame,
    params: DisplayFinalizeParams,
) -> PublishedSurface {
    for _ in 0..16 {
        if let Some(surface) = compositor
            .finalize_display_face(scene, face, params)
            .expect("display finalize should not fail")
        {
            return surface;
        }
    }

    panic!("display finalize should produce a surface");
}

fn finalize_display_face_yuv420_blocking(
    compositor: &mut GpuSparkleFlinger,
    scene: &ProducerFrame,
    face: &ProducerFrame,
    mut params: DisplayFinalizeParams,
) -> DisplayYuv420Frame {
    params.cache_key.frame_format = DisplayFrameFormat::Jpeg;
    for _ in 0..16 {
        if let Some(frame) = compositor
            .finalize_display_face_yuv420(scene, face, params)
            .expect("display YUV finalize should not fail")
        {
            return frame;
        }
    }

    panic!("display YUV finalize should produce a frame");
}

fn defer_pending_preview_map(compositor: &mut GpuSparkleFlinger) {
    compositor.defer_next_preview_map_resolve();
    assert!(
        compositor
            .resolve_preview_surface()
            .expect("deferred preview finalize should not fail")
            .is_none()
    );

    if let Some(submission_index) = compositor.pending_preview_submission() {
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

    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_readback().is_none());
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
        zones: vec![Output {
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
        zones: vec![Output {
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
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let probe = compositor.probe.clone();

    assert!(!probe.adapter_name.is_empty());
    assert!(!probe.texture_format.is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn dx12_compositor_exposes_one_renderer_bound_screen_target() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    if compositor.probe.backend == "dx12" {
        let target = compositor
            .screen_native_execution_target()
            .expect("DX12 compositor should expose a D3D11On12 target");
        assert_eq!(
            target.max_texture_dimension().get(),
            compositor.probe.max_texture_dimension_2d
        );
    } else {
        assert!(compositor.screen_native_execution_target().is_none());
    }
}

#[cfg(target_os = "windows")]
#[test]
fn native_screen_copy_failure_policy_separates_pressure_from_stale_structure() {
    use std::num::NonZeroU64;

    use hypercolor_windows_capture::{CaptureError, GpuSurfaceDescriptorId};

    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::KeyedMutexTimeout),
        NativeScreenCopyFailurePolicy::Retain
    );
    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::Capture(
            CaptureError::GpuSurfaceUseUnavailable {
                descriptor_id: GpuSurfaceDescriptorId::new(NonZeroU64::MIN),
                source_sequence: 7,
            },
        )),
        NativeScreenCopyFailurePolicy::Retain
    );
    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::PreparedTargetMismatch {
            field: "plan_generation",
        },),
        NativeScreenCopyFailurePolicy::Reprepare
    );
    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::TargetContentUncertain {
            operation: "release capture keyed mutex",
            source: Box::new(D3d11On12ScreenInteropError::KeyedMutexTimeout),
        },),
        NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare
    );
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn macos_native_screen_copy_errors_invalidate_retained_output() {
    let error = anyhow::anyhow!("structural native screen copy failure");
    assert!(native_screen_copy_error_invalidates_frame(&error));
}

#[cfg(target_os = "windows")]
#[test]
fn native_screen_storage_turnover_purges_only_changed_targets() {
    assert!(screen_storage_requires_cache_turnover(None, 7));
    assert!(!screen_storage_requires_cache_turnover(Some(7), 7));
    assert!(screen_storage_requires_cache_turnover(Some(7), 8));
}

#[cfg(target_os = "windows")]
#[test]
fn native_screen_manifest_generation_is_an_exact_fence() {
    validate_windows_plan_generation(7, 7).expect("matching plan generation is accepted");
    assert!(validate_windows_plan_generation(7, 8).is_err());
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn metal_compositor_registers_and_composes_native_capture() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let target = compositor
        .screen_native_execution_target()
        .expect("Metal compositor should expose a native screen target");
    let bridge = Arc::clone(
        compositor
            .screen_bridge
            .as_ref()
            .expect("Metal compositor should retain its screen bridge"),
    );
    assert_eq!(target.accepted_api(), &PlatformGpuApi::Metal);
    assert_eq!(
        target.physical_gpu_device(),
        &ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(bridge.interop.metal_registry_id())
    );
    assert_eq!(
        target.max_texture_dimension().get(),
        compositor.probe.max_texture_dimension_2d
    );

    let pixels = [17, 43, 91, 255].repeat(12);
    let capture = Arc::new(macos_capture_frame(&pixels));
    let (imported, storage_id) = bridge
        .import_frame(&compositor.device, 11, Arc::clone(&capture))
        .expect("native capture should import through the daemon bridge");
    let (_, repeated_storage_id) = bridge
        .import_frame(&compositor.device, 11, capture)
        .expect("the same native storage should import again");
    assert_eq!(storage_id, repeated_storage_id);

    let plan = CompositionPlan::single(
        4,
        3,
        CompositionLayer::replace(ProducerFrame::GpuTexture(GpuTextureFrame {
            width: 4,
            height: 3,
            storage_id,
            content_generation: imported.content_sequence(),
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: imported
                .texture()
                .expect("BGRA imports expose a wgpu texture")
                .as_ref()
                .clone(),
            view: imported
                .view()
                .expect("BGRA imports expose a wgpu texture view")
                .as_ref()
                .clone(),
            immutable_lease: None,
            macos_screen_lease: None,
        })),
    );
    compositor
        .compose(&plan, false, full_preview_request(&plan))
        .expect("native capture should compose without CPU materialization");
    let preview = resolve_preview_surface_blocking(&mut compositor);
    assert!(
        preview
            .rgba_bytes()
            .chunks_exact(4)
            .all(|pixel| pixel == [91, 43, 17, 255])
    );
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn native_metal_target_formats_reject_disguised_source_storage() {
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Rgba8)
            .expect("RGBA8 is a truthful compositor target"),
        MacosNativeTargetFormat::Rgba8,
    );
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Bgra8)
            .expect("BGRA8 is a truthful compositor target"),
        MacosNativeTargetFormat::Bgra8,
    );
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Argb2101010)
            .expect_err("packed source storage cannot masquerade as a compositor target"),
        UnsupportedMacosNativeTargetFormat(CapturePixelFormat::Argb2101010),
    );
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
struct MacosLeaseTargetPreparer {
    bridge: Arc<super::MacosScreenBridge>,
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
impl ScreenNativeTargetPreparer for MacosLeaseTargetPreparer {
    fn quote_retained_bytes(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        super::prepared_macos_screen_target_exclusive_bytes(descriptor)
    }

    fn quote_retention(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeRetentionQuote> {
        super::prepared_macos_screen_target_retention(descriptor)
    }

    fn prepare(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        let prepared = self
            .bridge
            .prepare_target(descriptor, platform.plan_generation())?;
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(prepared),
            ),
            super::prepared_macos_screen_target_retention(descriptor)?,
        ))
    }
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn equal_native_physical_descriptors_share_the_reduction_target() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let target = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes a native screen target")
        .clone();
    let bridge = Arc::clone(
        compositor
            .screen_bridge
            .as_ref()
            .expect("Metal compositor retains its screen bridge"),
    );
    let target_color_capabilities = target.color_capabilities();
    let extent = PixelExtent::new(4, 3).expect("fixture extent is valid");
    let source = ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new("macos:fixture:shared-physical")
                .expect("fixture source id is valid"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new(
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent,
                extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("fixture geometry is valid"),
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Bgra8,
            CaptureColorimetry::SRGB,
            ScreenBackendResourceIdentity::new_with_physical_gpu_device(
                ScreenCaptureBackend::MacosScreenCaptureKit,
                ScreenResourceApi::PlatformGpu(PlatformGpuApi::Metal),
                target.physical_gpu_device().clone(),
                5,
                7,
            ),
        ),
    );
    let descriptor = ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
        ScreenExtentRequest::bounded(
            NonZeroU32::new(2),
            NonZeroU32::new(1),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Contain,
        Arc::new(ScreenProcessingProfile::new(
            ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Bgra8),
        )),
    )
    .resolve_with_executor_capabilities(
        &source,
        ScreenExecutorColorCapabilities::new(
            ScreenColorTransformCapabilities::NONE,
            target_color_capabilities,
        ),
    )
    .expect("native fixture descriptor resolves");

    let plan_generation = hypercolor_core::input::screen::ScreenPlanGeneration::default();
    let first = bridge
        .prepare_target(&descriptor, plan_generation)
        .expect("first native target prepares");
    let second = bridge
        .prepare_target(&descriptor, plan_generation)
        .expect("equal native target prepares");
    let first_physical = first
        .physical
        .as_ref()
        .expect("bounded native descriptor has physical work");
    let second_physical = second
        .physical
        .as_ref()
        .expect("equal bounded descriptor has physical work");

    assert!(Arc::ptr_eq(first_physical, second_physical));
    assert_eq!(first_physical.storage_id, second_physical.storage_id);

    let edge_extended = ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenPublicationExecutorRequest::SourceNative(target),
        ScreenExtentRequest::bounded(
            NonZeroU32::new(2),
            NonZeroU32::new(1),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Contain,
        Arc::new(ScreenProcessingProfile::new(
            ScreenProcessingProfileConfig {
                letterbox_fill: ScreenLetterboxFill::EdgeExtend,
                ..ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Bgra8)
            },
        )),
    )
    .resolve_with_executor_capabilities(
        &source,
        ScreenExecutorColorCapabilities::new(
            ScreenColorTransformCapabilities::NONE,
            target_color_capabilities,
        ),
    )
    .expect("edge-extended native descriptor resolves");
    let error = bridge
        .prepare_target(&edge_extended, plan_generation)
        .expect_err("edge extension must fail native preparation");
    assert!(error.to_string().contains("edge-extended letterbox fill"));
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn macos_texture_lease_retains_exclusive_shared_and_capture_admissions() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let registered_target = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes a native screen target")
        .clone();
    let bridge = Arc::clone(
        compositor
            .screen_bridge
            .as_ref()
            .expect("Metal compositor retains its screen bridge"),
    );
    let target = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(991).expect("fixture target id is non-zero"),
        ),
        PlatformGpuApi::Metal,
        registered_target.physical_gpu_device().clone(),
        NonZeroU32::new(compositor.probe.max_texture_dimension_2d)
            .expect("fixture texture limit is non-zero"),
        Arc::new(MacosLeaseTargetPreparer {
            bridge: Arc::clone(&bridge),
        }),
    )
    .with_color_capabilities(registered_target.color_capabilities());
    let extent = PixelExtent::new(4, 3).expect("fixture extent is valid");
    let source_id =
        CaptureSourceId::new("macos:fixture:lease").expect("fixture source id is valid");
    let source = ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source_id.clone(),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new(
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent,
                extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("fixture geometry is valid"),
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Bgra8,
            CaptureColorimetry::SRGB,
            ScreenBackendResourceIdentity::new_with_physical_gpu_device(
                ScreenCaptureBackend::MacosScreenCaptureKit,
                ScreenResourceApi::PlatformGpu(PlatformGpuApi::Metal),
                registered_target.physical_gpu_device().clone(),
                5,
                7,
            ),
        ),
    );
    let demand = hypercolor_core::input::screen::RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::SourceNative(target),
            ScreenExtentRequest::bounded(
                NonZeroU32::new(2),
                NonZeroU32::new(1),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        NonZeroU32::new(60).expect("fixture cadence is non-zero"),
    )
    .resolve_with_executor_capabilities(
        &source,
        ScreenExecutorColorCapabilities::new(
            ScreenColorTransformCapabilities::NONE,
            registered_target.color_capabilities(),
        ),
    )
    .expect("native lease demand resolves");
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let mut builder = ScreenPlanBuilder::with_publication_slots_and_admission(
        ScreenPublicationSlotPolicy::default(),
        coordinator.clone(),
    );
    let revision = InputPublicationDemandRevision::new(1);
    let graph = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            [demand],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("native lease plan prepares");
    let ticket = preparing
        .worker_ticket(&source_id)
        .expect("native lease source owns a worker ticket");
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("native lease ledger begins");
    let descriptor = ledger.ticket().candidate_plan().branches()[0]
        .descriptor()
        .clone();
    let ScreenPublicationExecutor::SourceNative(target) = descriptor.executor() else {
        panic!("native lease descriptor keeps its native executor");
    };
    let prepared = ledger
        .prepare_native_target(
            target,
            &descriptor,
            &hypercolor_core::input::screen::ScreenNativePreparationPayload::new(
                &descriptor,
                ledger.ticket().plan_generation(),
                Arc::new(()),
            ),
            "native-target-test",
            "worker-runtime-total",
        )
        .expect("native renderer target is admitted");
    let shared_resource_name = prepared
        .shared_resource_name()
        .cloned()
        .expect("native reduction has a shared physical resource");
    ledger
        .preflight_additional_bytes(1)
        .expect("capture admission byte fits");
    ledger
        .report_scoped("capture-plan-test", "worker-runtime-total", 1)
        .expect("capture admission is exact");
    let required = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, bytes) in required {
        ledger
            .report(&name, bytes)
            .expect("required native lease resource is exact");
    }
    let (token, lifetimes) = ledger
        .finish()
        .expect("native lease ledger finishes")
        .into_parts();
    preparing
        .acknowledge(token)
        .expect("native lease worker acknowledges");
    let target_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "native-target-test")
        .cloned()
        .expect("target lifetime is present");
    let capture_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "capture-plan-test")
        .cloned()
        .expect("capture lifetime is present");
    let shared_target_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name() == &shared_resource_name)
        .cloned()
        .expect("shared physical lifetime is present");
    let bound = prepared
        .bind_with_shared(
            target_lifetime.clone(),
            Some(shared_target_lifetime.clone()),
        )
        .expect("prepared target binds its exclusive and shared lifetimes");
    let capture = Arc::new(macos_capture_frame(&[17, 43, 91, 255].repeat(12)));
    let imported = bridge
        .interop
        .import_frame(&compositor.device, 7, Arc::clone(&capture))
        .expect("lease fixture imports");
    let surface = bound
        .retain_on_surface_with_capture_allocation(
            PlatformGpuSurface::new(
                PlatformGpuApi::Metal,
                u64::from(capture.surface.iosurface_id),
                extent,
                CapturePixelFormat::Bgra8,
                capture,
            )
            .expect("lease fixture surface is valid"),
            capture_lifetime.clone(),
        )
        .expect("surface retains both exact allocations");
    let capture_owner = surface
        .owner::<MacosCaptureFrame>()
        .expect("surface retains the capture owner");
    let target_owner = surface
        .retained_owner::<PreparedMacosScreenTarget>()
        .expect("surface retains the renderer owner");
    assert_eq!(
        surface
            .shared_resource_lifetime()
            .expect("surface retains the shared physical lifetime")
            .resource()
            .name(),
        shared_target_lifetime.resource().name()
    );
    let lease = MacosScreenTextureLease::new(
        imported,
        capture_owner,
        target_owner,
        target_lifetime,
        Some(shared_target_lifetime),
        capture_lifetime,
    );
    drop(surface);
    drop(bound);
    drop(lifetimes);
    drop(preparing);
    drop(builder);
    let retained_bytes = coordinator.snapshot().reserved_bytes();
    assert!(retained_bytes > 0);
    drop(lease);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn native_metal_reduction_feeds_gpu_zone_sampling_without_readback() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge = Arc::clone(
        compositor
            .screen_bridge
            .as_ref()
            .expect("Metal compositor retains its screen bridge"),
    );
    let capture = Arc::new(macos_capture_frame(&[17, 43, 91, 255].repeat(12)));
    let imported = bridge
        .interop
        .import_frame(&compositor.device, 23, capture)
        .expect("native zone fixture imports");
    let target = bridge
        .reducer
        .create_target(&compositor.device, 4, 4, MacosNativeTargetFormat::Rgba8)
        .expect("native zone target allocates");
    let descriptor = MacosNativeReductionDescriptor::new(
        [4, 4],
        [0, 0, 4, 4],
        [0.0, 0.0, 4.0, 3.0],
        MacosNativeReductionFilter::Area,
        None,
    )
    .expect("native zone reduction geometry is valid");
    let mut encoder = compositor
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SparkleFlinger native zone reduction"),
        });
    bridge
        .reducer
        .encode(&imported, &target, descriptor, &mut encoder)
        .expect("native zone reduction encodes");
    let _ = compositor.queue.submit(Some(encoder.finish()));

    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(GpuTextureFrame {
            width: 4,
            height: 4,
            storage_id: 29,
            content_generation: imported.content_sequence(),
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: target.texture().clone(),
            view: target.view().clone(),
            immutable_lease: None,
            macos_screen_lease: None,
        })),
    );
    compositor
        .compose(&plan, false, None)
        .expect("native reduced texture composes without readback");
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let mut expected = Canvas::new(4, 4);
    expected.fill(Rgba::new(91, 43, 17, 255));
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("native reduced texture samples into zones")
    );
    assert_eq!(sampled, engine.sample(&expected));
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn native_metal_color_pipeline_matches_shared_sdr_p3_pq_hlg_extended_linear_and_yuv_vectors() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge = Arc::clone(
        compositor
            .screen_bridge
            .as_ref()
            .expect("Metal compositor should retain its screen bridge"),
    );
    for (format, source, color, planes) in managed_native_vectors() {
        let capture = Arc::new(macos_native_capture_frame(format, color, &planes));
        let imported = bridge
            .interop
            .import_frame(&compositor.device, 19, Arc::clone(&capture))
            .expect("managed native vector imports");
        let prepared = PreparedLedToneMap::prepare(
            source,
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("managed native vector prepares");
        let constants = prepared.constants();
        let target = bridge
            .reducer
            .create_target(&compositor.device, 1, 1, MacosNativeTargetFormat::Rgba8)
            .expect("managed native target allocates");
        let descriptor = MacosNativeReductionDescriptor::new(
            [1, 1],
            [0, 0, 1, 1],
            [0.0, 0.0, 1.0, 1.0],
            MacosNativeReductionFilter::Nearest,
            Some((
                MacosNativeOutputTransfer::Srgb,
                MacosNativeColorTransform::new(
                    constants.source_to_target,
                    constants.source_luminance_and_exposure,
                    constants.curve,
                ),
            )),
        )
        .expect("managed native descriptor is valid");
        let mut encoder =
            compositor
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger managed native color parity"),
                });
        bridge
            .reducer
            .encode(&imported, &target, descriptor, &mut encoder)
            .expect("managed native vector encodes");
        let _ = compositor.queue.submit(Some(encoder.finish()));
        let actual = read_texture_rgba8(
            &compositor.device,
            &compositor.queue,
            target.texture(),
            1,
            1,
        );
        let encoded = capture
            .with_cpu_source(|source| source.sample_rgba32f(0, 0))
            .expect("scalar source maps")
            .expect("scalar source decodes");
        let mapped = prepared.decode_and_map_source(encoded);
        let expected = prepared.encode(mapped);
        assert_eq!(actual.as_slice(), expected, "{format:?} managed parity");
    }
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
#[test]
fn native_metal_sdr_output_transfers_match_the_shared_encoder() {
    let Some(compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge = Arc::clone(
        compositor
            .screen_bridge
            .as_ref()
            .expect("Metal compositor should retain its screen bridge"),
    );
    let source = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Linear,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("linear source contract is valid");
    let color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Srgb,
        transfer: MacosTransferFunction::Linear,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let planes = vec![
        [0x3400_u16, 0x3800, 0x3a00, 0x3c00]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect(),
    ];
    let capture = Arc::new(macos_native_capture_frame(
        MacosCapturePixelFormat::Rgba16Float,
        color,
        &planes,
    ));
    let imported = bridge
        .interop
        .import_frame(&compositor.device, 29, Arc::clone(&capture))
        .expect("SDR transfer fixture imports");
    let encoded_source = capture
        .with_cpu_source(|source| source.sample_rgba32f(0, 0))
        .expect("scalar source maps")
        .expect("scalar source decodes");

    for (transfer, native_transfer, color_space) in [
        (
            CaptureTransferFunction::Srgb,
            MacosNativeOutputTransfer::Srgb,
            CaptureColorSpace::Srgb,
        ),
        (
            CaptureTransferFunction::Linear,
            MacosNativeOutputTransfer::Linear,
            CaptureColorSpace::Srgb,
        ),
        (
            CaptureTransferFunction::Rec709,
            MacosNativeOutputTransfer::Rec709,
            CaptureColorSpace::Srgb,
        ),
        (
            CaptureTransferFunction::Rec2020,
            MacosNativeOutputTransfer::Rec2020,
            CaptureColorSpace::Rec2020,
        ),
    ] {
        let output = KnownCaptureColorimetry::try_new(
            color_space,
            transfer,
            CaptureDynamicRange::Standard,
            None,
        )
        .expect("SDR output contract is valid");
        let prepared = PreparedLedToneMap::prepare(source, output, LedToneMapCalibration::DEFAULT)
            .expect("SDR output fixture prepares");
        let constants = prepared.constants();
        let target = bridge
            .reducer
            .create_target(&compositor.device, 1, 1, MacosNativeTargetFormat::Rgba8)
            .expect("SDR output target allocates");
        let descriptor = MacosNativeReductionDescriptor::new(
            [1, 1],
            [0, 0, 1, 1],
            [0.0, 0.0, 1.0, 1.0],
            MacosNativeReductionFilter::Nearest,
            Some((
                native_transfer,
                MacosNativeColorTransform::new(
                    constants.source_to_target,
                    constants.source_luminance_and_exposure,
                    constants.curve,
                ),
            )),
        )
        .expect("SDR output descriptor is valid");
        let mut encoder =
            compositor
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger native SDR output parity"),
                });
        bridge
            .reducer
            .encode(&imported, &target, descriptor, &mut encoder)
            .expect("SDR output vector encodes");
        let _ = compositor.queue.submit(Some(encoder.finish()));
        let actual = read_texture_rgba8(
            &compositor.device,
            &compositor.queue,
            target.texture(),
            1,
            1,
        );
        let expected = prepared.encode(prepared.decode_and_map_source(encoded_source));
        assert_eq!(actual.as_slice(), expected, "{transfer:?} output parity");
    }
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
fn managed_native_vectors() -> Vec<(
    MacosCapturePixelFormat,
    KnownCaptureColorimetry,
    MacosCaptureColorimetry,
    Vec<Vec<u8>>,
)> {
    let p3 = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::DisplayP3,
        CaptureTransferFunction::Srgb,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("P3 source contract is valid");
    let hdr_luminance = CaptureLuminanceContext::new(
        CapturePositiveScalar::try_new(203.0).expect("reference white is valid"),
        CapturePositiveScalar::try_new(1_000.0).expect("peak is valid"),
    )
    .expect("HDR luminance is ordered");
    let rec2020_pq = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Rec2020,
        CaptureTransferFunction::Pq,
        CaptureDynamicRange::High,
        Some(hdr_luminance),
    )
    .expect("PQ source contract is valid");
    let rec2020_linear = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Rec2020,
        CaptureTransferFunction::Linear,
        CaptureDynamicRange::High,
        Some(hdr_luminance),
    )
    .expect("extended-linear source contract is valid");
    let rec2020_hlg = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Rec2020,
        CaptureTransferFunction::Hlg,
        CaptureDynamicRange::High,
        Some(hdr_luminance),
    )
    .expect("HLG source contract is valid");
    vec![
        (
            MacosCapturePixelFormat::Bgra8,
            KnownCaptureColorimetry::SRGB,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            vec![vec![208, 72, 24, 255]],
        ),
        (
            MacosCapturePixelFormat::Bgra8,
            p3,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::DisplayP3,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            vec![vec![32, 96, 224, 255]],
        ),
        (
            MacosCapturePixelFormat::Argb2101010,
            rec2020_pq,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: MacosTransferFunction::Pq,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            vec![
                ((3_u32 << 30) | (600_u32 << 20) | (450_u32 << 10) | 0x012c_u32)
                    .to_le_bytes()
                    .to_vec(),
            ],
        ),
        (
            MacosCapturePixelFormat::Rgba16Float,
            rec2020_linear,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: MacosTransferFunction::Linear,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            vec![
                [0x4000_u16, 0x3c00, 0x3800, 0x3c00]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ],
        ),
        (
            MacosCapturePixelFormat::Yuv420VideoRange,
            rec2020_pq,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: MacosTransferFunction::Pq,
                matrix: Some(MacosYuvMatrix::Bt2020),
                range: MacosColorRange::Video,
                chroma_location: Some(MacosChromaLocation::Center),
            },
            vec![vec![128], vec![64, 192]],
        ),
        (
            MacosCapturePixelFormat::Yuv420FullRange,
            rec2020_pq,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: MacosTransferFunction::Pq,
                matrix: Some(MacosYuvMatrix::Bt2020),
                range: MacosColorRange::Full,
                chroma_location: Some(MacosChromaLocation::Left),
            },
            vec![vec![144], vec![80, 176]],
        ),
        (
            MacosCapturePixelFormat::Yuv44410BiPlanar,
            rec2020_pq,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: MacosTransferFunction::Pq,
                matrix: Some(MacosYuvMatrix::Bt2020),
                range: MacosColorRange::Video,
                chroma_location: Some(MacosChromaLocation::TopLeft),
            },
            vec![
                (600_u16 << 6).to_le_bytes().to_vec(),
                [(320_u16 << 6), (700_u16 << 6)]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ],
        ),
        (
            MacosCapturePixelFormat::Bgra8,
            rec2020_hlg,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: MacosTransferFunction::Hlg,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            vec![vec![64, 128, 192, 255]],
        ),
    ]
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
fn macos_native_capture_frame(
    format: MacosCapturePixelFormat,
    color: MacosCaptureColorimetry,
    planes: &[Vec<u8>],
) -> MacosCaptureFrame {
    let extent = MacosPixelExtent::new(1, 1).expect("fixture extent is valid");
    let borrowed = planes.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let (surface, planes) =
        MacosCaptureSurface::new_native_fixture(extent, format, color, &borrowed)
            .expect("native managed fixture is valid");
    MacosCaptureFrame {
        epoch: 5,
        sequence: 1,
        display_time: 13,
        storage_extent: extent,
        planes: Arc::from(planes),
        pixel_format: format,
        color,
        geometry: MacosCaptureGeometry {
            display_scale_factor: MacosScale::display(1.0).expect("fixture display scale is valid"),
            content_scale: MacosScale::new(1.0).expect("fixture content scale is valid"),
            content_rect_points: MacosPointRect::new(0.0, 0.0, 1.0, 1.0)
                .expect("fixture content points are valid"),
            content_rect_pixels: MacosPixelRect::new(0, 0, 1, 1)
                .expect("fixture content pixels are valid"),
            screen_rect_points: None,
            bounding_rect_points: None,
            bounding_rect_pixels: None,
        },
        damage: Arc::from([]),
        cursor_composed: false,
        surface,
    }
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
fn read_texture_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let row_bytes = width * 4;
    let padded =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SparkleFlinger managed native color readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("SparkleFlinger managed native color readback"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .expect("managed native color readback poll succeeds");
    receiver
        .recv()
        .expect("managed native color callback arrives")
        .expect("managed native color buffer maps");
    let mapped = slice.get_mapped_range();
    let mut result = Vec::with_capacity((row_bytes * height) as usize);
    for row in mapped.chunks_exact(padded as usize) {
        result.extend_from_slice(&row[..row_bytes as usize]);
    }
    result
}

#[cfg(all(feature = "screen-capture", target_os = "macos"))]
fn macos_capture_frame(pixels: &[u8]) -> MacosCaptureFrame {
    let extent = MacosPixelExtent::new(4, 3).expect("fixture extent should be valid");
    let (surface, plane) = MacosCaptureSurface::new_native_bgra_fixture(extent, pixels)
        .expect("native BGRA fixture should be valid");
    MacosCaptureFrame {
        epoch: 5,
        sequence: 0,
        display_time: 13,
        storage_extent: extent,
        planes: Arc::from([plane]),
        pixel_format: MacosCapturePixelFormat::Bgra8,
        color: MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Srgb,
            transfer: MacosTransferFunction::Srgb,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        },
        geometry: MacosCaptureGeometry {
            display_scale_factor: MacosScale::display(1.0)
                .expect("fixture display scale should be valid"),
            content_scale: MacosScale::new(1.0).expect("fixture content scale should be valid"),
            content_rect_points: MacosPointRect::new(0.0, 0.0, 4.0, 3.0)
                .expect("fixture content points should be valid"),
            content_rect_pixels: MacosPixelRect::new(0, 0, 4, 3)
                .expect("fixture content pixels should be valid"),
            screen_rect_points: None,
            bounding_rect_points: None,
            bounding_rect_pixels: None,
        },
        damage: Arc::from([]),
        cursor_composed: true,
        surface,
    }
}

#[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
#[test]
fn gpu_macos_imported_frame_composes_without_cpu_readback() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let width = 2;
    let height = 2;
    let texture = compositor.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SparkleFlinger test BGRA imported source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let bgra_bottom_left_origin = [
        255, 0, 0, 255, 0, 255, 255, 255, 0, 0, 255, 255, 0, 255, 0, 255,
    ];
    compositor.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bgra_bottom_left_origin,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let frame = hypercolor_core::effect::ImportedEffectFrame {
        width,
        height,
        format: hypercolor_core::effect::ImportedFrameFormat::Bgra8Unorm,
        storage_id: 1,
        texture: Arc::new(texture),
        view: Arc::new(view),
        timings: hypercolor_core::effect::ImportedFrameTimings::default(),
    };

    let composed = compositor
        .compose(
            &CompositionPlan::single(
                width,
                height,
                CompositionLayer::replace(ProducerFrame::Gpu(frame)),
            ),
            false,
            None,
        )
        .expect("imported frame should compose on the GPU");

    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(
        compositor
            .current_output_frame()
            .is_ok_and(|frame| frame.is_some())
    );
}

#[test]
fn gpu_compositor_passthroughs_current_output_texture() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let source_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(7))),
    );

    compositor
        .compose(&source_plan, false, None)
        .expect("initial GPU composition should succeed");
    let output_generation = compositor.output_generation;
    let output_surface = compositor.current_output;
    let frame = compositor
        .current_output_frame()
        .expect("current output frame lookup should succeed")
        .expect("current output frame should exist");

    assert_eq!(frame.origin, GpuTextureFrameOrigin::CompositorOutput);
    assert_eq!(frame.content_generation, output_generation);

    let passthrough_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(frame)),
    );
    let composed = compositor
        .compose(&passthrough_plan, false, None)
        .expect("current output texture pass-through should succeed");

    assert!(composed.sampling_canvas.is_none());
    assert_eq!(compositor.output_generation, output_generation);
    assert_eq!(compositor.current_output, output_surface);
}

#[test]
fn gpu_compositor_rejects_stale_mutable_output_handles() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let preparation = compositor.prepare_canvas_resize(4, 4, None);
    assert!(preparation.is_admitted());
    compositor.apply_canvas_resize(preparation);
    compositor
        .compose(
            &CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(17))),
            ),
            false,
            None,
        )
        .expect("first GPU output should compose");
    let stale = compositor
        .current_output_frame()
        .expect("first output should submit")
        .expect("first output should exist");
    compositor
        .compose(
            &CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(23))),
            ),
            false,
            None,
        )
        .expect("second GPU output should compose");
    let stale_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(stale)),
    );

    assert!(!compositor.supports_plan(&stale_plan));
    let error = compositor
        .compose(&stale_plan, false, None)
        .expect_err("stale mutable output must not be sampled");
    assert!(error.to_string().contains("aliases compositor storage"));
}

#[test]
fn gpu_compositor_rejects_every_cached_surface_texture_before_reactivation() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let preparation = compositor.prepare_canvas_resize(4, 4, None);
    assert!(preparation.is_admitted());
    compositor.apply_canvas_resize(preparation);
    compositor
        .compose(
            &CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(29))),
            ),
            false,
            None,
        )
        .expect("initial 4x4 output should compose");

    let content_generation = compositor.output_generation;
    let alias_frames = {
        let surfaces = compositor
            .surfaces
            .as_ref()
            .expect("4x4 surfaces should be active");
        let make_frame = |texture: &super::GpuCompositorTexture, origin: GpuTextureFrameOrigin| {
            GpuTextureFrame {
                width: 4,
                height: 4,
                storage_id: texture.storage_id,
                content_generation,
                origin,
                texture: texture.texture.clone(),
                view: texture.view.clone(),
                immutable_lease: None,
                #[cfg(target_os = "windows")]
                windows_screen_lease: None,
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                macos_screen_lease: None,
            }
        };
        [
            (
                "front",
                make_frame(&surfaces.front, GpuTextureFrameOrigin::CompositorOutput),
            ),
            (
                "back",
                make_frame(&surfaces.back, GpuTextureFrameOrigin::CompositorOutput),
            ),
            (
                "source",
                make_frame(&surfaces.source, GpuTextureFrameOrigin::ProducerTexture),
            ),
        ]
    };
    assert_ne!(alias_frames[0].1.storage_id, alias_frames[1].1.storage_id);
    assert_ne!(alias_frames[0].1.storage_id, alias_frames[2].1.storage_id);
    assert_ne!(alias_frames[1].1.storage_id, alias_frames[2].1.storage_id);

    compositor
        .compose(
            &CompositionPlan::single(
                8,
                8,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas_with_size(
                    8, 8, 31,
                ))),
            ),
            false,
            None,
        )
        .expect("8x8 output should cache the 4x4 surfaces");

    for cycle in 0..3 {
        assert!(
            compositor
                .compositor_surface_cache
                .get(&(4, 4))
                .is_some_and(Option::is_some),
            "cycle {cycle} should retain the 4x4 surface set in the cache"
        );
        for (name, frame) in &alias_frames {
            let plan = CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::GpuTexture(frame.clone())),
            );
            assert!(
                !compositor.supports_plan(&plan),
                "cached {name} storage must fail GPU admission"
            );
            let error = compositor
                .compose(&plan, false, None)
                .expect_err("cached compositor storage must be rejected before reactivation");
            assert!(error.to_string().contains("aliases compositor storage"));
            assert!(
                compositor
                    .surfaces
                    .as_ref()
                    .is_some_and(|surfaces| { (surfaces.width, surfaces.height) == (8, 8) })
            );
        }

        compositor
            .compose(
                &CompositionPlan::single(
                    4,
                    4,
                    CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(37 + cycle))),
                ),
                false,
                None,
            )
            .expect("valid 4x4 source should reactivate its cached surfaces");
        compositor
            .compose(
                &CompositionPlan::single(
                    8,
                    8,
                    CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas_with_size(
                        8,
                        8,
                        43 + cycle,
                    ))),
                ),
                false,
                None,
            )
            .expect("valid 8x8 source should cache the 4x4 surfaces again");
    }
}

#[test]
fn immutable_scene_pool_covers_two_overlapping_generations_without_growth() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let preparation = compositor.prepare_canvas_resize(4, 4, None);
    assert!(preparation.is_admitted());
    compositor.apply_canvas_resize(preparation);
    let allocations = compositor.snapshot_texture_allocation_count();

    compositor
        .compose(
            &CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(31))),
            ),
            false,
            None,
        )
        .expect("first generation should compose");
    let first = compositor
        .snapshot_current_output_frame()
        .expect("first generation should snapshot")
        .expect("first generation should exist");
    compositor
        .compose(
            &CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(37))),
            ),
            false,
            None,
        )
        .expect("second generation should compose");
    let second = compositor
        .snapshot_current_output_frame()
        .expect("second generation should snapshot")
        .expect("second generation should exist");
    assert_ne!(first.storage_id, second.storage_id);

    compositor
        .compose(
            &CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(41))),
            ),
            false,
            None,
        )
        .expect("third current output should compose");
    assert!(
        compositor.snapshot_current_output_frame().is_err(),
        "a third overlapping lease is outside the serial executor ownership bound"
    );
    let first_storage_id = first.storage_id;
    drop(first);
    let recycled = compositor
        .snapshot_current_output_frame()
        .expect("released generation should make its slot reusable")
        .expect("recycled generation should exist");
    assert_eq!(recycled.storage_id, first_storage_id);
    assert_eq!(compositor.snapshot_texture_allocation_count(), allocations);
}

#[test]
fn gpu_compositor_does_not_passthrough_producer_texture() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let source_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(9))),
    );

    compositor
        .compose(&source_plan, false, None)
        .expect("initial GPU composition should succeed");
    let output_generation = compositor.output_generation;
    let producer_frame = compositor
        .upload_canvas_frame(&patterned_canvas(11))
        .expect("producer canvas upload should succeed");

    assert_eq!(
        producer_frame.origin,
        GpuTextureFrameOrigin::ProducerTexture
    );
    assert_eq!(producer_frame.content_generation, 1);

    let producer_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(producer_frame)),
    );
    compositor
        .compose(&producer_plan, false, None)
        .expect("producer texture composition should not be passed through");

    assert_eq!(compositor.output_generation, output_generation + 1);
}

#[test]
fn gpu_compositor_matches_cpu_alpha_composition() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
}

#[test]
fn gpu_compositor_matches_cpu_add_composition() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
}

#[test]
fn gpu_compositor_matches_cpu_screen_composition() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 0);
}

#[test]
fn gpu_compositor_matches_cpu_for_distinct_multi_pass_params() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(220, 28, 16, 255))),
                0.45,
            ),
            CompositionLayer::add(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(24, 180, 64, 255))),
                0.3,
            ),
            CompositionLayer::screen(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(32, 48, 240, 255))),
                0.55,
            ),
        ],
    );
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
}

#[test]
fn gpu_compositor_bypasses_single_replace_surfaces() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
fn gpu_steady_state_animated_compose_keeps_params_in_uniform_ring() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let base = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(0), &patterned_canvas(9))
        .expect("base media upload should succeed");
    let overlay = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(1), &patterned_canvas(63))
        .expect("overlay media upload should succeed");

    let frames = 8_u32;
    for frame in 0..frames {
        let opacity = 0.2 + frame as f32 * 0.05;
        let plan = CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::GpuTexture(base.clone())),
                CompositionLayer::alpha(ProducerFrame::GpuTexture(overlay.clone()), opacity),
            ],
        );
        compositor
            .compose(&plan, false, None)
            .expect("animated GPU composition should succeed");
        assert!(
            compositor
                .current_output_frame()
                .expect("current output frame lookup should succeed")
                .is_some()
        );
    }

    let surfaces = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should exist after composition");
    assert_eq!(surfaces.pending_upload_buffers.creation_count, 0);
    assert_eq!(surfaces.compose_param_write_count, frames as usize);
    assert_eq!(
        compositor.pipeline.compose_params.ring_write_count,
        frames as usize
    );
    assert_eq!(compositor.pipeline.compose_params.fallback_write_count, 0);
}

#[test]
fn gpu_compose_params_ring_wrap_falls_back_to_staging_uploads() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    compositor
        .pipeline
        .compose_params
        .set_slot_count_for_test(1);

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                220, 28, 16, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(24, 180, 64, 255))),
                0.45,
            ),
            CompositionLayer::add(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(32, 48, 240, 255))),
                0.3,
            ),
        ],
    );
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
    assert_eq!(compositor.pipeline.compose_params.ring_write_count, 1);
    assert_eq!(compositor.pipeline.compose_params.fallback_write_count, 1);

    // Once the wrapped writes are submitted and retired, the ring resumes.
    let second_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                10, 20, 30, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(200, 100, 50, 255))),
                0.6,
            ),
        ],
    );
    assert_gpu_samples_match_cpu(&mut compositor, &second_plan, 1);
    assert_eq!(compositor.pipeline.compose_params.ring_write_count, 2);
    assert_eq!(compositor.pipeline.compose_params.fallback_write_count, 1);
}

#[test]
fn gpu_compositor_latches_sampling_canvas_for_animated_gpu_plans() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let first_content = patterned_canvas(9);
    let second_content = patterned_canvas(63);
    let first_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(0), &first_content)
        .expect("first media upload should succeed");
    let second_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(1), &second_content)
        .expect("second media upload should succeed");

    // GPU producer frames keep `cached_readback_key` at None, so the keyed
    // readback cache can never service CPU sampling for this plan shape.
    let first_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(first_frame)),
    );
    let composed = compositor
        .compose(&first_plan, true, None)
        .expect("first animated GPU compose should succeed");
    assert!(
        composed.sampling_canvas.is_none(),
        "the very first compose has no previous frame to latch",
    );

    // Let the staged readback finish so the next compose can resolve it.
    compositor
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("GPU wait for the staged sampling readback should succeed");

    let second_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(second_frame)),
    );
    let composed = compositor
        .compose(&second_plan, true, None)
        .expect("second animated GPU compose should succeed");
    let sampling_canvas = composed
        .sampling_canvas
        .expect("the second compose should latch the previous frame's output");
    assert_eq!(
        sampling_canvas.as_rgba_bytes(),
        first_content.as_rgba_bytes(),
        "latched sampling canvas should hold the previous frame's composed pixels",
    );
    assert!(
        composed.sampling_surface.is_some(),
        "the latched frame should also expose a published sampling surface",
    );
    let counts = compositor.surface_pool_counts().compositor;
    assert_eq!(counts.free + counts.published + counts.dequeued, 3);
}

#[test]
fn gpu_failed_sampling_preparation_preserves_last_good_readback() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let first_content = patterned_canvas(27);
    let second_content = patterned_canvas(83);
    let mut resized_content = Canvas::new(8, 8);
    resized_content.fill(Rgba::new(170, 40, 210, 255));
    let first_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(10), &first_content)
        .expect("first media upload should succeed");
    let second_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(11), &second_content)
        .expect("second media upload should succeed");
    let resized_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(12), &resized_content)
        .expect("resized media upload should succeed");

    let first_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(first_frame)),
    );
    compositor
        .compose(&first_plan, true, None)
        .expect("last-good sampling compose should succeed");

    compositor.fail_next_sampling_readback_preparation();
    let resized_plan = CompositionPlan::single(
        8,
        8,
        CompositionLayer::replace(ProducerFrame::GpuTexture(resized_frame)),
    );
    let error = compositor
        .compose(&resized_plan, true, None)
        .expect_err("injected sampling preparation should fail");
    assert!(
        error
            .to_string()
            .contains("injected sampling readback preparation failure")
    );

    compositor
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("GPU wait for the retained sampling readback should succeed");
    let second_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(second_frame)),
    );
    let composed = compositor
        .compose(&second_plan, true, None)
        .expect("sampling should recover from failed replacement preparation");
    let sampling_canvas = composed
        .sampling_canvas
        .expect("the retained readback should still latch after preparation failure");
    assert_eq!(
        sampling_canvas.as_rgba_bytes(),
        first_content.as_rgba_bytes()
    );
}

#[test]
fn gpu_compositor_scales_preview_surface_to_requested_size() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
fn gpu_full_size_preview_stages_publication_without_sampling_canvas() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
        .expect("GPU composition should preserve a full-size GPU preview");

    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(composed.preview_surface.is_none());
    assert!(compositor.preview_surfaces.is_some());
    assert!(compositor.pending_preview_readback().is_some());
    assert!(compositor.has_pending_output_submission());
    assert!(compositor.cached_readback_surface.is_none());
    assert!(compositor.cached_preview_surfaces.is_empty());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 4);
    assert_eq!(preview_surface.height(), 4);
    assert!(compositor.cached_readback_surface.is_some());
    assert!(compositor.cached_preview_surfaces.is_empty());
}

#[test]
fn gpu_full_size_preview_uses_texture_copy_for_aligned_rows() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let plan = CompositionPlan::with_layers(
        64,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface_with_size(
                64,
                4,
                Rgba::new(255, 32, 0, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface_with_size(64, 4, Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };

    let composed = compositor
        .compose(&plan, false, Some(request))
        .expect("GPU composition should preserve a full-size GPU preview");

    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(composed.preview_surface.is_none());
    assert_eq!(
        compositor
            .preview_surfaces
            .as_ref()
            .expect("preview surfaces should be allocated")
            .scale_param_write_count,
        0
    );
    assert!(
        compositor
            .preview_surfaces
            .as_ref()
            .is_some_and(|surfaces| !surfaces.has_scale_output()),
        "direct texture copies should not allocate preview storage output"
    );

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 64);
    assert_eq!(preview_surface.height(), 4);
}

#[test]
fn gpu_scaled_preview_reuses_bind_groups_and_scale_params() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
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
    let counts = compositor.surface_pool_counts().preview;
    assert_eq!(counts.free + counts.published + counts.dequeued, 6);
}

#[test]
fn gpu_compositor_reuses_source_bind_groups_across_frames() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };

    let producer_frame = compositor
        .upload_canvas_frame(&patterned_canvas(7))
        .expect("producer canvas upload should succeed");

    let plan = |seed: u8| {
        CompositionPlan::with_layers(
            4,
            4,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(seed))),
                CompositionLayer::alpha(ProducerFrame::GpuTexture(producer_frame.clone()), 0.5),
            ],
        )
    };

    compositor
        .compose(&plan(1), false, None)
        .expect("first GPU-source compose should succeed");
    compositor
        .compose(&plan(2), false, None)
        .expect("second GPU-source compose should succeed");

    let surfaces = compositor
        .surfaces
        .as_ref()
        .expect("compose should allocate surfaces");
    assert_eq!(
        surfaces.compose_source_bind_groups.creation_count, 1,
        "the same GPU source texture should reuse its compose bind group"
    );
}

#[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
#[test]
fn gpu_blend_modes_flip_imported_frames_like_replace_path() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let width = 4;
    let height = 4;
    let imported_frame = |storage_id: u64| {
        let texture = compositor.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SparkleFlinger test flipped imported source"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        // Bottom-up source: physical row 0 is white, every other row black.
        let mut bgra = vec![0_u8; (width * height * 4) as usize];
        for pixel in 0..width as usize {
            bgra[pixel * 4..pixel * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
        for row in 1..height as usize {
            for pixel in 0..width as usize {
                let offset = (row * width as usize + pixel) * 4;
                bgra[offset + 3] = 255;
            }
        }
        compositor.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bgra,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        hypercolor_core::effect::ImportedEffectFrame {
            width,
            height,
            format: hypercolor_core::effect::ImportedFrameFormat::Bgra8Unorm,
            storage_id,
            texture: Arc::new(texture),
            view: Arc::new(view),
            timings: hypercolor_core::effect::ImportedFrameTimings::default(),
        }
    };

    let readback = |compositor: &mut GpuSparkleFlinger, plan: &CompositionPlan| {
        compositor
            .compose(plan, false, full_preview_request(plan))
            .expect("imported frame compose should succeed");
        resolve_preview_surface_blocking(compositor)
    };

    let replace_frame = imported_frame(1);
    let blend_frame = imported_frame(2);

    let replace_plan = CompositionPlan::single(
        width,
        height,
        CompositionLayer::replace(ProducerFrame::Gpu(replace_frame)),
    );
    let replace_output = readback(&mut compositor, &replace_plan);

    let blend_plan = CompositionPlan::with_layers(
        width,
        height,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas_with_size(
                width,
                height,
                Rgba::new(40, 40, 40, 255),
            ))),
            CompositionLayer::alpha(ProducerFrame::Gpu(blend_frame), 1.0),
        ],
    );
    let blend_output = readback(&mut compositor, &blend_plan);

    // The opaque source must land identically through the Replace direct-copy
    // shader and the blend path's in-shader flip.
    let replace_rgba = replace_output.rgba_bytes();
    let blend_rgba = blend_output.rgba_bytes();
    assert_eq!(replace_rgba.len(), blend_rgba.len());
    for (index, (replace_byte, blend_byte)) in
        replace_rgba.iter().zip(blend_rgba.iter()).enumerate()
    {
        assert!(
            replace_byte.abs_diff(*blend_byte) <= 1,
            "byte {index}: replace {replace_byte} vs blend {blend_byte}",
        );
    }

    // Bottom-up physical row 0 (white) must surface as the bottom output row.
    let bottom_row_offset = ((height - 1) * width * 4) as usize;
    assert_eq!(
        &blend_rgba[bottom_row_offset..bottom_row_offset + 4],
        &[255, 255, 255, 255],
        "flipped source bottom row should be white",
    );
    assert_eq!(
        &blend_rgba[0..4],
        &[0, 0, 0, 255],
        "flipped source top row should be black",
    );
}

mod display_finalize;
mod media_upload;
mod preview;
mod sampler;
mod shaders;
mod surface;
