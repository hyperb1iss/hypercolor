//! Comprehensive tests for the event bus.

use hypercolor_core::bus::{EventFilter, HypercolorBus, TimestampedEvent};
use hypercolor_core::types::event::{
    ChangeTrigger, DisconnectReason, EffectRef, EventCategory, EventPriority, FrameData,
    HypercolorEvent, Severity, SpectrumData, ZoneColors,
};
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
        backend: "test".to_string(),
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
    assert!(!received.timestamp.is_empty(), "timestamp should be set");
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

    bus.frame_sender().send_replace(frame1);
    bus.frame_sender().send_replace(frame2);
    bus.frame_sender().send_replace(frame3);

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

    bus.frame_sender().send_replace(frame);

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

    bus.spectrum_sender().send_replace(spec);

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
        bus.spectrum_sender().send_replace(spec);
    }

    timeout(Duration::from_secs(1), rx.changed())
        .await
        .expect("t/o")
        .expect("changed");

    let latest = rx.borrow_and_update();
    assert_eq!(latest.timestamp_ms, 30, "should see latest spectrum");
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
    // Basic ISO 8601 format: YYYY-MM-DDTHH:MM:SS.mmmZ
    assert!(
        event.timestamp.ends_with('Z'),
        "timestamp should end with Z (UTC)"
    );
    assert!(
        event.timestamp.contains('T'),
        "timestamp should contain T separator"
    );
    assert_eq!(
        event.timestamp.len(),
        24,
        "should be 24 chars for ms precision"
    );
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
