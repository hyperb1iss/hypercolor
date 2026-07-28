use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    AdoptionAuthority, AdoptionWaitError, AnalysisEvent, AnalysisExchange, CaptureCallbackMetrics,
    CapturedScreenSnapshot, ChunkDropReason, CopyStats, DoubleBuffer, NegotiatedFormat,
    NegotiatedPipeWireFormat, PendingPipeWireAdoption, PipeWireFormatAcknowledgment,
    PipeWireFormatRequest, PipeWireFormatState, PipeWireLoopExit, RestoreTokenSink,
    SettingsDecision, SharedSettings, SpaChunkView, SpaVideoFormat, UnavailablePark,
    WaylandAnalysisState, WaylandCaptureUserData, WaylandScreenCaptureInput, WaylandSourceMetadata,
    WaylandTopologySignature, WorkerCommand, build_format_params, commit_if_authorized,
    convert_packed_to_rgba, decode_chunk, fence_previous_publication, initial_worker_demand,
    park_unavailable_worker, publish_unexpected_exit_status, request_active_worker_demand,
    settle_pipewire_restoration, unavailable_format_outcome, wait_for_adoption_result,
    worker_demand_epoch, worker_demanded,
};
use crate::input::screen::{
    AnalyzedScreenSnapshot, CaptureColorimetry, CaptureConfig, CaptureFrameError, CaptureRotation,
    CaptureSourceId, MAX_REPRESENTABLE_CAPTURE_FPS, PhysicalOrigin, PixelExtent, PixelRect,
    ScreenCaptureDemand, analyze_screen_frame,
};
use crate::input::{SourceIssue, SourceKind, SourceState, SourceStatusReporter};

fn settings(session_generation: u64) -> Arc<SharedSettings> {
    Arc::new(SharedSettings {
        config: Mutex::new(CaptureConfig::default()),
        demand: Mutex::new(active_demand()),
        generation: 0.into(),
        frame_generation: 0.into(),
        topology_generation: 0.into(),
        topology: Mutex::new(None),
        session_generation: session_generation.into(),
        expected_epoch: Mutex::new(None),
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
    PipeWireFormatRequest::new(extent(width, height), target_fps)
        .expect("test PipeWire format request is valid")
}

fn negotiated_format(width: u32, height: u32, target_fps: u32) -> NegotiatedPipeWireFormat {
    NegotiatedPipeWireFormat {
        frame: NegotiatedFormat {
            width,
            height,
            format: SpaVideoFormat::Rgba,
        },
        framerate: pipewire::spa::utils::Fraction {
            num: target_fps,
            denom: 1,
        },
    }
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
            format_bytes: vec![u8::try_from(id).unwrap_or_default()],
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
    let frame = state
        .capture_frame(
            Instant::now(),
            width,
            height,
            None,
            CaptureRotation::Identity,
            plane.freeze(),
            CaptureColorimetry::SRGB,
        )
        .expect("test frame is valid");
    analyze_screen_frame(&mut state.analyzer, frame)
        .expect("screen analysis accepts canonical test geometry")
}

fn rgba_view<'a>(
    data: &'a [u8],
    offset: usize,
    size: usize,
    stride: i32,
    width: u32,
    height: u32,
) -> SpaChunkView<'a> {
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
fn physical_topology_persists_across_storage_resize_and_session_restart() {
    let settings = settings(7);
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
    let physical_origin = PhysicalOrigin { x: -1920, y: 0 };
    let mut first_worker = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(7, physical_origin, extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");

    let first = capture_legacy(&mut first_worker, 4, 2, 1);
    let resized = capture_legacy(&mut first_worker, 2, 1, 2);
    assert_eq!(first.frame().metadata().topology_generation, 1);
    assert_eq!(resized.frame().metadata().topology_generation, 1);
    assert_eq!(
        resized.frame().metadata().geometry.native_extent(),
        extent(4, 2)
    );
    assert_eq!(
        resized.frame().metadata().geometry.storage_extent(),
        extent(2, 1)
    );

    let next_session = settings.begin_session();
    let mut successor = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(next_session, physical_origin, extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let restarted = capture_legacy(&mut successor, 1, 1, 3);
    assert_eq!(restarted.frame().metadata().topology_generation, 1);
    assert_eq!(
        restarted.frame().metadata().geometry.native_extent(),
        extent(4, 2)
    );

    let mut moved_source = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::new(Mutex::new(None)),
        source(
            next_session,
            PhysicalOrigin { x: 0, y: -1080 },
            extent(1920, 1080),
        ),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let moved = capture_legacy(&mut moved_source, 2, 1, 4);
    assert_eq!(moved.frame().metadata().topology_generation, 2);
    assert_eq!(
        moved.frame().metadata().geometry.native_extent(),
        extent(2, 1)
    );
}

#[test]
fn stale_worker_cannot_overwrite_the_successor_snapshot() {
    let settings = settings(9);
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
    let physical_origin = PhysicalOrigin::default();
    let logical_extent = extent(1920, 1080);
    let mut retiring = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(9, physical_origin, logical_extent),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let stale = capture_legacy(&mut retiring, 4, 2, 1);

    let active_session = settings.begin_session();
    let mut active = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(active_session, physical_origin, logical_extent),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let current = capture_legacy(&mut active, 2, 1, 2);
    assert!(settings.publish_snapshot(&latest, current));
    assert!(!settings.publish_snapshot(&latest, stale));

    let published = latest
        .lock()
        .expect("latest snapshot mutex is healthy")
        .clone()
        .expect("successor snapshot remains published");
    assert_eq!(
        published
            .analysis
            .geometry_frame()
            .metadata()
            .session_generation,
        active_session
    );
    assert_eq!(published.generation, 1);
}

#[test]
fn retired_worker_cannot_read_or_update_successor_settings() {
    let settings = settings(31);
    settings
        .config
        .lock()
        .expect("capture config mutex is healthy")
        .restore_token = Some("retiring".to_owned());
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
    settings
        .config
        .lock()
        .expect("capture config mutex is healthy")
        .restore_token = Some("successor".to_owned());
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink_calls_for_callback = Arc::clone(&sink_calls);
    let sink: RestoreTokenSink = Arc::new(move |_| {
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
        settings
            .config
            .lock()
            .expect("capture config mutex is healthy")
            .restore_token
            .as_deref(),
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
        settings
            .config
            .lock()
            .expect("capture config mutex is healthy")
            .restore_token
            .as_deref(),
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
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
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
    assert!(settings.expected_epoch.try_lock().is_err());
    let cancellation_settings = Arc::clone(&settings);
    let cancellation_latest = Arc::clone(&latest);
    let cancellation_flag = Arc::clone(&cancel);
    let cancellation_generation = Arc::clone(&active_session_generation);
    let (cancel_started_tx, cancel_started_rx) = mpsc::sync_channel(0);
    let cancellation = thread::spawn(move || {
        cancel_started_tx
            .send(())
            .expect("test observes cancellation invocation");
        cancellation_settings.cancel_worker_session(
            &cancellation_latest,
            &cancellation_flag,
            &cancellation_generation,
        );
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
        Arc::new(Mutex::new(None)),
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
        Arc::new(Mutex::new(None)),
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
        .expect_err("unknown encoded samples must not reach legacy averaging");
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
        current_format_bytes: vec![1],
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
    let mut state = PipeWireFormatState {
        current,
        current_format_bytes: vec![1],
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
        current_format_bytes: vec![1],
        current_acknowledged: true,
        pending: Some(pending),
        restoring: None,
    };

    assert_eq!(
        state.acknowledgment(negotiated_format(1280, 720, 60)),
        PipeWireFormatAcknowledgment::Rejected
    );
    assert_eq!(
        state.acknowledgment(negotiated_format(1920, 1080, 59)),
        PipeWireFormatAcknowledgment::Rejected
    );
    assert!(state.cancel(10).is_none());
    assert_eq!(state.pending.as_ref().map(|pending| pending.id), Some(11));

    let rejected = state.cancel(11).expect("matching epoch owns adoption");
    assert!(rejected.authority.cancel());
    assert_eq!(
        state.begin_restoring(rejected, "fixture rejection".to_owned()),
        vec![1]
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
    settle_pipewire_restoration(&mut callback, &state, negotiated_format(640, 480, 30).frame)
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
        current_format_bytes: vec![1],
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
            cancelled.store(true, Ordering::Release)
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
fn exact_pipewire_format_has_no_local_cadence_or_extent_range() {
    let requested = extent(7680, 4320);
    let bytes = build_format_params(10_000, requested)
        .expect("representable high cadence and extent serialize");
    let pod = pipewire::spa::pod::Pod::from_bytes(&bytes).expect("format pod deserializes");
    let mut info = pipewire::spa::param::video::VideoInfoRaw::default();
    info.parse(pod).expect("exact video format parses");

    assert_eq!(info.size().width, requested.width());
    assert_eq!(info.size().height, requested.height());
    assert_eq!(info.framerate().num, 10_000);
    assert_eq!(info.framerate().denom, 1);
}

#[test]
fn pipewire_format_uses_the_shared_scheduler_boundary_without_a_product_cap() {
    let requested = extent(7680, 4320);
    let request = PipeWireFormatRequest::new(requested, MAX_REPRESENTABLE_CAPTURE_FPS)
        .expect("scheduler boundary cadence is admitted");

    assert_eq!(request.extent, requested);
    assert_eq!(request.target_fps, MAX_REPRESENTABLE_CAPTURE_FPS);
    assert!(PipeWireFormatRequest::new(requested, MAX_REPRESENTABLE_CAPTURE_FPS + 1).is_err());
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
    let latest = Arc::new(Mutex::new(None));
    let mut state = WaylandAnalysisState::new(
        settings,
        latest,
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
            &AtomicBool::new(false),
        ) {
            Some(AnalysisEvent::Frame(frame)) => Some(frame),
            Some(AnalysisEvent::Adoption(_)) | None => None,
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

    let Some(AnalysisEvent::Frame(latest)) =
        exchange.wait_for_event(Instant::now(), &AtomicBool::new(false))
    else {
        panic!("latest frame is immediately eligible");
    };
    assert_eq!(latest.bytes(), &[3; 4]);
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
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
    let mut analysis = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(20, PhysicalOrigin::default(), extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let snapshot = capture_legacy(&mut analysis, 2, 1, 4);
    assert!(settings.publish_snapshot(&latest, snapshot));

    fence_previous_publication(
        &mut latest
            .lock()
            .expect("snapshot mutex is healthy before commit"),
    );

    assert!(latest.lock().expect("snapshot mutex is healthy").is_none());
}

#[test]
fn terminal_session_invalidation_clears_only_its_snapshot() {
    let settings = settings(21);
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
    let mut analysis = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(21, PhysicalOrigin::default(), extent(1920, 1080)),
        CaptureConfig::default(),
        active_demand(),
    )
    .expect("test analysis extent allocates");
    let snapshot = capture_legacy(&mut analysis, 2, 1, 4);
    assert!(settings.publish_snapshot(&latest, snapshot));
    assert!(settings.invalidate_session(&latest, 21));
    assert!(latest.lock().expect("snapshot mutex is healthy").is_none());

    let successor_session = settings.begin_session();
    let mut successor = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
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
    assert!(settings.publish_snapshot(&latest, successor_snapshot));
    assert!(!settings.invalidate_session(&latest, 21));
    assert!(latest.lock().expect("snapshot mutex is healthy").is_some());
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

    assert_eq!(metrics.snapshot().dropped_frames, 5);
}

#[test]
fn failed_command_recovery_reapplies_activation_to_replacement_worker() {
    let (failed_tx, failed_rx) = pipewire::channel::channel();
    drop(failed_rx);
    let failed_demand = AtomicBool::new(false);
    let demand = active_demand();
    failed_demand.store(demand.is_active(), Ordering::Release);
    assert!(failed_tx.send(WorkerCommand::SetDemand(demand)).is_err());

    let (replacement_tx, _replacement_rx) = pipewire::channel::channel();
    let replacement_demand = AtomicBool::new(false);
    replacement_demand.store(demand.is_active(), Ordering::Release);
    assert!(
        replacement_tx
            .send(WorkerCommand::SetDemand(demand))
            .is_ok()
    );
    assert!(replacement_demand.load(Ordering::Acquire));
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
