//! Exact CPU reduction into committed writable screen publications.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::consumer::{
    CaptureEpoch, CaptureSourceId, ColorTuning, PixelExtent,
};
use hypercolor_core::input::screen::implementer::{
    CaptureColorSpace, CaptureColorimetry, CaptureCursor, CaptureDamage, CaptureDynamicRange,
    CaptureFrame, CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, CpuReductionBatchJob,
    CpuReductionError, CpuReductionExecutor, CpuSurfaceReductionJob, CpuZoneMaterializationError,
    KnownCaptureColorimetry, PhysicalOrigin, PreparedCpuLogicalFanoutKind,
    PreparedCpuPublicationFanout, PreparedCpuZoneMaterializer, RawCaptureSurface,
    ScreenBranchPayload, ScreenPublicationHealth, ScreenPublicationMetadata, SourceScale,
};
use hypercolor_core::input::screen::planner::{
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenCapturePlan, ScreenColorTuning,
    ScreenContentBarsPolicy, ScreenCursorCapabilities, ScreenExactResource,
    ScreenExactResourceLedger, ScreenExtentRequest, ScreenGridPolicy, ScreenInputGraphGeneration,
    ScreenPayloadKind, ScreenPhysicalReductionDescriptor, ScreenPlanBuilder, ScreenPlanError,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenProfileScalar,
    ScreenPublicationExecutorRequest, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenPublicationRetirement, ScreenReductionFilter, ScreenResourceApi, ScreenResourceLifetime,
    ScreenSourceReflection, ScreenSourceSelector, ScreenTargetColorimetry, ScreenUpscalePolicy,
    ScreenWorkerBinding, ScreenWorkerPreparationTicket,
};
use hypercolor_types::canvas::linear_to_srgb_u8;

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn source_id() -> CaptureSourceId {
    CaptureSourceId::new("synthetic:cpu-publication").expect("test source id is non-empty")
}

fn source(source_extent: PixelExtent) -> ResolvedScreenSource {
    source_with_colorimetry(source_extent, CaptureColorimetry::SRGB)
}

fn source_with_colorimetry(
    source_extent: PixelExtent,
    colorimetry: CaptureColorimetry,
) -> ResolvedScreenSource {
    source_with_identity(source_id(), source_extent, colorimetry)
}

fn source_with_identity(
    source_id: CaptureSourceId,
    source_extent: PixelExtent,
    colorimetry: CaptureColorimetry,
) -> ResolvedScreenSource {
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        source_extent,
        source_extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test source geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id,
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            source_extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            colorimetry,
            ScreenCursorCapabilities::clean_only(),
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn executor() -> CpuReductionExecutor {
    CpuReductionExecutor::new(
        NonZeroUsize::new(4).expect("test worker count is non-zero"),
        non_zero(3),
    )
    .expect("test worker pool builds")
}

fn demand(
    source: &ResolvedScreenSource,
    output_extent: PixelExtent,
    profile: ScreenProcessingProfileConfig,
    executor: &CpuReductionExecutor,
) -> ResolvedScreenBranchDemand {
    demand_for_kind(
        source,
        output_extent,
        ScreenPublicationKind::Surface,
        profile,
        executor,
    )
}

fn demand_for_kind(
    source: &ResolvedScreenSource,
    output_extent: PixelExtent,
    kind: ScreenPublicationKind,
    profile: ScreenProcessingProfileConfig,
    executor: &CpuReductionExecutor,
) -> ResolvedScreenBranchDemand {
    demand_for_kind_at_hz(source, output_extent, kind, profile, executor, 60)
}

fn demand_for_kind_at_hz(
    source: &ResolvedScreenSource,
    output_extent: PixelExtent,
    kind: ScreenPublicationKind,
    profile: ScreenProcessingProfileConfig,
    executor: &CpuReductionExecutor,
    requested_hz: u32,
) -> ResolvedScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                Some(non_zero(output_extent.width())),
                Some(non_zero(output_extent.height())),
                ScreenUpscalePolicy::Allow,
            ),
            ScreenAspectPolicy::Cover,
            Arc::new(ScreenProcessingProfile::new(profile)),
        ),
        non_zero(requested_hz),
    )
    .resolve_with_color_capabilities(source, executor.capabilities())
    .expect("test demand resolves")
}

fn exact_resources(
    ticket: &ScreenWorkerPreparationTicket,
) -> Result<(ScreenExactResourceLedger, Vec<ScreenResourceLifetime>), ScreenPlanError> {
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
        .collect::<Result<Vec<_>, _>>()?;
    let lifetimes = resources
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((ScreenExactResourceLedger::try_new(resources)?, lifetimes))
}

fn commit(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> (ScreenCapturePlan, ScreenWorkerBinding) {
    let (plan, binding, retirement) = commit_with_retirement(builder, demands);
    retirement
        .try_reclaim()
        .expect("unobserved retired pools reclaim immediately");
    (plan, binding)
}

fn commit_with_retirement(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> (
    ScreenCapturePlan,
    ScreenWorkerBinding,
    ScreenPublicationRetirement,
) {
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision remains representable");
    let mut preparing = builder
        .prepare(
            demands,
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan prepares");
    let mut worker_lifetimes = Vec::new();
    for required_source in preparing.required_sources().to_vec() {
        let ticket = preparing
            .worker_ticket(&required_source)
            .expect("required source has a worker ticket");
        let (ledger, lifetimes) = exact_resources(&ticket).expect("exact resources bind");
        let token = ticket
            .acknowledge(ledger, &lifetimes)
            .expect("worker resources satisfy the ticket");
        preparing
            .acknowledge(token)
            .expect("worker token belongs to the candidate");
        worker_lifetimes.push(lifetimes);
    }
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
    drop(worker_lifetimes);
    let (plan, retirement) = committed.into_parts();
    let binding = builder
        .committed_state()
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == &source_id())
        .cloned()
        .expect("committed source has a worker binding");
    (plan, binding, retirement)
}

fn frame(source: &ResolvedScreenSource) -> CaptureFrame<RawCaptureSurface> {
    frame_with_sequence(source, 17)
}

fn frame_with_sequence(
    source: &ResolvedScreenSource,
    sequence: u64,
) -> CaptureFrame<RawCaptureSurface> {
    let captured_at = Instant::now();
    let source_extent = source.config().geometry().storage_extent();
    let mut pixels = Vec::with_capacity(
        usize::try_from(u64::from(source_extent.width()) * u64::from(source_extent.height()) * 4)
            .expect("test pixels are addressable"),
    );
    for y in 0..source_extent.height() {
        for x in 0..source_extent.width() {
            pixels.extend_from_slice(&[
                ((u64::from(x) * 37 + u64::from(y) * 11 + 19) % 256) as u8,
                ((u64::from(x) * 13 + u64::from(y) * 53 + 71) % 256) as u8,
                ((u64::from(x) * 97 + u64::from(y) * 7 + 3) % 256) as u8,
                u8::MAX,
            ]);
        }
    }
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
            i64::from(source_extent.width()) * 4,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid")
}

fn surface_branch<'a>(
    plan: &'a ScreenCapturePlan,
    physical: &ScreenPhysicalReductionDescriptor,
) -> &'a ResolvedScreenPublicationDescriptor {
    let reduction = plan
        .physical_reductions()
        .iter()
        .find(|reduction| reduction.descriptor() == physical)
        .expect("committed plan retains the prepared physical key");
    reduction
        .branch_indices()
        .iter()
        .filter_map(|index| plan.branches().get(*index))
        .find(|branch| matches!(branch.descriptor().kind(), ScreenPublicationKind::Surface))
        .map(hypercolor_core::input::screen::ScreenBranchDemand::descriptor)
        .expect("test physical key owns a surface branch")
}

fn zones_branch<'a>(
    plan: &'a ScreenCapturePlan,
    physical: &ScreenPhysicalReductionDescriptor,
    grid: ScreenGridPolicy,
) -> &'a ResolvedScreenPublicationDescriptor {
    let reduction = plan
        .physical_reductions()
        .iter()
        .find(|reduction| reduction.descriptor() == physical)
        .expect("committed plan retains the prepared physical key");
    reduction
        .branch_indices()
        .iter()
        .filter_map(|index| plan.branches().get(*index))
        .find(|branch| {
            matches!(
                branch.descriptor().kind(),
                ScreenPublicationKind::Zones { .. }
            ) && branch.descriptor().processing_profile().grid() == grid
        })
        .map(hypercolor_core::input::screen::ScreenBranchDemand::descriptor)
        .expect("test physical key owns a zones branch")
}

#[test]
fn one_exact_reduction_fans_out_to_surface_and_oversubscribed_zones() {
    let executor = executor();
    let source = source(extent(17, 11));
    let profile = ScreenProcessingProfileConfig::default();
    let surface = demand(&source, extent(17, 11), profile.clone(), &executor);
    let zones = demand_for_kind(
        &source,
        extent(17, 11),
        ScreenPublicationKind::Zones {
            columns: non_zero(29),
            rows: non_zero(23),
        },
        profile,
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [surface, zones]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("shared physical batch prepares");
    let mut workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("shared physical workspace prepares");
    let fanout = PreparedCpuPublicationFanout::prepare_candidate(&batch, &workspace, &plan)
        .expect("shared physical fanout candidate prepares");
    assert!(fanout.allocation_byte_len() > 0);
    let mut fanout = fanout
        .bind(&builder.committed_state(), &binding)
        .expect("shared physical fanout binds");
    let frame = frame(&source);

    assert_eq!(batch.len(), 1);
    assert_eq!(workspace.len(), 1);
    assert_eq!(fanout.plan_generation(), plan.generation());
    assert_eq!(fanout.batch().len(), batch.len());
    assert_eq!(fanout.len(), 1);
    assert!(!fanout.is_empty());
    assert_eq!(fanout.branch_count(), 2);
    assert_eq!(fanout.physical_descriptor(0), batch.descriptor(0));
    assert_eq!(fanout.physical_descriptor(1), None);
    assert!(fanout.allocation_byte_len() > 0);
    let shared_route = &fanout.physical()[0];
    assert_eq!(shared_route.batch_index(), 0);
    assert_eq!(shared_route.workspace_index(), Some(0));
    assert_eq!(shared_route.branches().len(), 2);
    assert!(shared_route.branches().iter().any(|branch| {
        branch.kind() == PreparedCpuLogicalFanoutKind::DirectSurface
            && branch.zone_materializer().is_none()
    }));
    assert!(shared_route.branches().iter().any(|branch| {
        branch.kind() == PreparedCpuLogicalFanoutKind::Zones
            && branch.zone_materializer().is_some()
            && branch.publisher().descriptor() == branch.descriptor()
    }));
    assert_eq!(workspace.pixels(0), None);
    assert_eq!(workspace.completed_source_sequence(0), None);
    let physical = batch.descriptor(0).expect("shared physical key exists");
    let surface_descriptor = surface_branch(&plan, physical);
    let zones_descriptor = zones_branch(&plan, physical, ScreenGridPolicy::AreaWeighted);
    assert!(matches!(
        PreparedCpuZoneMaterializer::prepare(surface_descriptor),
        Err(CpuZoneMaterializationError::BranchNotZones)
    ));
    let stateless_materializer = PreparedCpuZoneMaterializer::prepare(zones_descriptor)
        .expect("oversubscribed zones prepare");
    assert_eq!(stateless_materializer.descriptor(), zones_descriptor);
    assert_eq!(stateless_materializer.physical_descriptor(), physical);
    assert_eq!(stateless_materializer.zone_count(), 29 * 23);
    assert!(stateless_materializer.precomputed_byte_len() > 0);

    let surface_publisher = hub
        .publisher(surface_descriptor, &binding)
        .expect("surface publisher is committed");
    let zones_publisher = hub
        .publisher(zones_descriptor, &binding)
        .expect("zones publisher is committed");
    let mut surface_publication = hub
        .prepare_writable_publication(
            &surface_publisher,
            ScreenPayloadKind::Surface,
            &intent(surface_descriptor, &binding, &frame),
        )
        .expect("surface slot reserves");
    let mut zones_publication = hub
        .prepare_writable_publication(
            &zones_publisher,
            ScreenPayloadKind::Zones,
            &intent(zones_descriptor, &binding, &frame),
        )
        .expect("zones slot reserves");

    surface_publication
        .surface_pixels_mut()
        .expect("surface slot is writable")
        .fill(0xA5);
    {
        let mut duplicate_jobs = [CpuSurfaceReductionJob::new(0, &mut surface_publication)];
        assert_eq!(duplicate_jobs[0].batch_index(), 0);
        assert_eq!(
            duplicate_jobs[0].publication().descriptor(),
            surface_descriptor
        );
        assert_eq!(
            executor.execute_scheduled_publications(
                &batch,
                &frame,
                &mut workspace,
                &[0],
                &mut duplicate_jobs,
            ),
            Err(CpuReductionError::ScheduledPhysicalKeyDuplicated { batch_index: 0 })
        );
    }
    assert!(
        surface_publication
            .surface_pixels_mut()
            .expect("duplicate schedule preserves the reservation")
            .iter()
            .all(|byte| *byte == 0xA5)
    );
    let report = {
        let mut surface_jobs = [CpuSurfaceReductionJob::new(0, &mut surface_publication)];
        executor
            .execute_scheduled_publications(&batch, &frame, &mut workspace, &[], &mut surface_jobs)
            .expect("one physical reduction writes the surface slot")
    };
    assert_eq!(report.completed_jobs(), 1);
    assert_eq!(report.output_bytes(), 17 * 11 * 4);
    assert_eq!(workspace.pixels(0), None);
    let stateful_materializer = fanout
        .physical_mut()
        .first_mut()
        .expect("shared physical route exists")
        .branches_mut()
        .iter_mut()
        .find(|branch| branch.kind() == PreparedCpuLogicalFanoutKind::Zones)
        .and_then(hypercolor_core::input::screen::PreparedCpuLogicalFanout::zone_materializer_mut)
        .expect("Zones route owns plan-lifetime state");
    assert_eq!(
        stateful_materializer.plan_generation(),
        Some(plan.generation())
    );
    let staged = stateful_materializer
        .stage(
            plan.generation(),
            physical,
            surface_publication
                .surface_pixels_mut()
                .expect("physical surface remains writable"),
            frame.metadata().captured_at,
            false,
            &mut zones_publication,
        )
        .expect("the same physical bytes stage Zones");
    assert_eq!(staged.columns(), 29);
    assert_eq!(staged.rows(), 23);
    assert_eq!(staged.color_count(), 29 * 23);

    hub.finalize_writable_publication(
        surface_publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("surface publication finalizes");
    hub.finalize_writable_publication(
        zones_publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("zones publication finalizes");
    stateful_materializer
        .commit_staged(plan.generation())
        .expect("accepted Zones state commits");
    let surface_latest = hub
        .lease(surface_descriptor)
        .expect("surface branch has a lease")
        .read()
        .expect("surface branch is live");
    let ScreenBranchPayload::Surface(surface) = surface_latest.payload() else {
        panic!("surface descriptor publishes a Surface");
    };
    let rejected_frame = frame_with_sequence(&source, 18);
    let mut rejected_publication = hub
        .prepare_writable_publication(
            &zones_publisher,
            ScreenPayloadKind::Zones,
            &intent(zones_descriptor, &binding, &rejected_frame),
        )
        .expect("next Zones slot reserves");
    stateful_materializer
        .stage(
            plan.generation(),
            physical,
            surface.pixels(),
            rejected_frame.metadata().captured_at,
            false,
            &mut rejected_publication,
        )
        .expect("next Zones state stages");
    let rejected_at = rejected_frame
        .metadata()
        .captured_at
        .checked_sub(Duration::from_nanos(1))
        .expect("test capture timestamp has a predecessor");
    assert!(matches!(
        hub.finalize_writable_publication(
            rejected_publication,
            rejected_at,
            ScreenPublicationHealth::Healthy,
        ),
        Err(hypercolor_core::input::screen::ScreenPublicationHubError::InvalidPublicationTimeline)
    ));
    stateful_materializer
        .discard_staged(plan.generation())
        .expect("rejected Zones state discards");
    let zones_latest = hub
        .lease(zones_descriptor)
        .expect("zones branch has a lease")
        .read()
        .expect("zones branch is live");
    let ScreenBranchPayload::Zones(zones) = zones_latest.payload() else {
        panic!("zones descriptor publishes zones");
    };
    assert_eq!(zones.columns(), non_zero(29));
    assert_eq!(zones.rows(), non_zero(23));
    assert_eq!(zones.colors().len(), 29 * 23);
    assert!(zones.colors().iter().any(|color| *color != [0, 0, 0]));
}

#[test]
fn exact_zone_publication_retains_dynamic_bars() {
    let executor = executor();
    let source = source(extent(5, 3));
    let zones = demand_for_kind(
        &source,
        extent(5, 3),
        ScreenPublicationKind::Zones {
            columns: non_zero(5),
            rows: non_zero(3),
        },
        ScreenProcessingProfileConfig {
            grid: ScreenGridPolicy::PointSample,
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: ScreenProfileScalar::try_new(0.02)
                    .expect("test threshold is finite"),
            },
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [zones]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("dynamic zone batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("dynamic zone workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor, &batch, workspace, &plan,
    )
    .expect("dynamic zone fanout prepares");
    let mut fanout = candidate
        .bind(&builder.committed_state(), &binding)
        .expect("dynamic zone fanout binds");

    let colors = [
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [255, 0, 255],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
    ];
    let pixels = colors
        .into_iter()
        .flat_map(|color| [color[0], color[1], color[2], u8::MAX])
        .collect::<Vec<_>>();
    let captured_at = Instant::now();
    let frame = CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation: source.epoch().session_generation,
            sequence: 1,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(2),
            geometry: source.config().geometry(),
            colorimetry: source.config().colorimetry(),
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::from(pixels),
            CapturePixelFormat::Rgba8,
            20,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("dynamic zone frame is valid");
    let report = fanout
        .publish_due(
            &hub,
            Some(&frame),
            captured_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("dynamic zone fanout publishes");
    assert_eq!(report.published(), 1);

    let descriptor = plan.branches()[0].descriptor();
    let publication = hub
        .lease(descriptor)
        .expect("dynamic zone branch has a lease")
        .read()
        .expect("dynamic zone branch is live");
    let ScreenBranchPayload::Zones(zones) = publication.payload() else {
        panic!("dynamic zone branch publishes zones");
    };
    assert_eq!(zones.columns(), non_zero(5));
    assert_eq!(zones.rows(), non_zero(1));
    assert_eq!((zones.bars().top, zones.bars().bottom), (1, 1));
    assert_eq!((zones.bars().left, zones.bars().right), (0, 0));
    assert_eq!(
        zones.colors(),
        &[
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [255, 0, 255],
        ]
    );
}

#[test]
fn executable_fanout_preserves_branch_cadence_pressure_and_authority() {
    let executor = executor();
    let source = source(extent(17, 11));
    let profile = ScreenProcessingProfileConfig::default();
    let surface = demand_for_kind_at_hz(
        &source,
        extent(17, 11),
        ScreenPublicationKind::Surface,
        profile.clone(),
        &executor,
        60,
    );
    let zones = demand_for_kind_at_hz(
        &source,
        extent(17, 11),
        ScreenPublicationKind::Zones {
            columns: non_zero(5),
            rows: non_zero(3),
        },
        profile,
        &executor,
        30,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [surface.clone(), zones.clone()]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("shared batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("multi-branch key retains one physical plane");
    assert_eq!(workspace.len(), 1);
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor, &batch, workspace, &plan,
    )
    .expect("executable fanout prepares");
    let mut fanout = candidate
        .bind(&builder.committed_state(), &binding)
        .expect("executable fanout binds");
    let first = frame_with_sequence(&source, 1);
    let initial = Instant::now();
    let report = fanout
        .publish_due(
            &hub,
            Some(&first),
            initial,
            ScreenPublicationHealth::Healthy,
        )
        .expect("initial branches publish");
    assert_eq!((report.published(), report.pressured()), (2, 0));

    let surface_due = fanout.next_due_at().expect("next branch deadline exists");
    let second = frame_with_sequence(&source, 2);
    let report = fanout
        .publish_due(
            &hub,
            Some(&second),
            surface_due,
            ScreenPublicationHealth::Healthy,
        )
        .expect("fast Surface cadence publishes");
    assert_eq!(report.published(), 1);
    let shared_due = surface_due + Duration::from_millis(20);
    let report = fanout
        .publish_due(
            &hub,
            Some(&second),
            shared_due,
            ScreenPublicationHealth::Healthy,
        )
        .expect("slow Zones cadence reuses retained static frame");
    assert_eq!(report.published(), 1);
    assert!(report.needs_source());

    let physical = batch.descriptor(0).expect("shared physical key exists");
    let surface_descriptor = surface_branch(&plan, physical);
    let surface_publisher = hub
        .publisher(surface_descriptor, &binding)
        .expect("surface publisher remains committed");
    fanout
        .publish_due(
            &hub,
            None,
            shared_due + Duration::from_millis(40),
            ScreenPublicationHealth::Healthy,
        )
        .expect("later cadence marks both branches pending");
    let third = frame_with_sequence(&source, 3);
    let pressure_intent = intent(surface_descriptor, &binding, &third);
    let mut held = Vec::new();
    loop {
        match hub.prepare_writable_publication(
            &surface_publisher,
            ScreenPayloadKind::Surface,
            &pressure_intent,
        ) {
            Ok(publication) => held.push(publication),
            Err(
                hypercolor_core::input::screen::ScreenPublicationHubError::PublicationPressure {
                    ..
                },
            ) => break,
            Err(error) => panic!("unexpected pressure preparation error: {error}"),
        }
    }
    let report = fanout
        .publish_due(
            &hub,
            Some(&third),
            third.metadata().captured_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("pressure remains branch-local");
    assert_eq!((report.published(), report.pressured()), (1, 1));
    drop(held);

    let stale_now = Instant::now();
    fanout
        .publish_due(
            &hub,
            None,
            stale_now + Duration::from_secs(1),
            ScreenPublicationHealth::Healthy,
        )
        .expect("old fanout records pending work before authority changes");
    let (_replacement, _replacement_binding) =
        commit(&mut builder, [surface.clone(), zones.clone()]);
    let fourth = frame_with_sequence(&source, 4);
    fanout
        .publish_due(
            &hub,
            Some(&fourth),
            fourth.metadata().captured_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("unchanged source authority retains its executable fanout");

    let replacement_surface = demand_for_kind_at_hz(
        &source,
        extent(13, 9),
        ScreenPublicationKind::Surface,
        ScreenProcessingProfileConfig::default(),
        &executor,
        60,
    );
    let (_replacement, _replacement_binding, retirement) =
        commit_with_retirement(&mut builder, [replacement_surface, zones]);
    let fifth = frame_with_sequence(&source, 5);
    assert!(matches!(
        fanout.publish_due(
            &hub,
            Some(&fifth),
            fifth.metadata().captured_at,
            ScreenPublicationHealth::Healthy,
        ),
        Err(
            hypercolor_core::input::screen::CpuPublicationFanoutError::Publisher(
                hypercolor_core::input::screen::ScreenPublicationHubError::PublisherStale { .. }
            )
        )
    ));
    drop(fanout);
    drop(surface_publisher);
    retirement
        .try_reclaim()
        .expect("replaced fanout storage reclaims after stale handles drop");
}

#[test]
fn mixed_fanout_materializes_retained_and_added_branch_bindings() {
    let executor = executor();
    let source = source(extent(17, 11));
    let zones = demand_for_kind(
        &source,
        extent(17, 11),
        ScreenPublicationKind::Zones {
            columns: non_zero(5),
            rows: non_zero(3),
        },
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let surface = demand(
        &source,
        extent(17, 11),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (initial_plan, initial_binding) = commit(&mut builder, [zones.clone()]);

    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision remains representable");
    let mut preparing = builder
        .prepare(
            [zones, surface],
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("mixed plan prepares");
    let ticket = preparing
        .worker_ticket(&source_id())
        .expect("mixed source has a worker ticket");
    let (ledger, lifetimes) = exact_resources(&ticket).expect("mixed resources bind");
    let token = ticket
        .acknowledge(ledger, &lifetimes)
        .expect("mixed resources satisfy the ticket");
    let runtime_binding = token.binding().clone();
    let candidate_plan = preparing.candidate_plan().clone();
    let batch = executor
        .prepare_batch(&source, &candidate_plan)
        .expect("mixed batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&candidate_plan)
        .expect("mixed workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor,
        &batch,
        workspace,
        &candidate_plan,
    )
    .expect("mixed fanout candidate prepares");
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
    drop(lifetimes);
    let (mixed_plan, retirement) = committed.into_parts();
    let mut fanout = candidate
        .bind(&builder.committed_state(), &runtime_binding)
        .expect("mixed fanout binds through its runtime authority");
    let captured = frame_with_sequence(&source, 41);
    let report = fanout
        .publish_due(
            &hub,
            Some(&captured),
            captured.metadata().captured_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("mixed fanout publishes retained and added branches");
    assert_eq!(report.published(), 2);
    for branch in mixed_plan.branches() {
        let publication = hub
            .lease(branch.descriptor())
            .expect("mixed branch has a lease")
            .read()
            .expect("mixed branch receives the frame");
        assert_eq!(publication.plan_generation(), mixed_plan.generation());
        match publication.payload() {
            ScreenBranchPayload::Zones(_) => assert_eq!(
                publication.worker_plan_generation(),
                initial_binding.plan_generation()
            ),
            ScreenBranchPayload::Surface(_) => assert_eq!(
                publication.worker_plan_generation(),
                runtime_binding.plan_generation()
            ),
            ScreenBranchPayload::GpuSurface(_) | ScreenBranchPayload::NativeWork(_) => {
                panic!("CPU fanout cannot publish GPU storage")
            }
        }
    }
    assert_ne!(initial_plan.generation(), mixed_plan.generation());
    drop(fanout);
    retirement
        .try_reclaim()
        .expect("mixed transition has no pinned retired storage");
}

#[test]
fn unbound_fanout_survives_an_unrelated_source_commit() {
    let executor = executor();
    let first_source = source(extent(17, 11));
    let second_source = source_with_identity(
        CaptureSourceId::new("synthetic:cpu-publication-second")
            .expect("second source id is non-empty"),
        extent(13, 9),
        CaptureColorimetry::SRGB,
    );
    let first_demand = demand_for_kind(
        &first_source,
        extent(17, 11),
        ScreenPublicationKind::Zones {
            columns: non_zero(5),
            rows: non_zero(3),
        },
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let second_demand = demand(
        &second_source,
        extent(13, 9),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (first_plan, first_binding) = commit(&mut builder, [first_demand.clone()]);
    let batch = executor
        .prepare_batch(&first_source, &first_plan)
        .expect("first-source batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&first_plan)
        .expect("first-source workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor,
        &batch,
        workspace,
        &first_plan,
    )
    .expect("first-source fanout candidate prepares");

    let (successor, carried_binding) = commit(&mut builder, [first_demand, second_demand]);
    assert_ne!(first_plan.generation(), successor.generation());
    assert_eq!(
        first_binding.plan_generation(),
        carried_binding.plan_generation()
    );
    let mut fanout = candidate
        .bind(&builder.committed_state(), &first_binding)
        .expect("unrelated source commit retains the first runtime authority");
    assert_eq!(fanout.plan_generation(), first_plan.generation());
    let captured = frame_with_sequence(&first_source, 43);
    let report = fanout
        .publish_due(
            &hub,
            Some(&captured),
            captured.metadata().captured_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("carried runtime publishes after the unrelated commit");
    assert_eq!(report.published(), 1);
}

#[test]
fn masked_native_and_prereduced_routes_share_one_sequence_without_duplicates() {
    let executor = executor();
    let source = source(extent(17, 11));
    let first = demand(
        &source,
        extent(9, 5),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let second = demand(
        &source,
        extent(5, 9),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [first, second]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("mixed physical batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("mixed physical workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor, &batch, workspace, &plan,
    )
    .expect("mixed executable fanout prepares");
    let mut fanout = candidate
        .bind(&builder.committed_state(), &binding)
        .expect("mixed executable fanout binds");
    assert_eq!(fanout.len(), 2);

    let now = Instant::now();
    assert!(matches!(
        fanout.publish_due_masked(&hub, None, now, ScreenPublicationHealth::Healthy, &[true],),
        Err(
            hypercolor_core::input::screen::CpuPublicationFanoutError::PhysicalMaskLengthMismatch {
                expected: 2,
                actual: 1,
            }
        )
    ));
    assert!(
        fanout
            .publish_due_masked(
                &hub,
                None,
                now,
                ScreenPublicationHealth::Healthy,
                &[false, true],
            )
            .expect("selected fallback route records source demand")
            .needs_source()
    );
    assert!(fanout.physical_pending(0));
    assert!(fanout.physical_pending(1));

    let reduced_descriptor = fanout
        .physical_descriptor(1)
        .expect("selected reduced descriptor exists");
    let reduced_len = usize::try_from(reduced_descriptor.reduction_extent().width())
        .expect("width fits usize")
        .checked_mul(
            usize::try_from(reduced_descriptor.reduction_extent().height())
                .expect("height fits usize"),
        )
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("fixture byte length fits usize");
    let reduced = vec![0x42; reduced_len];
    let reduced_report = fanout
        .publish_prereduced_physical(
            &hub,
            1,
            &reduced,
            1,
            now,
            now,
            ScreenPublicationHealth::Healthy,
        )
        .expect("GPU-reduced physical route publishes directly");
    assert_eq!(reduced_report.published(), 1);
    assert!(!fanout.physical_pending(1));
    assert!(fanout.physical_pending(0));

    let native = frame_with_sequence(&source, 1);
    let native_now = Instant::now();
    let native_report = fanout
        .publish_due_masked(
            &hub,
            Some(&native),
            native_now,
            ScreenPublicationHealth::Healthy,
            &[true, false],
        )
        .expect("native fallback publishes only its selected physical route");
    assert_eq!(native_report.published(), 1);
    assert!(!fanout.physical_pending(0));

    for physical in fanout.physical() {
        for branch in physical.branches() {
            assert_eq!(
                hub.lease(branch.descriptor())
                    .expect("mixed branch lease exists")
                    .read()
                    .expect("mixed branch publication is live")
                    .native_sequence(),
                NonZeroU64::MIN,
            );
        }
    }
}

#[test]
fn fanout_finalize_failure_is_atomic_and_discards_every_staged_branch() {
    let executor = executor();
    let source = source(extent(11, 11));
    let profile = ScreenProcessingProfileConfig {
        tuning: ScreenColorTuning::try_new(1.25, 0.8, 1.1).expect("test tuning is finite"),
        ..ScreenProcessingProfileConfig::default()
    };
    let wide = demand(&source, extent(9, 5), profile.clone(), &executor);
    let tall = demand(&source, extent(5, 9), profile, &executor);
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [wide, tall]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("materialized batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("materialized workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor, &batch, workspace, &plan,
    )
    .expect("materialized fanout prepares");
    let mut fanout = candidate
        .bind(&builder.committed_state(), &binding)
        .expect("materialized fanout binds");
    let first_descriptor = fanout.physical()[0].branches()[0].descriptor().clone();
    let second_descriptor = fanout.physical()[1].branches()[0].descriptor().clone();
    let seeded_frame = frame_with_sequence(&source, 10);
    let first_publisher = hub
        .publisher(&first_descriptor, &binding)
        .expect("first branch publisher is committed");
    let mut seeded = hub
        .prepare_writable_publication(
            &first_publisher,
            ScreenPayloadKind::Surface,
            &intent(&first_descriptor, &binding, &seeded_frame),
        )
        .expect("first branch seed slot reserves");
    seeded
        .surface_pixels_mut()
        .expect("seeded Surface is writable")
        .fill(0x33);
    hub.finalize_writable_publication(seeded, Instant::now(), ScreenPublicationHealth::Healthy)
        .expect("first branch seed finalizes");

    let rejected_frame = frame_with_sequence(&source, 5);
    let rejection = fanout
        .publish_due(
            &hub,
            Some(&rejected_frame),
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )
        .expect_err("batch rejects a non-monotonic branch before publishing peers");
    let hypercolor_core::input::screen::CpuPublicationFanoutError::Publisher(
        hypercolor_core::input::screen::ScreenPublicationHubError::NativeSequenceNotMonotonic {
            previous,
            observed,
        },
    ) = rejection
    else {
        panic!("unexpected fanout rejection: {rejection}");
    };
    assert_eq!((previous.get(), observed.get()), (10, 5));
    assert_eq!(
        hub.lease(&first_descriptor)
            .expect("first branch lease exists")
            .read()
            .expect("seed remains live")
            .native_sequence(),
        NonZeroU64::new(10).expect("test sequence is non-zero")
    );
    assert!(
        hub.lease(&second_descriptor)
            .expect("second branch lease exists")
            .read()
            .is_none()
    );

    let recovery_frame = frame_with_sequence(&source, 11);
    let report = fanout
        .publish_due(
            &hub,
            Some(&recovery_frame),
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )
        .expect("all reservations and stages recover after rejection");
    assert_eq!(report.published(), 2);
    for descriptor in [&first_descriptor, &second_descriptor] {
        assert_eq!(
            hub.lease(descriptor)
                .expect("branch lease exists")
                .read()
                .expect("recovery publication is live")
                .native_sequence(),
            NonZeroU64::new(11).expect("test sequence is non-zero")
        );
    }
}

#[test]
fn fanout_candidate_rejects_stale_runtime_authority() {
    let executor = executor();
    let source = source(extent(17, 11));
    let first = demand(
        &source,
        extent(13, 7),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let (first_plan, first_binding) = commit(&mut builder, [first]);
    let batch = executor
        .prepare_batch(&source, &first_plan)
        .expect("first exact batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&first_plan)
        .expect("first direct Surface needs no workspace");
    let candidate =
        PreparedCpuPublicationFanout::prepare_candidate(&batch, &workspace, &first_plan)
            .expect("first candidate allocation completes");
    let second = demand(
        &source,
        extent(11, 5),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let (second_plan, second_binding) = commit(&mut builder, [second]);
    let second_authority = builder.committed_state();

    assert!(matches!(
        candidate.bind(&second_authority, &first_binding),
        Err(
            hypercolor_core::input::screen::CpuPublicationFanoutError::WorkerRuntimeAuthorityMismatch
        )
    ));
    assert_eq!(
        second_authority.plan().generation(),
        second_plan.generation()
    );
    assert_ne!(first_plan.generation(), second_binding.plan_generation());
}

#[test]
fn fanout_candidate_failure_preserves_committed_authority() {
    let executor = executor();
    let source = source(extent(17, 11));
    let surface = demand(
        &source,
        extent(13, 7),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let (plan, _) = commit(&mut builder, [surface]);
    let authority = builder.committed_state();
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("first exact batch prepares");
    let unrelated_batch = executor
        .prepare_batch(&source, &plan)
        .expect("independent exact batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("first workspace prepares");

    assert!(matches!(
        PreparedCpuPublicationFanout::prepare_candidate(&unrelated_batch, &workspace, &plan),
        Err(hypercolor_core::input::screen::CpuPublicationFanoutError::WorkspaceBatchMismatch)
    ));
    assert!(Arc::ptr_eq(&authority, &builder.committed_state()));
    assert_eq!(authority.plan().generation(), plan.generation());
}

#[test]
fn materialization_workspace_retains_only_branch_local_physical_keys() {
    let executor = executor();
    let source = source(extent(17, 17));
    let identity_surface = demand(
        &source,
        extent(15, 3),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let zones = demand_for_kind(
        &source,
        extent(3, 15),
        ScreenPublicationKind::Zones {
            columns: non_zero(7),
            rows: non_zero(5),
        },
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let tuned_surface = demand(
        &source,
        extent(9, 9),
        ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(1.25, 1.0, 1.0).expect("test tuning is finite"),
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [identity_surface, zones, tuned_surface]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("mixed physical batch prepares");
    let mut workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("branch-local workspace prepares");
    let fanout = PreparedCpuPublicationFanout::prepare_candidate(&batch, &workspace, &plan)
        .expect("mixed physical fanout candidate prepares")
        .bind(&builder.committed_state(), &binding)
        .expect("mixed physical fanout binds");
    let frame = frame(&source);

    assert_eq!(batch.len(), 3);
    assert_eq!(fanout.len(), 3);
    assert_eq!(fanout.branch_count(), 3);
    assert_eq!(
        fanout
            .physical()
            .iter()
            .filter(|physical| physical.workspace_index().is_some())
            .count(),
        2
    );
    for expected_kind in [
        PreparedCpuLogicalFanoutKind::DirectSurface,
        PreparedCpuLogicalFanoutKind::MaterializedSurface,
        PreparedCpuLogicalFanoutKind::Zones,
    ] {
        assert_eq!(
            fanout
                .physical()
                .iter()
                .flat_map(hypercolor_core::input::screen::PreparedCpuPhysicalFanout::branches)
                .filter(|branch| branch.kind() == expected_kind)
                .count(),
            1
        );
    }
    assert_eq!(workspace.plan_generation(), plan.generation());
    assert_eq!(workspace.len(), 2);
    assert!(!workspace.is_empty());
    assert!(workspace.allocation_byte_len() >= 2 * (3 * 15 * 4 + 9 * 9 * 4));
    assert!((0..workspace.len()).all(|workspace_index| {
        workspace
            .physical_descriptor(workspace_index)
            .is_some_and(|physical| physical.reduction_extent() != extent(15, 3))
    }));
    assert!((0..workspace.len()).all(|workspace_index| {
        workspace.pixels(workspace_index).is_none()
            && workspace
                .completed_source_sequence(workspace_index)
                .is_none()
    }));

    let mut expected = (0..batch.len())
        .map(|index| vec![0; batch.output_byte_len(index).expect("output size exists")])
        .collect::<Vec<_>>();
    let mut jobs = expected
        .iter_mut()
        .enumerate()
        .map(|(index, output)| {
            CpuReductionBatchJob::new(
                batch.descriptor(index).expect("batch descriptor exists"),
                output,
            )
        })
        .collect::<Vec<_>>();
    executor
        .execute_batch(&batch, &frame, &mut jobs)
        .expect("reference batch executes");
    drop(jobs);
    let identity_batch_index = (0..batch.len())
        .find(|index| {
            batch
                .descriptor(*index)
                .is_some_and(|physical| physical.reduction_extent() == extent(15, 3))
        })
        .expect("identity Surface physical key exists");
    let identity_physical = batch
        .descriptor(identity_batch_index)
        .expect("identity Surface descriptor exists");
    let identity_descriptor = surface_branch(&plan, identity_physical);
    let identity_publisher = hub
        .publisher(identity_descriptor, &binding)
        .expect("identity Surface publisher is committed");
    let mut identity_publication = hub
        .prepare_writable_publication(
            &identity_publisher,
            ScreenPayloadKind::Surface,
            &intent(identity_descriptor, &binding, &frame),
        )
        .expect("identity Surface slot reserves");
    let report = {
        let mut surface_jobs = [CpuSurfaceReductionJob::new(
            identity_batch_index,
            &mut identity_publication,
        )];
        executor
            .execute_scheduled_publications(
                &batch,
                &frame,
                &mut workspace,
                &[0, 1],
                &mut surface_jobs,
            )
            .expect("mixed due set executes every physical key once")
    };
    assert_eq!(report.completed_jobs(), 3);
    assert_eq!(report.output_bytes(), 15 * 3 * 4 + 3 * 15 * 4 + 9 * 9 * 4);
    hub.finalize_writable_publication(
        identity_publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("identity Surface publication finalizes");
    let identity_latest = hub
        .lease(identity_descriptor)
        .expect("identity Surface branch has a lease")
        .read()
        .expect("identity Surface branch is live");
    let ScreenBranchPayload::Surface(identity_surface) = identity_latest.payload() else {
        panic!("identity Surface descriptor publishes a surface");
    };
    assert_eq!(identity_surface.pixels(), expected[identity_batch_index]);
    for workspace_index in 0..workspace.len() {
        let batch_index = workspace
            .batch_index(workspace_index)
            .expect("workspace index maps to the batch");
        assert_eq!(
            workspace
                .pixels(workspace_index)
                .expect("workspace pixels exist"),
            expected[batch_index]
        );
        assert_eq!(
            workspace.completed_source_sequence(workspace_index),
            Some(17)
        );
    }

    let zones_workspace_index = (0..workspace.len())
        .find(|workspace_index| {
            workspace
                .physical_descriptor(*workspace_index)
                .is_some_and(|physical| physical.reduction_extent() == extent(3, 15))
        })
        .expect("zones physical key owns retained storage");
    let physical = workspace
        .physical_descriptor(zones_workspace_index)
        .expect("zones physical descriptor exists");
    let descriptor = zones_branch(&plan, physical, ScreenGridPolicy::AreaWeighted);
    let materializer =
        PreparedCpuZoneMaterializer::prepare(descriptor).expect("zones materializer prepares");
    let publisher = hub
        .publisher(descriptor, &binding)
        .expect("zones publisher is committed");
    let mut publication = hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Zones,
            &intent(descriptor, &binding, &frame),
        )
        .expect("zones slot reserves without a surface slot");
    materializer
        .materialize(
            physical,
            workspace
                .pixels(zones_workspace_index)
                .expect("zones physical pixels are retained"),
            &mut publication,
        )
        .expect("zones materialize without a surface publication");
    hub.finalize_writable_publication(
        publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("zones-only publication finalizes");
    let latest = hub
        .lease(descriptor)
        .expect("zones-only branch has a lease")
        .read()
        .expect("zones-only branch is live");
    let ScreenBranchPayload::Zones(zones) = latest.payload() else {
        panic!("zones-only descriptor publishes zones");
    };
    assert_eq!(zones.colors().len(), 7 * 5);
    assert!(zones.colors().iter().any(|color| *color != [0, 0, 0]));

    let next_frame = frame_with_sequence(&source, 18);
    let workspace_report = executor
        .execute_materialization_workspace(&batch, &next_frame, &mut workspace)
        .expect("independent logical cadence refreshes retained planes only");
    assert_eq!(workspace_report.completed_jobs(), 2);
    assert_eq!(workspace_report.output_bytes(), 3 * 15 * 4 + 9 * 9 * 4);
    assert!((0..workspace.len()).all(|workspace_index| {
        workspace.completed_source_sequence(workspace_index) == Some(18)
    }));
    let before_mismatch = workspace
        .pixels(zones_workspace_index)
        .expect("zones pixels remain available")
        .to_vec();
    assert_eq!(
        executor.execute_materialization_workspace(&batch, &next_frame, &mut workspace),
        Err(CpuReductionError::WorkspaceFrameSequenceNotIncreasing {
            workspace_index: 0,
            previous: 18,
            actual: 18,
        })
    );
    assert_eq!(workspace.completed_source_sequence(0), Some(18));
    assert_eq!(
        workspace
            .pixels(zones_workspace_index)
            .expect("stale sequence preserves retained pixels"),
        before_mismatch
    );
    let independently_prepared = executor
        .prepare_batch(&source, &plan)
        .expect("equivalent independent batch prepares");
    assert_eq!(
        executor.execute_materialization_workspace(
            &independently_prepared,
            &next_frame,
            &mut workspace,
        ),
        Err(CpuReductionError::WorkspaceBatchMismatch)
    );
    assert_eq!(
        workspace
            .pixels(zones_workspace_index)
            .expect("mismatch preserves retained pixels"),
        before_mismatch
    );
}

#[test]
fn exact_zone_branches_share_pixels_but_keep_sampling_and_tuning_local() {
    let executor = executor();
    let source = source(extent(2, 1));
    let kind = ScreenPublicationKind::Zones {
        columns: non_zero(5),
        rows: non_zero(1),
    };
    let area = demand_for_kind(
        &source,
        extent(2, 1),
        kind,
        ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(0.5, 0.5, 2.0).expect("test tuning is finite"),
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let points = demand_for_kind(
        &source,
        extent(2, 1),
        kind,
        ScreenProcessingProfileConfig {
            grid: ScreenGridPolicy::PointSample,
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [area, points]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("shared zones batch prepares");
    let frame = frame(&source);

    assert_eq!(batch.len(), 1);
    let physical = batch.descriptor(0).expect("shared physical key exists");
    let area_descriptor = zones_branch(&plan, physical, ScreenGridPolicy::AreaWeighted);
    let point_descriptor = zones_branch(&plan, physical, ScreenGridPolicy::PointSample);
    let area_materializer =
        PreparedCpuZoneMaterializer::prepare(area_descriptor).expect("area materializer prepares");
    let point_materializer = PreparedCpuZoneMaterializer::prepare(point_descriptor)
        .expect("point materializer prepares");
    let area_publisher = hub
        .publisher(area_descriptor, &binding)
        .expect("area publisher is committed");
    let point_publisher = hub
        .publisher(point_descriptor, &binding)
        .expect("point publisher is committed");
    let mut area_publication = hub
        .prepare_writable_publication(
            &area_publisher,
            ScreenPayloadKind::Zones,
            &intent(area_descriptor, &binding, &frame),
        )
        .expect("area slot reserves");
    let mut point_publication = hub
        .prepare_writable_publication(
            &point_publisher,
            ScreenPayloadKind::Zones,
            &intent(point_descriptor, &binding, &frame),
        )
        .expect("point slot reserves");
    area_publication
        .zone_colors_mut()
        .expect("area colors are writable")
        .fill([0xA5; 3]);
    let physical_pixels = [255, 0, 0, 255, 0, 0, 255, 255];

    assert_eq!(
        area_materializer.materialize(
            physical,
            &physical_pixels[..physical_pixels.len() - 1],
            &mut area_publication,
        ),
        Err(CpuZoneMaterializationError::PhysicalByteLengthMismatch {
            expected: physical_pixels.len(),
            actual: physical_pixels.len() - 1,
        })
    );
    assert!(
        area_publication
            .zone_colors_mut()
            .expect("rejected reservation remains writable")
            .iter()
            .all(|color| *color == [0xA5; 3])
    );
    area_materializer
        .materialize(physical, &physical_pixels, &mut area_publication)
        .expect("area zones materialize");
    point_materializer
        .materialize(physical, &physical_pixels, &mut point_publication)
        .expect("point zones materialize");

    hub.finalize_writable_publication(
        area_publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("area publication finalizes");
    hub.finalize_writable_publication(
        point_publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("point publication finalizes");
    let area_latest = hub
        .lease(area_descriptor)
        .expect("area branch has a lease")
        .read()
        .expect("area branch is live");
    let point_latest = hub
        .lease(point_descriptor)
        .expect("point branch has a lease")
        .read()
        .expect("point branch is live");
    let ScreenBranchPayload::Zones(area_zones) = area_latest.payload() else {
        panic!("area descriptor publishes zones");
    };
    let ScreenBranchPayload::Zones(point_zones) = point_latest.payload() else {
        panic!("point descriptor publishes zones");
    };
    let midpoint = linear_to_srgb_u8(0.5);
    let mut expected_area = [
        [255, 0, 0],
        [255, 0, 0],
        [midpoint, 0, midpoint],
        [0, 0, 255],
        [0, 0, 255],
    ];
    ColorTuning {
        saturation: 0.5,
        brightness: 0.5,
        gamma: 2.0,
    }
    .apply(&mut expected_area);
    assert_eq!(area_zones.colors(), &expected_area);
    assert_eq!(
        point_zones.colors(),
        &[
            [255, 0, 0],
            [255, 0, 0],
            [0, 0, 255],
            [0, 0, 255],
            [0, 0, 255],
        ]
    );
}

#[test]
fn two_axis_bgra_area_materialization_is_exact() {
    let executor = executor();
    let source = source(extent(3, 2));
    let bgra = demand_for_kind(
        &source,
        extent(3, 2),
        ScreenPublicationKind::Zones {
            columns: non_zero(2),
            rows: non_zero(3),
        },
        ScreenProcessingProfileConfig {
            target_pixel_format: CapturePixelFormat::Bgra8,
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [bgra]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("BGRA physical reduction prepares");
    let frame = frame(&source);

    assert_eq!(batch.len(), 1);
    let physical = batch.descriptor(0).expect("BGRA physical key exists");
    assert_eq!(physical.target_pixel_format(), CapturePixelFormat::Bgra8);
    let descriptor = zones_branch(&plan, physical, ScreenGridPolicy::AreaWeighted);
    let materializer =
        PreparedCpuZoneMaterializer::prepare(descriptor).expect("BGRA materializer prepares");
    let publisher = hub
        .publisher(descriptor, &binding)
        .expect("BGRA publisher is committed");
    let mut publication = hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Zones,
            &intent(descriptor, &binding, &frame),
        )
        .expect("BGRA slot reserves");
    let pixels = [
        0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 255,
        255, 255,
    ];
    materializer
        .materialize(physical, &pixels, &mut publication)
        .expect("BGRA area zones materialize");
    hub.finalize_writable_publication(
        publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("BGRA publication finalizes");
    let latest = hub
        .lease(descriptor)
        .expect("BGRA branch has a lease")
        .read()
        .expect("BGRA branch is live");
    let ScreenBranchPayload::Zones(zones) = latest.payload() else {
        panic!("BGRA descriptor publishes zones");
    };
    let third = linear_to_srgb_u8(1.0 / 3.0);
    let half = linear_to_srgb_u8(0.5);
    let two_thirds = linear_to_srgb_u8(2.0 / 3.0);
    assert_eq!(
        zones.colors(),
        &[
            [two_thirds, third, 0],
            [0, third, two_thirds],
            [two_thirds, half, third],
            [third, half, third],
            [two_thirds, two_thirds, two_thirds],
            [two_thirds, two_thirds, 0],
        ]
    );
}

#[test]
fn linear_zone_materialization_preserves_quantized_channels() {
    let executor = executor();
    let linear_colorimetry = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Linear,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("linear SDR source is valid");
    let source = source_with_colorimetry(
        extent(2, 1),
        CaptureColorimetry::from_known(linear_colorimetry),
    );
    let linear = demand_for_kind(
        &source,
        extent(2, 1),
        ScreenPublicationKind::Zones {
            columns: non_zero(2),
            rows: non_zero(1),
        },
        ScreenProcessingProfileConfig {
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [linear]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("linear physical reduction prepares");
    let frame = frame(&source);

    assert_eq!(batch.len(), 1);
    let physical = batch.descriptor(0).expect("linear physical key exists");
    assert_eq!(
        physical.color_pipeline().output().transfer_function(),
        CaptureTransferFunction::Linear
    );
    let descriptor = zones_branch(&plan, physical, ScreenGridPolicy::AreaWeighted);
    let materializer =
        PreparedCpuZoneMaterializer::prepare(descriptor).expect("linear materializer prepares");
    let publisher = hub
        .publisher(descriptor, &binding)
        .expect("linear publisher is committed");
    let mut publication = hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Zones,
            &intent(descriptor, &binding, &frame),
        )
        .expect("linear slot reserves");
    let pixels = [128, 64, 32, 255, 7, 19, 251, 255];
    materializer
        .materialize(physical, &pixels, &mut publication)
        .expect("linear zones materialize");
    hub.finalize_writable_publication(
        publication,
        Instant::now(),
        ScreenPublicationHealth::Healthy,
    )
    .expect("linear publication finalizes");
    let latest = hub
        .lease(descriptor)
        .expect("linear branch has a lease")
        .read()
        .expect("linear branch is live");
    let ScreenBranchPayload::Zones(zones) = latest.payload() else {
        panic!("linear descriptor publishes zones");
    };
    assert_eq!(zones.colors(), &[[128, 64, 32], [7, 19, 251]]);
}

fn intent(
    descriptor: &ResolvedScreenPublicationDescriptor,
    binding: &ScreenWorkerBinding,
    frame: &CaptureFrame<RawCaptureSurface>,
) -> ScreenPublicationMetadata {
    ScreenPublicationMetadata::try_intent(
        descriptor.source_epoch().clone(),
        binding.plan_generation(),
        NonZeroU64::new(frame.metadata().sequence).expect("capture sequence is non-zero"),
        frame.metadata().captured_at,
        frame.metadata().fresh_until,
    )
    .expect("test publication intent is valid")
}

#[test]
fn incompatible_exact_surfaces_publish_directly_without_an_envelope() {
    let executor = executor();
    let source = source(extent(17, 17));
    let ultrawide = demand(
        &source,
        extent(15, 3),
        ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Bilinear,
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let portrait = demand(
        &source,
        extent(3, 15),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [ultrawide, portrait]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("committed exact batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&plan)
        .expect("identity-only surfaces need no retained planes");
    let frame = frame(&source);

    assert_eq!(batch.len(), 2);
    assert!(workspace.is_empty());
    assert_eq!(workspace.len(), 0);
    assert_eq!(workspace.allocation_byte_len(), 0);
    let extents = plan
        .physical_reductions()
        .iter()
        .map(|reduction| reduction.descriptor().reduction_extent())
        .collect::<Vec<_>>();
    assert!(extents.contains(&extent(15, 3)));
    assert!(extents.contains(&extent(3, 15)));
    assert!(!extents.contains(&extent(15, 15)));
    let physical_bytes = extents
        .iter()
        .map(|extent| u64::from(extent.width()) * u64::from(extent.height()) * 4)
        .sum::<u64>();
    assert_eq!(physical_bytes, 360);

    let mut expected = (0..batch.len())
        .map(|index| vec![0; batch.output_byte_len(index).expect("output size exists")])
        .collect::<Vec<_>>();
    let mut jobs = expected
        .iter_mut()
        .enumerate()
        .map(|(index, output)| {
            CpuReductionBatchJob::new(
                batch.descriptor(index).expect("batch descriptor exists"),
                output,
            )
        })
        .collect::<Vec<_>>();
    executor
        .execute_batch(&batch, &frame, &mut jobs)
        .expect("reference batch executes");
    drop(jobs);

    let descriptors = (0..batch.len())
        .map(|index| {
            surface_branch(
                &plan,
                batch.descriptor(index).expect("batch descriptor exists"),
            )
            .clone()
        })
        .collect::<Vec<_>>();
    let publishers = descriptors
        .iter()
        .map(|descriptor| {
            hub.publisher(descriptor, &binding)
                .expect("committed worker owns the exact branch")
        })
        .collect::<Vec<_>>();
    let mut publications = publishers
        .iter()
        .zip(descriptors.iter())
        .map(|(publisher, descriptor)| {
            hub.prepare_writable_publication(
                publisher,
                ScreenPayloadKind::Surface,
                &intent(descriptor, &binding, &frame),
            )
            .expect("exact surface slot reserves")
        })
        .collect::<Vec<_>>();

    let report = executor
        .execute_surface_publications(&batch, &frame, &mut publications)
        .expect("batch writes directly into exact publication slots");
    assert_eq!(report.source_sequence(), 17);
    assert_eq!(report.completed_jobs(), 2);
    assert_eq!(report.output_bytes(), physical_bytes);

    for (index, publication) in publications.into_iter().enumerate() {
        hub.finalize_writable_publication(
            publication,
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )
        .expect("exact surface publication finalizes");
        let lease = hub
            .lease(&descriptors[index])
            .expect("committed exact branch has a lease");
        let latest = lease.read().expect("exact branch is live");
        let ScreenBranchPayload::Surface(surface) = latest.payload() else {
            panic!("surface descriptor publishes a surface payload");
        };
        assert_eq!(
            surface.extent(),
            descriptors[index].geometry().output_extent()
        );
        assert_eq!(surface.pixels(), expected[index]);
    }
}

#[test]
fn branch_local_materialization_is_rejected_before_surface_writes() {
    let executor = executor();
    let source = source(extent(9, 9));
    let tuned = demand(
        &source,
        extent(7, 5),
        ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(1.5, 1.0, 1.0).expect("test tuning is finite"),
            ..ScreenProcessingProfileConfig::default()
        },
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (plan, binding) = commit(&mut builder, [tuned]);
    let batch = executor
        .prepare_batch(&source, &plan)
        .expect("tuned physical batch prepares");
    let frame = frame(&source);
    let descriptor = surface_branch(&plan, batch.descriptor(0).expect("batch descriptor exists"));
    let publisher = hub
        .publisher(descriptor, &binding)
        .expect("committed worker owns the tuned branch");
    let mut publications = [hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(descriptor, &binding, &frame),
        )
        .expect("tuned surface slot reserves")];
    publications[0]
        .surface_pixels_mut()
        .expect("surface slot is writable")
        .fill(0xA5);

    assert_eq!(
        executor.execute_surface_publications(&batch, &frame, &mut publications),
        Err(CpuReductionError::BatchPublicationRequiresMaterialization { index: 0 })
    );
    assert!(
        publications[0]
            .surface_pixels_mut()
            .expect("rejected slot remains reserved")
            .iter()
            .all(|byte| *byte == 0xA5)
    );
}

#[test]
fn superseded_source_reclaims_while_unrelated_bound_fanout_survives() {
    let executor = executor();
    let retained_source = source(extent(17, 11));
    let retired_source = source_with_identity(
        CaptureSourceId::new("synthetic:cpu-publication-retired")
            .expect("retired source id is non-empty"),
        extent(13, 9),
        CaptureColorimetry::SRGB,
    );
    let retained_demand = demand(
        &retained_source,
        extent(9, 5),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let retired_demand = demand(
        &retired_source,
        extent(7, 3),
        ScreenProcessingProfileConfig::default(),
        &executor,
    );
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let (first_plan, retained_binding) =
        commit(&mut builder, [retained_demand.clone(), retired_demand]);
    let batch = executor
        .prepare_batch(&retained_source, &first_plan)
        .expect("retained-source batch prepares");
    let workspace = batch
        .prepare_materialization_workspace(&first_plan)
        .expect("retained-source workspace prepares");
    let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
        &executor,
        &batch,
        workspace,
        &first_plan,
    )
    .expect("retained-source fanout candidate prepares");
    let fanout = candidate
        .bind(&builder.committed_state(), &retained_binding)
        .expect("retained-source fanout binds");

    let (_second_plan, _second_binding, retirement) =
        commit_with_retirement(&mut builder, [retained_demand]);
    retirement
        .try_reclaim()
        .expect("retired-source pools reclaim while an unrelated bound fanout survives");
    assert_eq!(hub.pending_retired_bytes(), 0);
    assert_eq!(fanout.plan_generation(), first_plan.generation());
}
