//! Tests for the per-device watch channel bolted onto `DisplayFrameRuntime`
//! to back the `display_preview` WS relay. The registry's core storage
//! semantics are covered by the API integration tests — these focus on
//! notification: subscribers see new frames, missing devices observe
//! closure, and late subscribers get the latest snapshot as initial value.

use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_daemon::display_frames::{DisplayFrameRuntime, DisplayFrameSnapshot};
use hypercolor_types::device::DeviceId;
use uuid::Uuid;

fn make_device() -> DeviceId {
    DeviceId::from_uuid(Uuid::now_v7())
}

fn snapshot(frame_number: u64) -> DisplayFrameSnapshot {
    DisplayFrameSnapshot {
        jpeg_data: Arc::new(vec![0xff; 16]),
        width: 480,
        height: 480,
        circular: true,
        frame_number,
        captured_at: SystemTime::now(),
    }
}

#[tokio::test]
async fn subscribe_delivers_latest_snapshot_as_initial_value() {
    let mut runtime = DisplayFrameRuntime::new();
    let device = make_device();
    runtime.set_frame(device, snapshot(7));

    let rx = runtime.subscribe(device);
    let initial = rx.borrow().clone();
    let Some(frame) = initial else {
        panic!("late subscriber should see the latest snapshot as initial value");
    };
    assert_eq!(frame.frame_number, 7);
}

#[tokio::test]
async fn set_frame_notifies_active_subscribers() {
    let mut runtime = DisplayFrameRuntime::new();
    let device = make_device();

    let mut rx = runtime.subscribe(device);
    // Consume the initial `None` — we subscribed before any frames.
    assert!(rx.borrow_and_update().is_none());

    runtime.set_frame(device, snapshot(42));
    rx.changed()
        .await
        .expect("watcher should observe the new frame");
    let Some(frame) = rx.borrow().clone() else {
        panic!("borrow should yield the latest snapshot after notification");
    };
    assert_eq!(frame.frame_number, 42);
}

#[tokio::test]
async fn remove_signals_stream_end() {
    let mut runtime = DisplayFrameRuntime::new();
    let device = make_device();
    runtime.set_frame(device, snapshot(1));
    let mut rx = runtime.subscribe(device);
    assert!(rx.borrow_and_update().is_some());

    runtime.remove(device);
    rx.changed()
        .await
        .expect("removal should propagate as a watch change");
    assert!(rx.borrow().is_none());
}

#[tokio::test]
async fn dropped_receivers_let_sender_clean_up() {
    let mut runtime = DisplayFrameRuntime::new();
    let device = make_device();
    {
        let _rx = runtime.subscribe(device);
        // Receiver drops at scope exit.
    }

    // After all receivers are gone, a subsequent `set_frame` should still
    // succeed (no panic) and silently discard the notification. The next
    // subscribe spins up a fresh watch channel so new consumers don't
    // inherit a closed state.
    runtime.set_frame(device, snapshot(99));
    let rx = runtime.subscribe(device);
    let Some(frame) = rx.borrow().clone() else {
        panic!(
            "new subscribers should receive the latest snapshot even after prior receivers dropped"
        );
    };
    assert_eq!(frame.frame_number, 99);
}

#[tokio::test]
async fn subscribed_device_ids_only_include_live_receivers() {
    let mut runtime = DisplayFrameRuntime::new();
    let kept_device = make_device();
    let dropped_device = make_device();

    let _kept_rx = runtime.subscribe(kept_device);
    let dropped_rx = runtime.subscribe(dropped_device);
    drop(dropped_rx);

    let subscribed = runtime.subscribed_device_ids();
    assert!(subscribed.contains(&kept_device));
    assert!(!subscribed.contains(&dropped_device));
}
