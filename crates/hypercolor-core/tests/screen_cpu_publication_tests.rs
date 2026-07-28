//! Exact CPU reduction into committed writable screen publications.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureColorimetry, CaptureCursor, CaptureDamage, CaptureDynamicRange,
    CaptureEpoch, CaptureFrame, CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat,
    CaptureRotation, CaptureSourceId, CaptureStorage, CaptureTransferFunction, ColorTuning,
    CommittedScreenPlan, CpuCaptureStorage, CpuReductionBatchJob, CpuReductionError,
    CpuReductionExecutor, CpuSurfaceReductionJob, CpuZoneMaterializationError,
    KnownCaptureColorimetry, PhysicalOrigin, PixelExtent, PreparedCpuZoneMaterializer,
    RawCaptureSurface, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenPublicationDescriptor, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenBranchPayload, ScreenCaptureBackend, ScreenCapturePlan, ScreenColorTuning,
    ScreenCursorCapabilities, ScreenExactResource, ScreenExactResourceLedger, ScreenExtentRequest,
    ScreenGridPolicy, ScreenInputGraphGeneration, ScreenPayloadKind,
    ScreenPhysicalReductionDescriptor, ScreenPlanBuilder, ScreenPlanError, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationHealth, ScreenPublicationKind,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenReductionFilter, ScreenResourceApi,
    ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector, ScreenTargetColorimetry,
    ScreenUpscalePolicy, ScreenWorkerBinding, ScreenWorkerPreparationTicket, SourceScale,
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
            source_id: source_id(),
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
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            ScreenExtentRequest::bounded(
                Some(non_zero(output_extent.width())),
                Some(non_zero(output_extent.height())),
                ScreenUpscalePolicy::Allow,
            ),
            ScreenAspectPolicy::Cover,
            Arc::new(ScreenProcessingProfile::new(profile)),
        ),
        non_zero(60),
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
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision remains representable");
    let mut preparing = builder
        .prepare(
            demands,
            None,
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
    let plan = reclaim(committed);
    let binding = builder
        .committed_state()
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == &source_id())
        .cloned()
        .expect("committed source has a worker binding");
    (plan, binding)
}

fn reclaim(committed: CommittedScreenPlan) -> ScreenCapturePlan {
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved retired pools reclaim immediately");
    plan
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
    let frame = frame(&source);

    assert_eq!(batch.len(), 1);
    assert_eq!(workspace.len(), 1);
    assert_eq!(workspace.pixels(0), None);
    assert_eq!(workspace.completed_source_sequence(0), None);
    let physical = batch.descriptor(0).expect("shared physical key exists");
    let surface_descriptor = surface_branch(&plan, physical);
    let zones_descriptor = zones_branch(&plan, physical, ScreenGridPolicy::AreaWeighted);
    assert!(matches!(
        PreparedCpuZoneMaterializer::prepare(surface_descriptor),
        Err(CpuZoneMaterializationError::BranchNotZones)
    ));
    let materializer = PreparedCpuZoneMaterializer::prepare(zones_descriptor)
        .expect("oversubscribed zones prepare");
    assert_eq!(materializer.descriptor(), zones_descriptor);
    assert_eq!(materializer.physical_descriptor(), physical);
    assert_eq!(materializer.zone_count(), 29 * 23);
    assert!(materializer.precomputed_byte_len() > 0);

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
    materializer
        .materialize(
            physical,
            surface_publication
                .surface_pixels_mut()
                .expect("physical surface remains writable"),
            &mut zones_publication,
        )
        .expect("the same physical bytes materialize zones");

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
    let frame = frame(&source);

    assert_eq!(batch.len(), 3);
    assert_eq!(workspace.plan_generation(), plan.generation());
    assert_eq!(workspace.len(), 2);
    assert!(!workspace.is_empty());
    assert!(workspace.allocation_byte_len() >= 3 * 15 * 4 + 9 * 9 * 4);
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
