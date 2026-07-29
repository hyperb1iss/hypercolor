use std::alloc::System;
use std::hint::black_box;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CpuCaptureStorage, CpuReductionExecutor, InputPublicationDemandRevision,
    PhysicalOrigin, PixelExtent, PreparedCpuPublicationFanout, RawCaptureSurface,
    RegisteredScreenBranchDemand, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenCaptureBackend, ScreenColorTuning, ScreenContentBarsPolicy, ScreenExactResource,
    ScreenExactResourceLedger, ScreenExtentRequest, ScreenInputGraphGeneration, ScreenPlanBuilder,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenProfileScalar,
    ScreenPublicationExecutorRequest, ScreenPublicationHealth, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenResourceApi, ScreenSceneCutPolicy, ScreenSmoothingPolicy,
    ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy, SourceScale,
};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
static ALLOCATION_TEST_LOCK: Mutex<()> = Mutex::new(());

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn source() -> ResolvedScreenSource {
    let extent = extent(17, 11);
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new("synthetic:fanout-allocation")
                .expect("test source id is non-empty"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new(
            geometry,
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn frame(source: &ResolvedScreenSource, sequence: u64) -> CaptureFrame<RawCaptureSurface> {
    frame_at(source, sequence, Instant::now())
}

fn frame_at(
    source: &ResolvedScreenSource,
    sequence: u64,
    captured_at: Instant,
) -> CaptureFrame<RawCaptureSurface> {
    let extent = source.config().geometry().storage_extent();
    let pixels = vec![
        127;
        usize::try_from(u64::from(extent.width()) * u64::from(extent.height()) * 4)
            .expect("test frame is addressable")
    ];
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation: source.epoch().session_generation,
            sequence,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(2),
            geometry: source.config().geometry(),
            colorimetry: source.config().colorimetry(),
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::from(pixels),
            source.config().pixel_format(),
            i64::from(extent.width()) * 4,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid")
}

#[test]
fn authority_binding_performs_no_heap_allocation() {
    let _guard = ALLOCATION_TEST_LOCK
        .lock()
        .expect("allocation test lock is healthy");
    let executor = CpuReductionExecutor::new(
        NonZeroUsize::new(2).expect("test worker count is non-zero"),
        non_zero(3),
    )
    .expect("test worker pool builds");
    let source = source();
    let demand = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                Some(non_zero(13)),
                Some(non_zero(7)),
                ScreenUpscalePolicy::Allow,
            ),
            ScreenAspectPolicy::Cover,
            Arc::new(ScreenProcessingProfile::new(
                ScreenProcessingProfileConfig::default(),
            )),
        ),
        non_zero(60),
    )
    .resolve_with_color_capabilities(&source, executor.capabilities())
    .expect("test demand resolves");
    let mut builder = ScreenPlanBuilder::new();
    let demand_revision = InputPublicationDemandRevision::new(1);
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            [demand],
            None,
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch().source_id)
        .expect("test source has a worker ticket");
    let resources = ticket
        .required_minimums()
        .iter()
        .map(|minimum| {
            ScreenExactResource::try_new(
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("exact resources prepare");
    let lifetimes = resources
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()
        .expect("resource lifetimes bind");
    let token = ticket
        .acknowledge(
            ScreenExactResourceLedger::try_new(resources).expect("resource ledger prepares"),
            &lifetimes,
        )
        .expect("worker resources satisfy the ticket");
    preparing
        .acknowledge(token)
        .expect("worker token belongs to the candidate");
    let armed = preparing
        .arm(
            builder.current().generation(),
            demand_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("test plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, demand_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("test plan commits: {}", failure.error()));
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved retired resources reclaim");
    let binding = builder
        .committed_state()
        .worker_bindings()
        .first()
        .cloned()
        .expect("committed source has one worker binding");
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("exact CPU batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("direct Surface needs no retained workspace");
    let candidate = PreparedCpuPublicationFanout::prepare_candidate(&batch, &workspace, &plan)
        .expect("allocation-complete candidate prepares");
    let authority = builder.committed_state();
    let mut region = Region::new(GLOBAL);
    region.reset();

    let fanout = black_box(candidate)
        .bind(black_box(authority), black_box(&binding))
        .expect("candidate binds to committed authority");
    black_box(&fanout);
    let change = region.change();

    assert_eq!(change.allocations, 0);
    assert_eq!(change.reallocations, 0);
    assert_eq!(change.bytes_allocated, 0);
    assert_eq!(fanout.len(), 1);
    assert_eq!(fanout.branch_count(), 1);
    assert!(workspace.is_empty());
    assert_eq!(workspace.allocation_byte_len(), 0);

    let executable_workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("executable workspace prepares");
    let executable = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor,
        &batch,
        executable_workspace,
        &plan,
    )
    .expect("executable fanout prepares");
    let mut executable = executable
        .bind(builder.committed_state(), &binding)
        .expect("executable fanout binds");
    let first = frame(&source, 1);
    let warm_now = Instant::now();
    assert_eq!(
        executable
            .publish_due(
                &builder.publication_hub(),
                Some(&first),
                warm_now,
                ScreenPublicationHealth::Healthy,
            )
            .expect("warm publication succeeds")
            .published(),
        1
    );
    let next_due = executable
        .next_due_at()
        .expect("bound branch has a next deadline");
    assert!(
        executable
            .publish_due(
                &builder.publication_hub(),
                None,
                next_due,
                ScreenPublicationHealth::Healthy,
            )
            .expect("missing frame records pending demand")
            .needs_source()
    );
    let second = frame(&source, 2);
    let mut publish_region = Region::new(GLOBAL);
    publish_region.reset();

    let report = executable
        .publish_due(
            black_box(&builder.publication_hub()),
            Some(black_box(&second)),
            black_box(next_due),
            ScreenPublicationHealth::Healthy,
        )
        .expect("warmed publication succeeds");
    black_box(report);
    let publish_change = publish_region.change();

    assert_eq!(report.published(), 1);
    assert_eq!(publish_change.allocations, 0);
    assert_eq!(publish_change.reallocations, 0);
    assert_eq!(publish_change.bytes_allocated, 0);
}

#[test]
fn warmed_mixed_materialized_fanout_performs_no_heap_allocation() {
    let _guard = ALLOCATION_TEST_LOCK
        .lock()
        .expect("allocation test lock is healthy");
    let executor = CpuReductionExecutor::new(
        NonZeroUsize::new(4).expect("test worker count is non-zero"),
        non_zero(3),
    )
    .expect("test worker pool builds");
    let source = source();
    let smoothing = ScreenSmoothingPolicy::Exponential {
        time_constant: Duration::from_millis(250),
        scene_cut: ScreenSceneCutPolicy::Disabled,
    };
    let tuning = ScreenColorTuning::try_new(1.4, 0.8, 1.2).expect("test tuning is finite");
    let make_demand = |kind, output: PixelExtent, aspect, profile, requested_hz| {
        RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                kind,
                ScreenPublicationExecutorRequest::Cpu,
                ScreenExtentRequest::bounded(
                    Some(non_zero(output.width())),
                    Some(non_zero(output.height())),
                    ScreenUpscalePolicy::Allow,
                ),
                aspect,
                Arc::new(ScreenProcessingProfile::new(profile)),
            ),
            non_zero(requested_hz),
        )
        .resolve_with_color_capabilities(&source, executor.capabilities())
        .expect("test demand resolves")
    };
    let direct = make_demand(
        ScreenPublicationKind::Surface,
        extent(13, 7),
        ScreenAspectPolicy::Cover,
        ScreenProcessingProfileConfig::default(),
        60,
    );
    let shared_profile = ScreenProcessingProfileConfig {
        smoothing,
        tuning,
        ..ScreenProcessingProfileConfig::default()
    };
    let shared_surface = make_demand(
        ScreenPublicationKind::Surface,
        extent(9, 9),
        ScreenAspectPolicy::Cover,
        shared_profile.clone(),
        60,
    );
    let shared_zones = make_demand(
        ScreenPublicationKind::Zones {
            columns: non_zero(9),
            rows: non_zero(9),
        },
        extent(9, 9),
        ScreenAspectPolicy::Cover,
        shared_profile,
        30,
    );
    let dynamic_surface = make_demand(
        ScreenPublicationKind::Surface,
        extent(7, 9),
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: ScreenProfileScalar::try_new(0.02)
                    .expect("test threshold is finite"),
            },
            smoothing,
            tuning,
            ..ScreenProcessingProfileConfig::default()
        },
        45,
    );
    let demand_revision = InputPublicationDemandRevision::new(1);
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let mut builder = ScreenPlanBuilder::new();
    let mut preparing = builder
        .prepare(
            [direct, shared_surface, shared_zones, dynamic_surface],
            None,
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("mixed plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch().source_id)
        .expect("mixed source has a worker ticket");
    let resources = ticket
        .required_minimums()
        .iter()
        .map(|minimum| {
            ScreenExactResource::try_new(
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("mixed exact resources prepare");
    let lifetimes = resources
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()
        .expect("mixed resource lifetimes bind");
    let token = ticket
        .acknowledge(
            ScreenExactResourceLedger::try_new(resources).expect("mixed ledger prepares"),
            &lifetimes,
        )
        .expect("mixed resources satisfy the ticket");
    preparing
        .acknowledge(token)
        .expect("mixed worker token belongs to the candidate");
    let armed = preparing
        .arm(
            builder.current().generation(),
            demand_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("mixed plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, demand_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("mixed plan commits: {}", failure.error()));
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("mixed retired resources reclaim");
    let binding = builder
        .committed_state()
        .worker_bindings()
        .first()
        .cloned()
        .expect("mixed source has one worker binding");
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("mixed CPU batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("mixed materialization workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor, &batch, workspace, &plan,
    )
    .expect("mixed executable fanout prepares");
    let mut fanout = candidate
        .bind(builder.committed_state(), &binding)
        .expect("mixed executable fanout binds");
    assert_eq!(fanout.branch_count(), 4);
    assert_eq!(fanout.batch().len(), 3);
    let hub = builder.publication_hub();
    let first = frame(&source, 1);
    let warm_now = Instant::now();
    assert_eq!(
        fanout
            .publish_due(
                &hub,
                Some(&first),
                warm_now,
                ScreenPublicationHealth::Healthy,
            )
            .expect("mixed warm publication succeeds")
            .published(),
        4
    );
    let measured_at = warm_now + Duration::from_millis(100);
    assert!(
        fanout
            .publish_due(&hub, None, measured_at, ScreenPublicationHealth::Healthy,)
            .expect("mixed cadences become pending")
            .needs_source()
    );
    let second = frame_at(&source, 2, measured_at);
    let mut region = Region::new(GLOBAL);
    region.reset();

    let report = fanout
        .publish_due(
            black_box(&hub),
            Some(black_box(&second)),
            black_box(measured_at),
            ScreenPublicationHealth::Healthy,
        )
        .expect("warmed mixed publication succeeds");
    black_box(report);
    let change = region.change();

    assert_eq!(report.published(), 4);
    assert_eq!(change.allocations, 0);
    assert_eq!(change.reallocations, 0);
    assert_eq!(change.bytes_allocated, 0);
}
