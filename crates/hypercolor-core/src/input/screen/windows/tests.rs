use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_windows_capture::{
    CaptureError, DisplayRotation, ReductionPath, ReductionTelemetry,
};

use super::{
    ActiveCaptureEpoch, CapturePublication, CaptureWorker, WindowsScreenCaptureInput,
    WorkerCommand, capture_epoch, capture_geometry, capture_issue, record_capture_health,
    settle_inactive_capture,
};
use crate::input::screen::{
    CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage, CaptureFrame,
    CaptureFrameError, CaptureFrameMetadata, CapturePixelFormat, CaptureRotation, CaptureStorage,
    CaptureTransferFunction, CpuCaptureStorage, PhysicalOrigin, PixelExtent, RawCaptureSurface,
    ScreenCaptureDemand,
};
use crate::input::status::{ScreenCaptureReductionPath, SourceDiagnostics};
use crate::input::traits::InputSource;
use crate::input::{SourceKind, SourceState, SourceStatusReporter};

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
            .worker
            .as_ref()
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
    assert!(publication.activate(old_activity.clone()));
    assert!(publication.publish(&old_activity, "old frame"));
    publication.fence_activity(2);
    publication.fence_activity(3);

    assert!(!publication.activate(old_activity.clone()));
    assert!(!publication.publish(&old_activity, "in-flight old frame"));
    assert!(publication.active.is_none());
    assert!(publication.latest.is_none());
    assert!(publication.activate(reactivated.clone()));
    assert!(publication.publish(&reactivated, "reactivated frame"));
}

#[test]
fn deactivated_worker_is_reused_when_capture_reactivates() {
    let mut input = WindowsScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("idle capture source starts");
    assert!(input.worker.is_none());

    input
        .set_capture_demand_state(active_demand())
        .expect("first activation starts the worker");
    let first_activity = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, first_activity);
    let first_thread = input
        .worker
        .as_ref()
        .and_then(|worker| worker.join_handle.as_ref())
        .expect("active capture owns a worker thread")
        .thread()
        .id();
    let first_generation = input.settings.session_generation.load(Ordering::Acquire);

    input
        .set_capture_demand_state(ScreenCaptureDemand::Inactive)
        .expect("deactivation idles the worker");
    let inactive_activity = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, inactive_activity);
    assert!(
        input.worker.is_some(),
        "acknowledged idle capture retains worker ownership"
    );

    input
        .set_capture_demand_state(active_demand())
        .expect("reactivation reuses the idle worker");
    let reactivated_activity = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, reactivated_activity);
    let reactivated_thread = input
        .worker
        .as_ref()
        .and_then(|worker| worker.join_handle.as_ref())
        .expect("reactivated capture still owns a worker thread")
        .thread()
        .id();

    assert_eq!(reactivated_thread, first_thread);
    assert_eq!(
        input.settings.session_generation.load(Ordering::Acquire),
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
    input.worker = Some(CaptureWorker {
        command_tx,
        exit_rx,
        join_handle: Some(join_handle),
        cancel,
        processed_activity_generation: Arc::new(AtomicU64::new(0)),
    });

    input
        .set_capture_demand_state(active_demand())
        .expect("activation replaces the disconnected worker");
    let activity_generation = input.settings.activity_generation.load(Ordering::Acquire);
    wait_for_activity(&input, activity_generation);
    let replacement_thread = input
        .worker
        .as_ref()
        .and_then(|worker| worker.join_handle.as_ref())
        .expect("activation owns the replacement worker")
        .thread()
        .id();

    assert!(exited.load(Ordering::Acquire));
    assert_ne!(replacement_thread, disconnected_thread);
    assert_eq!(
        input.settings.session_generation.load(Ordering::Acquire),
        1,
        "only the replacement worker allocates a capture session"
    );

    input.stop();
}

#[test]
fn source_fence_prevents_an_in_flight_worker_from_reactivating_old_frames() {
    let old_source = active_epoch("display:old", 3, 7, 1);
    let mut new_source = active_epoch("display:new", 3, 7, 1);
    new_source.source_generation = 1;
    let mut publication = CapturePublication::default();

    assert!(publication.activate(old_source.clone()));
    assert!(publication.publish(&old_source, "old frame"));
    publication.fence_source(1);

    assert!(!publication.activate(old_source.clone()));
    assert!(!publication.publish(&old_source, "late old frame"));
    assert!(publication.active.is_none());
    assert!(publication.latest.is_none());
    assert!(publication.activate(new_source.clone()));
    assert!(publication.publish(&new_source, "new frame"));
}

#[test]
fn publication_rejects_frames_from_a_replaced_source() {
    let first = active_epoch("display:first", 3, 7, 1);
    let replacement = active_epoch("display:replacement", 3, 7, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()));
    assert!(publication.publish(&first, "first frame"));
    assert_eq!(publication.latest, Some("first frame"));

    assert!(publication.activate(replacement.clone()));
    assert!(publication.latest.is_none());
    assert!(!publication.publish(&first, "stale frame"));
    assert!(publication.publish(&replacement, "replacement frame"));
    assert_eq!(publication.latest, Some("replacement frame"));
}

#[test]
fn publication_rejects_frames_from_a_replaced_topology() {
    let first = active_epoch("display:main", 3, 7, 1);
    let replacement = active_epoch("display:main", 4, 7, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()));
    assert!(publication.publish(&first, "first frame"));
    assert!(publication.activate(replacement.clone()));

    assert!(publication.latest.is_none());
    assert!(!publication.publish(&first, "stale topology frame"));
    assert!(publication.publish(&replacement, "replacement frame"));
}

#[test]
fn publication_rejects_frames_from_a_replaced_worker_session() {
    let first = active_epoch("display:main", 3, 7, 1);
    let restarted = active_epoch("display:main", 3, 8, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()));
    assert!(publication.publish(&first, "first frame"));
    assert!(publication.activate(restarted.clone()));

    assert!(publication.latest.is_none());
    assert!(!publication.publish(&first, "stale worker frame"));
    assert!(publication.publish(&restarted, "restarted frame"));
}

#[test]
fn publication_rejects_frames_from_a_rebuilt_duplication_interface() {
    let first = active_epoch("display:main", 3, 7, 1);
    let rebuilt = active_epoch("display:main", 3, 7, 2);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(first.clone()));
    assert!(publication.publish(&first, "first frame"));
    assert!(publication.activate(rebuilt.clone()));

    assert!(publication.latest.is_none());
    assert!(!publication.publish(&first, "access-lost frame"));
    assert!(publication.publish(&rebuilt, "rebuilt frame"));
}

#[test]
fn cleared_publication_fails_closed_until_a_new_epoch_is_activated() {
    let active = active_epoch("display:main", 3, 7, 1);
    let mut publication = CapturePublication::default();

    assert!(publication.activate(active.clone()));
    assert!(publication.publish(&active, "frame"));
    publication.clear();

    assert!(publication.active.is_none());
    assert!(publication.latest.is_none());
    assert!(!publication.publish(&active, "late frame"));
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
            color_space: CaptureColorSpace::Unknown,
            transfer_function: CaptureTransferFunction::Unknown,
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
    ] {
        let issue = capture_issue(&error);
        assert_eq!(&*issue.code, expected);
        assert!(issue.retryable);
    }
}
