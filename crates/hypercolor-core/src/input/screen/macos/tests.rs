use super::admission::prepare_macos_exact_runtime;
use super::publication::{
    bind_current_macos_exact_runtime, capture_colorimetry, capture_pixel_format,
    legacy_analysis_decimation, macos_native_descriptor_is_identity, native_cpu_capture_frame,
    publish_macos_native_exact, publish_macos_scalar_exact, resolve_macos_publication_branch,
    resolve_macos_publication_branch_with_telemetry,
};
use super::worker::{report_macos_worker_health, synchronize_macos_invalidation_generation};
use super::*;
use crate::input::screen::adapter::CaptureSessionAuthoritySequencer;
use crate::input::screen::{
    CpuReductionLayout, CpuReductionRequest, ExactBoxList, InputPublicationDemandRevision,
    PreparedLedToneMap, ResolvedScreenColorTransform, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBranchDeliveryLifecycle, ScreenBranchPublication, ScreenExtentRequest, ScreenHdrPolicy,
    ScreenInputGraphGeneration, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
    ScreenNativeTargetPreparation, ScreenNativeTargetPreparer, ScreenPayloadKind,
    ScreenPlanBuilder, ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenProfileScalar,
    ScreenPublicationFreshness, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenReductionFilter, ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenToneMapOperator,
    ScreenToneMapPolicy,
};
use hypercolor_macos_capture::{
    MacosAttachment, MacosCaptureColorimetry, MacosCaptureSurface, MacosColorRange,
    MacosDeliveredFrameMetadata, MacosFrameDecoder, MacosPixelExtent, MacosPointRect,
    MacosRawCapturePlane, MacosRawCaptureSample, MacosRawCompleteFrame, MacosRawFrameAttachments,
};

#[test]
fn staged_worker_rollback_does_not_wait_for_native_exit() {
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let start = Arc::new(AtomicBool::new(false));
    let worker_start = Arc::clone(&start);
    let (started_tx, started_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let (exit_tx, exit_rx) = mpsc::channel();
    let (command_tx, _command_rx) = mpsc::channel();
    let join = thread::spawn(move || {
        while !worker_start.load(Ordering::Acquire) && !worker_stop.load(Ordering::Acquire) {
            thread::park();
        }
        started_tx.send(()).expect("test worker reports startup");
        release_rx.recv().expect("test releases native exit");
        let _ = exit_tx.send(Ok(()));
    });
    let reservation = CaptureSessionAuthoritySequencer::default()
        .reserve()
        .expect("test authority reserves");
    let prepared = CaptureSessionTransaction::new(
        CaptureWorker {
            authority: CaptureSessionAuthority::new(1),
            stop: Arc::clone(&stop),
            start: Arc::clone(&start),
            mailbox: MacosFrameMailbox::default(),
            command_tx,
            exit_rx,
            join: Some(join),
        },
        (),
        reservation,
    )
    .prepare(CaptureSessionDeadline::after(Duration::ZERO))
    .expect("test worker stages");
    let staged = StagedCaptureWorker {
        generation: 1,
        prepared,
    };
    let (returned_tx, returned_rx) = mpsc::sync_channel(0);

    thread::spawn(move || {
        drop(staged);
        returned_tx.send(()).expect("rollback reports completion");
    });

    returned_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("rollback returns before native exit");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("rollback releases the staged worker start gate");
    assert!(!start.load(Ordering::Acquire));
    assert!(stop.load(Ordering::Acquire));
    release_tx.send(()).expect("native worker may exit");
}

#[test]
fn rolled_back_worker_generation_is_not_reused() {
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let (input, _fixture) = MacosScreenCaptureFixture::source(CaptureConfig::default(), admission);
    let extent = PixelExtent::new(64, 32).expect("test extent is nonzero");
    let first = input
        .prepare_worker(extent)
        .and_then(|prepared| input.stage_worker(prepared))
        .expect("first worker stages");
    let first_generation = first.generation;
    drop(first);

    let second = input
        .prepare_worker(extent)
        .and_then(|prepared| input.stage_worker(prepared))
        .expect("second worker stages");

    assert_eq!(second.generation, first_generation + 1);
}

#[test]
fn retired_worker_cannot_republish_after_stop_returns() {
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let (mut input, _fixture) =
        MacosScreenCaptureFixture::source(CaptureConfig::default(), admission);
    let authority = CaptureSessionAuthority::new(1);
    input.worker_generation = 1;
    let reservation = input.exact.reserve_authority().expect("authority reserves");
    assert_eq!(reservation.authority(), authority);
    drop(
        input
            .exact
            .activate_reserved_authority(reservation)
            .expect("authority activates"),
    );
    {
        let mut publication = lock(&input.publication);
        publication
            .replace_fence_preserving_latest(MacosPublicationFence(1), 1)
            .expect("test worker owns compatibility publication");
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let (exit_tx, exit_rx) = mpsc::channel();
    let (command_tx, _command_rx) = mpsc::channel();
    let join = thread::spawn(move || {
        release_rx.recv().expect("test releases retired worker");
        let _ = exit_tx.send(Ok(()));
    });
    assert!(
        input
            .sessions
            .install(CaptureWorker {
                authority,
                stop,
                start: Arc::new(AtomicBool::new(true)),
                mailbox: MacosFrameMailbox::default(),
                command_tx,
                exit_rx,
                join: Some(join),
            })
            .is_ok()
    );

    input.stop_worker();

    assert!(worker_stop.load(Ordering::Acquire));
    assert!(!input.exact.is_current_authority(authority));
    assert!(
        !input
            .exact
            .replace_source_if_current(authority, Some(source(&frame())))
    );
    let mut publication = lock(&input.publication);
    assert!(!publication.is_active(&1));
    assert!(publication.publish(&1, Arc::new(InputData::None)).is_err());
    drop(publication);
    release_tx.send(()).expect("retired worker may exit");
}

const BGRA8: u32 = 0x4247_5241;
const ARGB2101010: u32 = 0x6c31_3072;
const RGBA16_FLOAT: u32 = 0x5247_6841;
const YUV420_VIDEO_RANGE: u32 = 0x3432_3076;
const YUV420_FULL_RANGE: u32 = 0x3432_3066;
const YUV44410_FULL_RANGE: u32 = 0x7866_3434;

#[test]
fn runtime_timing_percentiles_are_bounded_by_the_exact_maximum() {
    let timing = AtomicTimingHistogram::default();
    timing.record(Duration::from_nanos(1));
    timing.record(Duration::from_micros(250));

    let snapshot = timing.snapshot();
    assert_eq!(snapshot.sample_count, 2);
    assert_eq!(snapshot.total_ns, 250_001);
    assert_eq!(snapshot.max_ns, 250_000);
    assert_eq!(snapshot.p95_ns, 250_000);
    assert_eq!(snapshot.p99_ns, 250_000);
}

#[test]
fn runtime_timing_snapshot_retries_when_population_changes() {
    let timing = AtomicTimingHistogram::default();
    timing.record(Duration::from_nanos(40));
    let mut injected = false;

    let snapshot = timing.snapshot_with_hooks(
        || {},
        || {
            if !injected {
                timing.record(Duration::from_nanos(70));
                injected = true;
            }
        },
    );

    assert_eq!(snapshot.sample_count, 2);
    assert_eq!(snapshot.total_ns, 110);
    assert_eq!(snapshot.max_ns, 70);
    assert_eq!(snapshot.p95_ns, 70);
    assert_eq!(snapshot.p99_ns, 70);
}

#[test]
fn worker_invalidation_is_atomic_across_branches_and_stale_safe() {
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    let hub = builder.publication_hub();
    exact.install_hub(Arc::clone(&hub));
    let mut runtimes = Vec::new();
    let source = source(&frame());
    exact.install_test_source(Some(source.clone()));
    let surface = resolve_macos_publication_branch(
        &source,
        &cpu_demand_for_kind(
            ScreenProcessingProfile::default(),
            ScreenPublicationKind::Surface,
        ),
    )
    .expect("surface demand resolves")
    .expect("configured source owns surface demand");
    let zones = resolve_macos_publication_branch(
        &source,
        &cpu_demand_for_kind(
            ScreenProcessingProfile::default(),
            ScreenPublicationKind::Zones {
                columns: NonZeroU32::new(2).expect("nonzero columns"),
                rows: NonZeroU32::new(1).expect("nonzero rows"),
            },
        ),
    )
    .expect("zone demand resolves")
    .expect("configured source owns zone demand");
    let descriptors = commit_cpu_runtimes(
        &mut builder,
        &exact,
        &source,
        [surface, zones],
        &mut runtimes,
    );
    let bound_at = Instant::now();
    bind_current_macos_exact_runtime(&mut runtimes, &source, &hub, bound_at)
        .expect("current runtime binds")
        .expect("current runtime exists");
    let captured_at = bound_at + Duration::from_millis(20);
    let first = cpu_capture_frame(&source, 1, captured_at, [32, 64, 96, 255]);
    publish_cpu_frame(&exact, &mut runtimes, &source, &first);
    let leases = descriptors
        .iter()
        .map(|descriptor| hub.lease(descriptor).expect("branch lease remains live"))
        .collect::<Vec<_>>();
    let initial = leases
        .iter()
        .map(|lease| lease.observe(captured_at))
        .collect::<Vec<_>>();
    assert!(initial.iter().all(|(publication, delivery)| {
        publication.is_some()
            && delivery.lifecycle() == ScreenBranchDeliveryLifecycle::Live
            && delivery.freshness() == Some(ScreenPublicationFreshness::Fresh)
            && delivery.source_health() == Some(ScreenPublicationHealth::Healthy)
            && delivery.invalidation_epoch() == 0
    }));

    report_macos_worker_health(&exact, &runtimes, ScreenPublicationHealth::Recovering)
        .expect("recoverable health report succeeds");
    for (lease, (publication, _)) in leases.iter().zip(&initial) {
        let (retained, delivery) = lease.observe(captured_at);
        assert!(
            retained
                .as_ref()
                .zip(publication.as_ref())
                .is_some_and(|(retained, publication)| Arc::ptr_eq(retained, publication))
        );
        assert_eq!(delivery.lifecycle(), ScreenBranchDeliveryLifecycle::Live);
        assert_eq!(
            delivery.source_health(),
            Some(ScreenPublicationHealth::Recovering)
        );
        assert_eq!(delivery.invalidation_epoch(), 0);
    }

    let old_binding = runtimes
        .last()
        .expect("committed runtime exists")
        .binding
        .clone();
    let publisher = hub
        .publisher(&descriptors[0], &old_binding)
        .expect("current worker owns surface publisher");
    let prepared_at = captured_at + Duration::from_millis(10);
    let intent = ScreenPublicationMetadata::try_intent(
        source.epoch.clone(),
        publisher.plan_generation(),
        NonZeroU64::new(2).expect("nonzero sequence"),
        prepared_at,
        prepared_at + Duration::from_secs(1),
    )
    .expect("pre-invalidation intent is valid");
    let prepared = hub
        .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &intent)
        .expect("pre-invalidation publication reserves");
    let worker_publication = Arc::new(Mutex::new(MacosPublication::default()));
    let mut observed_invalidation_generation = 0;
    assert!(
        synchronize_macos_invalidation_generation(
            &mut observed_invalidation_generation,
            1,
            &worker_publication,
            &exact,
            &runtimes,
        )
        .expect("new terminal generation invalidates")
    );
    assert!(matches!(
        hub.finalize_writable_publication(
            prepared,
            prepared_at + Duration::from_millis(1),
            ScreenPublicationHealth::Healthy,
        ),
        Err(ScreenPublicationHubError::PublicationInvalidated)
    ));
    let invalidated = leases
        .iter()
        .map(|lease| lease.observe(captured_at))
        .collect::<Vec<_>>();
    let invalidation_epoch = invalidated[0].1.invalidation_epoch();
    assert_ne!(invalidation_epoch, 0);
    assert!(invalidated.iter().all(|(publication, delivery)| {
        publication.is_none()
            && delivery.lifecycle() == ScreenBranchDeliveryLifecycle::Pending
            && delivery.freshness().is_none()
            && delivery.source_health() == Some(ScreenPublicationHealth::Failed)
            && delivery.invalidation_epoch() == invalidation_epoch
    }));
    assert!(
        synchronize_macos_invalidation_generation(
            &mut observed_invalidation_generation,
            1,
            &worker_publication,
            &exact,
            &runtimes,
        )
        .expect("duplicate terminal generation is accepted without reinvalidation")
    );
    assert!(
        leases.iter().all(|lease| {
            lease.observe(captured_at).1.invalidation_epoch() == invalidation_epoch
        })
    );
    assert!(
        !synchronize_macos_invalidation_generation(
            &mut observed_invalidation_generation,
            0,
            &worker_publication,
            &exact,
            &runtimes,
        )
        .expect("pre-terminal frame generation is rejected")
    );

    let recovered_at = captured_at + Duration::from_millis(20);
    let recovered = cpu_capture_frame(&source, 2, recovered_at, [96, 64, 32, 255]);
    publish_cpu_frame(&exact, &mut runtimes, &source, &recovered);
    assert!(leases.iter().all(|lease| {
        let (publication, delivery) = lease.observe(recovered_at);
        publication.is_some()
            && delivery.lifecycle() == ScreenBranchDeliveryLifecycle::Live
            && delivery.source_health() == Some(ScreenPublicationHealth::Healthy)
            && delivery.invalidation_epoch() == invalidation_epoch
    }));

    let replacement =
        resolve_macos_publication_branch(&source, &cpu_demand(transition_profile(false)))
            .expect("replacement demand resolves")
            .expect("configured source owns replacement demand");
    let replacement_descriptor =
        commit_cpu_runtime(&mut builder, &exact, &source, replacement, &mut runtimes);
    let replacement_bound_at = recovered_at + Duration::from_millis(20);
    bind_current_macos_exact_runtime(&mut runtimes, &source, &hub, replacement_bound_at)
        .expect("replacement runtime binds")
        .expect("replacement runtime exists");
    let replacement_at = replacement_bound_at + Duration::from_millis(20);
    let replacement_frame = cpu_capture_frame(&source, 3, replacement_at, [48, 48, 48, 255]);
    publish_cpu_frame(&exact, &mut runtimes, &source, &replacement_frame);
    let replacement_lease = hub
        .lease(&replacement_descriptor)
        .expect("replacement lease is committed");
    let replacement_publication = replacement_lease
        .read()
        .expect("replacement branch published");
    assert!(matches!(
        hub.invalidate_worker(&old_binding),
        Err(ScreenPublicationHubError::WorkerAuthorityStale { .. })
    ));
    let after_stale_invalidation = replacement_lease
        .read()
        .expect("stale worker cannot clear replacement publication");
    assert!(Arc::ptr_eq(
        &replacement_publication,
        &after_stale_invalidation
    ));
}

#[test]
fn cpu_reduction_timing_excludes_frames_when_branch_cadence_is_not_due() {
    let native_frame = frame();
    let native_source = source(&native_frame);
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    exact.install_test_source(Some(native_source.clone()));
    let demand = cpu_demand_for_kind_at_hz(
        ScreenProcessingProfile::default(),
        ScreenPublicationKind::Surface,
        NonZeroU32::MIN,
    );
    let resolved = resolve_macos_publication_branch(&native_source, &demand)
        .expect("CPU demand resolves")
        .expect("configured source owns CPU demand");
    let mut runtimes = Vec::new();
    commit_cpu_runtime(
        &mut builder,
        &exact,
        &native_source,
        resolved,
        &mut runtimes,
    );
    let telemetry = MacosScreenRuntimeTelemetry::default();
    let captured_at = Instant::now();
    let first = native_cpu_capture_frame(
        &native_frame,
        captured_at,
        captured_at + Duration::from_secs(2),
        &native_source,
        native_source.epoch.source_id.clone(),
    )
    .expect("first native scalar envelope is valid");
    publish_macos_scalar_exact(
        &native_frame,
        &first,
        &native_source,
        &exact,
        &mut runtimes,
        &telemetry,
    )
    .expect("first due CPU branch publishes");
    assert_eq!(telemetry.cpu_reduction_timing.snapshot().sample_count, 1);

    let mut next_native_frame = (*native_frame).clone();
    next_native_frame.sequence = next_native_frame
        .sequence
        .checked_add(1)
        .expect("fixture sequence advances");
    let next_native_frame = Arc::new(next_native_frame);
    let next = native_cpu_capture_frame(
        &next_native_frame,
        captured_at,
        captured_at + Duration::from_secs(2),
        &native_source,
        native_source.epoch.source_id.clone(),
    )
    .expect("second native scalar envelope is valid");
    publish_macos_scalar_exact(
        &next_native_frame,
        &next,
        &native_source,
        &exact,
        &mut runtimes,
        &telemetry,
    )
    .expect("not-due CPU branch is skipped");
    assert_eq!(telemetry.cpu_reduction_timing.snapshot().sample_count, 1);
}

#[derive(Debug)]
struct TestPreparedTarget;

struct TestTargetPreparer;

impl ScreenNativeTargetPreparer for TestTargetPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        MacosNativeTargetManifest::new(platform.descriptor())?;
        Ok(0)
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        MacosNativeTargetManifest::new(platform.descriptor())?;
        Ok(ScreenNativeTargetPreparation::new(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(TestPreparedTarget),
            ),
            0,
        ))
    }
}

fn frame() -> Arc<MacosCaptureFrame> {
    frame_with_color(
        MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Srgb,
            transfer: MacosTransferFunction::Srgb,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        },
        BGRA8,
        &[0, 0, 255, 255],
        None,
    )
}

fn frame_with_color(
    color: MacosCaptureColorimetry,
    pixel_format_fourcc: u32,
    encoded_pixel: &[u8],
    delivered: Option<MacosDeliveredFrameMetadata>,
) -> Arc<MacosCaptureFrame> {
    let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
    let byte_len = u64::try_from(encoded_pixel.len() * 8).expect("fixture length fits");
    let mut surface = MacosCaptureSurface::new_cpu_fixture(
        7,
        byte_len,
        1,
        vec![Arc::<[u8]>::from(encoded_pixel.repeat(8))],
    )
    .expect("fixture surface is valid");
    if let Some(delivered) = delivered {
        surface = surface
            .with_delivery_metadata(delivered)
            .expect("fixture delivery metadata is valid");
    }
    let sample = MacosRawCaptureSample {
        frame: Some(MacosRawCompleteFrame {
            storage_extent: extent,
            planes: vec![MacosRawCapturePlane {
                index: 0,
                extent,
                bytes_per_row: encoded_pixel.len() * 4,
                length_bytes: byte_len,
            }],
            pixel_format_fourcc,
            color,
            cursor_composed: false,
            surface,
        }),
        attachments: MacosRawFrameAttachments {
            status: MacosAttachment::Value(0),
            display_time: MacosAttachment::Value(1_000),
            display_scale_factor: MacosAttachment::Value(1.0),
            content_scale: MacosAttachment::Value(1.0),
            content_rect: MacosAttachment::Value(
                MacosPointRect::new(0.0, 0.0, 4.0, 2.0).expect("fixture content rect is valid"),
            ),
            dirty_rects: MacosAttachment::Missing,
            screen_rect: MacosAttachment::Missing,
            bounding_rect: MacosAttachment::Missing,
        },
    };
    let mut decoder = MacosFrameDecoder::new(7);
    let MacosFrameEvent::Frame(frame) = decoder.decode(sample).expect("fixture frame decodes")
    else {
        panic!("complete fixture sample produces a frame");
    };
    Arc::from(frame)
}

fn frame_with_planes(
    color: MacosCaptureColorimetry,
    pixel_format_fourcc: u32,
    planes: &[(&[u8], MacosPixelExtent, usize)],
    delivered: Option<MacosDeliveredFrameMetadata>,
) -> Arc<MacosCaptureFrame> {
    let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
    let allocation_bytes = planes
        .iter()
        .try_fold(0_u64, |total, (bytes, _, _)| {
            total.checked_add(u64::try_from(bytes.len()).ok()?)
        })
        .expect("fixture allocation fits");
    let mut surface = MacosCaptureSurface::new_cpu_fixture(
        7,
        allocation_bytes,
        1,
        planes
            .iter()
            .map(|(bytes, _, _)| Arc::<[u8]>::from(*bytes))
            .collect(),
    )
    .expect("fixture surface is valid");
    if let Some(delivered) = delivered {
        surface = surface
            .with_delivery_metadata(delivered)
            .expect("fixture delivery metadata is valid");
    }
    let sample = MacosRawCaptureSample {
        frame: Some(MacosRawCompleteFrame {
            storage_extent: extent,
            planes: planes
                .iter()
                .enumerate()
                .map(|(index, (bytes, extent, stride))| MacosRawCapturePlane {
                    index: u32::try_from(index).expect("fixture plane index fits"),
                    extent: *extent,
                    bytes_per_row: *stride,
                    length_bytes: u64::try_from(bytes.len()).expect("fixture length fits"),
                })
                .collect(),
            pixel_format_fourcc,
            color,
            cursor_composed: false,
            surface,
        }),
        attachments: MacosRawFrameAttachments {
            status: MacosAttachment::Value(0),
            display_time: MacosAttachment::Value(1_000),
            display_scale_factor: MacosAttachment::Value(1.0),
            content_scale: MacosAttachment::Value(1.0),
            content_rect: MacosAttachment::Value(
                MacosPointRect::new(0.0, 0.0, 4.0, 2.0).expect("fixture content rect is valid"),
            ),
            dirty_rects: MacosAttachment::Missing,
            screen_rect: MacosAttachment::Missing,
            bounding_rect: MacosAttachment::Missing,
        },
    };
    let mut decoder = MacosFrameDecoder::new(7);
    let MacosFrameEvent::Frame(frame) = decoder.decode(sample).expect("fixture frame decodes")
    else {
        panic!("complete fixture sample produces a frame");
    };
    Arc::from(frame)
}

fn source(frame: &MacosCaptureFrame) -> MacosPublicationSource {
    MacosPublicationSource::from_frame(
        CaptureSourceId::new("display:test").expect("fixture source id is valid"),
        3,
        5,
        frame,
    )
    .expect("fixture source resolves")
}

fn cpu_demand(profile: ScreenProcessingProfile) -> RegisteredScreenBranchDemand {
    cpu_demand_for_kind(profile, ScreenPublicationKind::Surface)
}

fn cpu_demand_for_kind(
    profile: ScreenProcessingProfile,
    kind: ScreenPublicationKind,
) -> RegisteredScreenBranchDemand {
    cpu_demand_for_kind_at_hz(profile, kind, NonZeroU32::new(60).expect("nonzero cadence"))
}

fn cpu_demand_for_kind_at_hz(
    profile: ScreenProcessingProfile,
    kind: ScreenPublicationKind,
    requested_hz: NonZeroU32,
) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Cover,
            Arc::new(profile),
        ),
        requested_hz,
    )
}

fn execute_resolved_cpu(
    source: &MacosPublicationSource,
    descriptor: &ResolvedScreenPublicationDescriptor,
    encoded_bgra: [u8; 4],
) -> Vec<u8> {
    let source_extent = source.logical_extent;
    let source_bytes = Arc::<[u8]>::from(
        encoded_bgra.repeat(
            usize::try_from(source_extent.width() * source_extent.height())
                .expect("fixture pixel count fits"),
        ),
    );
    let storage = CpuCaptureStorage::new(
        source_bytes,
        CapturePixelFormat::Bgra8,
        i64::from(source_extent.width()) * 4,
        0,
    );
    let layout = CpuReductionLayout::new(source_extent, descriptor.physical().reduction_extent())
        .expect("fixture reduction layout is valid");
    let mut output = vec![0; layout.target_byte_len_usize()];
    CpuReductionExecutor::new(NonZeroUsize::MIN, NonZeroU32::MIN)
        .expect("fixture executor prepares")
        .reduce(
            CpuReductionRequest::new(
                &storage,
                layout,
                descriptor.physical().target_pixel_format(),
                descriptor.physical().reduction_filter(),
                descriptor.physical().color_pipeline(),
            ),
            &mut output,
        )
        .expect("resolved macOS CPU color pipeline executes");
    output
}

fn commit_cpu_runtime(
    builder: &mut ScreenPlanBuilder,
    exact: &MacosExactPublicationShared,
    source: &MacosPublicationSource,
    resolved: ResolvedScreenBranchDemand,
    runtimes: &mut Vec<MacosExactRuntime>,
) -> ResolvedScreenPublicationDescriptor {
    commit_cpu_runtimes(builder, exact, source, [resolved], runtimes)
        .pop()
        .expect("single-demand fixture commits one descriptor")
}

fn commit_cpu_runtimes(
    builder: &mut ScreenPlanBuilder,
    exact: &MacosExactPublicationShared,
    source: &MacosPublicationSource,
    resolved: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
    runtimes: &mut Vec<MacosExactRuntime>,
) -> Vec<ResolvedScreenPublicationDescriptor> {
    let resolved = resolved.into_iter().collect::<Vec<_>>();
    let descriptors = resolved
        .iter()
        .map(|demand| demand.descriptor().clone())
        .collect();
    let revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("fixture demand revision advances");
    let graph = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            resolved,
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("macOS CPU candidate plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch.source_id)
        .expect("macOS source owns the candidate worker");
    let (token, runtime) = prepare_macos_exact_runtime(ticket, Some(source), exact)
        .expect("macOS CPU runtime prepares");
    let (runtime, owned_source) = runtime.expect("CPU plan owns a runtime");
    exact.register_test_owned_source(ExactBoxList::boxed_node(owned_source));
    runtimes.push(runtime);
    preparing
        .acknowledge(token)
        .expect("macOS CPU worker token matches candidate");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .unwrap_or_else(|failure| panic!("macOS CPU plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, revision, graph)
        .unwrap_or_else(|failure| panic!("macOS CPU plan commits: {}", failure.error()));
    let (_, retirement) = committed.into_parts();
    drop(retirement);
    descriptors
}

fn cpu_capture_frame(
    source: &MacosPublicationSource,
    sequence: u64,
    captured_at: Instant,
    encoded_bgra: [u8; 4],
) -> CaptureFrame<RawCaptureSurface> {
    let byte_len = usize::try_from(
        u64::from(source.geometry.storage_extent().width())
            * u64::from(source.geometry.storage_extent().height())
            * 4,
    )
    .expect("fixture CPU bytes fit");
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch.source_id.clone(),
            topology_generation: source.epoch.topology_generation,
            session_generation: source.epoch.session_generation,
            sequence,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry: source.geometry,
            colorimetry: source.colorimetry,
            cursor: CaptureCursor {
                visible: false,
                position: None,
                hotspot: None,
                shape_extent: None,
                shape_generation: None,
                content: CaptureCursorContent::Hidden,
            },
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::from(encoded_bgra.repeat(byte_len / 4)),
            CapturePixelFormat::Bgra8,
            i64::from(source.geometry.storage_extent().width()) * 4,
            0,
        )),
        CaptureDamage::new(Vec::new(), Vec::new()),
    )
    .expect("fixture CPU frame is valid")
}

fn publish_cpu_bytes(
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    descriptor: &ResolvedScreenPublicationDescriptor,
    frame: &CaptureFrame<RawCaptureSurface>,
) -> Vec<u8> {
    publish_cpu_frame(exact, runtimes, source, frame);
    published_surface_bytes(exact, descriptor)
}

fn publish_cpu_frame(
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    frame: &CaptureFrame<RawCaptureSurface>,
) {
    let hub = exact.hub().expect("fixture hub remains installed");
    let runtime =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)
            .expect("current macOS runtime binds")
            .expect("committed runtime is current");
    let report = runtime
        .fanout
        .as_mut()
        .expect("CPU runtime owns a fanout")
        .publish_due(
            &hub,
            Some(frame),
            frame.metadata().captured_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("CPU fanout publishes");
    assert!(
        report.published() > 0,
        "CPU fixture had no due branch: {report:?}"
    );
}

fn publish_scalar_frame(
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
) {
    let capture = native_cpu_capture_frame(
        frame,
        captured_at,
        captured_at + Duration::from_secs(1),
        source,
        source.epoch.source_id.clone(),
    )
    .expect("native scalar fixture envelope is valid");
    let hub = exact.hub().expect("fixture hub remains installed");
    let runtime = bind_current_macos_exact_runtime(runtimes, source, &hub, captured_at)
        .expect("current macOS runtime binds")
        .expect("committed runtime is current");
    let report = runtime
        .fanout
        .as_mut()
        .expect("CPU runtime owns a fanout")
        .publish_due_scalar(
            &hub,
            &capture,
            captured_at,
            ScreenPublicationHealth::Healthy,
            |execute| {
                frame
                    .with_cpu_source(|samples| execute(&samples))
                    .map_err(|error| {
                        CpuPublicationFanoutError::ScalarSourceAccessFailed(error.to_string())
                    })?
            },
        )
        .expect("native scalar fanout publishes");
    assert!(report.published() > 0);
}

fn active_tone_map_transition_count(
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    captured_at: Instant,
) -> usize {
    let hub = exact.hub().expect("fixture hub remains installed");
    bind_current_macos_exact_runtime(runtimes, source, &hub, captured_at)
        .expect("current macOS runtime binds")
        .expect("committed runtime is current")
        .fanout
        .as_ref()
        .expect("CPU runtime owns a fanout")
        .active_tone_map_transition_count()
}

fn published_surface_bytes(
    exact: &MacosExactPublicationShared,
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Vec<u8> {
    let hub = exact.hub().expect("fixture hub remains installed");
    let lease = hub
        .lease(descriptor)
        .expect("committed Surface branch has a lease");
    let publication = lease.read().expect("Surface branch has published bytes");
    let ScreenBranchPayload::Surface(surface) = publication.payload() else {
        panic!("fixture branch publishes Surface bytes");
    };
    surface.pixels().to_vec()
}

fn published_zone_colors(
    exact: &MacosExactPublicationShared,
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Vec<[u8; 3]> {
    let hub = exact.hub().expect("fixture hub remains installed");
    let lease = hub
        .lease(descriptor)
        .expect("committed Zones branch has a lease");
    let publication = lease.read().expect("Zones branch has published colors");
    let ScreenBranchPayload::Zones(zones) = publication.payload() else {
        panic!("fixture branch publishes zone colors");
    };
    zones.colors().to_vec()
}

fn transition_profile(hdr: bool) -> ScreenProcessingProfile {
    transition_profile_with_smoothing(hdr, ScreenSmoothingPolicy::Disabled)
}

fn transition_profile_with_smoothing(
    hdr: bool,
    smoothing: ScreenSmoothingPolicy,
) -> ScreenProcessingProfile {
    let calibration = LedToneMapCalibration::DEFAULT;
    transition_profile_with_calibration(hdr, smoothing, calibration)
}

fn transition_profile_with_calibration(
    hdr: bool,
    smoothing: ScreenSmoothingPolicy,
    calibration: LedToneMapCalibration,
) -> ScreenProcessingProfile {
    ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        smoothing,
        hdr: if hdr {
            ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                ScreenToneMapOperator::Bt2390Eetf,
                calibration,
            ))
        } else {
            ScreenHdrPolicy::Reject
        },
        ..ScreenProcessingProfileConfig::default()
    })
    .with_led_tone_map(calibration)
}

fn hdr_transition_source(sdr_source: &MacosPublicationSource) -> MacosPublicationSource {
    let hdr_color = CaptureColorimetry::new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Pq,
        Some(CaptureDynamicRange::High),
        Some(
            CaptureLuminanceContext::new(
                CapturePositiveScalar::try_new(203.0).expect("reference white is valid"),
                CapturePositiveScalar::try_new(1_000.0).expect("peak is valid"),
            )
            .expect("HDR luminance is ordered"),
        ),
    )
    .expect("HDR fixture colorimetry is valid");
    MacosPublicationSource {
        colorimetry: hdr_color,
        ..sdr_source.clone()
    }
}

#[test]
fn delivered_hdr_luminance_is_defaulted_and_mapped_exactly() {
    assert_eq!(
        capture_colorimetry(&frame()).expect("SDR remains valid without delivery luminance"),
        CaptureColorimetry::SRGB
    );
    let hdr_color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Pq,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let headroom = 1_000.0 / 203.0;
    let delivered = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        hdr_color,
        Some(203.0),
        Some(headroom),
    )
    .expect("complete HDR metadata is valid");
    let hdr = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(delivered));
    let colorimetry = capture_colorimetry(&hdr).expect("complete HDR colorimetry maps");
    let luminance = colorimetry.luminance().expect("HDR luminance is retained");
    assert_eq!(luminance.reference_white_nits().value(), 203.0);
    assert_eq!(luminance.peak_nits().value(), 203.0 * headroom);

    let linear_hdr_color = MacosCaptureColorimetry {
        transfer: MacosTransferFunction::Linear,
        ..hdr_color
    };
    let linear_delivered = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        linear_hdr_color,
        Some(203.0),
        Some(headroom),
    )
    .expect("extended-linear HDR metadata is valid");
    let linear_hdr = frame_with_color(
        linear_hdr_color,
        RGBA16_FLOAT,
        &[0; 8],
        Some(linear_delivered),
    );
    let linear_colorimetry =
        capture_colorimetry(&linear_hdr).expect("extended-linear HDR colorimetry maps");
    assert_eq!(
        linear_colorimetry.transfer_function(),
        CaptureTransferFunction::Linear
    );
    assert_eq!(
        linear_colorimetry.dynamic_range(),
        Some(CaptureDynamicRange::High)
    );
    assert_eq!(linear_colorimetry.luminance(), colorimetry.luminance());
    let missing_linear = frame_with_color(linear_hdr_color, RGBA16_FLOAT, &[0; 8], None);
    assert!(capture_colorimetry(&missing_linear).is_err());

    let missing = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], None);
    assert!(capture_colorimetry(&missing).is_err());

    let no_reference_white = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        hdr_color,
        None,
        Some(headroom),
    )
    .expect("capture layer admits optional reference white");
    let no_reference_white =
        frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(no_reference_white));
    let luminance = capture_colorimetry(&no_reference_white)
        .expect("missing reference white uses the diffuse-white default")
        .luminance()
        .expect("HDR fallback retains luminance");
    assert_eq!(luminance.reference_white_nits().value(), 203.0);
    assert_eq!(luminance.peak_nits().value(), 203.0 * headroom);

    let no_headroom = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        hdr_color,
        Some(203.0),
        None,
    )
    .expect("capture layer admits optional headroom");
    let no_headroom = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(no_headroom));
    let luminance = capture_colorimetry(&no_headroom)
        .expect("missing headroom uses the one-stop default")
        .luminance()
        .expect("HDR fallback retains luminance");
    assert_eq!(luminance.reference_white_nits().value(), 203.0);
    assert_eq!(luminance.peak_nits().value(), 406.0);

    let no_peak_headroom = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        hdr_color,
        Some(203.0),
        Some(1.0),
    )
    .expect("capture layer admits unity headroom");
    let no_peak_headroom =
        frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(no_peak_headroom));
    let luminance = capture_colorimetry(&no_peak_headroom)
        .expect("unity headroom uses the one-stop default")
        .luminance()
        .expect("HDR fallback retains luminance");
    assert_eq!(luminance.reference_white_nits().value(), 203.0);
    assert_eq!(luminance.peak_nits().value(), 406.0);

    let contradictory_color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::DisplayP3,
        ..hdr_color
    };
    let contradictory = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        contradictory_color,
        Some(203.0),
        Some(headroom),
    )
    .expect("alternate HDR metadata is valid in isolation");
    let contradictory = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(contradictory));
    assert!(capture_colorimetry(&contradictory).is_err());
}

#[test]
fn macos_cpu_resolves_p3_and_full_precision_hdr() {
    let p3_color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::DisplayP3,
        transfer: MacosTransferFunction::Linear,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let p3_frame = frame_with_color(p3_color, BGRA8, &[255, 0, 255, 255], None);
    let p3_source = source(&p3_frame);
    let p3_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        ..ScreenProcessingProfileConfig::default()
    });
    let p3 = resolve_macos_publication_branch(&p3_source, &cpu_demand(p3_profile))
        .expect("P3 macOS demand resolves")
        .expect("configured source owns P3 demand");
    assert!(matches!(
        p3.descriptor().physical().color_pipeline().transform(),
        ResolvedScreenColorTransform::LinearRelativeColorimetric { .. }
    ));
    assert_eq!(
        &execute_resolved_cpu(&p3_source, p3.descriptor(), [255, 0, 255, 255])[..4],
        [255, 59, 242, 255]
    );

    let hdr_color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Pq,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let delivered = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        hdr_color,
        Some(203.0),
        Some(1_000.0 / 203.0),
    )
    .expect("HDR delivery metadata is valid");
    let hdr_frame = frame_with_color(hdr_color, RGBA16_FLOAT, &[0; 8], Some(delivered));
    let hdr_source = source(&hdr_frame);
    let calibration = LedToneMapCalibration::DEFAULT;
    let hdr_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
            ScreenToneMapOperator::Bt2390Eetf,
            calibration,
        )),
        ..ScreenProcessingProfileConfig::default()
    })
    .with_led_tone_map(calibration);
    let hdr = resolve_macos_publication_branch(&hdr_source, &cpu_demand(hdr_profile))
        .expect("full-precision HDR CPU demand resolves")
        .expect("configured source owns HDR demand");
    assert_eq!(
        hdr.descriptor().source_pixel_format(),
        CapturePixelFormat::Rgba16Float
    );
    assert!(matches!(
        hdr.descriptor().physical().color_pipeline().transform(),
        ResolvedScreenColorTransform::ToneMap(_)
    ));
}

#[test]
fn macos_publication_transition_is_deterministic_at_zero_midpoint_and_completion() {
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    let mut runtimes = Vec::new();
    let base_frame = frame();
    let sdr_source = source(&base_frame);
    exact.install_test_source(Some(sdr_source.clone()));
    let sdr = resolve_macos_publication_branch(&sdr_source, &cpu_demand(transition_profile(false)))
        .expect("SDR transition branch resolves")
        .expect("configured source owns SDR transition branch");
    let sdr_descriptor = commit_cpu_runtime(&mut builder, &exact, &sdr_source, sdr, &mut runtimes);
    let started = Instant::now() + Duration::from_millis(20);
    let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [148, 148, 148, 255]);
    let sdr_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &sdr_source,
        &sdr_descriptor,
        &sdr_frame,
    );
    assert_eq!(&sdr_bytes[..4], [148, 148, 148, 255]);

    let hdr_source = hdr_transition_source(&sdr_source);
    exact.install_test_source(Some(hdr_source.clone()));
    let hdr = resolve_macos_publication_branch(&hdr_source, &cpu_demand(transition_profile(true)))
        .expect("HDR transition branch resolves")
        .expect("configured source owns HDR transition branch");
    let hdr_descriptor = commit_cpu_runtime(&mut builder, &exact, &hdr_source, hdr, &mut runtimes);
    let at_zero = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
    let zero_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &hdr_source,
        &hdr_descriptor,
        &at_zero,
    );
    let at_midpoint = cpu_capture_frame(
        &hdr_source,
        3,
        started + Duration::from_millis(125),
        [148, 148, 148, 255],
    );
    let midpoint_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &hdr_source,
        &hdr_descriptor,
        &at_midpoint,
    );
    let at_complete = cpu_capture_frame(
        &hdr_source,
        4,
        started + Duration::from_millis(250),
        [148, 148, 148, 255],
    );
    let complete_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &hdr_source,
        &hdr_descriptor,
        &at_complete,
    );
    assert_eq!(&zero_bytes[..4], [255, 255, 255, 255]);
    assert_eq!(&midpoint_bytes[..4], [223, 223, 223, 255]);
    assert_eq!(&complete_bytes[..4], [187, 187, 187, 255]);
}

#[test]
fn macos_transition_inheritance_skips_matching_routes_without_curve_state() {
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    let mut runtimes = Vec::new();
    let base_frame = frame();
    let sdr_source = source(&base_frame);
    exact.install_test_source(Some(sdr_source.clone()));
    let identity_profile = ScreenProcessingProfile::new(
        ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Bgra8),
    );
    let calibration = LedToneMapCalibration::DEFAULT;
    let managed_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_pixel_format: CapturePixelFormat::Bgra8,
        ..ScreenProcessingProfileConfig::default()
    })
    .with_led_tone_map(calibration);
    let identity = resolve_macos_publication_branch(&sdr_source, &cpu_demand(identity_profile))
        .expect("encoded-identity branch resolves")
        .expect("configured source owns encoded-identity branch");
    let managed = resolve_macos_publication_branch(&sdr_source, &cpu_demand(managed_profile))
        .expect("managed SDR branch resolves")
        .expect("configured source owns managed SDR branch");
    let sdr_descriptors = commit_cpu_runtimes(
        &mut builder,
        &exact,
        &sdr_source,
        [identity, managed],
        &mut runtimes,
    );
    let started = Instant::now() + Duration::from_millis(20);
    let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [148, 148, 148, 255]);
    publish_cpu_frame(&exact, &mut runtimes, &sdr_source, &sdr_frame);
    assert_eq!(sdr_descriptors.len(), 2);

    let hdr_source = hdr_transition_source(&sdr_source);
    exact.install_test_source(Some(hdr_source.clone()));
    let hdr_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_pixel_format: CapturePixelFormat::Bgra8,
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
            ScreenToneMapOperator::Bt2390Eetf,
            calibration,
        )),
        ..ScreenProcessingProfileConfig::default()
    })
    .with_led_tone_map(calibration);
    let hdr = resolve_macos_publication_branch(&hdr_source, &cpu_demand(hdr_profile))
        .expect("managed HDR branch resolves")
        .expect("configured source owns managed HDR branch");
    let hdr_descriptor = commit_cpu_runtime(&mut builder, &exact, &hdr_source, hdr, &mut runtimes);
    let transition_start = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
    assert_eq!(
        &publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &hdr_source,
            &hdr_descriptor,
            &transition_start,
        )[..4],
        [255, 255, 255, 255]
    );
}

#[test]
fn macos_publication_transition_restarts_from_its_midpoint_curve() {
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    let mut runtimes = Vec::new();
    let base_frame = frame();
    let sdr_source = source(&base_frame);
    exact.install_test_source(Some(sdr_source.clone()));
    let sdr = resolve_macos_publication_branch(&sdr_source, &cpu_demand(transition_profile(false)))
        .expect("SDR transition branch resolves")
        .expect("configured source owns SDR transition branch");
    let sdr_descriptor = commit_cpu_runtime(&mut builder, &exact, &sdr_source, sdr, &mut runtimes);
    let started = Instant::now() + Duration::from_millis(20);
    let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [148, 148, 148, 255]);
    assert_eq!(
        &publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &sdr_source,
            &sdr_descriptor,
            &sdr_frame,
        )[..4],
        [148, 148, 148, 255]
    );

    let hdr_source = hdr_transition_source(&sdr_source);
    exact.install_test_source(Some(hdr_source.clone()));
    let hdr = resolve_macos_publication_branch(&hdr_source, &cpu_demand(transition_profile(true)))
        .expect("HDR transition branch resolves")
        .expect("configured source owns HDR transition branch");
    let hdr_descriptor = commit_cpu_runtime(&mut builder, &exact, &hdr_source, hdr, &mut runtimes);
    let at_zero = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
    assert_eq!(
        &publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &hdr_source,
            &hdr_descriptor,
            &at_zero,
        )[..4],
        [255, 255, 255, 255]
    );
    let at_midpoint = cpu_capture_frame(
        &hdr_source,
        3,
        started + Duration::from_millis(125),
        [148, 148, 148, 255],
    );
    assert_eq!(
        &publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &hdr_source,
            &hdr_descriptor,
            &at_midpoint,
        )[..4],
        [223, 223, 223, 255]
    );

    exact.install_test_source(Some(sdr_source.clone()));
    let restarted_sdr =
        resolve_macos_publication_branch(&sdr_source, &cpu_demand(transition_profile(false)))
            .expect("restarted SDR branch resolves")
            .expect("configured source owns restarted SDR branch");
    let restarted_descriptor = commit_cpu_runtime(
        &mut builder,
        &exact,
        &sdr_source,
        restarted_sdr,
        &mut runtimes,
    );
    let restart_boundary = cpu_capture_frame(
        &sdr_source,
        5,
        started + Duration::from_millis(125),
        [255, 255, 255, 255],
    );
    let restart_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &sdr_source,
        &restarted_descriptor,
        &restart_boundary,
    );
    let restart_midpoint = cpu_capture_frame(
        &sdr_source,
        6,
        started + Duration::from_millis(250),
        [255, 255, 255, 255],
    );
    let restart_midpoint_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &sdr_source,
        &restarted_descriptor,
        &restart_midpoint,
    );
    let restart_complete = cpu_capture_frame(
        &sdr_source,
        7,
        started + Duration::from_millis(375),
        [255, 255, 255, 255],
    );
    let restart_complete_bytes = publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &sdr_source,
        &restarted_descriptor,
        &restart_complete,
    );
    assert_eq!(&restart_bytes[..4], [224, 224, 224, 255]);
    assert_eq!(&restart_midpoint_bytes[..4], [238, 238, 238, 255]);
    assert_eq!(&restart_complete_bytes[..4], [255, 255, 255, 255]);
}

#[test]
fn sdr_exposure_reconfiguration_swaps_atomically_without_transition() {
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    let mut runtimes = Vec::new();
    let source = source(&frame());
    exact.install_test_source(Some(source.clone()));
    let initial = resolve_macos_publication_branch(&source, &cpu_demand(transition_profile(false)))
        .expect("initial SDR branch resolves")
        .expect("configured source owns the initial SDR branch");
    let initial_descriptor =
        commit_cpu_runtime(&mut builder, &exact, &source, initial, &mut runtimes);
    let started = Instant::now() + Duration::from_millis(20);
    let initial_frame = cpu_capture_frame(&source, 1, started, [96, 96, 96, 255]);
    publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &source,
        &initial_descriptor,
        &initial_frame,
    );

    let default = LedToneMapCalibration::DEFAULT;
    let calibration = LedToneMapCalibration::try_new(
        default.target_white_x(),
        default.target_white_y(),
        default.target_reference_white_nits(),
        default.target_peak_nits(),
        1.0,
    )
    .expect("updated SDR exposure is valid");
    let next = resolve_macos_publication_branch(
        &source,
        &cpu_demand(transition_profile_with_calibration(
            false,
            ScreenSmoothingPolicy::Disabled,
            calibration,
        )),
    )
    .expect("updated SDR branch resolves")
    .expect("configured source owns the updated SDR branch");
    let next_descriptor = commit_cpu_runtime(&mut builder, &exact, &source, next, &mut runtimes);
    let boundary = started + Duration::from_millis(20);
    assert_eq!(
        active_tone_map_transition_count(&exact, &mut runtimes, &source, boundary),
        0
    );
    let encoded = [96, 96, 96, 255];
    let expected = execute_resolved_cpu(&source, &next_descriptor, encoded);
    let at_zero = cpu_capture_frame(&source, 2, boundary, encoded);
    assert_eq!(
        publish_cpu_bytes(&exact, &mut runtimes, &source, &next_descriptor, &at_zero,),
        expected
    );
    let at_midpoint = cpu_capture_frame(&source, 3, boundary + Duration::from_millis(125), encoded);
    assert_eq!(
        publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &source,
            &next_descriptor,
            &at_midpoint,
        ),
        expected
    );
    assert_eq!(
        active_tone_map_transition_count(
            &exact,
            &mut runtimes,
            &source,
            boundary + Duration::from_millis(125),
        ),
        0
    );
}

#[test]
fn hdr_calibration_reconfiguration_swaps_atomically_without_transition() {
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    let mut runtimes = Vec::new();
    let source = hdr_transition_source(&source(&frame()));
    exact.install_test_source(Some(source.clone()));
    let initial = resolve_macos_publication_branch(&source, &cpu_demand(transition_profile(true)))
        .expect("initial HDR branch resolves")
        .expect("configured source owns the initial HDR branch");
    let initial_descriptor =
        commit_cpu_runtime(&mut builder, &exact, &source, initial, &mut runtimes);
    let started = Instant::now() + Duration::from_millis(20);
    let initial_frame = cpu_capture_frame(&source, 1, started, [148, 148, 148, 255]);
    publish_cpu_bytes(
        &exact,
        &mut runtimes,
        &source,
        &initial_descriptor,
        &initial_frame,
    );

    let default = LedToneMapCalibration::DEFAULT;
    let calibration = LedToneMapCalibration::try_new(
        default.target_white_x(),
        default.target_white_y(),
        160.0,
        640.0,
        default.exposure_ev(),
    )
    .expect("updated HDR calibration is valid");
    let next = resolve_macos_publication_branch(
        &source,
        &cpu_demand(transition_profile_with_calibration(
            true,
            ScreenSmoothingPolicy::Disabled,
            calibration,
        )),
    )
    .expect("updated HDR branch resolves")
    .expect("configured source owns the updated HDR branch");
    let next_descriptor = commit_cpu_runtime(&mut builder, &exact, &source, next, &mut runtimes);
    let boundary = started + Duration::from_millis(20);
    assert_eq!(
        active_tone_map_transition_count(&exact, &mut runtimes, &source, boundary),
        0
    );
    let encoded = [148, 148, 148, 255];
    let expected = execute_resolved_cpu(&source, &next_descriptor, encoded);
    let at_zero = cpu_capture_frame(&source, 2, boundary, encoded);
    assert_eq!(
        publish_cpu_bytes(&exact, &mut runtimes, &source, &next_descriptor, &at_zero,),
        expected
    );
    let at_midpoint = cpu_capture_frame(&source, 3, boundary + Duration::from_millis(125), encoded);
    assert_eq!(
        publish_cpu_bytes(
            &exact,
            &mut runtimes,
            &source,
            &next_descriptor,
            &at_midpoint,
        ),
        expected
    );
    assert_eq!(
        active_tone_map_transition_count(
            &exact,
            &mut runtimes,
            &source,
            boundary + Duration::from_millis(125),
        ),
        0
    );
}

#[test]
fn macos_publication_samples_once_and_suppresses_both_scene_cut_paths() {
    let smoothing = ScreenSmoothingPolicy::Exponential {
        time_constant: Duration::from_mins(1),
        scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
            threshold: ScreenProfileScalar::try_new(0.01).expect("scene-cut threshold is valid"),
        },
    };
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    let mut runtimes = Vec::new();
    let base_frame = frame();
    let sdr_source = source(&base_frame);
    exact.install_test_source(Some(sdr_source.clone()));
    let sdr_profile = transition_profile_with_smoothing(false, smoothing);
    let sdr_surface = resolve_macos_publication_branch(
        &sdr_source,
        &cpu_demand_for_kind(sdr_profile.clone(), ScreenPublicationKind::Surface),
    )
    .expect("SDR Surface branch resolves")
    .expect("configured source owns SDR Surface branch");
    let sdr_zones = resolve_macos_publication_branch(
        &sdr_source,
        &cpu_demand_for_kind(
            sdr_profile,
            ScreenPublicationKind::Zones {
                columns: NonZeroU32::MIN,
                rows: NonZeroU32::MIN,
            },
        ),
    )
    .expect("SDR Zones branch resolves")
    .expect("configured source owns SDR Zones branch");
    let sdr_descriptors = commit_cpu_runtimes(
        &mut builder,
        &exact,
        &sdr_source,
        [sdr_surface, sdr_zones],
        &mut runtimes,
    );
    assert_eq!(sdr_descriptors.len(), 2);
    assert_eq!(sdr_descriptors[0].physical(), sdr_descriptors[1].physical());
    let started = Instant::now() + Duration::from_millis(20);
    let sdr_frame = cpu_capture_frame(&sdr_source, 1, started, [255, 255, 255, 255]);
    publish_cpu_frame(&exact, &mut runtimes, &sdr_source, &sdr_frame);
    assert_eq!(
        &published_surface_bytes(&exact, &sdr_descriptors[0])[..4],
        [255, 255, 255, 255]
    );
    assert_eq!(
        published_zone_colors(&exact, &sdr_descriptors[1])[0],
        [255, 255, 255]
    );

    let hdr_source = hdr_transition_source(&sdr_source);
    exact.install_test_source(Some(hdr_source.clone()));
    let hdr_profile = transition_profile_with_smoothing(true, smoothing);
    let hdr_surface = resolve_macos_publication_branch(
        &hdr_source,
        &cpu_demand_for_kind(hdr_profile.clone(), ScreenPublicationKind::Surface),
    )
    .expect("HDR Surface branch resolves")
    .expect("configured source owns HDR Surface branch");
    let hdr_zones = resolve_macos_publication_branch(
        &hdr_source,
        &cpu_demand_for_kind(
            hdr_profile,
            ScreenPublicationKind::Zones {
                columns: NonZeroU32::MIN,
                rows: NonZeroU32::MIN,
            },
        ),
    )
    .expect("HDR Zones branch resolves")
    .expect("configured source owns HDR Zones branch");
    let hdr_descriptors = commit_cpu_runtimes(
        &mut builder,
        &exact,
        &hdr_source,
        [hdr_surface, hdr_zones],
        &mut runtimes,
    );
    assert_eq!(hdr_descriptors.len(), 2);
    assert_eq!(hdr_descriptors[0].physical(), hdr_descriptors[1].physical());
    let transition_start = cpu_capture_frame(&hdr_source, 2, started, [148, 148, 148, 255]);
    publish_cpu_frame(&exact, &mut runtimes, &hdr_source, &transition_start);
    assert_eq!(
        &published_surface_bytes(&exact, &hdr_descriptors[0])[..4],
        [255, 255, 255, 255]
    );
    assert_eq!(
        published_zone_colors(&exact, &hdr_descriptors[1])[0],
        [255, 255, 255]
    );

    let midpoint = cpu_capture_frame(
        &hdr_source,
        3,
        started + Duration::from_millis(125),
        [148, 148, 148, 255],
    );
    publish_cpu_frame(&exact, &mut runtimes, &hdr_source, &midpoint);
    let surface = published_surface_bytes(&exact, &hdr_descriptors[0]);
    let zones = published_zone_colors(&exact, &hdr_descriptors[1]);
    assert!(surface[0] > 250);
    assert!(zones[0][0] > 250);
    assert_eq!(&surface[..3], zones[0]);
}

fn target() -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(NonZeroU64::new(11).expect("nonzero target")),
        PlatformGpuApi::Metal,
        ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(91),
        NonZeroU32::new(16_384).expect("nonzero texture limit"),
        Arc::new(TestTargetPreparer),
    )
}

fn native_demand(target: &ScreenNativeExecutionTarget) -> RegisteredScreenBranchDemand {
    native_demand_for_format(target, CapturePixelFormat::Bgra8)
}

fn required_native_demand(target: &ScreenNativeExecutionTarget) -> RegisteredScreenBranchDemand {
    let mut demand = native_demand(target);
    let request = demand.request();
    demand = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            request.selector().clone(),
            request.kind(),
            ScreenPublicationExecutorRequest::SourceNativeRequired(target.clone()),
            request.extent(),
            request.aspect(),
            Arc::clone(request.processing_profile()),
        ),
        demand.requested_hz(),
    );
    demand
}

fn native_demand_for_format(
    target: &ScreenNativeExecutionTarget,
    format: CapturePixelFormat,
) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::SourceNative(target.clone()),
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::new(
                ScreenProcessingProfileConfig::exact_encoded_identity(format),
            )),
        ),
        NonZeroU32::new(60).expect("nonzero cadence"),
    )
}

fn reduced_native_demand(target: &ScreenNativeExecutionTarget) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::SourceNative(target.clone()),
            ScreenExtentRequest::bounded(
                NonZeroU32::new(2),
                NonZeroU32::new(1),
                super::super::ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        NonZeroU32::new(60).expect("nonzero cadence"),
    )
}

fn publish_native_fixture(
    frame: &Arc<MacosCaptureFrame>,
    source: &MacosPublicationSource,
    resolved: ResolvedScreenBranchDemand,
) -> (
    Arc<ScreenBranchPublication>,
    Arc<MacosScreenRuntimeTelemetry>,
) {
    let exact = MacosExactPublicationShared::default();
    exact.install_test_source(Some(source.clone()));
    let mut builder = ScreenPlanBuilder::new();
    exact.install_hub(builder.publication_hub());
    let revision = InputPublicationDemandRevision::new(1);
    let graph = ScreenInputGraphGeneration::new(1);
    let mut preparing = builder
        .prepare(
            [resolved],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("native candidate plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch.source_id)
        .expect("macOS source owns its worker ticket");
    let (token, runtime) =
        prepare_macos_exact_runtime(ticket, Some(source), &exact).expect("native runtime prepares");
    let (runtime, owned_source) = runtime.expect("native branch owns a runtime");
    exact.register_test_owned_source(ExactBoxList::boxed_node(owned_source));
    let mut runtimes = vec![runtime];
    preparing
        .acknowledge(token)
        .expect("native worker token matches candidate");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .unwrap_or_else(|failure| panic!("native plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, revision, graph)
        .unwrap_or_else(|failure| panic!("native plan commits: {}", failure.error()));
    let (_, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("initial plan has no retired readers");

    let now = Instant::now();
    let (_, telemetry) = publish_macos_native_exact(
        frame,
        now,
        now + Duration::from_secs(1),
        source,
        &exact,
        &mut runtimes,
    )
    .expect("native frame publishes");
    let hub = exact.hub().expect("test hub remains installed");
    let (_, lease) = hub.observe_matching_lease(|_| true);
    let publication = lease
        .expect("committed native branch has a lease")
        .read()
        .expect("native branch has a publication");
    (publication, telemetry)
}

#[test]
fn native_publication_commits_owner_backed_metal_surface() {
    let frame = frame();
    let source = source(&frame);
    let demand = native_demand(&target());
    let resolved = resolve_macos_publication_branch(&source, &demand)
        .expect("native demand resolves")
        .expect("configured macOS source owns native demand");
    assert!(matches!(
        resolved.descriptor().executor(),
        ScreenPublicationExecutor::SourceNative(_)
    ));

    let (publication, telemetry) = publish_native_fixture(&frame, &source, resolved);
    assert_eq!(publication.native_sequence(), NonZeroU64::MIN);
    let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
        panic!("identity macOS native branch publishes its GPU surface");
    };
    let surface = payload.surface();
    assert_eq!(surface.api(), &PlatformGpuApi::Metal);
    assert_eq!(surface.handle_id(), 7);
    assert_eq!(surface.format(), CapturePixelFormat::Bgra8);
    assert_eq!(surface.extent(), source.geometry.storage_extent());
    assert_eq!(payload.colorimetry().value(), source.colorimetry);
    assert!(surface.owner::<MacosCaptureFrame>().is_some());
    assert!(surface.timing_sink().is_some());
    assert!(surface.retained_owner::<TestPreparedTarget>().is_some());
    assert!(surface.resource_lifetime().is_some());
    assert!(surface.capture_resource_lifetime().is_some());
    assert_eq!(
        telemetry
            .capture_to_native_publication_timing
            .snapshot()
            .sample_count,
        1
    );
}

#[test]
fn required_native_publication_reports_only_native_or_typed_unavailable() {
    let frame = frame();
    let source = source(&frame);
    let native_target = target();
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let resolved = resolve_macos_publication_branch_with_telemetry(
        &source,
        &required_native_demand(&native_target),
        &telemetry,
    )
    .expect("matching required native demand resolves")
    .expect("configured macOS source owns required native demand");
    assert!(matches!(
        resolved.descriptor().executor(),
        ScreenPublicationExecutor::SourceNative(_)
    ));
    assert_eq!(telemetry.publication_path().as_deref(), Some("native"));
    assert!(lock(&telemetry.fallback_reason).is_none());

    let incompatible = ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(NonZeroU64::new(12).expect("nonzero target")),
        PlatformGpuApi::Direct3d11,
        ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
            low_part: 7,
            high_part: 11,
        },
        NonZeroU32::new(16_384).expect("nonzero texture limit"),
        Arc::new(TestTargetPreparer),
    );
    let error = resolve_macos_publication_branch_with_telemetry(
        &source,
        &required_native_demand(&incompatible),
        &telemetry,
    )
    .expect_err("required native demand rejects a non-Metal target");
    assert_eq!(
        error.downcast_ref::<ScreenPublicationError>(),
        Some(&ScreenPublicationError::RequiredNativeUnavailable(
            ScreenPublicationExecutorFallbackReason::PlatformApiMismatch,
        ))
    );
    assert_eq!(
        telemetry.publication_path().as_deref(),
        Some("native_unavailable")
    );
    assert_eq!(
        lock(&telemetry.fallback_reason).as_deref(),
        Some("platform_api_mismatch")
    );
}

#[test]
fn passive_cpu_preview_cannot_overwrite_missing_native_renderer_diagnostics() {
    let frame = frame();
    let publication_source = source(&frame);
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let (mut input, _fixture) = MacosScreenCaptureFixture::renderer_authoritative_source(
        CaptureConfig::default(),
        admission,
    );
    input.set_screen_renderer_execution_state(ScreenRendererExecutionState::NativeUnavailable(
        ScreenNativeExecutionUnavailableReason::MissingTarget,
    ));

    let resolved = resolve_macos_publication_branch_with_telemetry(
        &publication_source,
        &cpu_demand(ScreenProcessingProfile::default()),
        &input.telemetry,
    )
    .expect("passive CPU preview demand resolves")
    .expect("configured macOS source owns passive CPU demand");
    assert!(matches!(
        resolved.descriptor().executor(),
        ScreenPublicationExecutor::Cpu
    ));
    assert_eq!(
        input.telemetry.publication_path().as_deref(),
        Some("native_unavailable")
    );
    assert_eq!(
        lock(&input.telemetry.fallback_reason).as_deref(),
        Some("missing_target")
    );
    input
        .refresh_platform_status()
        .expect("production diagnostics refresh");
    let diagnostics = input
        .source_status_handle()
        .expect("macOS source publishes status")
        .snapshot()
        .diagnostics
        .clone()
        .expect("macOS source publishes platform diagnostics");
    assert_eq!(
        diagnostics.payload()["publication_path"],
        "native_unavailable"
    );
    assert_eq!(diagnostics.payload()["fallback_reason"], "missing_target");
}

fn incompatible_target(id: u64) -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(NonZeroU64::new(id).expect("nonzero target")),
        PlatformGpuApi::Direct3d11,
        ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
            low_part: 7,
            high_part: 11,
        },
        NonZeroU32::new(16_384).expect("nonzero texture limit"),
        Arc::new(TestTargetPreparer),
    )
}

#[test]
fn stale_required_target_cannot_overwrite_successor_target_diagnostics() {
    let frame = frame();
    let source = source(&frame);
    let stale = incompatible_target(21);
    let current = target();
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::renderer_authoritative());
    telemetry.set_renderer_execution_state(ScreenRendererExecutionState::NativeReady(stale.id()));
    telemetry.set_renderer_execution_state(ScreenRendererExecutionState::NativeReady(current.id()));

    resolve_macos_publication_branch_with_telemetry(
        &source,
        &required_native_demand(&stale),
        &telemetry,
    )
    .expect_err("stale incompatible target still fails its own resolution");
    assert_eq!(telemetry.publication_path().as_deref(), Some("native"));
    assert!(lock(&telemetry.fallback_reason).is_none());
    assert_eq!(*lock(&telemetry.renderer_target), Some(current.id()));
}

#[test]
fn stale_required_target_cannot_overwrite_missing_target_diagnostics() {
    let frame = frame();
    let source = source(&frame);
    let stale = incompatible_target(22);
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::renderer_authoritative());
    telemetry.set_renderer_execution_state(ScreenRendererExecutionState::NativeReady(stale.id()));
    telemetry.set_renderer_execution_state(ScreenRendererExecutionState::NativeUnavailable(
        ScreenNativeExecutionUnavailableReason::MissingTarget,
    ));

    resolve_macos_publication_branch_with_telemetry(
        &source,
        &required_native_demand(&stale),
        &telemetry,
    )
    .expect_err("stale incompatible target still fails its own resolution");
    assert_eq!(
        telemetry.publication_path().as_deref(),
        Some("native_unavailable")
    );
    assert_eq!(
        lock(&telemetry.fallback_reason).as_deref(),
        Some("missing_target")
    );
    assert_eq!(*lock(&telemetry.renderer_target), None);
}

#[test]
fn every_extended_native_format_publishes_deferred_work_without_masquerading() {
    let mappings = [
        (
            MacosCapturePixelFormat::Argb2101010,
            CapturePixelFormat::Argb2101010,
        ),
        (
            MacosCapturePixelFormat::Rgba16Float,
            CapturePixelFormat::Rgba16Float,
        ),
        (
            MacosCapturePixelFormat::Yuv420VideoRange,
            CapturePixelFormat::Yuv420VideoRange,
        ),
        (
            MacosCapturePixelFormat::Yuv420FullRange,
            CapturePixelFormat::Yuv420FullRange,
        ),
        (
            MacosCapturePixelFormat::Yuv44410BiPlanar,
            CapturePixelFormat::Yuv44410BiPlanar,
        ),
    ];
    for (native, core) in mappings {
        assert_eq!(capture_pixel_format(native), core);
        let mut native_frame = (*frame()).clone();
        native_frame.pixel_format = native;
        let native_frame = Arc::new(native_frame);
        let mut native_source = source(&frame());
        native_source.pixel_format = native;
        let demand = native_demand_for_format(&target(), core);
        let resolved = resolve_macos_publication_branch(&native_source, &demand)
            .expect("extended native demand resolves")
            .expect("configured macOS source owns extended native demand");
        assert!(matches!(
            resolved.descriptor().executor(),
            ScreenPublicationExecutor::SourceNative(_)
        ));
        assert!(!macos_native_descriptor_is_identity(resolved.descriptor()));
        let (publication, _) = publish_native_fixture(&native_frame, &native_source, resolved);
        let ScreenBranchPayload::NativeWork(payload) = publication.payload() else {
            panic!("extended native source must publish deferred work");
        };
        assert_eq!(payload.source().format(), core);
        assert_eq!(
            payload.source().extent(),
            native_source.geometry.storage_extent()
        );
    }
}

#[test]
fn rec709_and_rec2020_transfer_metadata_remain_exact() {
    for (native, core) in [
        (
            MacosTransferFunction::Rec709,
            CaptureTransferFunction::Rec709,
        ),
        (
            MacosTransferFunction::Rec2020,
            CaptureTransferFunction::Rec2020,
        ),
    ] {
        let frame = frame_with_color(
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Rec2020,
                transfer: native,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            BGRA8,
            &[0, 0, 255, 255],
            None,
        );
        assert_eq!(
            capture_colorimetry(&frame)
                .expect("exact SDR transfer maps")
                .transfer_function(),
            core
        );
    }
}

#[test]
fn rgba16float_cpu_publication_matches_the_shared_scalar_oracle() {
    let color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Linear,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let headroom = 1_000.0 / 203.0;
    let delivered = MacosDeliveredFrameMetadata::new(
        MacosCapturePixelFormat::Rgba16Float,
        color,
        Some(203.0),
        Some(headroom),
    )
    .expect("extended-linear HDR delivery metadata is valid");
    let encoded = [0x00, 0x38, 0x00, 0x3c, 0x00, 0x40, 0x00, 0x3c];
    let native_frame = frame_with_color(color, RGBA16_FLOAT, &encoded, Some(delivered));
    let native_source = source(&native_frame);
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    exact.install_test_source(Some(native_source.clone()));
    let resolved =
        resolve_macos_publication_branch(&native_source, &cpu_demand(transition_profile(true)))
            .expect("extended-linear CPU demand resolves")
            .expect("configured source owns extended-linear CPU demand");
    let mut runtimes = Vec::new();
    let descriptor = commit_cpu_runtime(
        &mut builder,
        &exact,
        &native_source,
        resolved,
        &mut runtimes,
    );
    let captured_at = Instant::now() + Duration::from_millis(20);
    publish_scalar_frame(
        &exact,
        &mut runtimes,
        &native_source,
        &native_frame,
        captured_at,
    );
    let output = published_surface_bytes(&exact, &descriptor);
    let pipeline = descriptor.physical().color_pipeline();
    let prepared = PreparedLedToneMap::prepare(
        pipeline
            .effective_source()
            .expect("managed pipeline retains source"),
        pipeline
            .output()
            .try_known()
            .expect("managed output is known"),
        pipeline.calibration().expect("managed calibration exists"),
    )
    .expect("shared scalar oracle prepares");
    let expected = prepared.encode(prepared.decode_and_map_source([0.5, 1.0, 2.0, 1.0]));
    assert_eq!(&output[..4], &expected);
}

#[test]
fn malformed_native_planes_fail_before_cpu_publication() {
    let native_frame = frame();
    let native_source = source(&native_frame);
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    exact.install_test_source(Some(native_source.clone()));
    let resolved = resolve_macos_publication_branch(
        &native_source,
        &cpu_demand(ScreenProcessingProfile::default()),
    )
    .expect("CPU demand resolves")
    .expect("configured source owns CPU demand");
    let mut runtimes = Vec::new();
    let descriptor = commit_cpu_runtime(
        &mut builder,
        &exact,
        &native_source,
        resolved,
        &mut runtimes,
    );
    let mut malformed = (*native_frame).clone();
    let mut planes = malformed.planes.to_vec();
    planes[0].bytes_per_row = 1;
    malformed.planes = planes.into();
    let captured_at = Instant::now() + Duration::from_millis(20);
    let capture = native_cpu_capture_frame(
        &Arc::new(malformed.clone()),
        captured_at,
        captured_at + Duration::from_secs(1),
        &native_source,
        native_source.epoch.source_id.clone(),
    )
    .expect("malformed plane metadata does not alter native ownership envelope");
    assert!(
        publish_macos_scalar_exact(
            &malformed,
            &capture,
            &native_source,
            &exact,
            &mut runtimes,
            &MacosScreenRuntimeTelemetry::default(),
        )
        .is_err()
    );
    let hub = exact.hub().expect("fixture hub remains installed");
    let lease = hub
        .lease(&descriptor)
        .expect("committed branch has a lease");
    assert!(lease.read().is_none());
}

#[test]
fn every_retained_format_cpu_publication_matches_the_shared_scalar_oracle() {
    let sdr_rgb = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Srgb,
        transfer: MacosTransferFunction::Srgb,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let hdr_linear = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Linear,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let yuv_video = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Pq,
        matrix: Some(hypercolor_macos_capture::MacosYuvMatrix::Bt2020),
        range: MacosColorRange::Video,
        chroma_location: Some(hypercolor_macos_capture::MacosChromaLocation::Left),
    };
    let yuv_full = MacosCaptureColorimetry {
        transfer: MacosTransferFunction::Hlg,
        range: MacosColorRange::Full,
        chroma_location: Some(hypercolor_macos_capture::MacosChromaLocation::Center),
        ..yuv_video
    };
    let hdr_delivery = |format, color| {
        MacosDeliveredFrameMetadata::new(format, color, Some(203.0), Some(1_000.0 / 203.0))
            .expect("HDR delivery metadata is valid")
    };
    let bgra = frame_with_planes(
        sdr_rgb,
        BGRA8,
        &[(
            &[32, 64, 128, 255].repeat(8),
            MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
            16,
        )],
        None,
    );
    let packed_l10r = (3_u32 << 30) | (512 << 20) | (256 << 10) | 128;
    let l10r_bytes = packed_l10r.to_le_bytes().repeat(8);
    let l10r = frame_with_planes(
        hdr_linear,
        ARGB2101010,
        &[(
            &l10r_bytes,
            MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
            16,
        )],
        Some(hdr_delivery(
            MacosCapturePixelFormat::Argb2101010,
            hdr_linear,
        )),
    );
    let rgba16_pixel = [0x00, 0x38, 0x00, 0x3c, 0x00, 0x40, 0x00, 0x3c];
    let rgba16_bytes = rgba16_pixel.repeat(8);
    let rgba16 = frame_with_planes(
        hdr_linear,
        RGBA16_FLOAT,
        &[(
            &rgba16_bytes,
            MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
            32,
        )],
        Some(hdr_delivery(
            MacosCapturePixelFormat::Rgba16Float,
            hdr_linear,
        )),
    );
    let y_plane_video = vec![126; 8];
    let chroma_video = vec![96, 160, 96, 160];
    let yuv420v = frame_with_planes(
        yuv_video,
        YUV420_VIDEO_RANGE,
        &[
            (
                &y_plane_video,
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                4,
            ),
            (
                &chroma_video,
                MacosPixelExtent::new(2, 1).expect("fixture extent is valid"),
                4,
            ),
        ],
        Some(hdr_delivery(
            MacosCapturePixelFormat::Yuv420VideoRange,
            yuv_video,
        )),
    );
    let y_plane_full = vec![128; 8];
    let chroma_full = vec![96, 160, 96, 160];
    let yuv420f = frame_with_planes(
        yuv_full,
        YUV420_FULL_RANGE,
        &[
            (
                &y_plane_full,
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                4,
            ),
            (
                &chroma_full,
                MacosPixelExtent::new(2, 1).expect("fixture extent is valid"),
                4,
            ),
        ],
        Some(hdr_delivery(
            MacosCapturePixelFormat::Yuv420FullRange,
            yuv_full,
        )),
    );
    let yuv444_color = MacosCaptureColorimetry {
        chroma_location: Some(hypercolor_macos_capture::MacosChromaLocation::TopLeft),
        ..yuv_full
    };
    let y10 = (512_u16 << 6).to_le_bytes();
    let cb10 = (384_u16 << 6).to_le_bytes();
    let cr10 = (640_u16 << 6).to_le_bytes();
    let y444 = y10.repeat(8);
    let mut chroma444 = Vec::new();
    for _ in 0..8 {
        chroma444.extend_from_slice(&cb10);
        chroma444.extend_from_slice(&cr10);
    }
    let yuv444 = frame_with_planes(
        yuv444_color,
        YUV44410_FULL_RANGE,
        &[
            (
                &y444,
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                8,
            ),
            (
                &chroma444,
                MacosPixelExtent::new(4, 2).expect("fixture extent is valid"),
                16,
            ),
        ],
        Some(hdr_delivery(
            MacosCapturePixelFormat::Yuv44410BiPlanar,
            yuv444_color,
        )),
    );

    for frame in [bgra, l10r, rgba16, yuv420v, yuv420f, yuv444] {
        assert_scalar_publication_matches_oracle(&frame);
    }
}

fn assert_scalar_publication_matches_oracle(frame: &Arc<MacosCaptureFrame>) {
    let native_source = source(frame);
    let hdr = native_source.colorimetry.dynamic_range() == Some(CaptureDynamicRange::High);
    let mut builder = ScreenPlanBuilder::new();
    let exact = MacosExactPublicationShared::default();
    exact.install_hub(builder.publication_hub());
    exact.install_test_source(Some(native_source.clone()));
    let resolved =
        resolve_macos_publication_branch(&native_source, &cpu_demand(transition_profile(hdr)))
            .expect("native scalar CPU demand resolves")
            .expect("configured source owns native scalar demand");
    let mut runtimes = Vec::new();
    let descriptor = commit_cpu_runtime(
        &mut builder,
        &exact,
        &native_source,
        resolved,
        &mut runtimes,
    );
    let source_sample = frame
        .with_cpu_source(|samples| samples.sample_rgba32f(0, 0))
        .expect("native scalar source validates")
        .expect("first source sample decodes");
    let captured_at = Instant::now() + Duration::from_millis(20);
    publish_scalar_frame(&exact, &mut runtimes, &native_source, frame, captured_at);
    let output = published_surface_bytes(&exact, &descriptor);
    let pipeline = descriptor.physical().color_pipeline();
    let prepared = PreparedLedToneMap::prepare(
        pipeline
            .effective_source()
            .expect("managed pipeline retains source"),
        pipeline
            .output()
            .try_known()
            .expect("managed output is known"),
        pipeline.calibration().expect("managed calibration exists"),
    )
    .expect("shared scalar oracle prepares");
    assert_eq!(
        &output[..4],
        &prepared.encode(prepared.decode_and_map_source(source_sample))
    );
}

#[test]
fn reduced_rgba_demand_falls_back_until_native_reducer_exists() {
    let frame = frame();
    let source = source(&frame);
    let demand = reduced_native_demand(&target());
    let resolved = resolve_macos_publication_branch(&source, &demand)
        .expect("reduced demand resolves")
        .expect("configured macOS source owns reduced demand");
    assert!(matches!(
        resolved.descriptor().executor(),
        ScreenPublicationExecutor::Cpu
    ));

    let capable_target =
        target().with_color_capabilities(CpuReductionExecutor::supported_color_capabilities());
    let capable =
        resolve_macos_publication_branch(&source, &reduced_native_demand(&capable_target))
            .expect("capable reduced demand resolves")
            .expect("configured macOS source owns capable demand");
    assert!(matches!(
        capable.descriptor().executor(),
        ScreenPublicationExecutor::SourceNative(_)
    ));
    assert!(!macos_native_descriptor_is_identity(capable.descriptor()));
    let output_extent = capable.descriptor().geometry().output_extent();
    let (publication, _) = publish_native_fixture(&frame, &source, capable);
    let ScreenBranchPayload::NativeWork(payload) = publication.payload() else {
        panic!("reduced macOS native branch publishes deferred GPU work");
    };
    assert_eq!(payload.source().extent(), source.geometry.storage_extent());
    assert_ne!(payload.source().extent(), output_extent);
    assert_eq!(payload.source().format(), CapturePixelFormat::Bgra8);
    assert_eq!(payload.source_colorimetry().value(), source.colorimetry);
}

#[test]
fn processing_reconfiguration_preserves_the_native_capture_runtime() {
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let (mut input, fixture) =
        MacosScreenCaptureFixture::source(CaptureConfig::default(), admission);
    let native_source = source(&frame());
    input.exact.install_test_source(Some(native_source));
    fixture.control.set_active(true);
    let active_transitions = fixture.control.active_transitions.load(Ordering::Acquire);
    let worker_generation = input.worker_generation;
    let revision = input.screen_publication_resolution_revision();
    let mut config = input.config.clone();
    config.target_led_white_x = 0.3000;
    config.target_led_white_y = 0.3200;
    config.target_led_reference_white_nits = 180.0;
    config.target_led_peak_nits = 500.0;
    config.exposure_ev = 1.25;

    input
        .reconfigure_screen_processing(&config)
        .expect("valid calibration updates without rebuilding capture");

    assert_eq!(input.worker_generation, worker_generation);
    assert!(fixture.is_active());
    assert_eq!(
        fixture.control.active_transitions.load(Ordering::Acquire),
        active_transitions
    );
    assert_eq!(input.screen_publication_resolution_revision(), revision + 1);
    let resolved = input
        .resolve_screen_publication_branch(&RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenPublicationExecutorRequest::Cpu,
                ScreenExtentRequest::bounded(
                    NonZeroU32::new(2),
                    NonZeroU32::new(1),
                    super::super::ScreenUpscalePolicy::Never,
                ),
                ScreenAspectPolicy::Contain,
                Arc::new(ScreenProcessingProfile::default()),
            ),
            NonZeroU32::new(60).expect("nonzero cadence"),
        ))
        .expect("calibrated branch resolves")
        .expect("configured macOS source owns the demand");
    assert_eq!(
        resolved
            .descriptor()
            .physical()
            .color_pipeline()
            .calibration(),
        Some(
            LedToneMapCalibration::try_new(0.3000, 0.3200, 180.0, 500.0, 1.25)
                .expect("fixture calibration is valid")
        )
    );
}

#[test]
fn legacy_analysis_decimation_keeps_the_surface_near_the_analyzer_budget() {
    // Native 4K HDR display (retina 2x): must decimate hard enough to fit
    // the tone-map conversion inside the publication freshness budget.
    let native_4k = PixelExtent::new(4112, 2658).expect("valid extent");
    assert_eq!(legacy_analysis_decimation(native_4k), 7);
    // Surfaces already at or under the analyzer budget pass through exact.
    let analyzer_sized = PixelExtent::new(640, 480).expect("valid extent");
    assert_eq!(legacy_analysis_decimation(analyzer_sized), 1);
    let small_window = PixelExtent::new(320, 200).expect("valid extent");
    assert_eq!(legacy_analysis_decimation(small_window), 1);
    // The limiting axis wins: a wide-but-short surface decimates by width.
    let ultrawide = PixelExtent::new(5120, 400).expect("valid extent");
    assert_eq!(legacy_analysis_decimation(ultrawide), 8);
    // Every decimated sample position stays inside the source surface.
    for extent in [native_4k, analyzer_sized, small_window, ultrawide] {
        let step = legacy_analysis_decimation(extent);
        let last_x = (extent.width().div_ceil(step) - 1) * step;
        let last_y = (extent.height().div_ceil(step) - 1) * step;
        assert!(last_x < extent.width());
        assert!(last_y < extent.height());
    }
}

#[test]
fn invalid_processing_reconfiguration_preserves_the_active_profile() {
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    let (mut input, fixture) =
        MacosScreenCaptureFixture::source(CaptureConfig::default(), admission);
    fixture.control.set_active(true);
    let revision = input.screen_publication_resolution_revision();
    let previous = input.config.clone();
    let mut invalid = previous.clone();
    invalid.exposure_ev = f32::INFINITY;

    assert!(input.reconfigure_screen_processing(&invalid).is_err());
    assert_eq!(input.config, previous);
    assert_eq!(input.screen_publication_resolution_revision(), revision);
    assert!(fixture.is_active());
}
