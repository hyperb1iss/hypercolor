use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    AdoptionAuthority, AdoptionWaitError, AnalysisEvent, AnalysisExchange, CaptureCallbackMetrics,
    CaptureExactCommand, CaptureFormatRequest, ChunkDropReason, CopyStats, DoubleBuffer,
    FormatOffer, NegotiatedFormat, NegotiatedVideoFormat, PendingPipeWireAdoption,
    PipeWireFormatAcknowledgment, PipeWireFormatRequest, PipeWireFormatState, PipeWireLoopExit,
    RestoreTokenSink, SharedSettings, SpaChunkView, SpaVideoFormat, UnavailablePark,
    VersionedCaptureSettings, WaylandAnalysisState, WaylandCapturePublication,
    WaylandCaptureUserData, WaylandExactPublicationShared, WaylandScreenCaptureInput,
    WaylandSourceMetadata, WaylandTopologySignature, commit_if_authorized, convert_packed_to_rgba,
    decode_chunk, fence_previous_publication, initial_native_extent_correction,
    initial_worker_demand, park_unavailable_worker, prepare_wayland_exact_runtime,
    publish_unexpected_exit_status, request_active_worker_demand, set_worker_demand,
    settle_pipewire_restoration, unavailable_format_outcome, wait_for_adoption_result,
    worker_demand_epoch, worker_demanded,
};
use crate::input::screen::adapter::{CaptureSessionAuthority, reap_capture_exact_runtimes};
use crate::input::screen::{
    AnalyzedScreenSnapshot, CaptureColorimetry, CaptureConfig, CaptureFrame, CaptureFrameError,
    CaptureRotation, CaptureSourceId, InputPublicationDemandRevision,
    MAX_REPRESENTABLE_CAPTURE_FPS, PhysicalOrigin, PixelExtent, PixelRect, RawCaptureSurface,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ScreenAdmissionCapacity,
    ScreenAnalysisComputeCapacity, ScreenAspectPolicy, ScreenBranchPayload,
    ScreenByteAdmissionCoordinator, ScreenCaptureBackend, ScreenCaptureDemand,
    ScreenComputeCapacityPolicy, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenPlanBuilder, ScreenProcessingProfile, ScreenPublicationExecutorRequest,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenResourceApi, ScreenResourceKind,
    ScreenSourceSelector, ScreenUpscalePolicy, ScreenWorkerBindingState, SourceScale,
    analyze_screen_frame,
};
use crate::input::{SourceIssue, SourceKind, SourceState, SourceStatusReporter};

fn settings(session_generation: u64) -> Arc<SharedSettings> {
    let publication = Arc::new(Mutex::new(WaylandCapturePublication::default()));
    let exact = WaylandExactPublicationShared::default();
    drop(exact.activate_authority(CaptureSessionAuthority::new(session_generation)));
    Arc::new(SharedSettings {
        values: VersionedCaptureSettings::new(CaptureConfig::default(), active_demand()),
        admission_coordinator: ScreenByteAdmissionCoordinator::default(),
        compute_capacity_policy: ScreenComputeCapacityPolicy::UNBOUNDED,
        topology_generation: 0.into(),
        topology: Mutex::new(None),
        session_generation: session_generation.into(),
        session_guard: Mutex::new(()),
        publication,
        exact,
    })
}

fn source_id(value: &str) -> CaptureSourceId {
    CaptureSourceId::new(Arc::<str>::from(value)).expect("test source id is valid")
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is valid")
}

fn active_demand() -> ScreenCaptureDemand {
    ScreenCaptureDemand::active(extent(640, 480))
}

fn format_request(width: u32, height: u32, target_fps: u32) -> PipeWireFormatRequest {
    let config = CaptureConfig {
        target_fps,
        ..CaptureConfig::default()
    };
    PipeWireFormatRequest::new_with_compute_capacity(
        extent(width, height),
        extent(640, 480),
        &config,
        unlimited_compute_capacity(),
    )
    .expect("test PipeWire format request is valid")
}

fn unlimited_compute_capacity() -> ScreenAnalysisComputeCapacity {
    ScreenAnalysisComputeCapacity::new(NonZeroUsize::MIN, NonZeroU64::MAX)
}

fn negotiated_format(width: u32, height: u32, target_fps: u32) -> NegotiatedVideoFormat {
    NegotiatedVideoFormat {
        width,
        height,
        format: SpaVideoFormat::Rgba,
        framerate: hypercolor_pipewire_interop::VideoFraction {
            numerator: target_fps,
            denominator: 1,
        },
    }
}

fn format_offer(request: PipeWireFormatRequest) -> FormatOffer {
    FormatOffer::new(CaptureFormatRequest {
        width: request.extent.width(),
        height: request.extent.height(),
        target_fps: request.target_fps,
    })
    .expect("test format offer is valid")
}

fn pending_adoption(id: u64, request: PipeWireFormatRequest) -> PendingPipeWireAdoption {
    pending_adoption_with_done(id, request).0
}

fn pending_adoption_with_done(
    id: u64,
    request: PipeWireFormatRequest,
) -> (PendingPipeWireAdoption, mpsc::Receiver<Result<(), String>>) {
    let (analysis_decision, _analysis_decision_rx) = mpsc::sync_channel(1);
    let (_analysis_done_tx, analysis_done) = mpsc::sync_channel(1);
    let (done, done_rx) = mpsc::sync_channel(1);
    (
        PendingPipeWireAdoption {
            id,
            request,
            offer: format_offer(request),
            callback_buffers: DoubleBuffer::try_with_capacity(4)
                .expect("test callback storage allocates"),
            analysis_decision,
            analysis_done,
            done,
            authority: Arc::new(AdoptionAuthority::default()),
        },
        done_rx,
    )
}

fn source(
    session_generation: u64,
    origin: PhysicalOrigin,
    logical_extent: PixelExtent,
) -> WaylandSourceMetadata {
    WaylandSourceMetadata {
        signature: WaylandTopologySignature {
            source_id: source_id("wayland:portal:stable"),
            origin,
            logical_extent: Some(logical_extent),
            native_extent: None,
            transform: CaptureRotation::Identity,
        },
        session_generation,
        topology: None,
    }
}

fn capture_legacy(
    state: &mut WaylandAnalysisState,
    width: u32,
    height: u32,
    fill: u8,
) -> AnalyzedScreenSnapshot {
    let frame = capture_raw(state, width, height, fill);
    analyze_screen_frame(&mut state.analyzer, frame)
        .expect("screen analysis accepts canonical test geometry")
}

fn capture_raw(
    state: &mut WaylandAnalysisState,
    width: u32,
    height: u32,
    fill: u8,
) -> CaptureFrame<RawCaptureSurface> {
    capture_raw_with_transform(state, width, height, fill, CaptureRotation::Identity)
}

fn capture_raw_with_transform(
    state: &mut WaylandAnalysisState,
    width: u32,
    height: u32,
    fill: u8,
    transform: CaptureRotation,
) -> CaptureFrame<RawCaptureSurface> {
    let plane_len = usize::try_from(width)
        .expect("test width fits usize")
        .checked_mul(usize::try_from(height).expect("test height fits usize"))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("test plane length fits usize");
    let mut plane = state
        .plane_pool
        .try_acquire(plane_len)
        .expect("test plane allocation succeeds");
    plane.resize(plane_len, fill);
    state
        .capture_frame(
            Instant::now(),
            width,
            height,
            None,
            transform,
            plane.freeze(),
            CaptureColorimetry::SRGB,
        )
        .expect("test frame is valid")
}

fn exact_demand(
    source: &super::WaylandPublicationSource,
    exact: &WaylandExactPublicationShared,
    kind: ScreenPublicationKind,
) -> ResolvedScreenBranchDemand {
    let executor = exact.cpu_executor().expect("test CPU executor prepares");
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Exact(source.epoch.source_id.clone()),
            kind,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                NonZeroU32::new(4),
                NonZeroU32::new(2),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        NonZeroU32::new(60).expect("test cadence is nonzero"),
    )
    .resolve_with_color_capabilities(
        &source.resolved(ScreenSourceSelector::Exact(source.epoch.source_id.clone())),
        executor.capabilities(),
    )
    .expect("test exact branch resolves")
}

fn rgba_view(
    data: &[u8],
    offset: usize,
    size: usize,
    stride: i32,
    width: u32,
    height: u32,
) -> SpaChunkView<'_> {
    SpaChunkView::new(
        data,
        offset,
        size,
        stride,
        width,
        height,
        SpaVideoFormat::Rgba,
        None,
        CaptureRotation::Identity,
    )
}

#[test]
fn physical_topology_advances_for_extent_transform_and_source_changes() {
    let settings = settings(7);
    let physical_origin = PhysicalOrigin { x: -1920, y: 0 };
    let mut first_worker = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(7, physical_origin, extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");

    let first = capture_legacy(&mut first_worker, 4, 2, 1);
    let resized = capture_legacy(&mut first_worker, 2, 1, 2);
    assert_eq!(first.geometry_frame().metadata().topology_generation, 1);
    assert_eq!(resized.geometry_frame().metadata().topology_generation, 2);
    assert_eq!(
        resized.geometry_frame().metadata().geometry.native_extent(),
        extent(2, 1)
    );
    assert_eq!(
        resized
            .geometry_frame()
            .metadata()
            .geometry
            .storage_extent(),
        extent(2, 1)
    );

    let stable = capture_legacy(&mut first_worker, 2, 1, 3);
    assert_eq!(stable.geometry_frame().metadata().topology_generation, 2);

    let transformed =
        capture_raw_with_transform(&mut first_worker, 2, 1, 4, CaptureRotation::Clockwise90);
    assert_eq!(transformed.metadata().topology_generation, 3);
    assert_eq!(
        transformed.metadata().geometry.source_scale(),
        SourceScale::new(1920, 1).expect("test source scale is valid")
    );

    let next_session = settings.begin_session();
    let mut successor = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(next_session, physical_origin, extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let restarted = capture_legacy(&mut successor, 2, 1, 5);
    assert_eq!(restarted.geometry_frame().metadata().topology_generation, 4);
    assert_eq!(
        restarted
            .geometry_frame()
            .metadata()
            .geometry
            .native_extent(),
        extent(2, 1)
    );

    let mut moved_source = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(
            next_session,
            PhysicalOrigin { x: 0, y: -1080 },
            extent(1920, 1080),
        ),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let moved = capture_legacy(&mut moved_source, 2, 1, 6);
    assert_eq!(moved.geometry_frame().metadata().topology_generation, 5);
    assert_eq!(
        moved.geometry_frame().metadata().geometry.native_extent(),
        extent(2, 1)
    );
}

#[test]
fn exact_publication_source_is_stable_until_capture_identity_changes() {
    let settings = settings(31);
    let mut worker = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(31, PhysicalOrigin { x: -2560, y: 0 }, extent(2560, 1440)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");

    let first = capture_legacy(&mut worker, 4, 2, 7);
    let first_revision = settings.exact.resolution_revision();
    let publication = settings
        .exact
        .source()
        .expect("first capture resolves an exact publication source");

    assert_eq!(first_revision, 1);
    assert_eq!(
        publication.epoch.source_id,
        first.geometry_frame().metadata().source_id
    );
    assert!(publication.matches_selector(&ScreenSourceSelector::Configured));
    assert!(publication.matches_selector(&ScreenSourceSelector::Primary));
    assert!(publication.matches_selector(&ScreenSourceSelector::Exact(
        publication.epoch.source_id.clone()
    )));
    assert!(
        !publication.matches_selector(&ScreenSourceSelector::Exact(source_id(
            "wayland:portal:other"
        )))
    );
    assert_eq!(publication.config.geometry().storage_extent(), extent(4, 2));
    assert_eq!(publication.config.logical_extent(), extent(2560, 1440));
    assert_eq!(
        publication.config.resources().backend(),
        &ScreenCaptureBackend::WaylandPipeWire
    );
    assert_eq!(
        publication.config.resources().api(),
        &ScreenResourceApi::Cpu
    );

    capture_legacy(&mut worker, 4, 2, 8);
    assert_eq!(
        settings.exact.resolution_revision(),
        first_revision,
        "pixel contents and sequence changes do not invalidate prepared resources"
    );

    capture_legacy(&mut worker, 2, 1, 9);
    assert!(settings.exact.resolution_revision() > first_revision);
}

#[test]
fn exact_box_list_mutation_preserves_node_exactness_and_iterative_cleanup() {
    let mut values = crate::input::screen::ExactBoxList::default();
    for value in 0_u64..10_000 {
        values.push_boxed(crate::input::screen::ExactBoxList::boxed_node(value));
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
fn cpu_executor_is_shared_across_exact_plan_generations() {
    let exact = WaylandExactPublicationShared::default();
    let first = exact.cpu_executor().expect("test CPU executor prepares");

    for _ in 0..100 {
        let next = exact.cpu_executor().expect("test CPU executor is reused");
        assert!(Arc::ptr_eq(&first, &next));
    }
}

#[test]
fn exact_runtime_publishes_surface_and_zones_from_one_captured_frame() {
    let settings = settings(73);
    let mut worker = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(73, PhysicalOrigin::default(), extent(4, 2)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let frame = capture_raw(&mut worker, 4, 2, 19);
    let source = settings
        .exact
        .source()
        .expect("captured frame resolves the publication source");
    let demands = [
        exact_demand(&source, &settings.exact, ScreenPublicationKind::Surface),
        exact_demand(
            &source,
            &settings.exact,
            ScreenPublicationKind::Zones {
                columns: NonZeroU32::new(2).expect("test grid width is nonzero"),
                rows: NonZeroU32::MIN,
            },
        ),
    ];
    let retained_demands = [demands[0].clone()];
    let mixed_demands = demands.clone();
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    settings.exact.install_hub(Arc::clone(&hub));
    let revision = InputPublicationDemandRevision::new(1);
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            demands,
            None,
            revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test exact plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch.source_id)
        .expect("test source owns an exact worker ticket");
    let (token, runtime) = prepare_wayland_exact_runtime(
        ticket,
        Some(&source),
        &settings.exact,
        ScreenComputeCapacityPolicy::UNBOUNDED,
    )
    .expect("Wayland exact runtime prepares");
    let (runtime, owned_source) = runtime.expect("test plan owns exact branches");
    worker
        .exact_runtimes
        .push_boxed(super::WaylandExactRuntimes::boxed_node(runtime));
    settings
        .exact
        .register_test_owned_source(crate::input::screen::ExactBoxList::boxed_node(owned_source));
    preparing
        .acknowledge(token)
        .expect("exact worker token belongs to the candidate plan");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph_generation)
        .unwrap_or_else(|failure| panic!("test exact plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, revision, graph_generation)
        .unwrap_or_else(|failure| panic!("test exact plan commits: {}", failure.error()));
    let (_, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("initial plan has no observed retired publications");
    let initial_generation = builder.current().generation();

    worker
        .publish_exact(&frame)
        .expect("captured frame fans out into exact publications");

    let plan = builder.current();
    assert_eq!(plan.branches().len(), 2);
    let mut surface_seen = false;
    let mut zones_seen = false;
    for branch in plan.branches() {
        let publication = hub
            .lease(branch.descriptor())
            .expect("committed branch has a lease")
            .read()
            .expect("first captured frame publishes every branch");
        match publication.payload() {
            ScreenBranchPayload::Surface(surface) => {
                surface_seen = true;
                assert_eq!(surface.extent(), extent(4, 2));
                assert_eq!(surface.pixels().len(), 32);
            }
            ScreenBranchPayload::Zones(zones) => {
                zones_seen = true;
                assert_eq!(zones.columns(), NonZeroU32::new(2).expect("nonzero"));
                assert_eq!(zones.rows(), NonZeroU32::MIN);
                assert_eq!(zones.colors().len(), 2);
            }
            ScreenBranchPayload::GpuSurface(_) | ScreenBranchPayload::NativeWork(_) => {
                panic!("Wayland exact CPU runtime cannot publish a GPU surface")
            }
        }
    }
    assert!(surface_seen && zones_seen);
    drop(plan);

    let retained_revision = revision.next().expect("test revision advances");
    let mut retained_preparing = builder
        .prepare(
            retained_demands,
            None,
            retained_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("retained-only successor plan prepares");
    let retained_ticket = retained_preparing
        .worker_ticket(&source.epoch.source_id)
        .expect("retained source owns a successor worker ticket");
    assert!(
        retained_ticket
            .required_minimums()
            .iter()
            .all(|minimum| { minimum.resource() != ScreenResourceKind::ProcessingProfileState })
    );
    let (retained_token, retained_runtime) = prepare_wayland_exact_runtime(
        retained_ticket,
        Some(&source),
        &settings.exact,
        ScreenComputeCapacityPolicy::UNBOUNDED,
    )
    .expect("retained-only Wayland exact runtime prepares");
    let (retained_runtime, retained_owned_source) =
        retained_runtime.expect("retained-only plan owns an exact runtime");
    assert!(
        retained_token
            .exact_ledger()
            .resources()
            .iter()
            .any(|resource| { resource.name().as_ref() == "wayland-cpu-fanout" })
    );
    let retained_runtime_binding = retained_token.binding().clone();
    worker
        .exact_runtimes
        .push_boxed(super::WaylandExactRuntimes::boxed_node(retained_runtime));
    settings
        .exact
        .register_test_owned_source(crate::input::screen::ExactBoxList::boxed_node(
            retained_owned_source,
        ));
    retained_preparing
        .acknowledge(retained_token)
        .expect("retained-only token belongs to the successor plan");
    let armed = retained_preparing
        .arm(
            builder.current().generation(),
            retained_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("retained-only plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, retained_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("retained-only plan commits: {}", failure.error()));
    let (_, retained_retirement) = committed.into_parts();
    let retained_generation = builder.current().generation();
    assert!(
        builder
            .committed_state()
            .owns_runtime_binding(&retained_runtime_binding)
    );
    assert_eq!(
        retained_runtime_binding.state(),
        ScreenWorkerBindingState::Retired
    );
    reap_capture_exact_runtimes(
        CaptureSessionAuthority::new(source.epoch.session_generation),
        &mut worker.exact_runtimes,
        &settings.exact,
    );
    assert_eq!(worker.exact_runtimes.iter().count(), 1);

    let retained_frame = capture_raw(&mut worker, 4, 2, 29);
    worker
        .publish_exact(&retained_frame)
        .expect("retained-only runtime publishes through inherited branch authority");
    let retained_plan = builder.current();
    assert_eq!(retained_plan.branches().len(), 1);
    let retained_publication = hub
        .lease(retained_plan.branches()[0].descriptor())
        .expect("retained surface has a lease")
        .read()
        .expect("retained surface receives the second frame");
    assert_eq!(
        retained_publication.native_sequence(),
        NonZeroU64::new(2).expect("test sequence is nonzero")
    );
    assert_eq!(retained_publication.plan_generation(), retained_generation);
    assert_eq!(
        retained_publication.worker_plan_generation(),
        initial_generation
    );
    drop(retained_publication);
    drop(retained_plan);

    let mixed_revision = retained_revision.next().expect("test revision advances");
    let mut mixed_preparing = builder
        .prepare(
            mixed_demands,
            None,
            mixed_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("mixed retained and added plan prepares");
    let mixed_ticket = mixed_preparing
        .worker_ticket(&source.epoch.source_id)
        .expect("mixed successor owns a worker ticket");
    let (mixed_token, mixed_runtime) = prepare_wayland_exact_runtime(
        mixed_ticket,
        Some(&source),
        &settings.exact,
        ScreenComputeCapacityPolicy::UNBOUNDED,
    )
    .expect("mixed retained and added Wayland runtime prepares");
    let (mixed_runtime, mixed_owned_source) =
        mixed_runtime.expect("mixed plan owns an exact runtime");
    let mixed_runtime_binding = mixed_token.binding().clone();
    worker
        .exact_runtimes
        .push_boxed(super::WaylandExactRuntimes::boxed_node(mixed_runtime));
    settings
        .exact
        .register_test_owned_source(crate::input::screen::ExactBoxList::boxed_node(
            mixed_owned_source,
        ));
    mixed_preparing
        .acknowledge(mixed_token)
        .expect("mixed worker token belongs to the successor plan");
    let armed = mixed_preparing
        .arm(
            builder.current().generation(),
            mixed_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("mixed successor plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, mixed_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("mixed successor plan commits: {}", failure.error()));
    let (_, mixed_retirement) = committed.into_parts();
    let mixed_generation = builder.current().generation();
    assert!(
        builder
            .committed_state()
            .owns_runtime_binding(&mixed_runtime_binding)
    );
    reap_capture_exact_runtimes(
        CaptureSessionAuthority::new(source.epoch.session_generation),
        &mut worker.exact_runtimes,
        &settings.exact,
    );
    assert_eq!(worker.exact_runtimes.iter().count(), 1);

    let mixed_frame = capture_raw(&mut worker, 4, 2, 31);
    worker
        .publish_exact(&mixed_frame)
        .expect("mixed runtime publishes retained and added branches atomically");
    let mixed_plan = builder.current();
    assert_eq!(mixed_plan.branches().len(), 2);
    for branch in mixed_plan.branches() {
        let publication = hub
            .lease(branch.descriptor())
            .expect("mixed branch has a lease")
            .read()
            .expect("mixed branch receives the third frame");
        assert_eq!(
            publication.native_sequence(),
            NonZeroU64::new(3).expect("test sequence is nonzero")
        );
        assert_eq!(publication.plan_generation(), mixed_generation);
        match publication.payload() {
            ScreenBranchPayload::Surface(_) => {
                assert_eq!(publication.worker_plan_generation(), initial_generation);
            }
            ScreenBranchPayload::Zones(_) => {
                assert_eq!(publication.worker_plan_generation(), mixed_generation);
            }
            ScreenBranchPayload::GpuSurface(_) | ScreenBranchPayload::NativeWork(_) => {
                panic!("Wayland exact CPU runtime cannot publish a GPU surface")
            }
        }
    }
    drop(mixed_plan);

    let owned_source_id = source.epoch.source_id.clone();
    settings.exact.replace_source_if_current(
        CaptureSessionAuthority::new(source.epoch.session_generation),
        None,
    );
    assert!(settings.exact.owns_source(&owned_source_id));
    let retirement_revision = mixed_revision.next().expect("test revision advances");
    let mut preparing = builder
        .prepare(
            std::iter::empty::<ResolvedScreenBranchDemand>(),
            None,
            retirement_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("empty successor plan prepares");
    let removal_ticket = preparing
        .worker_ticket(&owned_source_id)
        .expect("retiring source owns a removal-only worker ticket");
    assert!(removal_ticket.source_delta().added_branches().is_empty());
    assert!(removal_ticket.source_delta().retained_branches().is_empty());
    let (removal_token, removal_runtime) = prepare_wayland_exact_runtime(
        removal_ticket,
        None,
        &settings.exact,
        ScreenComputeCapacityPolicy::UNBOUNDED,
    )
    .expect("removal-only Wayland acknowledgement prepares");
    assert!(removal_runtime.is_none());
    preparing
        .acknowledge(removal_token)
        .expect("removal-only token belongs to the empty successor");
    let armed = preparing
        .arm(
            builder.current().generation(),
            retirement_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("empty successor plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, retirement_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("empty successor plan commits: {}", failure.error()));
    let (_, retirement) = committed.into_parts();
    reap_capture_exact_runtimes(
        CaptureSessionAuthority::new(source.epoch.session_generation),
        &mut worker.exact_runtimes,
        &settings.exact,
    );
    assert!(!settings.exact.owns_source(&owned_source_id));
    assert_eq!(worker.exact_runtimes.iter().count(), 0);
    for retirement in [retained_retirement, mixed_retirement, retirement] {
        retirement
            .try_reclaim()
            .expect("retired exact resources reclaim after runtime ownership is reaped");
    }
}

#[test]
fn already_cancelled_exact_preparation_never_creates_runtime_state() {
    let settings = settings(74);
    let mut worker = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(74, PhysicalOrigin::default(), extent(4, 2)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    drop(capture_raw(&mut worker, 4, 2, 23));
    let source = settings
        .exact
        .source()
        .expect("captured frame resolves the publication source");
    let mut builder = ScreenPlanBuilder::new();
    let revision = InputPublicationDemandRevision::new(1);
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            [exact_demand(
                &source,
                &settings.exact,
                ScreenPublicationKind::Surface,
            )],
            None,
            revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test exact plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch.source_id)
        .expect("test source owns an exact worker ticket");
    let (completion, completed) = tokio::sync::oneshot::channel();

    worker.handle_exact_command(CaptureExactCommand::Prepare {
        authority: CaptureSessionAuthority::new(source.epoch.session_generation),
        ticket,
        cancelled: Arc::new(AtomicBool::new(true)),
        completion,
    });

    let error = completed
        .blocking_recv()
        .expect("cancelled preparation reports completion")
        .expect_err("cancelled preparation cannot return a worker token");
    assert!(error.to_string().contains("was cancelled"));
    assert_eq!(worker.exact_runtimes.iter().count(), 0);
    assert_eq!(settings.exact.owned_source_count(), 0);
}

#[test]
fn stale_frame_cannot_replace_the_resolved_exact_source() {
    let settings = settings(42);
    let mut worker = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(42, PhysicalOrigin::default(), extent(1, 1)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    capture_legacy(&mut worker, 1, 1, 1);
    settings.clear_expected_epoch();

    let mut plane = worker
        .plane_pool
        .try_acquire(4)
        .expect("test plane allocation succeeds");
    plane.resize(4, 2);
    worker
        .capture_frame(
            Instant::now(),
            1,
            1,
            None,
            CaptureRotation::Identity,
            plane.freeze(),
            CaptureColorimetry::SRGB,
        )
        .expect_err("capture without an active epoch is stale");

    assert!(settings.exact.source().is_none());
}

#[test]
fn stale_worker_cannot_overwrite_the_successor_snapshot() {
    let settings = settings(9);
    let latest = Arc::clone(&settings.publication);
    let physical_origin = PhysicalOrigin::default();
    let logical_extent = extent(1920, 1080);
    let mut retiring = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(9, physical_origin, logical_extent),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let stale = capture_legacy(&mut retiring, 4, 2, 1);

    let active_session = settings.begin_session();
    let mut active = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(active_session, physical_origin, logical_extent),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let current = capture_legacy(&mut active, 2, 1, 2);
    assert!(settings.publish_snapshot(current));
    assert!(!settings.publish_snapshot(stale));
    drop(retiring);

    assert_eq!(
        settings
            .exact
            .source()
            .expect("successor exact source remains installed")
            .epoch
            .session_generation,
        active_session
    );

    let published = latest
        .lock()
        .expect("latest snapshot mutex is healthy")
        .snapshot()
        .expect("successor snapshot remains published");
    assert_eq!(
        published
            .value
            .analysis
            .geometry_frame()
            .metadata()
            .session_generation,
        active_session
    );
    assert_eq!(published.revision, 1);
}

#[test]
fn retired_worker_cannot_read_or_update_successor_settings() {
    let settings = settings(31);
    settings.values.lock_config().restore_token = Some("retiring".to_owned());
    let not_cancelled = AtomicBool::new(false);
    let active_session_generation = AtomicU64::new(31);
    assert_eq!(
        settings
            .snapshot_for_session(31, &not_cancelled)
            .expect("current worker may read its settings")
            .config
            .restore_token
            .as_deref(),
        Some("retiring")
    );

    let successor_generation = settings.begin_session();
    settings.values.lock_config().restore_token = Some("successor".to_owned());
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink_calls_for_callback = Arc::clone(&sink_calls);
    let publication_for_callback = Arc::clone(&settings.publication);
    let sink: RestoreTokenSink = Arc::new(move |_| {
        assert!(publication_for_callback.try_lock().is_ok());
        sink_calls_for_callback.fetch_add(1, Ordering::Relaxed);
    });

    assert!(settings.snapshot_for_session(31, &not_cancelled).is_none());
    assert!(!settings.persist_restore_token_for_session(
        31,
        &not_cancelled,
        Some("stale".to_owned()),
        Some(&sink),
    ));
    assert!(
        settings
            .begin_successor_session(31, &not_cancelled, &active_session_generation)
            .is_none()
    );
    assert_eq!(sink_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        settings.values.lock_config().restore_token.as_deref(),
        Some("successor")
    );

    assert!(settings.persist_restore_token_for_session(
        successor_generation,
        &not_cancelled,
        Some("current".to_owned()),
        Some(&sink),
    ));
    assert_eq!(sink_calls.load(Ordering::Relaxed), 1);

    let cancelled = AtomicBool::new(true);
    assert!(
        settings
            .snapshot_for_session(successor_generation, &cancelled)
            .is_none()
    );
    assert!(!settings.persist_restore_token_for_session(
        successor_generation,
        &cancelled,
        Some("cancelled".to_owned()),
        Some(&sink),
    ));
    assert_eq!(sink_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        settings.values.lock_config().restore_token.as_deref(),
        Some("current")
    );
}

#[test]
fn current_worker_can_advance_its_session_exactly_once() {
    let settings = settings(41);
    let not_cancelled = AtomicBool::new(false);
    let active_session_generation = AtomicU64::new(41);

    assert_eq!(
        settings.begin_successor_session(41, &not_cancelled, &active_session_generation),
        Some(42)
    );
    assert!(
        settings
            .begin_successor_session(41, &not_cancelled, &active_session_generation)
            .is_none()
    );
    assert!(settings.session_is_current(42, &not_cancelled));
    assert_eq!(active_session_generation.load(Ordering::Acquire), 42);
}

fn assert_stale_status_publication_is_rejected(
    publish: impl FnOnce(&crate::input::SourceSessionWriter) -> bool + Send + 'static,
) {
    let settings = settings(51);
    let mut reporter = SourceStatusReporter::new(
        "wayland_screen_capture",
        SourceKind::Screen,
        "pipewire",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(1);
    let writer = reporter
        .begin_session()
        .expect("status session starts")
        .expect("manager-bound reporter yields a writer");
    let status = reporter.handle();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_settings = Arc::clone(&settings);
    let worker_cancel = Arc::clone(&cancel);
    let (checked_tx, checked_rx) = mpsc::sync_channel(0);
    let (resume_tx, resume_rx) = mpsc::sync_channel(0);
    let retired = thread::spawn(move || {
        assert!(worker_settings.session_is_current(51, &worker_cancel));
        checked_tx
            .send(())
            .expect("test observes the retired worker pre-check");
        resume_rx
            .recv()
            .expect("test releases the retired worker after activation");
        worker_settings.publish_status_for_session(51, &worker_cancel, &writer, publish)
    });

    checked_rx
        .recv()
        .expect("retired worker reaches the vulnerable interleaving");
    assert_eq!(settings.begin_session(), 52);
    resume_tx
        .send(())
        .expect("retired worker resumes after successor activation");
    assert!(!retired.join().expect("retired worker exits cleanly"));

    let snapshot = status.snapshot();
    assert_eq!(snapshot.state, SourceState::Starting);
    assert!(snapshot.issue.is_none());
}

#[test]
fn stale_unavailable_is_rejected_after_successor_activation() {
    assert_stale_status_publication_is_rejected(|status| {
        status.unavailable(SourceIssue::new(
            "stale_wayland_unavailable",
            "retired worker",
            true,
        ))
    });
}

#[test]
fn stale_degraded_is_rejected_after_successor_activation() {
    assert_stale_status_publication_is_rejected(|status| {
        status.degraded(SourceIssue::new(
            "stale_wayland_degraded",
            "retired worker",
            true,
        ))
    });
}

#[test]
fn cancellation_after_status_validation_is_serialized_with_publication() {
    let settings = settings(61);
    let mut reporter = SourceStatusReporter::new(
        "wayland_screen_capture",
        SourceKind::Screen,
        "pipewire",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(1);
    let writer = reporter
        .begin_session()
        .expect("status session starts")
        .expect("manager-bound reporter yields a writer");
    let status = reporter.handle();
    let cancel = Arc::new(AtomicBool::new(false));
    let active_session_generation = Arc::new(AtomicU64::new(61));
    let publisher_settings = Arc::clone(&settings);
    let publisher_cancel = Arc::clone(&cancel);
    let (validated_tx, validated_rx) = mpsc::sync_channel(0);
    let (publish_tx, publish_rx) = mpsc::sync_channel(0);
    let publisher = thread::spawn(move || {
        publisher_settings.publish_status_for_session(61, &publisher_cancel, &writer, |status| {
            validated_tx
                .send(())
                .expect("test observes validation under the session gate");
            publish_rx
                .recv()
                .expect("test releases publication after cancellation starts");
            status.degraded(SourceIssue::new(
                "pre_cancel_publication",
                "linearized before cancellation",
                true,
            ))
        })
    });

    validated_rx
        .recv()
        .expect("publisher reaches its internal validation point");
    assert!(settings.session_guard.try_lock().is_err());
    assert!(settings.publication.try_lock().is_ok());
    let cancellation_settings = Arc::clone(&settings);
    let cancellation_flag = Arc::clone(&cancel);
    let cancellation_generation = Arc::clone(&active_session_generation);
    let (cancel_started_tx, cancel_started_rx) = mpsc::sync_channel(0);
    let cancellation = thread::spawn(move || {
        cancel_started_tx
            .send(())
            .expect("test observes cancellation invocation");
        cancellation_settings.cancel_worker_session(&cancellation_flag, &cancellation_generation);
    });
    cancel_started_rx
        .recv()
        .expect("cancellation starts after publication validation");
    assert!(!cancel.load(Ordering::Acquire));
    publish_tx
        .send(())
        .expect("validated publication may finish before cancellation linearizes");
    assert!(publisher.join().expect("publisher exits cleanly"));
    cancellation.join().expect("cancellation exits cleanly");
    assert!(cancel.load(Ordering::Acquire));

    let current_writer = reporter
        .session()
        .expect("reporter retains the shared worker status session");
    assert!(
        !settings.publish_status_for_session(61, &cancel, &current_writer, |status| {
            status.unavailable(SourceIssue::new(
                "post_cancel_publication",
                "must be rejected",
                true,
            ))
        })
    );
    let snapshot = status.snapshot();
    assert_eq!(snapshot.state, SourceState::Degraded);
    assert_eq!(
        snapshot.issue.as_ref().map(|issue| issue.code.as_ref()),
        Some("pre_cancel_publication")
    );
}

#[test]
fn unexpected_exit_after_reconnect_uses_final_session_generation() {
    let settings = settings(71);
    let cancel = AtomicBool::new(false);
    let active_session_generation = AtomicU64::new(71);
    let mut reporter = SourceStatusReporter::new(
        "wayland_screen_capture",
        SourceKind::Screen,
        "pipewire",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(1);
    let writer = reporter
        .begin_session()
        .expect("status session starts")
        .expect("manager-bound reporter yields a writer");
    let status = reporter.handle();

    assert_eq!(
        settings.begin_successor_session(71, &cancel, &active_session_generation,),
        Some(72)
    );
    assert!(publish_unexpected_exit_status(
        &settings,
        &active_session_generation,
        &cancel,
        &writer,
        "worker exited after reconnect".to_owned(),
    ));

    let snapshot = status.snapshot();
    assert_eq!(active_session_generation.load(Ordering::Acquire), 72);
    assert_eq!(snapshot.state, SourceState::Degraded);
    assert_eq!(
        snapshot.issue.as_ref().map(|issue| issue.code.as_ref()),
        Some("wayland_screen_worker_exited")
    );
}

#[test]
fn decode_honors_chunk_offset_size_and_row_padding() {
    let mapped = [
        90, 91, 1, 2, 3, 4, 5, 6, 7, 8, 70, 71, 9, 10, 11, 12, 13, 14, 15, 16, 72, 73,
    ];
    let mut buffers = DoubleBuffer::try_with_capacity(16).expect("test callback planes allocate");
    let stats = decode_chunk(&rgba_view(&mapped, 2, 18, 10, 2, 2), &mut buffers);

    assert_eq!(stats.bytes_copied(), 16);
    assert_eq!(stats.rows_copied(), 2);
    assert_eq!(stats.drop_reason(), None);
    assert_eq!(
        buffers.latest_bytes(),
        Some(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16][..])
    );
}

#[test]
fn callback_planes_are_admitted_before_allocation_and_retained_by_inflight_frames() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(32, 32));
    let mut buffers = DoubleBuffer::try_with_capacity_and_admission(16, &coordinator)
        .expect("two callback planes fit the shared fence");
    let mapped = [1_u8; 16];

    assert_eq!(coordinator.snapshot().reserved_bytes(), 32);
    assert_eq!(
        decode_chunk(&rgba_view(&mapped, 0, 16, 16, 4, 1), &mut buffers).drop_reason(),
        None
    );
    let completed = buffers
        .take_completed()
        .expect("decoded frame owns one admitted callback plane");
    drop(buffers);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 32);
    drop(completed);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn callback_plane_capacity_rejects_before_allocation() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(31, 31));

    assert!(matches!(
        DoubleBuffer::try_with_capacity_and_admission(16, &coordinator),
        Err(CaptureFrameError::PlaneCapacityExceeded {
            requested_bytes: 32,
            available_bytes: 31,
        })
    ));
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn decode_normalizes_negative_stride_without_touching_pixels() {
    let mapped = [9, 10, 11, 12, 13, 14, 15, 16, 1, 2, 3, 4, 5, 6, 7, 8];
    let mut buffers =
        DoubleBuffer::try_with_capacity(mapped.len()).expect("test callback planes allocate");
    let stats = decode_chunk(&rgba_view(&mapped, 0, mapped.len(), -8, 2, 2), &mut buffers);

    assert_eq!(stats.drop_reason(), None);
    assert_eq!(
        buffers.latest_bytes(),
        Some(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16][..])
    );
}

#[test]
fn negotiated_format_conversion_runs_only_after_callback_copy() {
    let mapped = [30, 20, 10, 40, 70, 60, 50, 80];
    let view = SpaChunkView::new(
        &mapped,
        0,
        mapped.len(),
        8,
        2,
        1,
        SpaVideoFormat::Bgra,
        None,
        CaptureRotation::Identity,
    );
    let mut buffers =
        DoubleBuffer::try_with_capacity(mapped.len()).expect("test callback planes allocate");
    assert_eq!(decode_chunk(&view, &mut buffers).drop_reason(), None);
    assert_eq!(buffers.latest_bytes(), Some(&mapped[..]));

    let decoded = buffers
        .take_completed()
        .expect("successful callback copy retains one plane");
    let mut rgba = [0_u8; 8];
    convert_packed_to_rgba(&decoded, &mut rgba);
    assert_eq!(rgba, [10, 20, 30, 40, 50, 60, 70, 80]);
}

#[test]
fn decode_rejects_truncated_and_malformed_chunks() {
    let mapped = [0_u8; 32];
    let mut buffers =
        DoubleBuffer::try_with_capacity(mapped.len()).expect("test callback planes allocate");
    let cases = [
        (
            rgba_view(&mapped, 4, 15, 8, 2, 2),
            ChunkDropReason::TruncatedChunk,
        ),
        (
            rgba_view(&mapped, 24, 16, 8, 2, 2),
            ChunkDropReason::InvalidChunkBounds,
        ),
        (
            rgba_view(&mapped, 0, 16, 7, 2, 2),
            ChunkDropReason::InvalidStride,
        ),
        (
            rgba_view(&mapped, 0, 0, 0, 2, 2),
            ChunkDropReason::InvalidChunkBounds,
        ),
    ];

    for (view, reason) in cases {
        assert_eq!(
            decode_chunk(&view, &mut buffers).drop_reason(),
            Some(reason)
        );
    }
    assert!(buffers.latest_bytes().is_none());
}

#[test]
fn decode_retains_valid_crop_and_transform_metadata() {
    let mapped = [0_u8; 64];
    let crop = PixelRect::new(1, 0, 2, 2).expect("test crop is valid");
    let view = SpaChunkView::new(
        &mapped,
        0,
        mapped.len(),
        16,
        4,
        4,
        SpaVideoFormat::Bgra,
        Some(crop),
        CaptureRotation::Clockwise90,
    );
    let mut buffers =
        DoubleBuffer::try_with_capacity(mapped.len()).expect("test callback planes allocate");

    assert_eq!(decode_chunk(&view, &mut buffers).drop_reason(), None);
    assert_eq!(buffers.latest_crop(), Some(crop));
    assert_eq!(
        buffers.latest_transform(),
        Some(CaptureRotation::Clockwise90)
    );
    assert_eq!(buffers.latest_format(), Some(SpaVideoFormat::Bgra));

    let invalid_crop = PixelRect::new(3, 3, 2, 2).expect("test crop is non-empty");
    let invalid = SpaChunkView::new(
        &mapped,
        0,
        mapped.len(),
        16,
        4,
        4,
        SpaVideoFormat::Bgra,
        Some(invalid_crop),
        CaptureRotation::Identity,
    );
    assert_eq!(
        decode_chunk(&invalid, &mut buffers).drop_reason(),
        Some(ChunkDropReason::InvalidCrop)
    );
}

#[test]
fn analysis_envelope_reports_pending_crop_and_transform() {
    let settings = settings(13);
    let mut analysis = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(13, PhysicalOrigin { x: -100, y: 50 }, extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let crop = PixelRect::new(1, 0, 2, 2).expect("test crop is valid");
    let mut plane = analysis
        .plane_pool
        .try_acquire(64)
        .expect("test plane allocation succeeds");
    plane.resize(64, 0);
    let frame = analysis
        .capture_frame(
            Instant::now(),
            4,
            4,
            Some(crop),
            CaptureRotation::Clockwise270,
            plane.freeze(),
            CaptureColorimetry::unknown(),
        )
        .expect("raw frame accepts pending geometry metadata");

    assert_eq!(frame.metadata().geometry.crop(), Some(crop));
    assert_eq!(
        frame.metadata().geometry.rotation(),
        CaptureRotation::Clockwise270
    );
}

#[test]
fn legacy_analysis_rejects_unknown_wayland_samples_before_averaging() {
    let settings = settings(17);
    let mut analysis = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(17, PhysicalOrigin::default(), extent(4, 2)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let mut plane = analysis
        .plane_pool
        .try_acquire(32)
        .expect("test plane allocation succeeds");
    plane.resize(32, 0);
    let frame = analysis
        .capture_frame(
            Instant::now(),
            4,
            2,
            None,
            CaptureRotation::Identity,
            plane.freeze(),
            CaptureColorimetry::unknown(),
        )
        .expect("raw frame accepts unknown backend metadata");

    let error = analyze_screen_frame(&mut analysis.analyzer, frame)
        .err()
        .expect("unknown encoded samples must not reach legacy averaging");
    assert!(matches!(
        error.downcast_ref::<CaptureFrameError>(),
        Some(CaptureFrameError::UnsupportedLegacyAnalysisColorimetry { colorimetry })
            if *colorimetry == CaptureColorimetry::unknown()
    ));
}

#[test]
fn negotiated_format_change_rebuilds_callback_planes_outside_decode() {
    let exchange = Arc::new(AnalysisExchange::default());
    let metrics = Arc::new(CaptureCallbackMetrics::default());
    let mut callback = WaylandCaptureUserData::new(exchange, Arc::clone(&metrics));
    callback
        .set_negotiated_format(NegotiatedFormat {
            width: 2,
            height: 2,
            format: SpaVideoFormat::Rgb,
        })
        .expect("test RGB callback planes allocate");
    let rgb = [1_u8; 12];
    let rgb_view = SpaChunkView::new(
        &rgb,
        0,
        rgb.len(),
        6,
        2,
        2,
        SpaVideoFormat::Rgb,
        None,
        CaptureRotation::Identity,
    );
    let first = decode_chunk(&rgb_view, &mut callback.buffers);
    callback.metrics.record(first);
    assert_eq!(first.drop_reason(), None);

    callback
        .set_negotiated_format(NegotiatedFormat {
            width: 4,
            height: 2,
            format: SpaVideoFormat::Rgba,
        })
        .expect("test RGBA callback planes allocate");
    let rgba = [2_u8; 32];
    let second = decode_chunk(
        &rgba_view(&rgba, 0, rgba.len(), 16, 4, 2),
        &mut callback.buffers,
    );
    callback.metrics.record(second);
    assert_eq!(second.drop_reason(), None);
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.copied_frames, 2);
    assert_eq!(snapshot.copied_bytes, 44);
}

#[test]
fn initial_format_rejection_becomes_typed_unavailable() {
    let state = PipeWireFormatState {
        current: format_request(640, 480, 30),
        current_offer: format_offer(format_request(640, 480, 30)),
        current_acknowledged: false,
        pending: None,
        restoring: None,
    };

    assert_eq!(
        state.acknowledgment(negotiated_format(1280, 720, 30)),
        PipeWireFormatAcknowledgment::Rejected
    );
    assert!(!state.can_begin_adoption());
    assert!(matches!(
        unavailable_format_outcome(false, "rejected fixture".to_owned()),
        PipeWireLoopExit::Unavailable(reason) if reason.contains("initial exact screen format")
    ));
}

#[test]
fn delayed_current_ack_does_not_consume_pending_format_adoption() {
    let current = format_request(640, 480, 30);
    let requested = format_request(3840, 2160, 144);
    let state = PipeWireFormatState {
        current,
        current_offer: format_offer(current),
        current_acknowledged: true,
        pending: Some(pending_adoption(7, requested)),
        restoring: None,
    };

    assert_eq!(
        state.acknowledgment(negotiated_format(640, 480, 30)),
        PipeWireFormatAcknowledgment::Current
    );
    assert_eq!(state.pending.as_ref().map(|pending| pending.id), Some(7));
    assert_eq!(
        state.acknowledgment(negotiated_format(3840, 2160, 144)),
        PipeWireFormatAcknowledgment::Pending
    );
}

#[test]
fn rejected_adoption_settles_only_after_prior_format_ack() {
    let current = format_request(640, 480, 30);
    let requested = format_request(1920, 1080, 60);
    let (pending, done_rx) = pending_adoption_with_done(11, requested);
    let mut state = PipeWireFormatState {
        current,
        current_offer: format_offer(current),
        current_acknowledged: true,
        pending: Some(pending),
        restoring: None,
    };

    assert_eq!(
        state.acknowledgment(negotiated_format(1280, 720, 60)),
        PipeWireFormatAcknowledgment::Rejected
    );
    assert_eq!(
        state.acknowledgment(negotiated_format(1920, 1088, 60)),
        PipeWireFormatAcknowledgment::Rejected
    );
    assert_eq!(
        state.acknowledgment(negotiated_format(1920, 1080, 59)),
        PipeWireFormatAcknowledgment::Pending,
        "transport rate differences are advisory, not rejections"
    );
    assert!(state.cancel(10).is_none());
    assert_eq!(state.pending.as_ref().map(|pending| pending.id), Some(11));

    let rejected = state.cancel(11).expect("matching epoch owns adoption");
    assert!(rejected.authority.cancel());
    assert_eq!(
        state.begin_restoring(rejected, "fixture rejection".to_owned()),
        format_offer(current)
    );
    assert!(!state.can_begin_adoption());
    assert_eq!(state.restoring_id(), Some(11));
    assert_eq!(
        state.acknowledgment(negotiated_format(1920, 1080, 60)),
        PipeWireFormatAcknowledgment::Restoring
    );
    assert_eq!(
        state.acknowledgment(negotiated_format(1280, 720, 60)),
        PipeWireFormatAcknowledgment::Restoring
    );
    assert!(state.cancel(12).is_none());
    assert_eq!(state.restoring_id(), Some(11));
    assert_eq!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert_eq!(
        state.acknowledgment(negotiated_format(640, 480, 30)),
        PipeWireFormatAcknowledgment::Restored
    );

    let state = Mutex::new(state);
    let mut callback = WaylandCaptureUserData::new(
        Arc::new(AnalysisExchange::default()),
        Arc::new(CaptureCallbackMetrics::default()),
    );
    settle_pipewire_restoration(
        &mut callback,
        &state,
        NegotiatedFormat::from_native(negotiated_format(640, 480, 30)),
    )
    .expect("authoritative prior format settles restoration");

    assert_eq!(
        done_rx
            .recv_timeout(Duration::ZERO)
            .expect("restoration releases the failed caller"),
        Err("fixture rejection".to_owned())
    );
    assert!(
        state
            .lock()
            .expect("format state mutex is healthy")
            .can_begin_adoption()
    );
}

#[test]
fn timed_out_adoption_cannot_consume_its_late_exact_ack() {
    let requested = format_request(1920, 1080, 60);
    let pending = pending_adoption(13, requested);
    assert!(pending.authority.cancel());
    let state = PipeWireFormatState {
        current: format_request(640, 480, 30),
        current_offer: format_offer(format_request(640, 480, 30)),
        current_acknowledged: true,
        pending: Some(pending),
        restoring: None,
    };

    assert_eq!(
        state.acknowledgment(negotiated_format(1920, 1080, 60)),
        PipeWireFormatAcknowledgment::Cancelled
    );
    assert_eq!(
        state.acknowledgment(negotiated_format(640, 480, 30)),
        PipeWireFormatAcknowledgment::CancelledCurrent
    );
}

#[test]
fn adoption_cancellation_has_a_bounded_settling_window() {
    let (_done_tx, done_rx) = mpsc::sync_channel(1);
    let cancelled = AtomicBool::new(false);
    let authority = AdoptionAuthority::default();

    let result =
        wait_for_adoption_result(&done_rx, Duration::ZERO, Duration::ZERO, &authority, || {
            cancelled.store(true, Ordering::Release);
        });

    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(
        result,
        Err(AdoptionWaitError::CancellationUnsettled(
            mpsc::RecvTimeoutError::Timeout
        ))
    );
}

#[test]
fn unavailable_parking_and_rearm_are_linearized_in_both_orders() {
    let rearm_first = Arc::new(AtomicU64::new(initial_worker_demand(true)));
    let session_epoch = worker_demand_epoch(rearm_first.load(Ordering::Acquire));
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let worker_state = Arc::clone(&rearm_first);
    let worker = thread::spawn(move || {
        release_rx.recv().expect("test releases unavailable park");
        park_unavailable_worker(&worker_state, session_epoch)
    });

    assert!(!request_active_worker_demand(&rearm_first));
    release_tx.send(()).expect("worker may attempt to park");
    assert_eq!(
        worker.join().expect("worker settles rearm-first race"),
        UnavailablePark::Rearmed
    );
    assert!(worker_demanded(&rearm_first));

    let park_first = Arc::new(AtomicU64::new(initial_worker_demand(true)));
    let session_epoch = worker_demand_epoch(park_first.load(Ordering::Acquire));
    let worker_state = Arc::clone(&park_first);
    let (parked_tx, parked_rx) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        let outcome = park_unavailable_worker(&worker_state, session_epoch);
        parked_tx
            .send(outcome)
            .expect("test observes unavailable park");
    });

    assert_eq!(
        parked_rx.recv().expect("worker parks before rearm"),
        UnavailablePark::Parked
    );
    assert!(request_active_worker_demand(&park_first));
    worker.join().expect("worker settles park-first race");
    assert!(worker_demanded(&park_first));
}

#[test]
fn timeout_cancels_delayed_open_adoption_before_worker_mutation() {
    let authority = Arc::new(AdoptionAuthority::default());
    let mutations = Arc::new(AtomicUsize::new(0));
    let (armed_tx, armed_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker_authority = Arc::clone(&authority);
    let worker_mutations = Arc::clone(&mutations);
    let worker = thread::spawn(move || {
        armed_tx.send(()).expect("worker reaches commit boundary");
        release_rx.recv().expect("timeout releases delayed worker");
        let committed = commit_if_authorized(&worker_authority, true, || {
            worker_mutations.store(1, Ordering::Release);
        });
        done_tx
            .send(if committed {
                Ok(())
            } else {
                Err("cancelled".to_owned())
            })
            .expect("worker settles timed out adoption");
    });

    armed_rx.recv().expect("test observes delayed open worker");
    let result = wait_for_adoption_result(
        &done_rx,
        Duration::ZERO,
        Duration::from_secs(1),
        &authority,
        || release_tx.send(()).expect("cancellation releases worker"),
    );

    assert_eq!(result, Ok(Err("cancelled".to_owned())));
    worker.join().expect("cancelled worker exits");
    assert_eq!(mutations.load(Ordering::Acquire), 0);
}

#[test]
fn timeout_waits_for_claimed_format_transaction_before_success() {
    let authority = Arc::new(AdoptionAuthority::default());
    let mutations = Arc::new(AtomicUsize::new(0));
    let cancellation_called = Arc::new(AtomicBool::new(false));
    let (analysis_tx, analysis_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker_authority = Arc::clone(&authority);
    let worker_mutations = Arc::clone(&mutations);
    let worker = thread::spawn(move || {
        assert!(worker_authority.claim_commit());
        worker_mutations.store(0b01, Ordering::Release);
        worker_authority.complete_analysis();
        analysis_tx
            .send(())
            .expect("test observes applied analysis stage");
        release_rx.recv().expect("test releases format install");
        worker_mutations.store(0b11, Ordering::Release);
        worker_authority.complete_commit();
        let _ = done_tx.send(Ok(()));
    });

    analysis_rx
        .recv()
        .expect("analysis stage claims commit authority");
    let (wait_started_tx, wait_started_rx) = mpsc::sync_channel(0);
    let (result_tx, result_rx) = mpsc::sync_channel(0);
    let wait_authority = Arc::clone(&authority);
    let wait_cancellation = Arc::clone(&cancellation_called);
    let waiter = thread::spawn(move || {
        wait_started_tx
            .send(())
            .expect("waiter reaches timeout path");
        let result = wait_for_adoption_result(
            &done_rx,
            Duration::ZERO,
            Duration::ZERO,
            &wait_authority,
            || wait_cancellation.store(true, Ordering::Release),
        );
        result_tx.send(result).expect("waiter reports settlement");
    });

    wait_started_rx.recv().expect("timeout waiter starts");
    assert_eq!(result_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert_eq!(mutations.load(Ordering::Acquire), 0b01);
    release_tx.send(()).expect("format install may complete");
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiter observes complete format transaction"),
        Ok(Ok(()))
    );
    waiter.join().expect("timeout waiter exits");
    worker.join().expect("format transaction exits");
    assert!(!cancellation_called.load(Ordering::Acquire));
    assert_eq!(mutations.load(Ordering::Acquire), 0b11);
}

#[test]
fn cancellation_winner_blocks_every_late_adoption_mutation() {
    let authority = Arc::new(AdoptionAuthority::default());
    let mutations = Arc::new(AtomicUsize::new(0));
    let (armed_tx, armed_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let worker_authority = Arc::clone(&authority);
    let worker_mutations = Arc::clone(&mutations);
    let worker = thread::spawn(move || {
        armed_tx
            .send(())
            .expect("worker reaches the armed commit boundary");
        release_rx.recv().expect("test releases the retired worker");
        commit_if_authorized(&worker_authority, true, || {
            worker_mutations.store(0b1_1111, Ordering::Release);
        })
    });

    armed_rx
        .recv()
        .expect("caller observes the old worker after its final check");
    assert!(authority.cancel());
    release_tx
        .send(())
        .expect("cancelled worker resumes after retirement");

    assert!(!worker.join().expect("cancelled worker exits cleanly"));
    assert_eq!(mutations.load(Ordering::Acquire), 0);
    assert!(!authority.committed());
}

#[test]
fn commit_winner_completes_install_before_signalling_success() {
    let authority = Arc::new(AdoptionAuthority::default());
    let mutations = Arc::new(AtomicUsize::new(0));
    let (claimed_tx, claimed_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let (done_tx, done_rx) = mpsc::sync_channel(0);
    let worker_authority = Arc::clone(&authority);
    let worker_mutations = Arc::clone(&mutations);
    let worker = thread::spawn(move || {
        assert!(worker_authority.claim_commit());
        worker_mutations.store(0b0_0111, Ordering::Release);
        claimed_tx
            .send(())
            .expect("analysis commit exposes its claimed authority");
        release_rx
            .recv()
            .expect("test releases the capture install");
        worker_mutations.store(0b1_1111, Ordering::Release);
        worker_authority.complete_commit();
        done_tx
            .send(())
            .expect("completed commit signals its caller");
    });

    claimed_rx
        .recv()
        .expect("caller observes commit authority ownership");
    assert!(!authority.cancel());
    assert_eq!(mutations.load(Ordering::Acquire), 0b0_0111);
    assert_eq!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    release_tx.send(()).expect("capture install may complete");
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("commit completion is coherently signalled");
    worker.join().expect("commit winner exits cleanly");

    assert!(authority.committed());
    assert_eq!(mutations.load(Ordering::Acquire), 0b1_1111);
}

#[test]
fn initial_extent_correction_only_fires_before_first_acknowledgment() {
    let make_state = |acknowledged: bool| {
        std::sync::Mutex::new(PipeWireFormatState {
            current: format_request(2560, 1440, 30),
            current_offer: format_offer(format_request(2560, 1440, 30)),
            current_acknowledged: acknowledged,
            pending: None,
            restoring: None,
        })
    };

    let corrected =
        initial_native_extent_correction(&make_state(false), negotiated_format(3840, 2160, 0))
            .expect("scaled-output fixation corrects the acquisition extent");
    assert_eq!((corrected.width(), corrected.height()), (3840, 2160));

    assert!(
        initial_native_extent_correction(&make_state(false), negotiated_format(2560, 1440, 0))
            .is_none(),
        "matching extent needs no correction"
    );
    assert!(
        initial_native_extent_correction(&make_state(true), negotiated_format(3840, 2160, 0))
            .is_none(),
        "post-acknowledgment renegotiation keeps the strict rejection path"
    );

    let pending_state = make_state(false);
    pending_state
        .lock()
        .expect("fresh mutex")
        .pending
        .replace(pending_adoption(7, format_request(1920, 1080, 30)));
    assert!(
        initial_native_extent_correction(&pending_state, negotiated_format(3840, 2160, 0))
            .is_none(),
        "in-flight adoption keeps the transactional path"
    );
}

#[test]
fn format_acknowledgment_treats_transport_rate_as_advisory() {
    let requested = extent(2560, 1440);
    let config = CaptureConfig {
        target_fps: 30,
        ..CaptureConfig::default()
    };
    let request = PipeWireFormatRequest::new_with_compute_capacity(
        requested,
        extent(640, 480),
        &config,
        unlimited_compute_capacity(),
    )
    .expect("format request builds");

    assert!(
        request.matches(negotiated_format(2560, 1440, 30)),
        "exact target rate acknowledges"
    );
    assert!(
        request.matches(negotiated_format(2560, 1440, 0)),
        "variable-rate transport acknowledges"
    );
    assert!(
        request.matches(negotiated_format(2560, 1440, 60)),
        "display-pinned transport acknowledges"
    );
    let malformed = NegotiatedVideoFormat {
        width: 2560,
        height: 1440,
        format: SpaVideoFormat::Rgba,
        framerate: hypercolor_pipewire_interop::VideoFraction {
            numerator: 30,
            denominator: 0,
        },
    };
    assert!(!request.matches(malformed), "zero-denominator rate rejects");
    assert!(
        !request.matches(negotiated_format(1920, 1080, 30)),
        "extent stays exact"
    );
}

#[test]
fn pipewire_format_uses_the_shared_scheduler_boundary_without_a_product_cap() {
    let requested = extent(7680, 4320);
    let config = CaptureConfig {
        target_fps: MAX_REPRESENTABLE_CAPTURE_FPS,
        ..CaptureConfig::default()
    };
    let request = PipeWireFormatRequest::new_with_compute_capacity(
        requested,
        extent(640, 480),
        &config,
        unlimited_compute_capacity(),
    )
    .expect("scheduler boundary cadence is admitted");

    assert_eq!(request.extent, requested);
    assert_eq!(request.target_fps, MAX_REPRESENTABLE_CAPTURE_FPS);
    let rejected = CaptureConfig {
        target_fps: MAX_REPRESENTABLE_CAPTURE_FPS + 1,
        ..CaptureConfig::default()
    };
    assert!(
        PipeWireFormatRequest::new_with_compute_capacity(
            requested,
            extent(640, 480),
            &rejected,
            unlimited_compute_capacity(),
        )
        .is_err()
    );
}

#[test]
fn inactive_wayland_reconfigure_rejects_invalid_cadence_transactionally() {
    let mut input = WaylandScreenCaptureInput::new(CaptureConfig::default());
    let baseline = input.settings.config_snapshot();
    let rejected = CaptureConfig {
        target_fps: MAX_REPRESENTABLE_CAPTURE_FPS + 1,
        ..baseline.clone()
    };

    let error = input
        .reconfigure(rejected)
        .expect_err("inactive Wayland capture must reject invalid cadence");

    assert!(error.to_string().contains("scheduler clock limit"));
    assert_eq!(input.settings.config_snapshot(), baseline);
}

#[test]
fn wayland_analysis_schedule_never_catches_up_in_a_burst() {
    let settings = settings(17);
    let mut state = WaylandAnalysisState::new(
        settings,
        source(17, PhysicalOrigin::default(), extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis state is admitted");
    let initial_deadline = state.next_analysis_at;

    state
        .advance_deadline(initial_deadline)
        .expect("live scheduler deadline fits Instant");
    assert!(state.next_analysis_at > initial_deadline);

    let late = state.next_analysis_at + Duration::from_secs(1);
    state
        .advance_deadline(late)
        .expect("live scheduler deadline fits Instant");
    assert!(
        state.next_analysis_at > late,
        "lateness must schedule a future interval instead of an immediate burst"
    );
}

#[test]
fn failed_format_negotiation_retains_last_good_metadata_and_planes() {
    let exchange = Arc::new(AnalysisExchange::default());
    let metrics = Arc::new(CaptureCallbackMetrics::default());
    let mut callback = WaylandCaptureUserData::new(exchange, metrics);
    callback
        .set_negotiated_format(NegotiatedFormat {
            width: 2,
            height: 2,
            format: SpaVideoFormat::Rgba,
        })
        .expect("test callback planes allocate");
    let rgba = [7_u8; 16];
    assert_eq!(
        decode_chunk(
            &rgba_view(&rgba, 0, rgba.len(), 8, 2, 2),
            &mut callback.buffers,
        )
        .drop_reason(),
        None
    );
    let previous_capacity = callback.buffers.inner.capacity;
    let previous_bytes = callback
        .buffers
        .latest_bytes()
        .expect("last-good callback frame exists")
        .to_vec();

    assert!(matches!(
        callback.set_negotiated_format(NegotiatedFormat {
            width: u32::MAX,
            height: u32::MAX,
            format: SpaVideoFormat::Rgba,
        }),
        Err(crate::input::screen::CaptureFrameError::StorageSizeOverflow)
    ));
    assert_eq!(
        callback.negotiated,
        Some(NegotiatedFormat {
            width: 2,
            height: 2,
            format: SpaVideoFormat::Rgba,
        })
    );
    assert_eq!(callback.buffers.inner.capacity, previous_capacity);
    assert_eq!(
        callback.buffers.latest_bytes(),
        Some(previous_bytes.as_slice())
    );
}

#[test]
fn stopped_analysis_wait_wakes_and_discards_queued_pixels() {
    let mapped = [7_u8; 16];
    let mut buffers =
        DoubleBuffer::try_with_capacity(mapped.len()).expect("test callback planes allocate");
    assert_eq!(
        decode_chunk(&rgba_view(&mapped, 0, mapped.len(), 8, 2, 2), &mut buffers,).drop_reason(),
        None
    );
    let exchange = Arc::new(AnalysisExchange::default());
    exchange.publish(
        buffers
            .take_completed()
            .expect("successful decode owns a completed frame"),
    );
    let waiter = Arc::clone(&exchange);
    let handle = thread::spawn(move || {
        match waiter.wait_for_event(
            Instant::now() + Duration::from_secs(30),
            Instant::now() + Duration::from_secs(30),
            &AtomicBool::new(false),
        ) {
            Some(AnalysisEvent::Frame(frame)) => Some(frame),
            Some(
                AnalysisEvent::Adoption(_) | AnalysisEvent::Exact(_) | AnalysisEvent::Diagnostics,
            )
            | None => None,
        }
    });
    exchange.stop();

    assert!(
        handle
            .join()
            .expect("analysis waiter exits cleanly")
            .is_none()
    );
}

#[test]
fn analysis_exchange_keeps_the_newest_eligible_frame() {
    let exchange = AnalysisExchange::default();
    let mut buffers = DoubleBuffer::try_with_capacity(4).expect("test callback planes allocate");
    for value in [1_u8, 2, 3] {
        let mapped = [value; 4];
        assert_eq!(
            decode_chunk(&rgba_view(&mapped, 0, mapped.len(), 4, 1, 1), &mut buffers,)
                .drop_reason(),
            None
        );
        exchange.publish(
            buffers
                .take_completed()
                .expect("successful decode owns one plane"),
        );
    }

    let Some(AnalysisEvent::Frame(latest)) = exchange.wait_for_event(
        Instant::now(),
        Instant::now() + Duration::from_secs(30),
        &AtomicBool::new(false),
    ) else {
        panic!("latest frame is immediately eligible");
    };
    assert_eq!(latest.bytes(), &[3; 4]);
}

#[test]
fn analysis_exchange_prioritizes_exact_control_over_ready_pixels() {
    let exchange = AnalysisExchange::default();
    let mut buffers = DoubleBuffer::try_with_capacity(4).expect("test callback planes allocate");
    let mapped = [4_u8; 4];
    assert_eq!(
        decode_chunk(&rgba_view(&mapped, 0, mapped.len(), 4, 1, 1), &mut buffers).drop_reason(),
        None
    );
    exchange.publish(
        buffers
            .take_completed()
            .expect("successful decode owns one plane"),
    );
    assert!(
        exchange
            .send_exact(CaptureExactCommand::Reap {
                authority: CaptureSessionAuthority::new(1),
                completion: None,
            })
            .is_ok()
    );

    assert!(matches!(
        exchange.wait_for_event(
            Instant::now(),
            Instant::now() + Duration::from_secs(30),
            &AtomicBool::new(false)
        ),
        Some(AnalysisEvent::Exact(CaptureExactCommand::Reap { .. }))
    ));
    assert!(matches!(
        exchange.wait_for_event(
            Instant::now(),
            Instant::now() + Duration::from_secs(30),
            &AtomicBool::new(false)
        ),
        Some(AnalysisEvent::Frame(_))
    ));
}

#[test]
fn analysis_exchange_wakes_for_diagnostics_without_a_frame() {
    let exchange = AnalysisExchange::default();

    assert!(matches!(
        exchange.wait_for_event(
            Instant::now() + Duration::from_secs(30),
            Instant::now(),
            &AtomicBool::new(false)
        ),
        Some(AnalysisEvent::Diagnostics)
    ));
}

#[test]
fn elapsed_frame_deadline_waits_for_diagnostics_without_spinning() {
    let now = Instant::now();
    let elapsed_frame_deadline = now
        .checked_sub(Duration::from_secs(1))
        .expect("current instant supports a one-second lookback");

    assert_eq!(
        super::analysis_wait_timeout(
            elapsed_frame_deadline,
            now + Duration::from_millis(750),
            now,
        ),
        Duration::from_millis(750)
    );
}

#[test]
fn transitionary_format_fences_decoding_and_discards_queued_pixels() {
    let exchange = Arc::new(AnalysisExchange::default());
    let mut callback = WaylandCaptureUserData::new(
        Arc::clone(&exchange),
        Arc::new(CaptureCallbackMetrics::default()),
    );
    callback
        .activate_negotiated_format(NegotiatedFormat {
            width: 1,
            height: 1,
            format: SpaVideoFormat::Rgba,
        })
        .expect("test callback format activates");
    let mapped = [9_u8; 4];
    assert_eq!(
        decode_chunk(
            &rgba_view(&mapped, 0, mapped.len(), 4, 1, 1),
            &mut callback.buffers,
        )
        .drop_reason(),
        None
    );
    exchange.publish(
        callback
            .buffers
            .take_completed()
            .expect("successful decode owns one plane"),
    );
    assert!(
        exchange
            .state
            .lock()
            .expect("analysis exchange mutex is healthy")
            .latest
            .is_some()
    );

    callback.fence_decoding();

    assert!(!callback.decoding_enabled.load(Ordering::Acquire));
    assert!(callback.negotiated.is_none());
    assert!(
        exchange
            .state
            .lock()
            .expect("analysis exchange mutex is healthy")
            .latest
            .is_none()
    );
}

#[test]
fn successful_descriptor_commit_fences_the_previous_publication() {
    let settings = settings(20);
    let latest = Arc::clone(&settings.publication);
    let mut analysis = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(20, PhysicalOrigin::default(), extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let snapshot = capture_legacy(&mut analysis, 2, 1, 4);
    assert!(settings.publish_snapshot(snapshot));

    fence_previous_publication(
        &mut latest
            .lock()
            .expect("snapshot mutex is healthy before commit"),
    );

    assert!(
        latest
            .lock()
            .expect("snapshot mutex is healthy")
            .snapshot()
            .is_none()
    );
}

#[test]
fn terminal_session_invalidation_clears_only_its_snapshot() {
    let settings = settings(21);
    let latest = Arc::clone(&settings.publication);
    let mut analysis = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(21, PhysicalOrigin::default(), extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let snapshot = capture_legacy(&mut analysis, 2, 1, 4);
    assert!(settings.publish_snapshot(snapshot));
    assert!(settings.invalidate_session(21));
    assert!(
        latest
            .lock()
            .expect("snapshot mutex is healthy")
            .snapshot()
            .is_none()
    );

    let successor_session = settings.begin_session();
    let mut successor = WaylandAnalysisState::new(
        Arc::clone(&settings),
        source(
            successor_session,
            PhysicalOrigin::default(),
            extent(1920, 1080),
        ),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let successor_snapshot = capture_legacy(&mut successor, 2, 1, 5);
    assert!(settings.publish_snapshot(successor_snapshot));
    assert!(!settings.invalidate_session(21));
    assert!(
        latest
            .lock()
            .expect("snapshot mutex is healthy")
            .snapshot()
            .is_some()
    );
}

#[test]
fn callback_metrics_count_copy_and_drop_outcomes() {
    let metrics = CaptureCallbackMetrics::default();
    metrics.record(CopyStats {
        bytes_copied: 16,
        rows_copied: 2,
        drop_reason: None,
    });
    metrics.record(CopyStats::dropped(ChunkDropReason::TruncatedChunk));

    assert_eq!(
        metrics.snapshot(),
        super::CaptureCallbackMetricsSnapshot {
            copied_frames: 1,
            dropped_frames: 1,
            copied_bytes: 16,
            drop_reasons: std::array::from_fn(|index| {
                u64::from(index == ChunkDropReason::TruncatedChunk.index())
            }),
        }
    );
}

#[test]
fn callback_predecode_failures_are_all_counted() {
    let metrics = Arc::new(CaptureCallbackMetrics::default());
    let callback =
        WaylandCaptureUserData::new(Arc::new(AnalysisExchange::default()), Arc::clone(&metrics));
    for reason in [
        ChunkDropReason::MissingBuffer,
        ChunkDropReason::MissingPlane,
        ChunkDropReason::MissingFormat,
        ChunkDropReason::UnmappedPlane,
        ChunkDropReason::InvalidChunkBounds,
    ] {
        callback.record_drop(reason);
    }

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.dropped_frames, 5);
    for reason in [
        ChunkDropReason::MissingBuffer,
        ChunkDropReason::MissingPlane,
        ChunkDropReason::MissingFormat,
        ChunkDropReason::UnmappedPlane,
        ChunkDropReason::InvalidChunkBounds,
    ] {
        assert_eq!(snapshot.drop_reasons[reason.index()], 1);
    }
    assert_eq!(
        snapshot.drop_reasons.iter().copied().sum::<u64>(),
        snapshot.dropped_frames
    );
}

#[test]
fn callback_diagnostics_publish_exact_drop_reasons_to_source_status() {
    let metrics = CaptureCallbackMetrics::default();
    metrics.record(CopyStats::dropped(ChunkDropReason::InvalidCrop));
    metrics.record(CopyStats::dropped(ChunkDropReason::InvalidCrop));
    metrics.record(CopyStats::dropped(ChunkDropReason::InvalidTransform));

    let mut reporter = SourceStatusReporter::new(
        "wayland:diagnostics",
        SourceKind::Screen,
        "wayland_pipewire",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(1);
    let writer = reporter
        .begin_session()
        .expect("diagnostics session should begin")
        .expect("manager generation should create a diagnostics session");
    assert!(writer.publish_status_diagnostics(Some(metrics.snapshot().diagnostics())));

    let status = reporter.handle().snapshot();
    let diagnostics = status
        .diagnostics
        .as_ref()
        .expect("Wayland callback diagnostics should be visible in source status");
    assert_eq!(diagnostics.schema(), "wayland.pipewire.capture");
    assert_eq!(diagnostics.payload()["dropped_frames"], 3);
    assert_eq!(diagnostics.payload()["drop_reasons"]["invalid_crop"], 2);
    assert_eq!(
        diagnostics.payload()["drop_reasons"]["invalid_transform"],
        1
    );
    assert_eq!(diagnostics.payload()["drop_reasons"]["missing_buffer"], 0);
}

#[test]
fn replacement_worker_reapplies_active_demand_state() {
    let demand = active_demand();
    let previous = AtomicU64::new(initial_worker_demand(false));
    set_worker_demand(&previous, demand.is_active());
    assert!(worker_demanded(&previous));

    let replacement = AtomicU64::new(initial_worker_demand(false));
    set_worker_demand(&replacement, demand.is_active());
    assert!(worker_demanded(&replacement));
    assert_eq!(worker_demand_epoch(previous.load(Ordering::Acquire)), 1);
    assert_eq!(worker_demand_epoch(replacement.load(Ordering::Acquire)), 1);
}

#[test]
fn callback_buffer_reuse_preserves_length_without_zero_filling() {
    let mut buffers = DoubleBuffer::try_with_capacity(16).expect("test callback planes allocate");
    {
        let mut available = buffers
            .inner
            .available
            .lock()
            .expect("callback buffer mutex is healthy");
        for plane in &mut *available {
            assert_eq!(plane.len(), 16);
            plane.fill(0xa5);
        }
    }

    let mapped = [1_u8, 2, 3, 4];
    assert_eq!(
        decode_chunk(&rgba_view(&mapped, 0, mapped.len(), 4, 1, 1), &mut buffers).drop_reason(),
        None
    );
    let decoded = buffers
        .take_completed()
        .expect("successful callback copy retains its plane");
    assert_eq!(decoded.bytes(), &mapped);
    let storage = decoded
        .plane
        .buffer
        .as_deref()
        .expect("decoded plane owns its storage");
    assert_eq!(storage.len(), 16);
    assert!(storage[4..].iter().all(|byte| *byte == 0xa5));
    drop(decoded);

    let available = buffers
        .inner
        .available
        .lock()
        .expect("callback buffer mutex is healthy");
    assert!(available.iter().all(|plane| plane.len() == 16));
}

#[test]
fn callback_drops_instead_of_allocating_a_third_plane() {
    let mapped = [4_u8; 4];
    let view = rgba_view(&mapped, 0, mapped.len(), 4, 1, 1);
    let mut buffers =
        DoubleBuffer::try_with_capacity(mapped.len()).expect("test callback planes allocate");
    assert_eq!(decode_chunk(&view, &mut buffers).drop_reason(), None);
    let first = buffers
        .take_completed()
        .expect("first callback plane remains externally owned");
    assert_eq!(decode_chunk(&view, &mut buffers).drop_reason(), None);
    assert_eq!(
        decode_chunk(&view, &mut buffers).drop_reason(),
        Some(ChunkDropReason::BufferUnavailable)
    );
    drop(first);
    assert_eq!(decode_chunk(&view, &mut buffers).drop_reason(), None);
}
