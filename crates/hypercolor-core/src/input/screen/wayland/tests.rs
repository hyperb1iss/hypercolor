use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    AnalysisExchange, CaptureCallbackMetrics, CapturedScreenSnapshot, ChunkDropReason, CopyStats,
    DoubleBuffer, NegotiatedFormat, RestoreTokenSink, SharedSettings, SpaChunkView, SpaVideoFormat,
    WaylandAnalysisState, WaylandCaptureUserData, WaylandSourceMetadata, WaylandTopologySignature,
    WorkerCommand, convert_packed_to_rgba, decode_chunk, dispatch_worker_command,
    publish_unexpected_exit_status,
};
use crate::input::screen::{
    CaptureConfig, CaptureRotation, CaptureSourceId, LegacyScreenSnapshot, PhysicalOrigin,
    PixelExtent, PixelRect, analyze_legacy_screen_frame,
};
use crate::input::{SourceIssue, SourceKind, SourceState, SourceStatusReporter};

fn settings(session_generation: u64) -> Arc<SharedSettings> {
    Arc::new(SharedSettings {
        config: Mutex::new(CaptureConfig::default()),
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
) -> LegacyScreenSnapshot {
    let plane_len = usize::try_from(width)
        .expect("test width fits usize")
        .checked_mul(usize::try_from(height).expect("test height fits usize"))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("test plane length fits usize");
    let mut plane = state.plane_pool.acquire(plane_len);
    plane.resize(plane_len, fill);
    let frame = state
        .capture_frame(
            Instant::now(),
            width,
            height,
            None,
            CaptureRotation::Identity,
            plane.freeze(),
        )
        .expect("test frame is valid");
    analyze_legacy_screen_frame(&mut state.analyzer, frame)
        .expect("legacy analysis accepts canonical test geometry")
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
    );

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
    );
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
    );
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
    );
    let stale = capture_legacy(&mut retiring, 4, 2, 1);

    let active_session = settings.begin_session();
    let mut active = WaylandAnalysisState::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(active_session, physical_origin, logical_extent),
        CaptureConfig::default(),
    );
    let current = capture_legacy(&mut active, 2, 1, 2);
    assert!(settings.publish_snapshot(&latest, current));
    assert!(!settings.publish_snapshot(&latest, stale));

    let published = latest
        .lock()
        .expect("latest snapshot mutex is healthy")
        .clone()
        .expect("successor snapshot remains published");
    assert_eq!(
        published.legacy.frame().metadata().session_generation,
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
    let mut buffers = DoubleBuffer::with_capacity(16);
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
    let mut buffers = DoubleBuffer::with_capacity(mapped.len());
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
    let mut buffers = DoubleBuffer::with_capacity(mapped.len());
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
    let mut buffers = DoubleBuffer::with_capacity(mapped.len());
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
    let mut buffers = DoubleBuffer::with_capacity(mapped.len());

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
    );
    let crop = PixelRect::new(1, 0, 2, 2).expect("test crop is valid");
    let mut plane = analysis.plane_pool.acquire(64);
    plane.resize(64, 0);
    let frame = analysis
        .capture_frame(
            Instant::now(),
            4,
            4,
            Some(crop),
            CaptureRotation::Clockwise270,
            plane.freeze(),
        )
        .expect("raw frame accepts pending geometry metadata");

    assert_eq!(frame.metadata().geometry.crop(), Some(crop));
    assert_eq!(
        frame.metadata().geometry.rotation(),
        CaptureRotation::Clockwise270
    );
}

#[test]
fn negotiated_format_change_rebuilds_callback_planes_outside_decode() {
    let exchange = Arc::new(AnalysisExchange::default());
    let metrics = Arc::new(CaptureCallbackMetrics::default());
    let mut callback = WaylandCaptureUserData::new(exchange, Arc::clone(&metrics));
    assert!(callback.set_negotiated_format(NegotiatedFormat {
        width: 2,
        height: 2,
        format: SpaVideoFormat::Rgb,
    }));
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

    assert!(callback.set_negotiated_format(NegotiatedFormat {
        width: 4,
        height: 2,
        format: SpaVideoFormat::Rgba,
    }));
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
fn stopped_analysis_wait_wakes_and_discards_queued_pixels() {
    let mapped = [7_u8; 16];
    let mut buffers = DoubleBuffer::with_capacity(mapped.len());
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
        waiter.wait_for_latest(
            Instant::now() + Duration::from_secs(30),
            &AtomicBool::new(false),
        )
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
    let mut buffers = DoubleBuffer::with_capacity(4);
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

    let latest = exchange
        .wait_for_latest(Instant::now(), &AtomicBool::new(false))
        .expect("latest frame is immediately eligible");
    assert_eq!(latest.bytes(), &[3; 4]);
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
    );
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
    );
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
    let activate = WorkerCommand::SetActive(true);
    assert!(!dispatch_worker_command(
        &failed_tx,
        &failed_demand,
        &activate,
    ));

    let (replacement_tx, _replacement_rx) = pipewire::channel::channel();
    let replacement_demand = AtomicBool::new(false);
    assert!(dispatch_worker_command(
        &replacement_tx,
        &replacement_demand,
        &activate,
    ));
    assert!(replacement_demand.load(Ordering::Acquire));
}

#[test]
fn callback_buffer_reuse_preserves_length_without_zero_filling() {
    let mut buffers = DoubleBuffer::with_capacity(16);
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
    let mut buffers = DoubleBuffer::with_capacity(mapped.len());
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
