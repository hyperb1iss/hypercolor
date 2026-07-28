//! Exact CPU reduction into committed writable screen publications.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CommittedScreenPlan, CpuCaptureStorage, CpuReductionBatchJob,
    CpuReductionError, CpuReductionExecutor, PhysicalOrigin, PixelExtent, RawCaptureSurface,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenBranchPayload, ScreenCaptureBackend, ScreenCapturePlan,
    ScreenColorTuning, ScreenCursorCapabilities, ScreenExactResource, ScreenExactResourceLedger,
    ScreenExtentRequest, ScreenInputGraphGeneration, ScreenPayloadKind,
    ScreenPhysicalReductionDescriptor, ScreenPlanBuilder, ScreenPlanError, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationHealth, ScreenPublicationKind,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenReductionFilter, ScreenResourceApi,
    ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy,
    ScreenWorkerBinding, ScreenWorkerPreparationTicket, SourceScale,
};

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
            CaptureColorimetry::SRGB,
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
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
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
            sequence: 17,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(2),
            geometry: source.config().geometry(),
            colorimetry: source.config().colorimetry(),
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::from(pixels),
            CapturePixelFormat::Rgba8,
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
    let frame = frame(&source);

    assert_eq!(batch.len(), 2);
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
