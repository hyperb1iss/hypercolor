use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_windows_capture::{
    CaptureError, CaptureResourceAdmission, CaptureResourceKind, DisplayRotation, GpuAdapterLuid,
    GpuSurfaceColorPipeline, GpuSurfaceDescriptorId, GpuSurfaceFilter, GpuSurfaceSourceColorSpace,
    ReductionPath, ReductionTelemetry,
};

use super::{
    ActiveCaptureEpoch, CapturePublication, CaptureWorker, ExactBoxList, ExactPublicationShared,
    WindowsCaptureResourceAdmission, WindowsExactRuntime, WindowsExactRuntimes,
    WindowsPhysicalReductionRoute, WindowsPublicationSource, WindowsScreenCaptureInput,
    WorkerCaptureSchedule, WorkerCommand, capture_epoch, capture_freshness, capture_geometry,
    capture_gpu_descriptor, capture_gpu_reduction_descriptor, capture_issue,
    classify_windows_physical_reduction, display_rotation, native_capture_extent,
    record_capture_health, resolve_windows_publication_branch, settle_inactive_capture,
    windows_gpu_attempt_at, windows_gpu_candidate_admission, windows_gpu_preparation_gate,
    windows_gpu_retry_at,
};
use crate::input::screen::adapter::{
    CaptureSessionAuthority, bind_current_capture_exact_runtime, reap_capture_exact_runtimes,
};

#[test]
fn capture_resource_adapter_reconciles_and_releases_source_bytes() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(100, 100));
    let admission = WindowsCaptureResourceAdmission {
        coordinator: coordinator.clone(),
    };

    let reservation = admission
        .try_reserve(CaptureResourceKind::CanonicalDesktop, 80)
        .expect("source quote fits the shared fence");
    assert_eq!(reservation.bytes(), 80);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 80);

    let lease = reservation
        .commit(50)
        .expect("temporary preparation slack reconciles down");
    assert_eq!(lease.kind(), CaptureResourceKind::CanonicalDesktop);
    assert_eq!(lease.bytes(), 50);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 50);

    drop(lease);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}
use crate::input::screen::{
    CaptureCadence, CaptureColorimetry, CaptureConfig, CaptureCursor, CaptureDamage, CaptureFrame,
    CaptureFrameError, CaptureFrameMetadata, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CpuCaptureStorage, MAX_REPRESENTABLE_CAPTURE_FPS, PhysicalOrigin, PixelExtent,
    RawCaptureSurface, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenPublicationDescriptor, ScreenAdmissionCapacity, ScreenAnalysisComputeCapacity,
    ScreenAspectPolicy, ScreenByteAdmissionCoordinator, ScreenCaptureDemand,
    ScreenComputeCapacityPolicy, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativePreparationPayload,
    ScreenNativeTargetPreparation, ScreenNativeTargetPreparer, ScreenPayloadKind,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanBuilder, ScreenPreparedWorkerToken,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenPublicationExecutor,
    ScreenPublicationExecutorFallbackReason, ScreenPublicationExecutorRequest,
    ScreenPublicationHealth, ScreenPublicationKind, ScreenPublicationMetadata,
    ScreenPublicationRequest, ScreenPublicationResidency, ScreenReductionFilter,
    ScreenSourceSelector, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparationTicket,
};
use crate::input::status::{ScreenCaptureReductionPath, SourceDiagnostics};
use crate::input::traits::InputSource;
use crate::input::{SourceKind, SourceState, SourceStatusReporter};

#[test]
fn exact_box_list_mutation_preserves_node_exactness_and_iterative_cleanup() {
    let mut values = ExactBoxList::default();
    for value in 0_u64..10_000 {
        values.push_boxed(ExactBoxList::boxed_node(value));
    }

    assert_eq!(values.iter().count(), 10_000);
    values.retain(|value| *value % 2 == 0);
    assert_eq!(values.iter().count(), 5_000);
    for value in values.iter_mut() {
        *value += 1;
    }
    assert!(values.iter().all(|value| value % 2 == 1));

    values.clear();
    assert_eq!(values.iter().count(), 0);
}

#[test]
fn healthy_gpu_reduction_counters_are_visible_through_source_diagnostics() {
    let mut reporter = SourceStatusReporter::new(
        "windows_screen_capture",
        SourceKind::Screen,
        "dxgi_desktop_duplication",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(1);
    let status = reporter
        .begin_session()
        .expect("status session starts")
        .expect("configured reporter yields a writer");
    let now = Instant::now();
    let telemetry = ReductionTelemetry {
        path: ReductionPath::Gpu,
        gpu_submitted: 12,
        gpu_completed: 11,
        ring_busy: 2,
        readback_bytes: 8192,
        ..ReductionTelemetry::default()
    };

    record_capture_health(&status, now, now + Duration::from_millis(50), &telemetry);

    let diagnostics = reporter
        .handle()
        .diagnostics_snapshot()
        .expect("healthy capture diagnostics are published");
    let SourceDiagnostics::ScreenCapture(diagnostics) = diagnostics.as_ref();
    assert_eq!(diagnostics.reduction_path, ScreenCaptureReductionPath::Gpu);
    assert_eq!(diagnostics.gpu_submitted, 12);
    assert_eq!(diagnostics.gpu_completed, 11);
    assert_eq!(diagnostics.ring_busy, 2);
    assert_eq!(diagnostics.readback_bytes, 8192);
}

#[tokio::test]
async fn gpu_reduction_degradation_survives_successful_sample_recording_without_flapping() {
    let mut reporter = SourceStatusReporter::new(
        "windows_screen_capture",
        SourceKind::Screen,
        "dxgi_desktop_duplication",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(1);
    let status = reporter
        .begin_session()
        .expect("status session starts")
        .expect("configured reporter yields a writer");
    let now = Instant::now();
    let mut telemetry = ReductionTelemetry {
        path: ReductionPath::CpuFallback,
        gpu_completed: 7,
        cpu_completed: 2,
        ring_busy: 3,
        readback_bytes: 4096,
        gpu_failures: 1,
        issue: Some(Arc::from("injected map failure")),
        ..ReductionTelemetry::default()
    };

    record_capture_health(&status, now, now + Duration::from_millis(50), &telemetry);
    let mut subscription = reporter.handle().subscribe();
    telemetry.cpu_completed += 1;
    record_capture_health(
        &status,
        now + Duration::from_millis(10),
        now + Duration::from_millis(60),
        &telemetry,
    );
    let mut duplicate = Box::pin(subscription.changed());
    tokio::select! {
        biased;
        result = &mut duplicate => panic!("fallback counters republished health: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    drop(duplicate);

    let snapshot = reporter.handle().snapshot();
    assert_eq!(snapshot.state, SourceState::Degraded);
    let issue = snapshot
        .issue
        .as_ref()
        .expect("degradation remains visible");
    assert_eq!(
        issue.code.as_ref(),
        "windows_capture_gpu_reduction_degraded"
    );
    assert!(issue.message.contains("injected map failure"));
    assert!(!issue.message.contains("cpu_completed"));
    let diagnostics = reporter
        .handle()
        .diagnostics_snapshot()
        .expect("fallback diagnostics remain visible");
    let SourceDiagnostics::ScreenCapture(diagnostics) = diagnostics.as_ref();
    assert_eq!(
        diagnostics.reduction_path,
        ScreenCaptureReductionPath::CpuFallback
    );
    assert_eq!(diagnostics.cpu_completed, 3);
    assert_eq!(diagnostics.gpu_failures, 1);
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is valid")
}

fn active_demand() -> ScreenCaptureDemand {
    ScreenCaptureDemand::active(extent(640, 480))
}

fn publication_source() -> WindowsPublicationSource {
    WindowsPublicationSource {
        epoch: capture_epoch("display:main", 3, 7).expect("test source epoch is valid"),
        native_extent: extent(2160, 3840),
        logical_extent: extent(3840, 2160),
        origin: PhysicalOrigin { x: -3840, y: 120 },
        rotation: CaptureRotation::Clockwise90,
        colorimetry: CaptureColorimetry::SRGB,
        source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
        adapter_luid: GpuAdapterLuid::new(41, -3),
        duplication_generation: 11,
        is_primary: true,
    }
}

fn publication_demand(
    selector: ScreenSourceSelector,
    executor: ScreenPublicationExecutorRequest,
    filter: ScreenReductionFilter,
) -> RegisteredScreenBranchDemand {
    publication_demand_with_format(selector, executor, filter, CapturePixelFormat::Rgba8)
}

fn publication_demand_with_format(
    selector: ScreenSourceSelector,
    executor: ScreenPublicationExecutorRequest,
    filter: ScreenReductionFilter,
    pixel_format: CapturePixelFormat,
) -> RegisteredScreenBranchDemand {
    publication_demand_with_kind_and_format(
        selector,
        ScreenPublicationKind::Surface,
        executor,
        filter,
        pixel_format,
    )
}

fn publication_demand_with_kind_and_format(
    selector: ScreenSourceSelector,
    kind: ScreenPublicationKind,
    executor: ScreenPublicationExecutorRequest,
    filter: ScreenReductionFilter,
    pixel_format: CapturePixelFormat,
) -> RegisteredScreenBranchDemand {
    publication_demand_with_kind_format_and_hz(selector, kind, executor, filter, pixel_format, 144)
}

fn publication_demand_with_kind_format_and_hz(
    selector: ScreenSourceSelector,
    kind: ScreenPublicationKind,
    executor: ScreenPublicationExecutorRequest,
    filter: ScreenReductionFilter,
    pixel_format: CapturePixelFormat,
    requested_hz: u32,
) -> RegisteredScreenBranchDemand {
    let mut profile = ScreenProcessingProfileConfig::exact_encoded_identity(pixel_format);
    profile.reduction_filter = filter;
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            selector,
            kind,
            executor,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::new(profile)),
        ),
        NonZeroU32::new(requested_hz).expect("test cadence is non-zero"),
    )
}

fn lifecycle_publication_source() -> WindowsPublicationSource {
    let mut source = publication_source();
    source.native_extent = extent(4, 2);
    source.logical_extent = extent(4, 2);
    source.origin = PhysicalOrigin::default();
    source.rotation = CaptureRotation::Identity;
    source
}

fn resolve_publication_demand(
    source: &WindowsPublicationSource,
    demand: &RegisteredScreenBranchDemand,
) -> ResolvedScreenBranchDemand {
    resolve_windows_publication_branch(source, demand)
        .expect("test publication demand resolves")
        .expect("configured test source owns the publication demand")
}

fn prepare_test_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    source: &WindowsPublicationSource,
) -> (
    ScreenPreparedWorkerToken,
    ScreenWorkerBinding,
    WindowsExactRuntime,
) {
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("test exact worker ledger starts");
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, bytes) in reports {
        ledger
            .report(&name, bytes)
            .expect("test worker reports every exact minimum");
    }
    let exact = ledger.finish().expect("test exact worker ledger finishes");
    let binding = exact.token().binding().clone();
    let (token, lifetimes) = exact.into_parts();
    (
        token,
        binding.clone(),
        WindowsExactRuntime {
            source: source.clone(),
            binding,
            gpu: None,
            cpu: None,
            _lifetimes: lifetimes,
        },
    )
}

fn publication_intent(
    descriptor: &ResolvedScreenPublicationDescriptor,
    binding: &ScreenWorkerBinding,
    native_sequence: u64,
    captured_at: Instant,
) -> ScreenPublicationMetadata {
    ScreenPublicationMetadata::try_intent(
        descriptor.source_epoch().clone(),
        binding.plan_generation(),
        NonZeroU64::new(native_sequence).expect("test sequence is nonzero"),
        captured_at,
        captured_at + Duration::from_secs(1),
    )
    .expect("test publication intent is valid")
}

fn native_target() -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(19).expect("test target id is non-zero"),
        ),
        crate::input::screen::PlatformGpuApi::Direct3d11,
        ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
            low_part: 41,
            high_part: -3,
        },
        NonZeroU32::new(16_384).expect("test texture limit is non-zero"),
        Arc::new(TestNativeTargetPreparer),
    )
}

struct TestNativeTargetPreparer;

impl ScreenNativeTargetPreparer for TestNativeTargetPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &crate::input::screen::ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(1)
    }

    fn prepare(
        &self,
        descriptor: &crate::input::screen::ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(()),
            ),
            0,
        ))
    }
}

#[test]
fn exact_native_surface_resolves_to_canonical_gpu_storage() {
    let source = publication_source();
    let demand = publication_demand(
        ScreenSourceSelector::Configured,
        ScreenPublicationExecutorRequest::SourceNative(native_target()),
        ScreenReductionFilter::Nearest,
    );

    let resolved = resolve_windows_publication_branch(&source, &demand)
        .expect("native branch resolves")
        .expect("configured source owns the branch");

    assert!(matches!(
        resolved.descriptor().executor(),
        ScreenPublicationExecutor::SourceNative(_)
    ));
    assert_eq!(
        resolved.descriptor().required_residency(),
        ScreenPublicationResidency::PlatformGpu(crate::input::screen::PlatformGpuApi::Direct3d11)
    );
    assert_eq!(
        resolved.descriptor().source_pixel_format(),
        CapturePixelFormat::Rgba8
    );
    assert_eq!(
        resolved.descriptor().geometry().output_extent(),
        extent(3840, 2160)
    );
    assert!(
        capture_gpu_reduction_descriptor(
            resolved.descriptor().physical(),
            &source,
            GpuSurfaceDescriptorId::new(NonZeroU64::MIN),
            capture_freshness(resolved.requested_hz()),
        )
        .expect_err("native renderer identity cannot enter the CPU readback plan")
        .to_string()
        .contains("CPU physical descriptor")
    );
}

#[test]
fn native_candidate_reserves_capture_and_renderer_textures_from_live_headroom() {
    let source = publication_source();
    let demand = publication_demand(
        ScreenSourceSelector::Configured,
        ScreenPublicationExecutorRequest::SourceNative(native_target()),
        ScreenReductionFilter::Nearest,
    );
    let resolved = resolve_windows_publication_branch(&source, &demand)
        .expect("native branch resolves")
        .expect("configured source owns the branch");
    let descriptor = capture_gpu_descriptor(
        resolved.descriptor(),
        &source,
        GpuSurfaceDescriptorId::new(NonZeroU64::MIN),
        capture_freshness(resolved.requested_hz()),
    )
    .expect("native descriptor is exact");
    let slots = NonZeroU32::new(3).expect("three slots are non-zero");
    let texture_bytes = u64::from(descriptor.output_extent().width())
        * u64::from(descriptor.output_extent().height())
        * 4;
    let required = texture_bytes * 4;

    assert!(matches!(
        windows_gpu_candidate_admission(
            native_capture_extent(source.logical_extent),
            std::slice::from_ref(&descriptor),
            slots,
            required - 1,
        ),
        Err(CaptureError::GpuSurfaceBudgetExceeded {
            requested_bytes,
            budget_bytes,
        }) if requested_bytes == required && budget_bytes == required - 1
    ));
    let admission = windows_gpu_candidate_admission(
        native_capture_extent(source.logical_extent),
        std::slice::from_ref(&descriptor),
        slots,
        required,
    )
    .expect("exact live headroom admits the full candidate");
    assert_eq!(admission.max_texture_bytes(), texture_bytes * 3);
}

#[test]
fn gpu_preparation_gate_is_shared_per_adapter() {
    let first = windows_gpu_preparation_gate(GpuAdapterLuid::new(1, 2));
    let same = windows_gpu_preparation_gate(GpuAdapterLuid::new(1, 2));
    let other = windows_gpu_preparation_gate(GpuAdapterLuid::new(3, 4));

    assert!(Arc::ptr_eq(&first, &same));
    assert!(!Arc::ptr_eq(&first, &other));
}

#[test]
fn cpu_executor_is_shared_across_exact_plan_generations() {
    let exact = ExactPublicationShared::default();
    let first = exact
        .cpu_executor()
        .expect("the source CPU executor prepares");

    for _ in 0..100 {
        let next = exact
            .cpu_executor()
            .expect("later plan generations reuse the executor");
        assert!(Arc::ptr_eq(&first, &next));
    }
}

#[test]
fn exact_runtime_identity_survives_retention_mixed_publication_and_removal() {
    let source = lifecycle_publication_source();
    let source_id = source.epoch.source_id.clone();
    let native_surface = resolve_publication_demand(
        &source,
        &publication_demand(
            ScreenSourceSelector::Configured,
            ScreenPublicationExecutorRequest::SourceNative(native_target()),
            ScreenReductionFilter::Nearest,
        ),
    );
    let cpu_surface = resolve_publication_demand(
        &source,
        &publication_demand(
            ScreenSourceSelector::Configured,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenReductionFilter::Nearest,
        ),
    );
    let zones = resolve_publication_demand(
        &source,
        &publication_demand_with_kind_and_format(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: NonZeroU32::new(2).expect("test grid width is nonzero"),
                rows: NonZeroU32::MIN,
            },
            ScreenPublicationExecutorRequest::Cpu,
            ScreenReductionFilter::Nearest,
            CapturePixelFormat::Rgba8,
        ),
    );
    let retained_native_surface = resolve_publication_demand(
        &source,
        &publication_demand_with_kind_format_and_hz(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::SourceNative(native_target()),
            ScreenReductionFilter::Nearest,
            CapturePixelFormat::Rgba8,
            120,
        ),
    );
    let retained_cpu_surface = resolve_publication_demand(
        &source,
        &publication_demand_with_kind_format_and_hz(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenReductionFilter::Nearest,
            CapturePixelFormat::Rgba8,
            120,
        ),
    );
    let native_descriptor = native_surface.descriptor().clone();
    let cpu_descriptor = cpu_surface.descriptor().clone();
    let zones_descriptor = zones.descriptor().clone();
    let initial_demands = [native_surface, cpu_surface];
    let retained_demands = [
        retained_native_surface.clone(),
        retained_cpu_surface.clone(),
    ];
    let mixed_demands = [retained_native_surface, retained_cpu_surface, zones];
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let exact = ExactPublicationShared::default();
    exact.install_hub(Arc::clone(&hub));
    exact.install_test_source(Some(source.clone()));
    let mut runtimes = WindowsExactRuntimes::default();

    let initial_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision advances");
    let mut stale_preparing = builder
        .prepare(
            mixed_demands.clone(),
            None,
            initial_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("concurrent stale Windows candidate prepares");
    let stale_ticket = stale_preparing
        .worker_ticket(&source_id)
        .expect("concurrent stale candidate owns a worker ticket");
    let (stale_token, stale_binding, stale_runtime) =
        prepare_test_exact_runtime(stale_ticket, &source);
    runtimes.push_boxed(WindowsExactRuntimes::boxed_node(stale_runtime));
    stale_preparing
        .acknowledge(stale_token)
        .expect("stale worker token belongs to its plan");
    let stale_armed = stale_preparing
        .arm(
            builder.current().generation(),
            initial_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("stale candidate arms: {}", failure.error()));
    assert_eq!(stale_binding.state(), ScreenWorkerBindingState::Armed);
    let mut preparing = builder
        .prepare(
            initial_demands,
            None,
            initial_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("initial Windows exact plan prepares");
    let ticket = preparing
        .worker_ticket(&source_id)
        .expect("initial source owns a worker ticket");
    let (token, initial_binding, runtime) = prepare_test_exact_runtime(ticket, &source);
    runtimes.push_boxed(WindowsExactRuntimes::boxed_node(runtime));
    preparing
        .acknowledge(token)
        .expect("initial worker token belongs to its plan");
    let armed = preparing
        .arm(
            builder.current().generation(),
            initial_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("initial plan arms: {}", failure.error()));
    assert_eq!(initial_binding.state(), ScreenWorkerBindingState::Armed);
    assert_eq!(
        armed.candidate_plan().generation(),
        stale_armed.candidate_plan().generation(),
        "concurrent candidates share a structural generation"
    );
    assert!(
        armed
            .candidate_state()
            .owns_runtime_binding(&initial_binding)
    );
    let committed = builder
        .commit(armed, initial_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("initial plan commits: {}", failure.error()));
    committed
        .into_parts()
        .1
        .try_reclaim()
        .expect("initial plan retires no exact runtime resources");
    reap_capture_exact_runtimes(CaptureSessionAuthority::new(1), &mut runtimes, &exact);
    let selected = bind_current_capture_exact_runtime(&mut runtimes, &source, &hub, |_, _| Ok(()))
        .expect("initial runtime binds")
        .expect("initial committed runtime is selected");
    assert!(selected.binding.is_same(&initial_binding));
    assert!(!selected.binding.is_same(&stale_binding));
    drop(stale_armed.abort());
    assert_eq!(stale_binding.state(), ScreenWorkerBindingState::Aborted);

    let retained_revision = initial_revision
        .next()
        .expect("test demand revision advances");
    let mut retained_preparing = builder
        .prepare(
            retained_demands,
            None,
            retained_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("retained-only Windows exact plan prepares");
    assert_ne!(
        retained_preparing.candidate_plan().generation(),
        builder.current().generation(),
        "cadence-only retention still advances the immutable plan generation"
    );
    let retained_ticket = retained_preparing
        .worker_ticket(&source_id)
        .expect("retained-only source owns a successor ticket");
    assert_eq!(retained_ticket.source_delta().retained_branches().len(), 2);
    assert!(retained_ticket.source_delta().added_branches().is_empty());
    assert!(retained_ticket.source_delta().removed_branches().is_empty());
    let (retained_token, retained_binding, retained_runtime) =
        prepare_test_exact_runtime(retained_ticket, &source);
    runtimes.push_boxed(WindowsExactRuntimes::boxed_node(retained_runtime));
    retained_preparing
        .acknowledge(retained_token)
        .expect("retained-only token belongs to its plan");
    let armed = retained_preparing
        .arm(
            builder.current().generation(),
            retained_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("retained-only plan arms: {}", failure.error()));
    assert_eq!(retained_binding.state(), ScreenWorkerBindingState::Armed);
    assert!(
        armed
            .candidate_state()
            .owns_runtime_binding(&retained_binding)
    );
    assert!(
        !armed
            .candidate_state()
            .owns_runtime_binding(&initial_binding),
        "retained successor replaces the previous runtime identity"
    );
    let committed = builder
        .commit(armed, retained_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("retained-only plan commits: {}", failure.error()));
    let (_, retained_retirement) = committed.into_parts();
    reap_capture_exact_runtimes(CaptureSessionAuthority::new(1), &mut runtimes, &exact);
    assert_eq!(runtimes.iter().count(), 1);
    let selected = bind_current_capture_exact_runtime(&mut runtimes, &source, &hub, |_, _| Ok(()))
        .expect("retained-only runtime binds")
        .expect("retained-only committed runtime is selected");
    assert!(selected.binding.is_same(&retained_binding));
    assert!(!selected.binding.is_same(&initial_binding));
    retained_retirement
        .try_reclaim()
        .expect("stale retained runtime resources reclaim after reaping");

    let mixed_revision = retained_revision
        .next()
        .expect("test demand revision advances");
    let mut mixed_preparing = builder
        .prepare(
            mixed_demands,
            None,
            mixed_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("mixed retained and added Windows exact plan prepares");
    let mixed_ticket = mixed_preparing
        .worker_ticket(&source_id)
        .expect("mixed source owns a successor ticket");
    assert_eq!(mixed_ticket.source_delta().retained_branches().len(), 2);
    assert_eq!(mixed_ticket.source_delta().added_branches().len(), 1);
    let (mixed_token, mixed_binding, mixed_runtime) =
        prepare_test_exact_runtime(mixed_ticket, &source);
    runtimes.push_boxed(WindowsExactRuntimes::boxed_node(mixed_runtime));
    mixed_preparing
        .acknowledge(mixed_token)
        .expect("mixed token belongs to its plan");
    let armed = mixed_preparing
        .arm(
            builder.current().generation(),
            mixed_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("mixed plan arms: {}", failure.error()));
    assert_eq!(mixed_binding.state(), ScreenWorkerBindingState::Armed);
    let candidate = armed.candidate_state();
    assert!(candidate.owns_runtime_binding(&mixed_binding));
    let native_publisher = candidate
        .publisher_for_runtime(&native_descriptor, &mixed_binding)
        .expect("mixed runtime binds the retained native branch");
    let cpu_publisher = candidate
        .publisher_for_runtime(&cpu_descriptor, &mixed_binding)
        .expect("mixed runtime binds the retained CPU branch");
    let zones_publisher = candidate
        .publisher_for_runtime(&zones_descriptor, &mixed_binding)
        .expect("mixed runtime binds the added Zones branch");
    assert_eq!(
        mixed_binding.plan_generation(),
        candidate.plan().generation()
    );
    assert_eq!(
        native_publisher.plan_generation(),
        initial_binding.plan_generation(),
        "retained GPU metadata keeps its branch worker generation"
    );
    assert_ne!(
        native_publisher.plan_generation(),
        mixed_binding.plan_generation(),
        "GPU runtime generation is separate from retained branch metadata"
    );
    assert_eq!(
        cpu_publisher.plan_generation(),
        initial_binding.plan_generation()
    );
    assert_eq!(
        zones_publisher.plan_generation(),
        mixed_binding.plan_generation()
    );
    let committed = builder
        .commit(armed, mixed_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("mixed plan commits: {}", failure.error()));
    let (_, mixed_retirement) = committed.into_parts();
    reap_capture_exact_runtimes(CaptureSessionAuthority::new(1), &mut runtimes, &exact);
    assert_eq!(runtimes.iter().count(), 1);
    let selected = bind_current_capture_exact_runtime(&mut runtimes, &source, &hub, |_, _| Ok(()))
        .expect("mixed runtime binds")
        .expect("mixed committed runtime is selected");
    assert!(selected.binding.is_same(&mixed_binding));
    mixed_retirement
        .try_reclaim()
        .expect("retained-only runtime resources reclaim after mixed reaping");

    let captured_at = Instant::now();
    let mut cpu_publication = hub
        .prepare_writable_publication(
            &cpu_publisher,
            ScreenPayloadKind::Surface,
            &publication_intent(&cpu_descriptor, &initial_binding, 1, captured_at),
        )
        .expect("retained CPU surface reserves a writable slot");
    cpu_publication
        .surface_pixels_mut()
        .expect("retained CPU surface exposes writable pixels")
        .fill(0x5a);
    let mut zones_publication = hub
        .prepare_writable_publication(
            &zones_publisher,
            ScreenPayloadKind::Zones,
            &publication_intent(&zones_descriptor, &mixed_binding, 1, captured_at),
        )
        .expect("added Zones branch reserves a writable slot");
    zones_publication
        .zone_colors_mut()
        .expect("added Zones branch exposes writable colors")
        .fill([0x21, 0x43, 0x65]);
    hub.finalize_writable_publications(
        &mut [cpu_publication, zones_publication],
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("mixed old and new bindings finalize atomically through one source gate");
    assert_eq!(
        hub.lease(&cpu_descriptor)
            .expect("retained CPU branch has a lease")
            .read()
            .expect("retained CPU branch publishes")
            .worker_plan_generation(),
        initial_binding.plan_generation()
    );
    assert_eq!(
        hub.lease(&zones_descriptor)
            .expect("added Zones branch has a lease")
            .read()
            .expect("added Zones branch publishes")
            .worker_plan_generation(),
        mixed_binding.plan_generation()
    );
    drop((native_publisher, cpu_publisher, zones_publisher));

    let removal_revision = mixed_revision
        .next()
        .expect("test demand revision advances");
    let mut removal_preparing = builder
        .prepare(
            std::iter::empty::<ResolvedScreenBranchDemand>(),
            None,
            removal_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("full Windows source removal prepares");
    let removal_ticket = removal_preparing
        .worker_ticket(&source_id)
        .expect("full removal owns a worker ticket");
    assert!(removal_ticket.source_delta().retained_branches().is_empty());
    assert_eq!(removal_ticket.source_delta().removed_branches().len(), 3);
    let (removal_token, removal_binding, removal_runtime) =
        prepare_test_exact_runtime(removal_ticket, &source);
    drop(removal_runtime);
    removal_preparing
        .acknowledge(removal_token)
        .expect("removal token belongs to the empty successor");
    let armed = removal_preparing
        .arm(
            builder.current().generation(),
            removal_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("removal plan arms: {}", failure.error()));
    assert_eq!(removal_binding.state(), ScreenWorkerBindingState::Armed);
    assert!(
        armed
            .candidate_state()
            .runtime_binding(&source_id)
            .is_none()
    );
    let committed = builder
        .commit(armed, removal_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("removal plan commits: {}", failure.error()));
    let (_, retirement) = committed.into_parts();
    reap_capture_exact_runtimes(CaptureSessionAuthority::new(1), &mut runtimes, &exact);
    assert_eq!(runtimes.iter().count(), 0);
    assert!(
        builder
            .committed_state()
            .runtime_binding(&source_id)
            .is_none()
    );
    assert_eq!(removal_binding.state(), ScreenWorkerBindingState::Retired);
    retirement
        .try_reclaim()
        .expect("removed branch and runtime resources reclaim after reaping");
}

#[test]
fn unsupported_native_filter_falls_back_to_exact_cpu_bgra() {
    let source = publication_source();
    let demand = publication_demand(
        ScreenSourceSelector::Configured,
        ScreenPublicationExecutorRequest::SourceNative(native_target()),
        ScreenReductionFilter::Area,
    );

    let resolved = resolve_windows_publication_branch(&source, &demand)
        .expect("CPU fallback resolves")
        .expect("configured source owns the branch");

    assert_eq!(
        resolved.descriptor().executor(),
        &ScreenPublicationExecutor::Cpu
    );
    assert_eq!(
        resolved.descriptor().executor_fallback(),
        Some(ScreenPublicationExecutorFallbackReason::CpuSource)
    );
    assert_eq!(
        resolved.descriptor().source_pixel_format(),
        CapturePixelFormat::Bgra8
    );
    assert_eq!(
        resolved.descriptor().source().geometry().native_extent(),
        extent(2160, 3840)
    );
    assert_eq!(
        resolved.descriptor().source().geometry().rotation(),
        CaptureRotation::Clockwise90
    );
    let reduced = capture_gpu_reduction_descriptor(
        resolved.descriptor().physical(),
        &source,
        GpuSurfaceDescriptorId::new(NonZeroU64::MIN),
        capture_freshness(resolved.requested_hz()),
    )
    .expect("area-filtered CPU physical work maps to compact GPU readback");
    assert_eq!(reduced.filter(), GpuSurfaceFilter::Area);
    assert_eq!(reduced.color_pipeline(), GpuSurfaceColorPipeline::LinearSdr);
    assert_eq!(reduced.source_rotation(), DisplayRotation::Clockwise90);
    assert_eq!(reduced.output_extent().width(), 3840);
    assert_eq!(reduced.output_extent().height(), 2160);
}

#[test]
fn exact_reduction_route_charges_only_work_that_can_execute_on_cpu() {
    let source = publication_source();
    let pre_reduced = publication_demand(
        ScreenSourceSelector::Configured,
        ScreenPublicationExecutorRequest::SourceNative(native_target()),
        ScreenReductionFilter::Area,
    );
    let pre_reduced = resolve_windows_publication_branch(&source, &pre_reduced)
        .expect("GPU-reduced CPU publication resolves")
        .expect("configured source owns the branch");
    assert_eq!(
        classify_windows_physical_reduction(
            pre_reduced.descriptor().physical(),
            &source,
            capture_freshness(pre_reduced.requested_hz()),
        ),
        WindowsPhysicalReductionRoute::GuaranteedGpuPreReduced
    );

    let cpu = publication_demand_with_format(
        ScreenSourceSelector::Configured,
        ScreenPublicationExecutorRequest::Cpu,
        ScreenReductionFilter::Nearest,
        CapturePixelFormat::Bgra8,
    );
    let cpu = resolve_windows_publication_branch(&source, &cpu)
        .expect("CPU publication resolves")
        .expect("configured source owns the branch");
    assert_eq!(
        classify_windows_physical_reduction(
            cpu.descriptor().physical(),
            &source,
            capture_freshness(cpu.requested_hz()),
        ),
        WindowsPhysicalReductionRoute::Cpu
    );
}

#[test]
fn large_native_source_admits_the_gpu_reduced_analysis_plane() {
    let mut source = publication_source();
    source.native_extent = extent(15_360, 8_640);
    source.logical_extent = source.native_extent;
    source.rotation = CaptureRotation::Identity;
    let input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    input
        .adapter
        .exact_state()
        .install_test_source(Some(source));

    let prepared = input
        .prepare_active_settings(CaptureConfig::default(), 0, active_demand())
        .expect("the reduced compatibility plane is admitted");
    let analysis = prepared
        .analyzer
        .analysis_work_plan()
        .expect("known topology pre-admits analysis work");

    assert_eq!(analysis.input_extent(), extent(640, 480));
}

#[test]
fn calibrated_analysis_capacity_rejects_known_work_before_preparation() {
    let capacity = ScreenAnalysisComputeCapacity::new_split(
        NonZeroUsize::MIN,
        NonZeroU64::MIN,
        NonZeroU64::MIN,
    );
    let policy = ScreenComputeCapacityPolicy::calibrated(capacity, NonZeroU64::MIN);
    let mut source = publication_source();
    source.native_extent = extent(15_360, 8_640);
    source.logical_extent = source.native_extent;
    let input =
        WindowsScreenCaptureInput::with_compute_capacity_policy(CaptureConfig::default(), policy);
    input
        .adapter
        .exact_state()
        .install_test_source(Some(source));

    let Err(error) = input.prepare_active_settings(CaptureConfig::default(), 0, active_demand())
    else {
        panic!("caller-calibrated capacity must reject known excess work");
    };

    assert!(error.to_string().contains("weighted work units/s"));
}

#[test]
fn selector_claims_only_the_current_windows_output() {
    let source = publication_source();
    let wrong = CaptureSourceId::new("windows:display:other").expect("test source id is non-empty");
    let exact = publication_demand(
        ScreenSourceSelector::Exact(wrong),
        ScreenPublicationExecutorRequest::Cpu,
        ScreenReductionFilter::Nearest,
    );

    assert!(
        resolve_windows_publication_branch(&source, &exact)
            .expect("unowned selector is not an error")
            .is_none()
    );
}

#[test]
fn unrepresentable_cadence_cannot_activate_capture() {
    let config = CaptureConfig {
        target_fps: MAX_REPRESENTABLE_CAPTURE_FPS + 1,
        ..CaptureConfig::default()
    };
    let mut input = WindowsScreenCaptureInput::new(config);

    let error = input
        .set_capture_demand_state(active_demand())
        .expect_err("an unrepresentable scheduler cadence must fail admission");

    assert!(error.to_string().contains("scheduler clock limit"));
    assert_eq!(input.capture_demand, ScreenCaptureDemand::Inactive);
}

#[test]
fn shared_admission_rejects_windows_analysis_before_backend_start() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(0, 0));
    let mut input = WindowsScreenCaptureInput::with_admission_coordinator(
        CaptureConfig::default(),
        coordinator.clone(),
    );

    let error = input
        .set_capture_demand_state(active_demand())
        .expect_err("manager-owned capacity must gate Windows analysis construction");

    assert!(error.to_string().contains("capacity is 0 bytes"));
    assert_eq!(input.capture_demand, ScreenCaptureDemand::Inactive);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn active_reconfigure_retains_last_good_cadence_after_admission_failure() {
    let mut input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    input
        .set_capture_demand_state(active_demand())
        .expect("baseline capture demand is admitted");
    let baseline = input.settings.snapshot().config;
    let mut rejected = baseline.clone();
    rejected.target_fps = MAX_REPRESENTABLE_CAPTURE_FPS + 1;

    let error = input
        .reconfigure(rejected)
        .expect_err("active capture must reject an unrepresentable cadence");

    assert!(error.to_string().contains("scheduler clock limit"));
    assert_eq!(input.settings.snapshot().config, baseline);
    assert_eq!(input.capture_demand, active_demand());
}

#[test]
fn inactive_reconfigure_retains_last_good_cadence_after_admission_failure() {
    let mut input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    let baseline = input.settings.snapshot().config;
    let mut rejected = baseline.clone();
    rejected.target_fps = MAX_REPRESENTABLE_CAPTURE_FPS + 1;

    let error = input
        .reconfigure(rejected)
        .expect_err("inactive capture must reject an unrepresentable cadence");

    assert!(error.to_string().contains("scheduler clock limit"));
    assert_eq!(input.settings.snapshot().config, baseline);
    assert_eq!(input.capture_demand, ScreenCaptureDemand::Inactive);
}

#[test]
fn worker_schedule_gates_analysis_and_never_catches_up_in_a_burst() {
    let cadence = CaptureCadence::new(30).expect("30 FPS is representable");
    let started_at = Instant::now();
    let mut schedule = WorkerCaptureSchedule::new(cadence, started_at);

    assert_eq!(schedule.wait_duration(started_at), None);
    schedule
        .record_frame(started_at, started_at)
        .expect("live scheduler deadline fits Instant");
    assert!(schedule.wait_duration(started_at).is_some());

    let late = started_at + Duration::from_secs(1);
    schedule
        .record_frame(late, late)
        .expect("live scheduler deadline fits Instant");
    assert!(
        schedule.wait_duration(late).is_some(),
        "lateness must schedule a future interval instead of an immediate burst"
    );
}

#[test]
fn exact_route_deadlines_are_independent_of_the_legacy_analysis_cadence() {
    let started_at = Instant::now();
    let mut legacy = WorkerCaptureSchedule::new(
        CaptureCadence::new(30).expect("30 FPS is representable"),
        started_at,
    );
    legacy
        .record_frame(started_at, started_at)
        .expect("legacy deadline fits Instant");
    let legacy_deadline = started_at
        + legacy
            .wait_duration(started_at)
            .expect("legacy cadence has a future deadline");
    let exact_144 = windows_gpu_retry_at(
        CaptureCadence::new(144)
            .expect("144 FPS is representable")
            .pacer(),
        started_at,
    )
    .expect("exact route deadline fits Instant");
    let exact_60 = windows_gpu_retry_at(
        CaptureCadence::new(60)
            .expect("60 FPS is representable")
            .pacer(),
        started_at,
    )
    .expect("exact route deadline fits Instant");

    assert!(exact_144 < exact_60);
    assert!(exact_60 < legacy_deadline);
    assert_eq!(
        windows_gpu_attempt_at(started_at, Some(exact_144)),
        exact_144
    );
}

fn active_epoch(
    source: &str,
    topology_generation: u64,
    session_generation: u64,
    duplication_generation: u64,
) -> ActiveCaptureEpoch {
    ActiveCaptureEpoch {
        epoch: capture_epoch(source, topology_generation, session_generation)
            .expect("test epoch is valid"),
        source_generation: 0,
        activity_generation: 0,
        duplication_generation,
    }
}

fn wait_for_activity(input: &WindowsScreenCaptureInput, expected: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let processed = input
            .adapter
            .active_worker()
            .expect("capture owns a worker while activity is changing")
            .processed_activity_generation
            .load(Ordering::Acquire);
        if processed == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not process activity generation {expected}; latest was {processed}"
        );
        thread::yield_now();
    }
}

#[test]
fn inactive_settlement_drops_capture_before_acknowledging_generation() {
    struct DropSignal(Arc<AtomicBool>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let mut capture = Some(DropSignal(Arc::clone(&dropped)));
    let processed = AtomicU64::new(0);

    settle_inactive_capture(&mut capture, &processed, 7);

    assert!(capture.is_none());
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(processed.load(Ordering::Acquire), 7);
}

#[test]
fn activity_fence_rejects_a_pre_deactivation_frame_after_reactivation() {
    let mut old_activity = active_epoch("display:main", 3, 7, 1);
    old_activity.activity_generation = 1;
    let mut reactivated = old_activity.clone();
    reactivated.activity_generation = 3;
    let mut publication = CapturePublication::default();

    publication.fence_activity(1);
    assert!(publication.activate(old_activity.clone()).is_ok());
    assert!(publication.publish(&old_activity, "old frame").is_ok());
    publication.fence_activity(2);
    publication.fence_activity(3);

    assert!(publication.activate(old_activity.clone()).is_err());
    assert!(
        publication
            .publish(&old_activity, "in-flight old frame")
            .is_err()
    );
    assert!(publication.active().is_none());
    assert!(publication.latest().is_none());
    assert!(publication.activate(reactivated.clone()).is_ok());
    assert!(
        publication
            .publish(&reactivated, "reactivated frame")
            .is_ok()
    );
}

#[test]
fn deactivated_worker_is_reused_when_capture_reactivates() {
    let mut input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("idle capture source starts");
    assert!(input.adapter.active_worker().is_none());

    input
        .set_capture_demand_state(active_demand())
        .expect("first activation starts the worker");
    let first_activity = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, first_activity);
    let first_thread = input
        .adapter
        .active_worker()
        .and_then(|worker| worker.join_handle.as_ref())
        .expect("active capture owns a worker thread")
        .thread()
        .id();
    let first_generation = input
        .adapter
        .exact_state()
        .current_authority()
        .expect("active worker owns exact authority")
        .generation();

    input
        .set_capture_demand_state(ScreenCaptureDemand::Inactive)
        .expect("deactivation idles the worker");
    let inactive_activity = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, inactive_activity);
    assert!(
        input.adapter.active_worker().is_some(),
        "acknowledged idle capture retains worker ownership"
    );

    input
        .set_capture_demand_state(active_demand())
        .expect("reactivation reuses the idle worker");
    let reactivated_activity = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, reactivated_activity);
    let reactivated_thread = input
        .adapter
        .active_worker()
        .and_then(|worker| worker.join_handle.as_ref())
        .expect("reactivated capture still owns a worker thread")
        .thread()
        .id();

    assert_eq!(reactivated_thread, first_thread);
    assert_eq!(
        input
            .adapter
            .exact_state()
            .current_authority()
            .expect("reactivated worker retains exact authority")
            .generation(),
        first_generation,
        "reactivation must not spawn a replacement worker"
    );
    assert_eq!(inactive_activity, first_activity.wrapping_add(1));
    assert_eq!(reactivated_activity, inactive_activity.wrapping_add(1));

    input.stop();
}

#[test]
fn disconnected_worker_is_reaped_before_activation_retries_once() {
    let mut input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("idle capture source starts");

    let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
    drop(command_rx);
    let (exit_tx, exit_rx) = mpsc::sync_channel(1);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let fake_cancel = Arc::clone(&cancel);
    let exited = Arc::new(AtomicBool::new(false));
    let fake_exited = Arc::clone(&exited);
    let join_handle = thread::spawn(move || {
        ready_tx.send(()).expect("fake worker announces readiness");
        while !fake_cancel.load(Ordering::Acquire) {
            thread::yield_now();
        }
        fake_exited.store(true, Ordering::Release);
        let _ = exit_tx.send(());
    });
    ready_rx.recv().expect("fake worker is running");
    let disconnected_thread = join_handle.thread().id();
    assert!(
        input
            .adapter
            .install_worker_for_test(CaptureWorker {
                authority: CaptureSessionAuthority::new(1),
                command_tx,
                exit_rx,
                join_handle: Some(join_handle),
                cancel,
                processed_activity_generation: Arc::new(AtomicU64::new(0)),
            })
            .is_ok()
    );

    input
        .set_capture_demand_state(active_demand())
        .expect("activation replaces the disconnected worker");
    let activity_generation = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, activity_generation);
    let replacement_thread = input
        .adapter
        .active_worker()
        .and_then(|worker| worker.join_handle.as_ref())
        .expect("activation owns the replacement worker")
        .thread()
        .id();

    assert!(exited.load(Ordering::Acquire));
    assert_ne!(replacement_thread, disconnected_thread);
    assert_eq!(
        input
            .adapter
            .exact_state()
            .current_authority()
            .expect("replacement worker owns exact authority")
            .generation(),
        1,
        "only the replacement worker allocates a capture session"
    );

    input.stop();
}

#[test]
fn retired_worker_cannot_republish_after_stop_returns() {
    let mut input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    let authority = CaptureSessionAuthority::new(1);
    let reservation = input
        .adapter
        .reserve_exact_authority()
        .expect("authority reserves");
    assert_eq!(reservation.authority(), authority);
    drop(
        input
            .adapter
            .exact_state()
            .activate_reserved_authority(reservation)
            .expect("authority activates"),
    );

    let epoch = active_epoch("display:main", 3, 1, 1);
    {
        let mut publication = input
            .adapter
            .compatibility_publication()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(publication.activate(epoch.clone()).is_ok());
        assert!(publication.publish(&epoch, "active frame").is_ok());
    }

    let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
    let (_exit_tx, exit_rx) = mpsc::sync_channel(1);
    let release = Arc::new(AtomicBool::new(false));
    let worker_release = Arc::clone(&release);
    let join_handle = thread::spawn(move || {
        let _command_rx = command_rx;
        while !worker_release.load(Ordering::Acquire) {
            thread::yield_now();
        }
    });
    assert!(
        input
            .adapter
            .install_worker_for_test(CaptureWorker {
                authority,
                command_tx,
                exit_rx,
                join_handle: Some(join_handle),
                cancel: Arc::new(AtomicBool::new(false)),
                processed_activity_generation: Arc::new(AtomicU64::new(0)),
            })
            .is_ok()
    );

    input.stop();

    assert!(!input.adapter.exact_state().is_current_authority(authority));
    let mut publication = input
        .adapter
        .compatibility_publication()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(publication.activate(epoch.clone()).is_err());
    assert!(publication.publish(&epoch, "retired frame").is_err());
    drop(publication);

    release.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while input.adapter.retiring_worker_count() != 0 {
        input.adapter.reap_finished_workers(|_, result| {
            result.expect("retired worker exits cleanly");
        });
        assert!(Instant::now() < deadline, "retired worker did not exit");
        thread::yield_now();
    }
}

#[test]
fn source_fence_prevents_an_in_flight_worker_from_reactivating_old_frames() {
    let old_source = active_epoch("display:old", 3, 7, 1);
    let mut new_source = active_epoch("display:new", 3, 7, 1);
    new_source.source_generation = 1;
    let mut publication = CapturePublication::default();

    assert!(publication.activate(old_source.clone()).is_ok());
    assert!(publication.publish(&old_source, "old frame").is_ok());
    publication.fence_source(1);

    assert!(publication.activate(old_source.clone()).is_err());
    assert!(publication.publish(&old_source, "late old frame").is_err());
    assert!(publication.active().is_none());
    assert!(publication.latest().is_none());
    assert!(publication.activate(new_source.clone()).is_ok());
    assert!(publication.publish(&new_source, "new frame").is_ok());
}

#[test]
fn publication_rejects_frames_from_a_replaced_source() {
    let first = active_epoch("display:first", 3, 7, 1);
    let replacement = active_epoch("display:replacement", 3, 7, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()).is_ok());
    assert!(publication.publish(&first, "first frame").is_ok());
    assert_eq!(publication.latest(), Some(&"first frame"));

    assert!(publication.activate(replacement.clone()).is_ok());
    assert!(publication.latest().is_none());
    assert!(publication.publish(&first, "stale frame").is_err());
    assert!(
        publication
            .publish(&replacement, "replacement frame")
            .is_ok()
    );
    assert_eq!(publication.latest(), Some(&"replacement frame"));
}

#[test]
fn publication_rejects_frames_from_a_replaced_topology() {
    let first = active_epoch("display:main", 3, 7, 1);
    let replacement = active_epoch("display:main", 4, 7, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()).is_ok());
    assert!(publication.publish(&first, "first frame").is_ok());
    assert!(publication.activate(replacement.clone()).is_ok());

    assert!(publication.latest().is_none());
    assert!(publication.publish(&first, "stale topology frame").is_err());
    assert!(
        publication
            .publish(&replacement, "replacement frame")
            .is_ok()
    );
}

#[test]
fn publication_rejects_frames_from_a_replaced_worker_session() {
    let first = active_epoch("display:main", 3, 7, 1);
    let restarted = active_epoch("display:main", 3, 8, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()).is_ok());
    assert!(publication.publish(&first, "first frame").is_ok());
    assert!(publication.activate(restarted.clone()).is_ok());

    assert!(publication.latest().is_none());
    assert!(publication.publish(&first, "stale worker frame").is_err());
    assert!(publication.publish(&restarted, "restarted frame").is_ok());
}

#[test]
fn publication_rejects_frames_from_a_rebuilt_duplication_interface() {
    let first = active_epoch("display:main", 3, 7, 1);
    let rebuilt = active_epoch("display:main", 3, 7, 2);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()).is_ok());
    assert!(publication.publish(&first, "first frame").is_ok());
    assert!(publication.activate(rebuilt.clone()).is_ok());

    assert!(publication.latest().is_none());
    assert!(publication.publish(&first, "access-lost frame").is_err());
    assert!(publication.publish(&rebuilt, "rebuilt frame").is_ok());
}

#[test]
fn cleared_publication_fails_closed_until_a_new_epoch_is_activated() {
    let active = active_epoch("display:main", 3, 7, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(active.clone()).is_ok());
    assert!(publication.publish(&active, "frame").is_ok());
    publication.clear();

    assert!(publication.active().is_none());
    assert!(publication.latest().is_none());
    assert!(publication.publish(&active, "late frame").is_err());
}

#[test]
fn adapter_preserves_native_and_stored_geometry_for_every_dxgi_rotation() {
    for (native_rotation, expected_rotation) in [
        (DisplayRotation::Identity, CaptureRotation::Identity),
        (DisplayRotation::Clockwise90, CaptureRotation::Clockwise90),
        (DisplayRotation::Clockwise180, CaptureRotation::Clockwise180),
        (DisplayRotation::Clockwise270, CaptureRotation::Clockwise270),
    ] {
        let geometry = capture_geometry(
            extent(3840, 2160),
            extent(1280, 720),
            PhysicalOrigin { x: -3840, y: 120 },
            native_rotation,
        )
        .expect("DXGI geometry is valid");

        assert_eq!(geometry.native_extent(), extent(3840, 2160));
        assert_eq!(geometry.storage_extent(), extent(1280, 720));
        assert_eq!(geometry.origin(), PhysicalOrigin { x: -3840, y: 120 });
        assert_eq!(geometry.rotation(), expected_rotation);
    }
}

#[test]
fn reflected_capture_transforms_cannot_cross_the_dxgi_rotation_boundary() {
    for transform in [
        CaptureRotation::Flipped,
        CaptureRotation::Flipped90,
        CaptureRotation::Flipped180,
        CaptureRotation::Flipped270,
    ] {
        assert!(display_rotation(transform).is_err());
    }
}

#[test]
fn adapter_epoch_rejects_a_frame_from_another_monitor_or_generation() {
    let captured_at = Instant::now();
    let geometry = capture_geometry(
        extent(4, 2),
        extent(2, 1),
        PhysicalOrigin::default(),
        DisplayRotation::Identity,
    )
    .expect("test geometry is valid");
    let frame = CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id: capture_epoch("display:left", 3, 7)
                .expect("test source id is valid")
                .source_id,
            topology_generation: 3,
            session_generation: 7,
            sequence: 1,
            captured_at,
            fresh_until: captured_at + Duration::from_millis(50),
            geometry,
            colorimetry: CaptureColorimetry::unknown(),
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::<[u8]>::from([0_u8; 8]),
            CapturePixelFormat::Rgba8,
            8,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid");

    assert!(matches!(
        frame
            .validate_epoch(&capture_epoch("display:main", 3, 7).expect("expected epoch is valid")),
        Err(CaptureFrameError::SourceMismatch { .. })
    ));
    assert!(matches!(
        frame
            .validate_epoch(&capture_epoch("display:left", 4, 7).expect("expected epoch is valid")),
        Err(CaptureFrameError::StaleTopology { .. })
    ));
    assert!(matches!(
        frame
            .validate_epoch(&capture_epoch("display:left", 3, 8).expect("expected epoch is valid")),
        Err(CaptureFrameError::StaleSession { .. })
    ));
}

#[test]
fn dxgi_failure_classes_publish_distinct_issue_codes() {
    for (error, expected) in [
        (
            CaptureError::AlreadyDuplicating,
            "windows_desktop_duplication_limit",
        ),
        (CaptureError::AccessDenied, "windows_desktop_access_denied"),
        (
            CaptureError::SessionUnavailable,
            "windows_session_unavailable",
        ),
        (CaptureError::DeviceLost, "windows_capture_device_lost"),
        (CaptureError::AccessLost, "windows_desktop_access_lost"),
        (CaptureError::Timeout, "windows_capture_timeout"),
        (
            CaptureError::SourceNotFound {
                requested: "display:missing".to_owned(),
            },
            "windows_capture_source_missing",
        ),
        (
            CaptureError::GpuSurfacePlanInvalidated,
            "windows_capture_gpu_surface_transient",
        ),
        (
            CaptureError::GpuSurfaceSynchronizationExhausted,
            "windows_capture_gpu_surface_lifecycle_failed",
        ),
    ] {
        let issue = capture_issue(&error);
        assert_eq!(&*issue.code, expected);
        assert!(issue.retryable);
    }
}
