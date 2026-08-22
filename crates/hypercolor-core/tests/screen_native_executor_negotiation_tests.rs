use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hypercolor_core::input::screen::{
    AdmittedScreenNativeTargetPreparation, CaptureColorimetry, CaptureEpoch, CaptureGeometry,
    CapturePixelFormat, CaptureRotation, CaptureSourceId, InputPublicationDemandRevision,
    PhysicalOrigin, PixelExtent, PlatformGpuApi, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenByteAdmissionCoordinator, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenCursorCapabilities, ScreenExecutorColorCapabilities, ScreenExtentRequest,
    ScreenInputGraphGeneration, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
    ScreenNativePreparationPayload, ScreenNativeRetentionQuote, ScreenNativeTargetPreparation,
    ScreenNativeTargetPreparationError, ScreenNativeTargetPreparer,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanBuilder, ScreenPlanGeneration,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenPublicationError,
    ScreenPublicationExecutor, ScreenPublicationExecutorFallbackReason,
    ScreenPublicationExecutorRequest, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenPublicationResidency, ScreenPublicationSlotPolicy, ScreenResourceApi,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerExactLedgerBuilder,
    ScreenWorkerLedgerBuildError, SourceScale,
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

#[test]
fn native_target_carries_its_exact_color_capabilities() {
    let capabilities = ScreenColorTransformCapabilities::new(
        true,
        true,
        true,
        NonZeroU32::new(7).expect("test revision is nonzero"),
    );
    let target = target(1, PlatformGpuApi::Direct3d11, gpu_device(1), 16_384)
        .with_color_capabilities(capabilities);

    assert_eq!(target.color_capabilities(), capabilities);
}

struct CountingPreparer {
    calls: Arc<AtomicUsize>,
}

struct SplitCountingPreparer {
    calls: Arc<AtomicUsize>,
}

struct DispatchCountingPreparer {
    quote_calls: Arc<AtomicUsize>,
    prepare_calls: Arc<AtomicUsize>,
}

impl ScreenNativeTargetPreparer for CountingPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(1)
    }

    fn prepare(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(()),
            ),
            1,
        ))
    }
}

impl ScreenNativeTargetPreparer for SplitCountingPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(1)
    }

    fn quote_retention(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeRetentionQuote> {
        Ok(ScreenNativeRetentionQuote::split(1, 1))
    }

    fn prepare(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(()),
            ),
            ScreenNativeRetentionQuote::split(1, 1),
        ))
    }
}

impl ScreenNativeTargetPreparer for DispatchCountingPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        self.quote_calls.fetch_add(1, Ordering::Relaxed);
        Ok(1)
    }

    fn prepare(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        self.prepare_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(()),
            ),
            1,
        ))
    }
}

struct WrongOutputPreparer {
    calls: Arc<AtomicUsize>,
    output_descriptor: hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
    output_generation: ScreenPlanGeneration,
}

struct MisquotingPreparer;

struct MisquotingSharedPreparer;

impl ScreenNativeTargetPreparer for MisquotingPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(7)
    }

    fn prepare(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(()),
            ),
            8,
        ))
    }
}

impl ScreenNativeTargetPreparer for MisquotingSharedPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(7)
    }

    fn quote_retention(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeRetentionQuote> {
        Ok(ScreenNativeRetentionQuote::split(7, 11))
    }

    fn prepare(
        &self,
        descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(()),
            ),
            ScreenNativeRetentionQuote::split(7, 12),
        ))
    }
}

impl ScreenNativeTargetPreparer for WrongOutputPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(1)
    }

    fn prepare(
        &self,
        _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                &self.output_descriptor,
                self.output_generation,
                Arc::new(()),
            ),
            1,
        ))
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
    let cpu_error = prepare_with_admission(
        &resolved_source,
        &first_native,
        &first,
        cpu.descriptor(),
        &platform,
    )
    .expect_err("CPU descriptor is rejected before renderer dispatch");
    assert_eq!(
        cpu_error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::CpuDescriptor)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let target_error = prepare_with_admission(
        &resolved_source,
        &first_native,
        &second,
        first_native.descriptor(),
        &platform,
    )
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
    let reused_id_error = prepare_with_admission(
        &resolved_source,
        &first_native,
        &reused_id,
        first_native.descriptor(),
        &platform,
    )
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
    let payload_error = prepare_with_admission(
        &resolved_source,
        &first_native,
        &first,
        other_native.descriptor(),
        &platform,
    )
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
            output_descriptor: foreign.descriptor().clone(),
            output_generation: ScreenPlanGeneration::default(),
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

    let error = prepare_with_admission(
        &resolved_source,
        &expected,
        &target,
        expected.descriptor(),
        &input,
    )
    .expect_err("substituted target output is rejected before binding");

    assert_eq!(
        error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(&ScreenNativeTargetPreparationError::PreparedPayloadMismatch)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn native_target_rejects_foreign_plan_generation_before_quote_or_prepare() {
    let device = gpu_device(10);
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let quote_calls = Arc::new(AtomicUsize::new(0));
    let prepare_calls = Arc::new(AtomicUsize::new(0));
    let target = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(91).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        device,
        non_zero_u32(16_384),
        Arc::new(DispatchCountingPreparer {
            quote_calls: Arc::clone(&quote_calls),
            prepare_calls: Arc::clone(&prepare_calls),
        }),
    );
    let demand = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
    );
    let mut plan_builder = ScreenPlanBuilder::new();
    let mut preparing = plan_builder
        .prepare(
            [demand.clone()],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("foreign-generation regression plan prepares");
    let ticket = preparing
        .worker_ticket(&resolved_source.epoch().source_id)
        .expect("foreign-generation regression owns one worker ticket");
    assert_ne!(ticket.plan_generation(), ScreenPlanGeneration::default());
    let foreign = ScreenNativePreparationPayload::new(
        demand.descriptor(),
        ScreenPlanGeneration::default(),
        Arc::new(()),
    );
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("native ledger metadata prepares");

    let error = ledger
        .prepare_native_target(
            &target,
            demand.descriptor(),
            &foreign,
            "native-foreign-generation",
            "worker-runtime-total",
        )
        .expect_err("foreign plan generation is rejected before renderer dispatch");
    assert!(matches!(
        error.downcast_ref::<ScreenWorkerLedgerBuildError>(),
        Some(ScreenWorkerLedgerBuildError::NativePlanGenerationMismatch { .. })
    ));
    assert_eq!(quote_calls.load(Ordering::Relaxed), 0);
    assert_eq!(prepare_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn native_target_rejects_renderer_allocation_drift_from_preflight_quote() {
    let device = gpu_device(6);
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let target = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(42).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        device,
        non_zero_u32(16_384),
        Arc::new(MisquotingPreparer),
    );
    let resolved = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
    );
    let mut plan_builder = ScreenPlanBuilder::new();
    let mut preparing = plan_builder
        .prepare(
            [resolved.clone()],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("native test plan prepares");
    let ticket = preparing
        .worker_ticket(&resolved_source.epoch().source_id)
        .expect("native source owns one worker ticket");
    let platform = ScreenNativePreparationPayload::new(
        resolved.descriptor(),
        ticket.plan_generation(),
        Arc::new(()),
    );
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("native ledger metadata prepares");
    let error = ledger
        .prepare_native_target(
            &target,
            resolved.descriptor(),
            &platform,
            "native-negotiation-drift",
            "worker-runtime-total",
        )
        .expect_err("allocation drift from the admitted quote is rejected");

    assert_eq!(
        error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(
            &ScreenNativeTargetPreparationError::PreparedRetainedBytesMismatch {
                quoted: 7,
                actual: 8,
            }
        )
    );
}

#[test]
fn native_target_rejects_shared_allocation_drift_from_preflight_quote() {
    let device = gpu_device(8);
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let target = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(43).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        device,
        non_zero_u32(16_384),
        Arc::new(MisquotingSharedPreparer),
    );
    let resolved = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
    );
    let platform = ScreenNativePreparationPayload::new(
        resolved.descriptor(),
        ScreenPlanGeneration::default(),
        Arc::new(()),
    );
    let error = prepare_with_admission(
        &resolved_source,
        &resolved,
        &target,
        resolved.descriptor(),
        &platform,
    )
    .expect_err("shared allocation drift from the admitted quote is rejected");

    assert_eq!(
        error.downcast_ref::<ScreenNativeTargetPreparationError>(),
        Some(
            &ScreenNativeTargetPreparationError::PreparedSharedRetainedBytesMismatch {
                quoted: 11,
                actual: 12,
            }
        )
    );
}

#[test]
fn admitted_native_target_keeps_its_quote_after_builder_and_plan_drop() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(gpu_device(7)),
    );
    let target = counting_target(70, gpu_device(7), Arc::new(AtomicUsize::new(0)));
    let demand = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
    );
    let mut plan_builder = ScreenPlanBuilder::with_publication_slots_and_admission(
        ScreenPublicationSlotPolicy::default(),
        coordinator.clone(),
    );
    let mut preparing = plan_builder
        .prepare(
            [demand.clone()],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("native escape regression plan prepares");
    let ticket = preparing
        .worker_ticket(&resolved_source.epoch().source_id)
        .expect("native escape regression owns one worker ticket");
    let platform = ScreenNativePreparationPayload::new(
        demand.descriptor(),
        ticket.plan_generation(),
        Arc::new(()),
    );
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("native ledger metadata prepares");
    let admitted = ledger
        .prepare_native_target(
            &target,
            demand.descriptor(),
            &platform,
            "native-escape-regression",
            "worker-runtime-total",
        )
        .expect("native target prepares under one dedicated byte lease");

    drop(ledger);
    drop(preparing.abort());
    assert_eq!(coordinator.snapshot().reserved_bytes(), 1);
    drop(admitted);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn shared_admission_failure_never_dispatches_renderer_preparation() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let resolved_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(gpu_device(9)),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let target = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(90).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        gpu_device(9),
        non_zero_u32(16_384),
        Arc::new(SplitCountingPreparer {
            calls: Arc::clone(&calls),
        }),
    );
    let demand = resolve_exact(
        &resolved_source,
        ScreenPublicationExecutorRequest::SourceNative(target.clone()),
    );
    let mut plan_builder = ScreenPlanBuilder::with_publication_slots_and_admission(
        ScreenPublicationSlotPolicy::default(),
        coordinator.clone(),
    );
    let mut preparing = plan_builder
        .prepare(
            [demand.clone()],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("split admission regression plan prepares");
    let ticket = preparing
        .worker_ticket(&resolved_source.epoch().source_id)
        .expect("split admission regression owns one worker ticket");
    let modeled_bytes = coordinator.snapshot().reserved_bytes();
    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(
            modeled_bytes + 1,
            modeled_bytes + 1,
        ))
        .expect("one exclusive byte remains available");
    let platform = ScreenNativePreparationPayload::new(
        demand.descriptor(),
        ticket.plan_generation(),
        Arc::new(()),
    );
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("native ledger metadata prepares");

    ledger
        .prepare_native_target(
            &target,
            demand.descriptor(),
            &platform,
            "native-shared-admission-failure",
            "worker-runtime-total",
        )
        .expect_err("shared physical byte exceeds the remaining exact capacity");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(coordinator.snapshot().reserved_bytes(), modeled_bytes);
    drop(preparing.abort());
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

fn prepare_with_admission(
    source: &ResolvedScreenSource,
    ticket_demand: &ResolvedScreenBranchDemand,
    target: &ScreenNativeExecutionTarget,
    descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
    platform: &ScreenNativePreparationPayload,
) -> anyhow::Result<AdmittedScreenNativeTargetPreparation> {
    let mut plan_builder = ScreenPlanBuilder::new();
    let mut preparing = plan_builder.prepare(
        [ticket_demand.clone()],
        None,
        InputPublicationDemandRevision::new(1),
        ScreenInputGraphGeneration::new(1),
        ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
    )?;
    let ticket = preparing
        .worker_ticket(&source.epoch().source_id)
        .expect("native test source owns one worker ticket");
    let platform = ScreenNativePreparationPayload::new(
        platform.descriptor(),
        ticket.plan_generation(),
        Arc::new(()),
    );
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
    ledger.prepare_native_target(
        target,
        descriptor,
        &platform,
        "native-negotiation-test",
        "worker-runtime-total",
    )
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
fn required_native_negotiation_rejects_every_structural_fallback() {
    let device = gpu_device(43);
    let matching_source = source(
        extent(1920, 1080),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        Some(device.clone()),
    );
    let cases = [
        (
            source(extent(1920, 1080), ScreenResourceApi::Cpu, None),
            target(1, PlatformGpuApi::Direct3d11, device.clone(), 16_384),
            ScreenPublicationExecutorFallbackReason::CpuSource,
        ),
        (
            matching_source.clone(),
            target(2, PlatformGpuApi::Vulkan, device.clone(), 16_384),
            ScreenPublicationExecutorFallbackReason::PlatformApiMismatch,
        ),
        (
            source(
                extent(1920, 1080),
                ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
                None,
            ),
            target(3, PlatformGpuApi::Direct3d11, device.clone(), 16_384),
            ScreenPublicationExecutorFallbackReason::MissingPhysicalGpuDevice,
        ),
        (
            matching_source.clone(),
            target(4, PlatformGpuApi::Direct3d11, gpu_device(44), 16_384),
            ScreenPublicationExecutorFallbackReason::PhysicalGpuDeviceMismatch,
        ),
        (
            matching_source,
            target(5, PlatformGpuApi::Direct3d11, device, 1_024),
            ScreenPublicationExecutorFallbackReason::TargetDimensionLimitExceeded,
        ),
    ];

    for (source, target, reason) in cases {
        let error = request(
            ScreenPublicationExecutorRequest::SourceNativeRequired(target),
            exact_profile(),
            60,
        )
        .resolve_with_executor_capabilities(&source, ScreenExecutorColorCapabilities::NONE)
        .expect_err("required native demand must not fall back to CPU");
        assert_eq!(
            error,
            ScreenPublicationError::RequiredNativeUnavailable(reason)
        );
    }
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

    let required = request(
        ScreenPublicationExecutorRequest::SourceNativeRequired(native_target.clone()),
        Arc::clone(&profile),
        60,
    );
    assert_eq!(
        required.resolve_with_executor_capabilities(
            &source,
            ScreenExecutorColorCapabilities::new(
                linear_light,
                ScreenColorTransformCapabilities::NONE,
            ),
        ),
        Err(ScreenPublicationError::RequiredNativeUnavailable(
            ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported,
        ))
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
