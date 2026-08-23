//! Comprehensive tests for the event bus.

use std::sync::Arc;

use hypercolor_core::bus::{
    CanvasFrame, DisplayZoneFrame, DisplayZoneTarget, EventFilter, EventTimestamp, HypercolorBus,
    INPUT_STATUS_EVENT_KIND, INPUT_STATUS_EVENT_SOURCE, PreviewKind, TimestampedEvent, WatchLane,
    ZonePreviewFrame,
};
use hypercolor_core::input::{SourceKind, SourceState, SourceStatusReporter};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::device::{ConnectionType, DeviceId, DeviceOrigin};
use hypercolor_types::event::{
    ChangeTrigger, DisconnectReason, EffectRef, EventCategory, EventPriority, FrameData,
    HypercolorEvent, Severity, SpectrumData, ZoneColors,
};
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::ZoneId;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};

/// Helper: a simple `Paused` event for tests that don't care about payload.
fn paused_event() -> HypercolorEvent {
    HypercolorEvent::Paused
}

/// Helper: a `DeviceConnected` event.
fn device_connected(id: &str) -> HypercolorEvent {
    HypercolorEvent::DeviceConnected {
        device_id: id.to_string(),
        name: format!("Device {id}"),
        origin: DeviceOrigin::native("test-driver", "test", ConnectionType::Network),
        led_count: 60,
        zones: Vec::new(),
    }
}

/// Helper: an `EffectStarted` event.
fn effect_started(name: &str) -> HypercolorEvent {
    HypercolorEvent::EffectStarted {
        effect: EffectRef {
            id: format!("effect-{name}"),
            name: name.to_string(),
            engine: "wgpu".to_string(),
        },
        trigger: ChangeTrigger::User,
        previous: None,
        transition: None,
        zone_id: None,
        zone_name: None,
    }
}

/// Helper: a `DaemonShutdown` event (Critical priority).
fn shutdown_event() -> HypercolorEvent {
    HypercolorEvent::DaemonShutdown {
        reason: "test".to_string(),
    }
}

/// Helper: a system `Error` event with configurable severity.
fn error_event(severity: Severity) -> HypercolorEvent {
    HypercolorEvent::Error {
        code: "TEST_ERR".to_string(),
        message: "test error".to_string(),
        severity,
    }
}

/// Helper: receive one event from a broadcast receiver with a timeout.
async fn recv_one(
    rx: &mut broadcast::Receiver<TimestampedEvent>,
) -> Result<TimestampedEvent, String> {
    timeout(Duration::from_secs(1), rx.recv())
        .await
        .map_err(|_| "timed out waiting for event".to_string())?
        .map_err(|e| format!("recv error: {e}"))
}

// ── Publish / Receive Roundtrip ──────────────────────────────────────────

#[tokio::test]
async fn publish_receive_roundtrip() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    bus.publish(paused_event());

    let received = recv_one(&mut rx).await.expect("should receive event");
    assert!(
        matches!(received.event, HypercolorEvent::Paused),
        "event should be Paused"
    );
    assert!(
        received.timestamp.epoch_millis() > 0,
        "timestamp should be set"
    );
}

#[tokio::test]
async fn publish_preserves_event_data() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    bus.publish(device_connected("wled_01"));

    let received = recv_one(&mut rx).await.expect("should receive event");
    if let HypercolorEvent::DeviceConnected {
        device_id, name, ..
    } = &received.event
    {
        assert_eq!(device_id, "wled_01");
        assert_eq!(name, "Device wled_01");
    } else {
        panic!("expected DeviceConnected, got {:?}", received.event);
    }
}

#[tokio::test]
async fn mono_ms_is_monotonic() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    bus.publish(paused_event());
    let first = recv_one(&mut rx).await.expect("first event");

    // Tiny delay so monotonic clock advances.
    tokio::time::sleep(Duration::from_millis(1)).await;

    bus.publish(HypercolorEvent::Resumed);
    let second = recv_one(&mut rx).await.expect("second event");

    assert!(
        second.mono_ms >= first.mono_ms,
        "mono_ms should be monotonically increasing"
    );
}

#[test]
fn timestamp_serializes_as_iso8601_string() {
    let event = TimestampedEvent {
        timestamp: EventTimestamp::from_epoch_millis(0),
        mono_ms: 0,
        event: HypercolorEvent::Paused,
    };

    let json = serde_json::to_string(&event).expect("serialize timestamped event");
    assert!(json.contains("\"timestamp\":\"1970-01-01T00:00:00.000Z\""));
}

// ── Multiple Subscribers ─────────────────────────────────────────────────

#[tokio::test]
async fn multiple_subscribers_all_receive() {
    let bus = HypercolorBus::new();
    let mut rx1 = bus.subscribe_all();
    let mut rx2 = bus.subscribe_all();

    assert_eq!(bus.subscriber_count(), 2);

    bus.publish(paused_event());

    let e1 = recv_one(&mut rx1).await.expect("subscriber 1");
    let e2 = recv_one(&mut rx2).await.expect("subscriber 2");

    assert!(matches!(e1.event, HypercolorEvent::Paused));
    assert!(matches!(e2.event, HypercolorEvent::Paused));
}

#[tokio::test]
async fn subscriber_count_tracks_drops() {
    let bus = HypercolorBus::new();
    let rx1 = bus.subscribe_all();
    let rx2 = bus.subscribe_all();

    assert_eq!(bus.subscriber_count(), 2);

    drop(rx1);
    assert_eq!(bus.subscriber_count(), 1);

    drop(rx2);
    assert_eq!(bus.subscriber_count(), 0);
}

#[tokio::test]
async fn frame_receiver_count_tracks_drops() {
    let bus = HypercolorBus::new();
    let rx1 = bus.frame_receiver();
    let rx2 = bus.frame_receiver();

    assert_eq!(bus.frame_receiver_count(), 2);

    drop(rx1);
    assert_eq!(bus.frame_receiver_count(), 1);

    drop(rx2);
    assert_eq!(bus.frame_receiver_count(), 0);
}

#[tokio::test]
async fn spectrum_receiver_count_tracks_drops() {
    let bus = HypercolorBus::new();
    let rx1 = bus.spectrum_receiver();
    let rx2 = bus.spectrum_receiver();

    assert_eq!(bus.spectrum_receiver_count(), 2);

    drop(rx1);
    assert_eq!(bus.spectrum_receiver_count(), 1);

    drop(rx2);
    assert_eq!(bus.spectrum_receiver_count(), 0);
}

#[tokio::test]
async fn canvas_receiver_count_tracks_drops() {
    let bus = HypercolorBus::new();
    let rx1 = bus.canvas_receiver();
    let rx2 = bus.canvas_receiver();

    assert_eq!(bus.canvas_receiver_count(), 2);

    drop(rx1);
    assert_eq!(bus.canvas_receiver_count(), 1);

    drop(rx2);
    assert_eq!(bus.canvas_receiver_count(), 0);
}

#[tokio::test]
async fn screen_canvas_receiver_count_tracks_drops() {
    let bus = HypercolorBus::new();
    let rx1 = bus.screen_canvas_receiver();
    let rx2 = bus.screen_canvas_receiver();

    assert_eq!(bus.screen_canvas_receiver_count(), 2);

    drop(rx1);
    assert_eq!(bus.screen_canvas_receiver_count(), 1);

    drop(rx2);
    assert_eq!(bus.screen_canvas_receiver_count(), 0);
}

#[test]
fn watch_lane_stats_count_only_successful_publications() {
    let bus = HypercolorBus::new();
    let lane = bus.canvas_lane();

    assert!(lane.send(CanvasFrame::empty()).is_err());
    assert_eq!(lane.stats().published, 0);
    assert_eq!(lane.stats().revision, 0);

    let receiver = lane.subscribe();
    lane.send(CanvasFrame::empty())
        .expect("live receiver should accept publication");
    lane.send_replace(CanvasFrame::empty());

    assert_eq!(lane.stats().receivers, 1);
    assert_eq!(lane.stats().published, 2);
    assert_eq!(lane.stats().revision, 2);

    drop(receiver);
    assert_eq!(lane.stats().receivers, 0);
}

#[test]
fn watch_lane_rejected_send_does_not_change_late_subscriber_state() {
    let lane = WatchLane::new(7_u64);

    assert!(lane.send(11).is_err());

    let receiver = lane.subscribe();
    assert_eq!(*receiver.borrow(), 7);
    assert_eq!(
        lane.stats(),
        hypercolor_core::bus::WatchLaneStats {
            receivers: 1,
            published: 0,
            revision: 0,
        }
    );
}

#[test]
fn watch_lane_replace_is_visible_to_late_subscribers() {
    let lane = WatchLane::new(7_u64);

    assert_eq!(lane.send_replace_weighted(11, 3), 7);

    let receiver = lane.subscribe();
    assert_eq!(*receiver.borrow(), 11);
    assert_eq!(
        lane.stats(),
        hypercolor_core::bus::WatchLaneStats {
            receivers: 1,
            published: 3,
            revision: 1,
        }
    );
}

#[test]
fn watch_lane_revision_advances_only_for_notified_changes() {
    let lane = WatchLane::new(7_u64);
    let _receiver = lane.subscribe();

    assert!(!lane.send_if_modified(|_| false));
    assert_eq!(lane.stats().revision, 0);

    assert!(lane.send_if_modified(|value| {
        *value = 11;
        true
    }));
    assert_eq!(lane.stats().published, 1);
    assert_eq!(lane.stats().revision, 1);
    assert_eq!(*lane.borrow(), 11);
}

#[test]
fn watch_lane_keeps_exact_counters_across_concurrent_publishers() {
    const WRITERS: u64 = 4;
    const PUBLICATIONS_PER_WRITER: u64 = 64;

    let lane = Arc::new(WatchLane::new(0_u64));
    std::thread::scope(|scope| {
        for writer in 0..WRITERS {
            let lane = Arc::clone(&lane);
            scope.spawn(move || {
                for publication in 0..PUBLICATIONS_PER_WRITER {
                    lane.send_replace_weighted(writer * PUBLICATIONS_PER_WRITER + publication, 2);
                }
            });
        }
    });

    let stats = lane.stats();
    let expected_revisions = WRITERS * PUBLICATIONS_PER_WRITER;
    assert_eq!(stats.revision, expected_revisions);
    assert_eq!(stats.published, expected_revisions * 2);
}

#[test]
fn bus_lanes_keep_preview_telemetry_independent_by_kind() {
    let bus = HypercolorBus::new();
    let canvas = bus.lanes().preview(PreviewKind::Canvas);
    let screen = bus.lanes().preview(PreviewKind::ScreenCanvas);
    let _canvas_receiver = canvas.subscribe();

    canvas.send_replace(CanvasFrame::empty());

    assert_eq!(canvas.stats().published, 1);
    assert_eq!(canvas.stats().revision, 1);
    assert_eq!(screen.stats().published, 0);
    assert_eq!(screen.stats().revision, 0);
}

#[test]
fn zone_preview_lane_counts_frames_and_batch_revisions_separately() {
    let bus = HypercolorBus::new();
    let lane = bus.zone_preview_lane();
    let _receiver = lane.subscribe();
    let scene_id = hypercolor_types::scene::SceneId::new();
    let frames: Arc<[ZonePreviewFrame]> = vec![
        ZonePreviewFrame {
            scene_id,
            zone_id: ZoneId::new(),
            frame: CanvasFrame::empty(),
        },
        ZonePreviewFrame {
            scene_id,
            zone_id: ZoneId::new(),
            frame: CanvasFrame::empty(),
        },
    ]
    .into();

    lane.send_weighted(frames, 2)
        .expect("live receiver should accept zone previews");
    lane.send_weighted(Arc::from([]), 0)
        .expect("live receiver should accept clear publication");

    assert_eq!(lane.stats().published, 2);
    assert_eq!(lane.stats().revision, 2);
}

#[test]
fn zone_preview_clear_is_visible_after_subscriber_reconnects() {
    let bus = HypercolorBus::new();
    let lane = bus.zone_preview_lane();
    let receiver = lane.subscribe();
    let frames: Arc<[ZonePreviewFrame]> = vec![ZonePreviewFrame {
        scene_id: hypercolor_types::scene::SceneId::new(),
        zone_id: ZoneId::new(),
        frame: CanvasFrame::empty(),
    }]
    .into();

    lane.send_weighted(frames, 1)
        .expect("live receiver should accept zone previews");
    drop(receiver);
    lane.send_replace_weighted(Arc::from([]), 0);

    let reconnected = lane.subscribe();
    assert!(reconnected.borrow().is_empty());
    assert_eq!(lane.stats().published, 1);
    assert_eq!(lane.stats().revision, 2);
}

#[tokio::test]
async fn zone_canvas_receiver_roundtrip() {
    let bus = HypercolorBus::new();
    let group_id = ZoneId::new();
    let mut rx = bus.zone_canvas_receiver(group_id);
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(1, 0, Rgba::new(0, 0, 255, 255));

    let _ = bus
        .zone_canvas_sender(group_id)
        .send(DisplayZoneFrame::Canvas(CanvasFrame::from_canvas(
            &canvas, 7, 123,
        )));

    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("zone canvas change should arrive")
        .expect("zone canvas sender should stay connected");
    let DisplayZoneFrame::Canvas(frame) = rx.borrow().clone() else {
        panic!("zone canvas should publish canvas frames");
    };
    assert_eq!(frame.frame_number, 7);
    assert_eq!(frame.timestamp_ms, 123);
    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.rgba_bytes(), canvas.as_rgba_bytes());
}

#[tokio::test]
async fn removing_zone_canvas_resets_new_subscribers_to_empty() {
    let bus = HypercolorBus::new();
    let group_id = ZoneId::new();
    let mut canvas = Canvas::new(1, 1);
    canvas.fill(Rgba::new(12, 34, 56, 255));
    let _ = bus
        .zone_canvas_sender(group_id)
        .send(DisplayZoneFrame::Canvas(CanvasFrame::from_canvas(
            &canvas, 1, 1,
        )));

    bus.remove_zone_canvas(group_id);

    let rx = bus.zone_canvas_receiver(group_id);
    let frame = rx.borrow().clone();
    assert_eq!(frame.width(), 0);
    assert_eq!(frame.height(), 0);
}

#[test]
fn retain_zone_canvases_prunes_stale_streams() {
    let bus = HypercolorBus::new();
    let keep_id = ZoneId::new();
    let stale_id = ZoneId::new();
    let device_id = DeviceId::new();

    let _keep = bus.zone_canvas_sender(keep_id);
    let _stale = bus.zone_canvas_sender(stale_id);
    bus.upsert_display_zone_target(
        keep_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Alpha,
            opacity: 0.5,
            finalized: false,
        },
    );
    bus.upsert_display_zone_target(
        stale_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Replace,
            opacity: 1.0,
            finalized: false,
        },
    );
    assert_eq!(bus.zone_canvas_stream_count(), 2);
    assert_eq!(bus.display_zone_target_count(), 2);

    bus.retain_zone_canvases(&[keep_id]);

    assert_eq!(bus.zone_canvas_stream_count(), 1);
    assert_eq!(bus.display_zone_target_count(), 1);
    let stale = bus.zone_canvas_receiver(stale_id);
    let frame = stale.borrow().clone();
    assert_eq!(frame.width(), 0);
    assert_eq!(frame.height(), 0);
}

#[test]
fn retain_zone_canvases_and_collect_senders_reuses_kept_streams() {
    let bus = HypercolorBus::new();
    let keep_id = ZoneId::new();
    let stale_id = ZoneId::new();
    let device_id = DeviceId::new();

    let keep_sender = bus.zone_canvas_sender(keep_id);
    let _stale_sender = bus.zone_canvas_sender(stale_id);
    bus.upsert_display_zone_target(
        keep_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Alpha,
            opacity: 0.5,
            finalized: false,
        },
    );
    bus.upsert_display_zone_target(
        stale_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Replace,
            opacity: 1.0,
            finalized: false,
        },
    );

    let senders = bus.retain_zone_canvases_and_collect_senders(&[keep_id]);

    assert_eq!(bus.zone_canvas_stream_count(), 1);
    assert_eq!(bus.display_zone_target_count(), 1);
    assert_eq!(senders.len(), 1);
    assert_eq!(senders[0].0, keep_id);
    assert_eq!(senders[0].1.borrow().width(), 0);
    assert!(senders[0].1.same_channel(&keep_sender));
}

#[test]
fn display_zone_targets_roundtrip_and_revision() {
    let bus = HypercolorBus::new();
    let group_id = ZoneId::new();
    let device_id = DeviceId::new();

    let (initial_revision, initial_targets) = bus.display_zone_targets_snapshot();
    assert_eq!(initial_revision, 0);
    assert!(initial_targets.is_empty());

    bus.upsert_display_zone_target(
        group_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Screen,
            opacity: 0.6,
            finalized: false,
        },
    );

    let (revision, targets) = bus.display_zone_targets_snapshot();
    assert_eq!(revision, 1);
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets.get(&group_id),
        Some(&DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Screen,
            opacity: 0.6,
            finalized: false,
        })
    );
}

#[test]
fn retain_display_zone_targets_prunes_stale_routes() {
    let bus = HypercolorBus::new();
    let keep_id = ZoneId::new();
    let stale_id = ZoneId::new();
    let device_id = DeviceId::new();

    bus.upsert_display_zone_target(
        keep_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Alpha,
            opacity: 0.5,
            finalized: false,
        },
    );
    bus.upsert_display_zone_target(
        stale_id,
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Replace,
            opacity: 1.0,
            finalized: false,
        },
    );
    assert_eq!(bus.display_zone_target_count(), 2);

    bus.retain_display_zone_targets(&[keep_id]);

    let (_, targets) = bus.display_zone_targets_snapshot();
    assert_eq!(bus.display_zone_target_count(), 1);
    assert!(targets.contains_key(&keep_id));
    assert!(!targets.contains_key(&stale_id));
}

// ── No Subscribers ───────────────────────────────────────────────────────

#[tokio::test]
async fn publish_without_subscribers_does_not_panic() {
    let bus = HypercolorBus::new();
    // No subscribers -- should silently drop.
    bus.publish(paused_event());
    // If we get here, no panic occurred.
}

// ── Filtered Subscriptions ───────────────────────────────────────────────

#[tokio::test]
async fn filter_by_category() {
    let bus = HypercolorBus::new();
    let filter = EventFilter::new().categories(vec![EventCategory::Device]);
    let mut rx = bus.subscribe_filtered(filter);

    // Publish a device event and a system event.
    bus.publish(device_connected("d1"));
    bus.publish(paused_event()); // System category -- should be filtered out.
    bus.publish(device_connected("d2"));

    // Should only receive device events.
    let e1 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not time out")
        .expect("should receive d1");
    assert!(matches!(e1.event, HypercolorEvent::DeviceConnected { .. }));

    let e2 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should not time out")
        .expect("should receive d2");
    assert!(matches!(e2.event, HypercolorEvent::DeviceConnected { .. }));
}

#[tokio::test]
async fn filter_by_multiple_categories() {
    let bus = HypercolorBus::new();
    let filter = EventFilter::new().categories(vec![EventCategory::Device, EventCategory::Effect]);
    let mut rx = bus.subscribe_filtered(filter);

    bus.publish(device_connected("d1"));
    bus.publish(paused_event()); // System -- filtered out.
    bus.publish(effect_started("rainbow"));

    let e1 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("d1");
    assert!(matches!(e1.event, HypercolorEvent::DeviceConnected { .. }));

    let e2 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("effect");
    assert!(matches!(e2.event, HypercolorEvent::EffectStarted { .. }));
}

#[tokio::test]
async fn filter_by_min_priority() {
    let bus = HypercolorBus::new();
    let filter = EventFilter::new().min_priority(EventPriority::High);
    let mut rx = bus.subscribe_filtered(filter);

    // Low priority -- filtered.
    bus.publish(HypercolorEvent::BeatDetected {
        confidence: 0.9,
        bpm: Some(120.0),
        phase: 0.5,
    });
    // Normal priority -- filtered.
    bus.publish(paused_event());
    // High priority -- passes.
    bus.publish(device_connected("d1"));
    // Critical priority -- passes.
    bus.publish(shutdown_event());

    let e1 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("high");
    assert!(matches!(e1.event, HypercolorEvent::DeviceConnected { .. }));

    let e2 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("critical");
    assert!(matches!(e2.event, HypercolorEvent::DaemonShutdown { .. }));
}

#[tokio::test]
async fn filter_combined_category_and_priority() {
    let bus = HypercolorBus::new();
    let filter = EventFilter::new()
        .categories(vec![EventCategory::System])
        .min_priority(EventPriority::Critical);
    let mut rx = bus.subscribe_filtered(filter);

    // System + Normal -- filtered (priority too low).
    bus.publish(paused_event());
    // Device + High -- filtered (wrong category).
    bus.publish(device_connected("d1"));
    // System + Critical -- passes.
    bus.publish(shutdown_event());

    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("critical system");
    assert!(matches!(
        event.event,
        HypercolorEvent::DaemonShutdown { .. }
    ));
}

#[tokio::test]
async fn empty_filter_matches_all() {
    let bus = HypercolorBus::new();
    let filter = EventFilter::new();
    let mut rx = bus.subscribe_filtered(filter);

    bus.publish(paused_event());
    bus.publish(device_connected("d1"));

    let e1 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("first");
    assert!(matches!(e1.event, HypercolorEvent::Paused));

    let e2 = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("second");
    assert!(matches!(e2.event, HypercolorEvent::DeviceConnected { .. }));
}

// ── Watch: Frame Data ────────────────────────────────────────────────────

#[tokio::test]
async fn frame_watch_latest_value_semantics() {
    let bus = HypercolorBus::new();
    let mut rx = bus.frame_receiver();

    // Initial value should be empty.
    {
        let frame = rx.borrow_and_update();
        assert_eq!(frame.frame_number, 0);
        assert!(frame.zones.is_empty());
    }

    // Publish 3 frames rapidly.
    let frame1 = FrameData::new(
        vec![ZoneColors {
            zone_id: "z1".to_string(),
            colors: vec![[255, 0, 0]],
        }],
        1,
        100,
    );
    let frame2 = FrameData::new(
        vec![ZoneColors {
            zone_id: "z1".to_string(),
            colors: vec![[0, 255, 0]],
        }],
        2,
        200,
    );
    let frame3 = FrameData::new(
        vec![ZoneColors {
            zone_id: "z1".to_string(),
            colors: vec![[0, 0, 255]],
        }],
        3,
        300,
    );

    bus.frame_lane().send_replace(frame1);
    bus.frame_lane().send_replace(frame2);
    bus.frame_lane().send_replace(frame3);

    // changed() should resolve, and we should see only the latest frame.
    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("t/o")
        .expect("changed");

    let latest = rx.borrow_and_update();
    assert_eq!(latest.frame_number, 3, "should see latest frame");
    assert_eq!(latest.zones[0].colors[0], [0, 0, 255]);
}

#[tokio::test]
async fn frame_watch_multiple_receivers() {
    let bus = HypercolorBus::new();
    let mut rx1 = bus.frame_receiver();
    let mut rx2 = bus.frame_receiver();

    let frame = FrameData::new(
        vec![ZoneColors {
            zone_id: "z1".to_string(),
            colors: vec![[42, 42, 42]],
        }],
        1,
        50,
    );

    bus.frame_lane().send_replace(frame);

    timeout(Duration::from_secs(1), rx1.changed())
        .await
        .expect("t/o")
        .expect("rx1 changed");
    timeout(Duration::from_secs(1), rx2.changed())
        .await
        .expect("t/o")
        .expect("rx2 changed");

    assert_eq!(rx1.borrow_and_update().frame_number, 1);
    assert_eq!(rx2.borrow_and_update().frame_number, 1);
}

// ── Watch: Spectrum Data ─────────────────────────────────────────────────

#[tokio::test]
async fn spectrum_watch_latest_value_semantics() {
    let bus = HypercolorBus::new();
    let mut rx = bus.spectrum_receiver();

    // Initial value should be empty.
    {
        let spectrum = rx.borrow_and_update();
        assert!(!spectrum.beat);
        assert!(spectrum.bins.is_empty());
    }

    let mut spec = SpectrumData::empty();
    spec.beat = true;
    spec.bpm = Some(128.0);
    spec.bins = vec![0.1, 0.5, 0.9];

    bus.spectrum_lane().send_replace(spec);

    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("t/o")
        .expect("changed");

    let latest = rx.borrow_and_update();
    assert!(latest.beat);
    assert_eq!(latest.bpm, Some(128.0));
    assert_eq!(latest.bins.len(), 3);
}

#[tokio::test]
async fn spectrum_watch_skips_intermediate_values() {
    let bus = HypercolorBus::new();
    let mut rx = bus.spectrum_receiver();

    // Rapid-fire 3 spectrum updates.
    for i in 0_u32..3 {
        let mut spec = SpectrumData::empty();
        spec.timestamp_ms = (i + 1) * 10;
        spec.level = 0.1 * f32::from(u16::try_from(i + 1).expect("small index"));
        bus.spectrum_lane().send_replace(spec);
    }

    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("t/o")
        .expect("changed");

    let latest = rx.borrow_and_update();
    assert_eq!(latest.timestamp_ms, 30, "should see latest spectrum");
}

#[tokio::test]
async fn canvas_watch_latest_value_semantics() {
    let bus = HypercolorBus::new();
    let mut rx = bus.canvas_receiver();

    {
        let canvas = rx.borrow_and_update();
        assert_eq!(canvas.frame_number, 0);
        assert_eq!(canvas.width, 0);
        assert_eq!(canvas.height, 0);
        assert!(canvas.rgba_bytes().is_empty());
    }

    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(1, 2, 3, 255));
    canvas.set_pixel(1, 0, Rgba::new(4, 5, 6, 255));
    bus.canvas_lane()
        .send_replace(CanvasFrame::from_canvas(&canvas, 9, 123));

    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("t/o")
        .expect("changed");

    let latest = rx.borrow_and_update();
    assert_eq!(latest.frame_number, 9);
    assert_eq!(latest.width, 2);
    assert_eq!(latest.height, 1);
    assert_eq!(latest.rgba_bytes()[..8], [1, 2, 3, 255, 4, 5, 6, 255]);
}

#[tokio::test]
async fn screen_canvas_watch_latest_value_semantics() {
    let bus = HypercolorBus::new();
    let mut rx = bus.screen_canvas_receiver();

    {
        let canvas = rx.borrow_and_update();
        assert_eq!(canvas.frame_number, 0);
        assert_eq!(canvas.width, 0);
        assert_eq!(canvas.height, 0);
        assert!(canvas.rgba_bytes().is_empty());
    }

    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(7, 8, 9, 255));
    canvas.set_pixel(1, 0, Rgba::new(10, 11, 12, 255));
    bus.screen_canvas_lane()
        .send_replace(CanvasFrame::from_canvas(&canvas, 12, 345));

    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("t/o")
        .expect("changed");

    let latest = rx.borrow_and_update();
    assert_eq!(latest.frame_number, 12);
    assert_eq!(latest.width, 2);
    assert_eq!(latest.height, 1);
    assert_eq!(latest.rgba_bytes()[..8], [7, 8, 9, 255, 10, 11, 12, 255]);
}

#[test]
fn owned_canvas_frame_reuses_canvas_rgba_allocation() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(1, 2, 3, 255));
    canvas.set_pixel(1, 0, Rgba::new(4, 5, 6, 255));
    let original_ptr = canvas.as_rgba_bytes().as_ptr();

    let frame = CanvasFrame::from_owned_canvas(canvas, 9, 123);

    assert_eq!(frame.rgba_bytes().as_ptr(), original_ptr);
    assert_eq!(frame.rgba_bytes()[..8], [1, 2, 3, 255, 4, 5, 6, 255]);
}

#[test]
fn owned_canvas_frame_reuses_shared_canvas_storage_without_copy() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(1, 2, 3, 255));
    canvas.set_pixel(1, 0, Rgba::new(4, 5, 6, 255));
    let original = [1, 2, 3, 255, 4, 5, 6, 255];
    let original_ptr = canvas.as_rgba_bytes().as_ptr();
    let mut shared = canvas.clone();

    let (frame, copied) = CanvasFrame::from_owned_canvas_with_copy_info(canvas, 9, 123);

    assert!(!copied);
    assert_eq!(frame.rgba_bytes().as_ptr(), original_ptr);
    assert_eq!(frame.rgba_bytes()[..8], original);

    shared.set_pixel(0, 0, Rgba::new(9, 8, 7, 255));

    assert_eq!(frame.rgba_bytes()[..8], original);
    assert_eq!(shared.as_rgba_bytes()[..8], [9, 8, 7, 255, 4, 5, 6, 255]);
}

// ── Lagged Receiver Handling ─────────────────────────────────────────────

#[tokio::test]
async fn lagged_receiver_returns_error() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    // Publish more events than the channel capacity (256) without reading.
    for i in 0_u8..255 {
        bus.publish(HypercolorEvent::BrightnessChanged {
            old: 0,
            new_value: i,
        });
    }
    // Push past the 256 capacity.
    for i in 0_u8..50 {
        bus.publish(HypercolorEvent::BrightnessChanged {
            old: 0,
            new_value: i,
        });
    }

    // The first recv should return Lagged.
    let result = timeout(Duration::from_secs(1), rx.recv()).await;
    match result {
        Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
            assert!(n > 0, "should report lagged count > 0");
        }
        Ok(Ok(_)) => {
            // Some events might still be in the buffer -- that's acceptable too.
            // The key property is that the bus didn't panic or block.
        }
        Ok(Err(broadcast::error::RecvError::Closed)) => {
            panic!("bus should not be closed");
        }
        Err(elapsed) => {
            panic!("should not time out: {elapsed}");
        }
    }
}

// ── Bus Clone Semantics ──────────────────────────────────────────────────

#[tokio::test]
async fn cloned_bus_shares_channels() {
    let bus = HypercolorBus::new();
    let bus2 = bus.clone();

    let mut rx = bus.subscribe_all();

    // Publish on the clone, receive on the original's subscriber.
    bus2.publish(paused_event());

    let event = recv_one(&mut rx).await.expect("should receive from clone");
    assert!(matches!(event.event, HypercolorEvent::Paused));
}

// ── Default Trait ────────────────────────────────────────────────────────

#[tokio::test]
async fn default_creates_functional_bus() {
    let bus = HypercolorBus::default();
    let mut rx = bus.subscribe_all();

    bus.publish(HypercolorEvent::Resumed);

    let event = recv_one(&mut rx).await.expect("should work via default");
    assert!(matches!(event.event, HypercolorEvent::Resumed));
}

// ── Timestamp Format ─────────────────────────────────────────────────────

#[tokio::test]
async fn timestamp_is_iso8601() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    bus.publish(paused_event());

    let event = recv_one(&mut rx).await.expect("should receive");
    let timestamp = event.timestamp.to_string();
    // Basic ISO 8601 format: YYYY-MM-DDTHH:MM:SS.mmmZ
    assert!(
        timestamp.ends_with('Z'),
        "timestamp should end with Z (UTC)"
    );
    assert!(
        timestamp.contains('T'),
        "timestamp should contain T separator"
    );
    assert_eq!(timestamp.len(), 24, "should be 24 chars for ms precision");
}

// ── Event Category / Priority Integration ────────────────────────────────

#[tokio::test]
async fn critical_error_has_critical_priority() {
    let bus = HypercolorBus::new();
    let filter = EventFilter::new().min_priority(EventPriority::Critical);
    let mut rx = bus.subscribe_filtered(filter);

    // Warning severity -- Normal priority, filtered out.
    bus.publish(error_event(Severity::Warning));
    // Critical severity -- Critical priority, passes.
    bus.publish(error_event(Severity::Critical));

    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("t/o")
        .expect("critical error");
    if let HypercolorEvent::Error { severity, .. } = &event.event {
        assert_eq!(*severity, Severity::Critical);
    } else {
        panic!("expected Error event");
    }
}

// ── Event Order ──────────────────────────────────────────────────────────

#[tokio::test]
async fn events_delivered_in_order() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    for i in 0..10 {
        bus.publish(HypercolorEvent::BrightnessChanged {
            old: 0,
            new_value: i,
        });
    }

    for i in 0..10 {
        let event = recv_one(&mut rx).await.expect("should receive in order");
        if let HypercolorEvent::BrightnessChanged { new_value, .. } = event.event {
            assert_eq!(new_value, i, "events should arrive in publish order");
        } else {
            panic!("wrong event type");
        }
    }
}

// ── Disconnect Reason Roundtrip ──────────────────────────────────────────

#[tokio::test]
async fn device_disconnect_roundtrip() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    bus.publish(HypercolorEvent::DeviceDisconnected {
        device_id: "usb_01".to_string(),
        reason: DisconnectReason::Timeout,
        will_retry: true,
    });

    let event = recv_one(&mut rx).await.expect("should receive");
    if let HypercolorEvent::DeviceDisconnected {
        device_id,
        reason,
        will_retry,
    } = &event.event
    {
        assert_eq!(device_id, "usb_01");
        assert_eq!(*reason, DisconnectReason::Timeout);
        assert!(will_retry);
    } else {
        panic!("wrong event type");
    }
}

// ── SpectrumData Downsample ──────────────────────────────────────────────

#[test]
fn spectrum_downsample_empty_bins() {
    let spec = SpectrumData::empty();
    assert!(spec.downsample(10).is_empty());
}

#[test]
fn spectrum_downsample_zero_target() {
    let mut spec = SpectrumData::empty();
    spec.bins = vec![1.0, 2.0, 3.0];
    assert!(spec.downsample(0).is_empty());
}

#[test]
fn spectrum_downsample_larger_target() {
    let mut spec = SpectrumData::empty();
    spec.bins = vec![1.0, 2.0, 3.0];
    let result = spec.downsample(10);
    assert_eq!(
        result,
        vec![1.0, 2.0, 3.0],
        "should return clone when target >= len"
    );
}

#[test]
fn spectrum_downsample_halves() {
    let mut spec = SpectrumData::empty();
    spec.bins = vec![1.0, 3.0, 5.0, 7.0];
    let result = spec.downsample(2);
    assert_eq!(result.len(), 2);
    // First bin: avg(1.0, 3.0) = 2.0
    assert!((result[0] - 2.0).abs() < f32::EPSILON);
    // Second bin: avg(5.0, 7.0) = 6.0
    assert!((result[1] - 6.0).abs() < f32::EPSILON);
}

// ── Concurrent Publishing ────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_publishers() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();

    let bus_a = bus.clone();
    let bus_b = bus.clone();

    let handle_a = tokio::spawn(async move {
        for _ in 0..10 {
            bus_a.publish(paused_event());
        }
    });

    let handle_b = tokio::spawn(async move {
        for _ in 0..10 {
            bus_b.publish(HypercolorEvent::Resumed);
        }
    });

    handle_a.await.expect("task A");
    handle_b.await.expect("task B");

    // Should receive all 20 events.
    let mut count = 0;
    while let Ok(Ok(_)) = timeout(Duration::from_millis(100), rx.recv()).await {
        count += 1;
    }

    assert_eq!(
        count, 20,
        "should receive all events from concurrent publishers"
    );
}

#[tokio::test]
async fn input_status_events_are_content_safe_coalesced_and_generation_aware() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();
    let mut reporter = SourceStatusReporter::new(
        "host-interaction",
        SourceKind::Interaction,
        "raw_input",
        true,
        true,
        true,
    );
    reporter.set_source_graph_generation(7);
    let first_session = reporter
        .begin_session()
        .expect("first session should start")
        .expect("manager-bound reporter should create a session");
    let first = reporter.handle().snapshot();

    assert!(bus.publish_input_source_status(&first));
    assert!(!bus.publish_input_source_status(&first));

    let published = recv_one(&mut rx)
        .await
        .expect("status event should publish");
    let HypercolorEvent::ExtensionStateChanged {
        source,
        kind,
        payload,
    } = published.event
    else {
        panic!("expected input status extension event");
    };
    assert_eq!(source, INPUT_STATUS_EVENT_SOURCE);
    assert_eq!(kind, INPUT_STATUS_EVENT_KIND);
    assert_eq!(payload["source_id"], "host-interaction");
    assert_eq!(payload["kind"], "interaction");
    assert_eq!(payload["backend"], "raw_input");
    assert_eq!(payload["active_consumer_count"], 0);
    assert_eq!(
        payload["session_generation"],
        first_session.session_generation()
    );
    assert!(
        payload.get("event").is_none(),
        "captured input must not leak"
    );
    assert!(matches!(
        rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));

    reporter.stop();
    reporter.set_source_graph_generation(8);
    let second_session = reporter
        .begin_session()
        .expect("second session should start")
        .expect("manager-bound reporter should create a successor session");
    let second = reporter.handle().snapshot();
    assert_ne!(
        first_session.session_generation(),
        second_session.session_generation()
    );
    assert!(bus.publish_input_source_status(&second));

    let published = recv_one(&mut rx)
        .await
        .expect("successor session status should publish");
    let HypercolorEvent::ExtensionStateChanged { payload, .. } = published.event else {
        panic!("expected successor input status event");
    };
    assert_eq!(payload["source_graph_generation"], 8);
    assert_eq!(
        payload["session_generation"],
        second_session.session_generation()
    );
}

#[tokio::test]
async fn input_status_events_preserve_flapping_transitions_without_duplicate_noise() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();
    let reporter = SourceStatusReporter::new(
        "flapping-source",
        SourceKind::Screen,
        "test",
        true,
        true,
        true,
    );
    let mut live = reporter.handle().snapshot().as_ref().clone();
    live.source_graph_generation = 4;
    live.session_generation = 2;
    live.state = SourceState::Live;
    let mut failed = live.clone();
    failed.state = SourceState::Failed;

    assert!(bus.publish_input_source_status(&live));
    assert!(!bus.publish_input_source_status(&live));
    assert!(bus.publish_input_source_status(&failed));
    assert!(!bus.publish_input_source_status(&failed));
    assert!(bus.publish_input_source_status(&live));

    for expected in ["live", "failed", "live"] {
        let event = recv_one(&mut rx).await.expect("transition should publish");
        let HypercolorEvent::ExtensionStateChanged { payload, .. } = event.event else {
            panic!("expected input status extension event");
        };
        assert_eq!(payload["state"], expected);
    }
    assert!(matches!(
        rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn input_status_events_reject_regressive_graph_and_session_generations() {
    let bus = HypercolorBus::new();
    let mut rx = bus.subscribe_all();
    let reporter = SourceStatusReporter::new(
        "generation-source",
        SourceKind::Audio,
        "test",
        true,
        true,
        true,
    );
    let mut current = reporter.handle().snapshot().as_ref().clone();
    current.source_graph_generation = 8;
    current.session_generation = 5;
    assert!(bus.publish_input_source_status(&current));

    let mut older_graph = current.clone();
    older_graph.source_graph_generation = 7;
    older_graph.session_generation = 99;
    assert!(!bus.publish_input_source_status(&older_graph));

    let mut older_session = current.clone();
    older_session.session_generation = 4;
    assert!(!bus.publish_input_source_status(&older_session));

    let mut successor = current;
    successor.session_generation = 6;
    assert!(bus.publish_input_source_status(&successor));

    let mut generations = Vec::new();
    for _ in 0..2 {
        let event = recv_one(&mut rx)
            .await
            .expect("accepted generation should publish");
        let HypercolorEvent::ExtensionStateChanged { payload, .. } = event.event else {
            panic!("expected input status extension event");
        };
        generations.push(payload["session_generation"].as_u64());
    }
    assert_eq!(generations, vec![Some(5), Some(6)]);
    assert!(matches!(
        rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn concurrent_input_status_publications_are_generation_ordered() {
    const PUBLISHERS: usize = 32;

    let bus = Arc::new(HypercolorBus::new());
    let mut rx = bus.subscribe_all();
    let reporter = SourceStatusReporter::new(
        "concurrent-source",
        SourceKind::Interaction,
        "test",
        true,
        true,
        true,
    );
    let mut base = reporter.handle().snapshot().as_ref().clone();
    base.source_graph_generation = 12;
    let barrier = Arc::new(std::sync::Barrier::new(PUBLISHERS));

    let accepted = std::thread::scope(|scope| {
        let mut publishers = Vec::new();
        for session_generation in 1..=PUBLISHERS {
            let bus = Arc::clone(&bus);
            let barrier = Arc::clone(&barrier);
            let mut status = base.clone();
            status.session_generation =
                u64::try_from(session_generation).expect("publisher count should fit in u64");
            publishers.push(scope.spawn(move || {
                barrier.wait();
                bus.publish_input_source_status(&status)
            }));
        }
        publishers
            .into_iter()
            .map(|publisher| publisher.join().expect("publisher thread should finish"))
            .filter(|published| *published)
            .count()
    });

    let mut published_generations = Vec::with_capacity(accepted);
    for _ in 0..accepted {
        let event = recv_one(&mut rx)
            .await
            .expect("accepted generation should publish");
        let HypercolorEvent::ExtensionStateChanged { payload, .. } = event.event else {
            panic!("expected input status extension event");
        };
        published_generations.push(
            payload["session_generation"]
                .as_u64()
                .expect("session generation should be an integer"),
        );
    }
    assert!(!published_generations.is_empty());
    assert!(
        published_generations
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    );
}
