use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, InputPublicationDemandRevision, PhysicalOrigin, PixelExtent, PlatformGpuApi,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenCursorCapabilities, ScreenExecutorColorCapabilities, ScreenExtentRequest,
    ScreenInputGraphGeneration, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
    ScreenNativePreparationPayload, ScreenNativeTargetPreparation,
    ScreenNativeTargetPreparationError, ScreenNativeTargetPreparer,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanBuilder, ScreenPlanGeneration,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenPublicationError,
    ScreenPublicationExecutor, ScreenPublicationExecutorFallbackReason,
    ScreenPublicationExecutorRequest, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenPublicationResidency, ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector,
    SourceScale,
};

#[path = "support/native_target.rs"]
mod native_target_support;

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn non_zero_u32(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn gpu_device(low_part: u32) -> ScreenPhysicalGpuDeviceIdentity {
    ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
        low_part,
        high_part: 7,
    }
}

fn target(
    id: u64,
    accepted_api: PlatformGpuApi,
    device: ScreenPhysicalGpuDeviceIdentity,
    max_texture_dimension: u32,
) -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(id).expect("test target identity is non-zero"),
        ),
        accepted_api,
        device,
        non_zero_u32(max_texture_dimension),
        native_target_support::preparer(),
    )
}

struct CountingPreparer {
    calls: Arc<AtomicUsize>,
}

impl ScreenNativeTargetPreparer for CountingPreparer {
    fn prepare(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenNativeTargetPreparation::new(platform.clone(), 1))
    }
}

struct WrongOutputPreparer {
    calls: Arc<AtomicUsize>,
    output: ScreenNativePreparationPayload,
}

impl ScreenNativeTargetPreparer for WrongOutputPreparer {
    fn prepare(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenNativeTargetPreparation::new(self.output.clone(), 1))
    }
}

fn counting_target(
    id: u64,
    device: ScreenPhysicalGpuDeviceIdentity,
    calls: Arc<AtomicUsize>,
) -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(id).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        device,
        non_zero_u32(16_384),
        Arc::new(CountingPreparer { calls }),
    )
}

#[test]
fn native_target_identity_ignores_preparer_object_identity() {
    assert_eq!(
        target(7, PlatformGpuApi::Direct3d11, gpu_device(3), 16_384),
        target(7, PlatformGpuApi::Direct3d11, gpu_device(3), 16_384),
    );
}

#[test]
fn native_target_rejects_cpu_and_foreign_descriptors_before_preparer_dispatch() {
    let device = gpu_device(4);
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let first = counting_target(1, device.clone(), Arc::clone(&calls));
    let second = counting_target(2, device, Arc::clone(&calls));
    let cpu = resolve_exact(&resolved_source, ScreenPublicationExecutorRequest::Cpu);
    let first_native = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(first.clone()),
    );
    let platform = ScreenNativePreparationPayload::new(
        first_native.descriptor(),
        ScreenPlanGeneration::default(),
        Arc::new(()),
    );
    let cpu_error = first
        .prepare(cpu.descriptor(), &platform)
        .expect_err("CPU descriptor is rejected before renderer dispatch");
    assert_eq!(
        cpu_error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::CpuDescriptor)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let target_error = second
        .prepare(first_native.descriptor(), &platform)
        .expect_err("foreign native descriptor is rejected before renderer dispatch");
    assert_eq!(
        target_error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::TargetMismatch)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let reused_id = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(NonZeroU64::MIN),
        PlatformGpuApi::Direct3d11,
        gpu_device(4),
        non_zero_u32(8_192),
        Arc::new(CountingPreparer {
            calls: Arc::clone(&calls),
        }),
    );
    let reused_id_error = reused_id
        .prepare(first_native.descriptor(), &platform)
        .expect_err("same identity with different target metadata is rejected");
    assert_eq!(
        reused_id_error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::TargetMismatch)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let other_source = source(
        extent(1280, 720),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(gpu_device(4)),
    );
    let other_native = resolve_exact(
        &other_source,
        ScreenPublicationExecutorRequest::SourceNative(first.clone()),
    );
    let payload_error = first
        .prepare(other_native.descriptor(), &platform)
        .expect_err("substituted platform payload is rejected before renderer dispatch");
    assert_eq!(
        payload_error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::PayloadDescriptorMismatch)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn native_target_rejects_substituted_preparer_output_before_binding() {
    let device = gpu_device(5);
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let foreign_target = counting_target(40, device.clone(), Arc::new(AtomicUsize::new(0)));
    let foreign = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(foreign_target),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let target = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(41).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        device,
        non_zero_u32(16_384),
        Arc::new(WrongOutputPreparer {
            calls: Arc::clone(&calls),
            output: ScreenNativePreparationPayload::new(
                foreign.descriptor(),
                ScreenPlanGeneration::default(),
                Arc::new(()),
            ),
        }),
    );
    let expected = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
    );
    let input = ScreenNativePreparationPayload::new(
        expected.descriptor(),
        ScreenPlanGeneration::default(),
        Arc::new(()),
    );

    let error = target
        .prepare(expected.descriptor(), &input)
        .expect_err("substituted target output is rejected before binding");

    assert_eq!(
        error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::PreparedPayloadMismatch)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

fn source(
    output_extent: PixelExtent,
    api: ScreenResourceApi,
    device: Option<ScreenPhysicalGpuDeviceIdentity>,
) -> ResolvedScreenSource {
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        output_extent,
        output_extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    let resources = match device {
        Some(device) => ScreenBackendResourceIdentity::new_with_physical_gpu_device(
            ScreenCaptureBackend::WindowsDesktopDuplication,
            api,
            device,
            3,
            5,
        ),
        None => ScreenBackendResourceIdentity::new(ScreenCaptureBackend::Synthetic, api, 3, 5),
    };
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new("native-negotiation")
                .expect("test source id is non-empty"),
            topology_generation: 11,
            session_generation: 13,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            output_extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenCursorCapabilities::clean_only(),
            resources,
        ),
    )
}

fn exact_profile() -> Arc<ScreenProcessingProfile> {
    Arc::new(ScreenProcessingProfile::new(
        ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Rgba8),
    ))
}

fn request(
    executor: ScreenPublicationExecutorRequest,
    profile: Arc<ScreenProcessingProfile>,
    requested_hz: u32,
) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            executor,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            profile,
        ),
        non_zero_u32(requested_hz),
    )
}

fn resolve_exact(
    source: &ResolvedScreenSource,
    executor: ScreenPublicationExecutorRequest,
) -> ResolvedScreenBranchDemand {
    request(executor, exact_profile(), 60)
        .resolve_with_executor_capabilities(source, ScreenExecutorColorCapabilities::NONE)
        .expect("exact byte-preserving request resolves")
}

fn assert_cpu_fallback(
    resolved: &ResolvedScreenBranchDemand,
    reason: ScreenPublicationExecutorFallbackReason,
) {
    assert_eq!(
        resolved.descriptor().executor(),
        &ScreenPublicationExecutor::Cpu
    );
    assert_eq!(
        resolved.descriptor().required_residency(),
        ScreenPublicationResidency::Cpu
    );
    assert_eq!(resolved.descriptor().executor_fallback(), Some(reason));
}

#[test]
fn native_negotiation_reports_source_api_and_device_fallbacks() {
    let device = gpu_device(41);
    let native_target = target(1, PlatformGpuApi::Direct3d11, device.clone(), 16_384);
    let matching_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let matching = resolve_exact(
        &matching_source,
        ScreenPublicationExecutorRequest::SourceNative(native_target.clone()),
    );

    assert_eq!(
        matching.descriptor().executor(),
        &ScreenPublicationExecutor::SourceNative(native_target.clone())
    );
    assert_eq!(matching.descriptor().executor_fallback(), None);
    assert_eq!(
        matching.descriptor().requested_executor(),
        &ScreenPublicationExecutorRequest::SourceNative(native_target.clone())
    );

    let cpu_source = source(extent(1920, 1080), ScreenResourceApi::Cpu, None);
    let cpu = resolve_exact(
        &cpu_source,
        ScreenPublicationExecutorRequest::SourceNative(native_target.clone()),
    );
    assert_cpu_fallback(&cpu, ScreenPublicationExecutorFallbackReason::CpuSource);

    let api_mismatch = resolve_exact(
        &matching_source,
        ScreenPublicationExecutorRequest::SourceNative(target(
            2,
            PlatformGpuApi::Vulkan,
            device.clone(),
            16_384,
        )),
    );
    assert_cpu_fallback(
        &api_mismatch,
        ScreenPublicationExecutorFallbackReason::PlatformApiMismatch,
    );

    let missing_device_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        None,
    );
    let missing_device = resolve_exact(
        &missing_device_source,
        ScreenPublicationExecutorRequest::SourceNative(native_target.clone()),
    );
    assert_cpu_fallback(
        &missing_device,
        ScreenPublicationExecutorFallbackReason::MissingPhysicalGpuDevice,
    );

    let device_mismatch = resolve_exact(
        &matching_source,
        ScreenPublicationExecutorRequest::SourceNative(target(
            3,
            PlatformGpuApi::Direct3d11,
            gpu_device(42),
            16_384,
        )),
    );
    assert_cpu_fallback(
        &device_mismatch,
        ScreenPublicationExecutorFallbackReason::PhysicalGpuDeviceMismatch,
    );
}

#[test]
fn native_target_limit_falls_back_without_changing_odd_geometry() {
    let device = gpu_device(51);
    let source = source(
        extent(641, 359),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let resolved = resolve_exact(
        &source,
        ScreenPublicationExecutorRequest::SourceNative(target(
            1,
            PlatformGpuApi::Direct3d11,
            device,
            640,
        )),
    );

    assert_cpu_fallback(
        &resolved,
        ScreenPublicationExecutorFallbackReason::TargetDimensionLimitExceeded,
    );
    assert_eq!(
        resolved.descriptor().geometry().output_extent(),
        extent(641, 359)
    );
}

#[test]
fn native_negotiation_preserves_exact_8k_geometry() {
    let device = gpu_device(61);
    let native_target = target(1, PlatformGpuApi::Direct3d11, device.clone(), 8192);
    let source = source(
        extent(7680, 4320),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device),
    );
    let resolved = resolve_exact(
        &source,
        ScreenPublicationExecutorRequest::SourceNative(native_target.clone()),
    );

    assert_eq!(
        resolved.descriptor().geometry().output_extent(),
        extent(7680, 4320)
    );
    assert_eq!(
        resolved.descriptor().executor(),
        &ScreenPublicationExecutor::SourceNative(native_target)
    );
}

#[test]
fn native_grouping_uses_target_identity_and_coalesces_equal_targets() {
    let device = gpu_device(71);
    let source = source(
        extent(3840, 2160),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let first_target = target(1, PlatformGpuApi::Direct3d11, device.clone(), 16_384);
    let second_target = target(2, PlatformGpuApi::Direct3d11, device, 16_384);
    let first = resolve_exact(
        &source,
        ScreenPublicationExecutorRequest::SourceNative(first_target.clone()),
    );
    let second = resolve_exact(
        &source,
        ScreenPublicationExecutorRequest::SourceNative(second_target),
    );
    let mut separate_builder = ScreenPlanBuilder::new();
    let separate = separate_builder
        .prepare(
            [first.clone(), second],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("distinct target contexts are admitted");
    assert_eq!(separate.candidate_plan().physical_reductions().len(), 2);

    let same_target = request(
        ScreenPublicationExecutorRequest::SourceNative(first_target),
        exact_profile(),
        30,
    )
    .resolve_with_executor_capabilities(&source, ScreenExecutorColorCapabilities::NONE)
    .expect("same target demand resolves");
    let mut shared_builder = ScreenPlanBuilder::new();
    let shared = shared_builder
        .prepare(
            [first, same_target],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("equal target contexts are admitted");
    assert_eq!(shared.candidate_plan().branches().len(), 1);
    assert_eq!(shared.candidate_plan().physical_reductions().len(), 1);
}

#[test]
fn lane_specific_color_capabilities_choose_native_or_exact_cpu() {
    let device = gpu_device(81);
    let source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let native_target = target(1, PlatformGpuApi::Direct3d11, device, 16_384);
    let profile = Arc::new(ScreenProcessingProfile::default());
    let linear_light =
        ScreenColorTransformCapabilities::new(true, false, false, profile.algorithm_revision());
    let registered = request(
        ScreenPublicationExecutorRequest::SourceNative(native_target.clone()),
        Arc::clone(&profile),
        60,
    );

    let cpu_fallback = registered
        .resolve_with_executor_capabilities(
            &source,
            ScreenExecutorColorCapabilities::new(
                linear_light,
                ScreenColorTransformCapabilities::NONE,
            ),
        )
        .expect("CPU lane implements the native target's missing transform");
    assert_cpu_fallback(
        &cpu_fallback,
        ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported,
    );

    let native = registered
        .resolve_with_executor_capabilities(
            &source,
            ScreenExecutorColorCapabilities::new(
                ScreenColorTransformCapabilities::NONE,
                linear_light,
            ),
        )
        .expect("native lane implements the requested transform");
    assert_eq!(
        native.descriptor().executor(),
        &ScreenPublicationExecutor::SourceNative(native_target)
    );
    assert_eq!(native.descriptor().executor_fallback(), None);

    assert_eq!(
        registered
            .resolve_with_executor_capabilities(&source, ScreenExecutorColorCapabilities::NONE,),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
}

#[test]
fn explicit_cpu_and_native_fallback_share_physical_reduction() {
    let device = gpu_device(91);
    let source = source(
        extent(2560, 1440),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let cpu = resolve_exact(&source, ScreenPublicationExecutorRequest::Cpu);
    let fallback = resolve_exact(
        &source,
        ScreenPublicationExecutorRequest::SourceNative(target(
            1,
            PlatformGpuApi::Vulkan,
            device,
            16_384,
        )),
    );

    assert_eq!(
        cpu.descriptor().physical(),
        fallback.descriptor().physical()
    );
    assert_ne!(cpu.descriptor(), fallback.descriptor());
    assert_cpu_fallback(
        &fallback,
        ScreenPublicationExecutorFallbackReason::PlatformApiMismatch,
    );

    let mut builder = ScreenPlanBuilder::new();
    let preparing = builder
        .prepare(
            [cpu, fallback],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("CPU-equivalent logical branches share physical work");
    assert_eq!(preparing.candidate_plan().branches().len(), 2);
    assert_eq!(preparing.candidate_plan().physical_reductions().len(), 1);
}
