use std::alloc::System;
use std::hint::black_box;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CpuReductionExecutor, InputPublicationDemandRevision, PhysicalOrigin,
    PixelExtent, PreparedCpuPublicationFanout, RegisteredScreenBranchDemand, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenExactResource,
    ScreenExactResourceLedger, ScreenExtentRequest, ScreenInputGraphGeneration, ScreenPlanBuilder,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenPublicationExecutorRequest,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenResourceApi, ScreenSourceReflection,
    ScreenSourceSelector, ScreenUpscalePolicy, SourceScale,
};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

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

#[test]
fn authority_binding_performs_no_heap_allocation() {
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
}
