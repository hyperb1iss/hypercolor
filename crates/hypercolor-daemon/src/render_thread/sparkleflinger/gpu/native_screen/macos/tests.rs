use std::num::NonZeroU64;

use hypercolor_core::input::screen::CapturePixelFormat;
use hypercolor_core::input::screen::ScreenNativeExecutionTargetId;
use hypercolor_macos_gpu_interop::{MacosGpuInteropError, MacosNativeTargetFormat};

use super::contract::{UnsupportedMacosNativeTargetFormat, macos_native_target_format};
use super::import::{is_transient_copy_failure, validate_target_id};

fn target_id(value: u64) -> ScreenNativeExecutionTargetId {
    ScreenNativeExecutionTargetId::new(NonZeroU64::new(value).expect("target id is non-zero"))
}

#[test]
fn failed_target_generation_is_fenced_after_replacement() {
    let failed = target_id(11);
    let replacement = target_id(12);

    validate_target_id(replacement, replacement).expect("the current target remains valid");
    let error = validate_target_id(failed, replacement)
        .expect_err("a publication prepared for the failed target must be rejected");
    assert!(error.to_string().contains("fenced"));
}

#[test]
fn only_explicit_gpu_fence_pressure_defers_copy() {
    let transient = anyhow::Error::new(MacosGpuInteropError::IosurfaceFenceTimeout)
        .context("native screen import did not complete");
    assert!(is_transient_copy_failure(&transient));
    assert!(!is_transient_copy_failure(&anyhow::anyhow!(
        "structural native screen import failure"
    )));
}
#[test]
fn target_formats_reject_disguised_source_storage() {
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Rgba8)
            .expect("RGBA8 is a native reduction target"),
        MacosNativeTargetFormat::Rgba8,
    );
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Bgra8)
            .expect("BGRA8 is a native reduction target"),
        MacosNativeTargetFormat::Bgra8,
    );
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Argb2101010)
            .expect_err("source-only storage must not masquerade as a target"),
        UnsupportedMacosNativeTargetFormat(CapturePixelFormat::Argb2101010),
    );
}

use std::num::NonZeroU32;

use std::time::{Duration, Instant};

use hypercolor_core::input::screen::consumer::{
    CaptureEpoch, CaptureSourceId, PixelExtent, ScreenByteAdmissionCoordinator,
};

use hypercolor_core::input::screen::implementer::{
    CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureGeometry,
    CaptureLuminanceContext, CapturePositiveScalar, CaptureRotation, CaptureTransferFunction,
    KnownCaptureColorimetry, PhysicalOrigin, PlatformGpuApi, PlatformGpuSurface,
    ScreenBranchPayload, ScreenBranchPublication, ScreenNativeWorkPayload,
    ScreenPublicationColorimetry, ScreenPublicationHealth, ScreenPublicationMetadata,
    ScreenWorkerExactLedgerBuilder, SourceScale,
};

use hypercolor_core::input::screen::planner::{
    InputPublicationDemandRevision, LedToneMapCalibration, PreparedLedToneMap,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenExecutorColorCapabilities, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenLetterboxFill, ScreenNativeExecutionTarget, ScreenNativePreparationPayload,
    ScreenNativeRetentionQuote, ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanBuilder, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationExecutor, ScreenPublicationExecutorRequest,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenPublicationSlotPolicy,
    ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy,
};

use hypercolor_macos_capture::{
    MacosCaptureColorimetry, MacosCaptureFrame, MacosCaptureGeometry, MacosCapturePixelFormat,
    MacosCaptureSurface, MacosChromaLocation, MacosColorPrimaries, MacosColorRange,
    MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosScale, MacosTransferFunction,
    MacosYuvMatrix,
};

use hypercolor_macos_gpu_interop::{
    MacosNativeColorTransform, MacosNativeOutputTransfer, MacosNativeReductionDescriptor,
    MacosNativeReductionFilter,
};

use super::super::NativeScreenCopyOutcome;
use super::{
    MacosScreenBridge, MacosScreenGpuRecoveryState, MacosScreenHost, PreparedMacosScreenTarget,
    macos_screen_lease, prepared_macos_screen_target_exclusive_bytes,
    prepared_macos_screen_target_retention,
};
use crate::render_thread::producer_queue::GpuTextureFrameOrigin;
use crate::render_thread::producer_queue::{GpuTextureFrame, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger;
use crate::render_thread::sparkleflinger::gpu::MediaTextureSourceKey;
use crate::render_thread::sparkleflinger::gpu::tests::{
    full_preview_request, gpu_test_compositor, patterned_canvas, resolve_preview_surface_blocking,
    sampling_layout, solid_canvas,
};
use crate::render_thread::sparkleflinger::{CompositionLayer, CompositionPlan};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::spatial::SamplingMode;
use std::sync::Arc;
use std::sync::mpsc;

fn macos_host(compositor: &mut GpuSparkleFlinger) -> &mut MacosScreenHost {
    compositor
        .screen_bridge_mut::<MacosScreenHost>()
        .expect("Metal compositor installs the macOS screen host")
}

fn screen_bridge(compositor: &mut GpuSparkleFlinger) -> Option<Arc<MacosScreenBridge>> {
    macos_host(compositor).bridge().cloned()
}

fn recovery(compositor: &mut GpuSparkleFlinger) -> MacosScreenGpuRecoveryState {
    macos_host(compositor).recovery().clone()
}

fn finish_copy(
    compositor: &mut GpuSparkleFlinger,
    result: anyhow::Result<Option<GpuTextureFrame>>,
) -> NativeScreenCopyOutcome {
    compositor
        .with_screen_bridge(|bridge, gpu| {
            bridge
                .as_any_mut()
                .downcast_mut::<MacosScreenHost>()
                .expect("Metal compositor installs the macOS screen host")
                .finish_copy(gpu, result)
        })
        .expect("Metal compositor installs a screen bridge")
}

#[test]
fn macos_structural_failure_clears_output_and_publishes_replacement_target() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let failed_target_id = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes an initial screen target")
        .id();
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor retains its initial screen bridge");
    let failed_publication = publish_macos_screen_fixture(&mut compositor, NonZeroU64::MIN);
    let ScreenBranchPayload::NativeWork(failed_payload) = failed_publication.publication.payload()
    else {
        panic!("failed-target fixture publishes native renderer work");
    };
    let failed_capture = failed_payload
        .source()
        .owner::<MacosCaptureFrame>()
        .expect("failed-target fixture retains its capture")
        .downgrade()
        .upgrade()
        .expect("failed-target capture remains live");
    let _ = bridge
        .import_frame(&compositor.device, 17, failed_capture)
        .expect("failed-target fixture seeds both native screen cache layers");
    assert!(!bridge.capture_caches_are_empty());
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            10, 20, 30, 255,
        )))),
    );
    compositor
        .compose(&plan, false, None)
        .expect("fixture output composes");
    assert!(compositor.current_output.is_some());

    macos_host(&mut compositor).fail_next_import = true;
    let outcome = compositor.copy_screen_publication(&failed_publication.publication);
    assert!(matches!(outcome, NativeScreenCopyOutcome::Invalidated(_)));
    assert!(compositor.current_output.is_none());
    assert!(compositor.cached_composition_key.is_none());
    assert!(compositor.cached_readback_surface.is_none());
    assert!(compositor.cached_preview_surfaces.is_empty());
    assert!(compositor.ready_preview_surface.is_none());
    assert!(compositor.cached_sample_result.is_none());
    assert!(bridge.capture_caches_are_empty());
    let replacement_target_id = compositor
        .screen_native_execution_target()
        .expect("structural failure publishes a replacement target")
        .id();
    assert_ne!(replacement_target_id, failed_target_id);
    assert_eq!(
        recovery(&mut compositor),
        MacosScreenGpuRecoveryState::Ready {
            target_id: replacement_target_id,
        }
    );

    let stale_outcome = compositor.copy_screen_publication(&failed_publication.publication);
    assert!(matches!(
        stale_outcome,
        NativeScreenCopyOutcome::Invalidated(_)
    ));
    assert_eq!(
        compositor
            .screen_native_execution_target()
            .expect("stale publication cannot replace the current target")
            .id(),
        replacement_target_id,
    );

    let replacement_bridge =
        screen_bridge(&mut compositor).expect("replacement bridge remains committed");
    let replacement_publication = publish_macos_screen_fixture(
        &mut compositor,
        NonZeroU64::new(2).expect("replacement sequence is non-zero"),
    );
    let replacement_frame =
        match compositor.copy_screen_publication(&replacement_publication.publication) {
            NativeScreenCopyOutcome::Copied(frame) => frame,
            outcome => panic!("replacement publication must copy, got {outcome:?}"),
        };
    let replacement_plan = CompositionPlan::single(
        replacement_frame.width,
        replacement_frame.height,
        CompositionLayer::replace(ProducerFrame::GpuTexture(replacement_frame)),
    );
    compositor
        .compose(&replacement_plan, false, None)
        .expect("replacement native publication composes");
    assert!(compositor.current_output.is_some());
    assert!(!replacement_bridge.capture_caches_are_empty());

    macos_host(&mut compositor).fail_next_import = true;
    let repeated = compositor.copy_screen_publication(&replacement_publication.publication);
    assert!(matches!(repeated, NativeScreenCopyOutcome::Invalidated(_)));
    assert!(compositor.current_output.is_none());
    assert!(replacement_bridge.capture_caches_are_empty());
    let second_replacement_target_id = compositor
        .screen_native_execution_target()
        .expect("repeated failure publishes another replacement target")
        .id();
    assert_ne!(second_replacement_target_id, replacement_target_id);
    assert_ne!(second_replacement_target_id, failed_target_id);
}

#[test]
fn macos_rebuild_failure_is_unavailable_until_a_valid_replacement() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let failed_target_id = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes an initial screen target")
        .id();
    macos_host(&mut compositor).fail_next_rebuild = true;

    let outcome = finish_copy(
        &mut compositor,
        Err(anyhow::anyhow!("injected repeated import failure")),
    );
    assert!(matches!(outcome, NativeScreenCopyOutcome::Unavailable(_)));
    assert!(screen_bridge(&mut compositor).is_none());
    assert!(compositor.screen_target.is_none());
    let MacosScreenGpuRecoveryState::Unavailable {
        failed_target_id: Some(target_id),
        error,
    } = recovery(&mut compositor)
    else {
        panic!("failed reconstruction remains typed unavailable");
    };
    assert_eq!(target_id, failed_target_id);
    assert!(error.contains("native screen reconstruction failed"));
    assert!(error.contains("injected macOS screen reconstruction failure"));

    let replacement_target_id = compositor
        .screen_native_execution_target()
        .expect("a later valid reconstruction restores native execution")
        .id();
    assert_ne!(replacement_target_id, failed_target_id);
    assert!(matches!(
        recovery(&mut compositor),
        MacosScreenGpuRecoveryState::Ready { target_id }
            if target_id == replacement_target_id
    ));
}

#[test]
fn repeated_macos_structural_failures_never_reuse_a_failed_target() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let first = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes an initial screen target")
        .id();
    let first_outcome = finish_copy(
        &mut compositor,
        Err(anyhow::anyhow!("first structural failure")),
    );
    assert!(matches!(
        first_outcome,
        NativeScreenCopyOutcome::Invalidated(_)
    ));
    let second = compositor
        .screen_native_execution_target()
        .expect("first replacement is available")
        .id();
    let second_outcome = finish_copy(
        &mut compositor,
        Err(anyhow::anyhow!("second structural failure")),
    );
    assert!(matches!(
        second_outcome,
        NativeScreenCopyOutcome::Invalidated(_)
    ));
    let third = compositor
        .screen_native_execution_target()
        .expect("second replacement is available")
        .id();

    assert_ne!(first, second);
    assert_ne!(second, third);
    assert_ne!(first, third);
}

#[test]
fn metal_compositor_registers_and_composes_native_capture() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let target = compositor
        .screen_native_execution_target()
        .expect("Metal compositor should expose a native screen target")
        .clone();
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor should retain its screen bridge");
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
            native_screen_lease: None,
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

struct MacosLeaseTargetPreparer {
    bridge: Arc<MacosScreenBridge>,
}

impl ScreenNativeTargetPreparer for MacosLeaseTargetPreparer {
    fn quote_retained_bytes(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        prepared_macos_screen_target_exclusive_bytes(descriptor)
    }

    fn quote_retention(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeRetentionQuote> {
        prepared_macos_screen_target_retention(descriptor)
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
            prepared_macos_screen_target_retention(descriptor)?,
        ))
    }
}

#[test]
fn equal_native_physical_descriptors_share_the_reduction_target() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let target = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes a native screen target")
        .clone();
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor retains its screen bridge");
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
                ScreenCaptureBackend::ScreenCaptureKit,
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

struct MacosPublishedSurfaceFixture {
    publication: Arc<ScreenBranchPublication>,
    coordinator: ScreenByteAdmissionCoordinator,
}

fn publish_macos_screen_fixture(
    compositor: &mut GpuSparkleFlinger,
    native_sequence: NonZeroU64,
) -> MacosPublishedSurfaceFixture {
    let registered_target = compositor
        .screen_native_execution_target()
        .expect("Metal compositor exposes a native screen target")
        .clone();
    let bridge = screen_bridge(compositor).expect("Metal compositor retains its screen bridge");
    let target = ScreenNativeExecutionTarget::new(
        registered_target.id(),
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
                ScreenCaptureBackend::ScreenCaptureKit,
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
    let hub = builder.publication_hub();
    let revision = InputPublicationDemandRevision::new(1);
    let graph = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            [demand],
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
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .unwrap_or_else(|failure| panic!("native lease plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, revision, graph)
        .unwrap_or_else(|failure| panic!("native lease plan commits: {}", failure.error()));
    let (_, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("initial native lease plan has no retired readers");
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
    let committed_state = builder.committed_state();
    let binding = committed_state
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == &source_id)
        .expect("native lease committed state retains its worker binding");
    let publisher = hub
        .publisher(&descriptor, binding)
        .expect("native lease branch issues a committed publisher");
    let now = Instant::now();
    let metadata = ScreenPublicationMetadata::try_new(
        descriptor.source_epoch().clone(),
        publisher.plan_generation(),
        native_sequence,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    )
    .expect("native lease publication timeline is valid");
    hub.publish(
        &publisher,
        ScreenBranchPayload::NativeWork(ScreenNativeWorkPayload::new(
            ScreenPublicationColorimetry::new(descriptor.source_colorimetry()),
            &surface,
        )),
        &metadata,
    )
    .expect("native lease surface publishes");
    let publication = hub
        .lease(&descriptor)
        .expect("native lease branch remains committed")
        .read()
        .expect("native lease branch retains its publication");
    MacosPublishedSurfaceFixture {
        publication,
        coordinator,
    }
}

#[test]
fn macos_texture_lease_retains_exclusive_shared_and_capture_admissions() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor retains its screen bridge");
    let fixture = publish_macos_screen_fixture(&mut compositor, NonZeroU64::MIN);
    let ScreenBranchPayload::NativeWork(payload) = fixture.publication.payload() else {
        panic!("native lease fixture publishes native renderer work");
    };
    let surface = payload.source();
    let capture_owner = surface
        .owner::<MacosCaptureFrame>()
        .expect("surface retains the capture owner");
    let target_owner = surface
        .retained_owner::<PreparedMacosScreenTarget>()
        .expect("surface retains the renderer owner");
    let target_lifetime = surface
        .resource_lifetime()
        .cloned()
        .expect("surface retains the renderer allocation");
    let shared_target_lifetime = surface.shared_resource_lifetime().cloned();
    let capture_lifetime = surface
        .capture_resource_lifetime()
        .cloned()
        .expect("surface retains the capture allocation");
    let capture = capture_owner
        .downgrade()
        .upgrade()
        .expect("fixture capture remains retained");
    let imported = bridge
        .interop
        .import_frame(&compositor.device, 7, capture)
        .expect("lease fixture imports");
    let lease = macos_screen_lease(
        imported,
        capture_owner,
        target_owner,
        target_lifetime,
        shared_target_lifetime,
        capture_lifetime,
    );
    let retained_bytes = fixture.coordinator.snapshot().reserved_bytes();
    assert!(retained_bytes > 0);
    drop(fixture.publication);
    assert!(fixture.coordinator.snapshot().reserved_bytes() > 0);
    drop(lease);
    assert_eq!(fixture.coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn native_metal_reduction_feeds_gpu_zone_sampling_without_readback() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor retains its screen bridge");
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
            native_screen_lease: None,
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

#[test]
fn native_metal_color_pipeline_matches_shared_sdr_p3_pq_hlg_extended_linear_and_yuv_vectors() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor should retain its screen bridge");
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

#[test]
fn native_metal_sdr_output_transfers_match_the_shared_encoder() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let bridge =
        screen_bridge(&mut compositor).expect("Metal compositor should retain its screen bridge");
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

#[test]
fn macos_structural_recovery_clears_latched_sampling_output() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let first_content = patterned_canvas(19);
    let second_content = patterned_canvas(73);
    let third_content = patterned_canvas(141);
    let first_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(31), &first_content)
        .expect("first native recovery sampling upload succeeds");
    let second_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(32), &second_content)
        .expect("second native recovery sampling upload succeeds");
    let third_frame = compositor
        .upload_media_canvas_frame(MediaTextureSourceKey::for_test(33), &third_content)
        .expect("third native recovery sampling upload succeeds");
    let first_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(first_frame)),
    );
    compositor
        .compose(&first_plan, true, None)
        .expect("first native recovery sampling compose succeeds");
    compositor
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("native recovery sampling readback completes");
    let second_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(second_frame)),
    );
    let before_recovery = compositor
        .compose(&second_plan, true, None)
        .expect("second native recovery sampling compose succeeds");
    assert_eq!(
        before_recovery
            .sampling_canvas
            .expect("pre-recovery compose exposes its latched predecessor")
            .as_rgba_bytes(),
        first_content.as_rgba_bytes(),
    );

    let outcome = finish_copy(
        &mut compositor,
        Err(anyhow::anyhow!("injected structural import failure")),
    );
    assert!(matches!(outcome, NativeScreenCopyOutcome::Invalidated(_)));

    let third_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(third_frame)),
    );
    let after_recovery = compositor
        .compose(&third_plan, true, None)
        .expect("post-recovery sampling compose succeeds");
    assert!(after_recovery.sampling_canvas.is_none());
    assert!(after_recovery.sampling_surface.is_none());
}
