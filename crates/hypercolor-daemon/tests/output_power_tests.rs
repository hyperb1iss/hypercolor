use std::sync::Arc;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::output_power::{OutputPower, OutputPowerTransition};
use hypercolor_types::session::OffOutputBehavior;

#[tokio::test]
async fn brightness_is_persisted_before_live_publication() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let power = OutputPower::new(DeviceSettingsStore::new(path.clone()));
    let mut updates = power.subscribe();
    let writer = power.clone();
    let event_bus = Arc::new(HypercolorBus::new());
    let writer_event_bus = Arc::clone(&event_bus);

    let write =
        tokio::spawn(async move { writer.set_global_brightness(&writer_event_bus, 0.42).await });
    updates
        .changed()
        .await
        .expect("brightness publication should arrive");

    let persisted = DeviceSettingsStore::load(&path).expect("settings should reload");
    assert!((persisted.global_brightness() - 0.42).abs() < f32::EPSILON);
    assert!((updates.borrow().global_brightness - 0.42).abs() < f32::EPSILON);
    assert_eq!(
        write
            .await
            .expect("brightness task should join")
            .expect("save should succeed"),
        1.0
    );
}

#[tokio::test]
async fn failed_brightness_persistence_leaves_live_state_unpublished() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let blocked_parent = tempdir.path().join("not-a-directory");
    std::fs::write(&blocked_parent, b"block directory creation")
        .expect("blocking file should write");
    let path = blocked_parent.join("device-settings.json");
    let power = OutputPower::new(DeviceSettingsStore::new(path.clone()));
    let updates = power.subscribe();
    let event_bus = HypercolorBus::new();

    power
        .set_global_brightness(&event_bus, 0.25)
        .await
        .expect_err("unwritable settings path should fail");

    assert_eq!(power.global_brightness(), 1.0);
    assert!(
        !updates
            .has_changed()
            .expect("sender should remain available")
    );

    std::fs::remove_file(&blocked_parent).expect("blocking file should be removable");
    std::fs::create_dir(&blocked_parent).expect("settings directory should build");
    assert_eq!(
        power
            .set_global_brightness(&event_bus, 0.25)
            .await
            .expect("retry should persist"),
        1.0
    );
    assert_eq!(
        DeviceSettingsStore::load(&path)
            .expect("settings should reload")
            .global_brightness(),
        0.25
    );
}

#[cfg(all(unix, feature = "persistence-test-hooks"))]
#[tokio::test]
async fn post_replacement_failure_keeps_memory_live_and_retained_bytes_aligned() {
    use std::time::Duration;

    use hypercolor_core::persistence::AtomicFileWriter;

    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let writer = AtomicFileWriter::new(&path).expect("persistence writer should build");
    writer.set_injected_directory_sync_failures(1);
    let power = OutputPower::new(DeviceSettingsStore::new(path.clone()));
    let mut updates = power.subscribe();
    let event_bus = HypercolorBus::new();

    assert_eq!(
        power
            .set_global_brightness(&event_bus, 0.25)
            .await
            .expect("an admitted retry must remain authoritative"),
        1.0
    );
    updates
        .changed()
        .await
        .expect("admitted brightness should publish live");
    assert_eq!(power.global_brightness(), 0.25);
    assert_eq!(
        DeviceSettingsStore::load(&path)
            .expect("visible replacement should reload")
            .global_brightness(),
        0.25
    );

    writer
        .flush(Duration::from_secs(1))
        .expect("retained brightness should become durable");
    assert_eq!(
        DeviceSettingsStore::load(&path)
            .expect("durable replacement should reload")
            .global_brightness(),
        0.25
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn pre_replacement_failure_retains_rollback_without_publishing_live_state() {
    use std::time::Duration;

    use hypercolor_core::persistence::AtomicFileWriter;

    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let writer = AtomicFileWriter::new(&path).expect("persistence writer should build");
    writer.set_injected_replace_failures(2);
    let power = OutputPower::new(DeviceSettingsStore::new(path.clone()));
    let updates = power.subscribe();
    let event_bus = HypercolorBus::new();
    let mut events = event_bus.subscribe_all();

    power
        .set_global_brightness(&event_bus, 0.25)
        .await
        .expect_err("a pre-replacement failure must reject the mutation");

    assert_eq!(power.global_brightness(), 1.0);
    assert!(
        !updates
            .has_changed()
            .expect("watch sender should remain open")
    );
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    writer
        .flush(Duration::from_secs(1))
        .expect("retained rollback should become durable");
    assert_eq!(
        DeviceSettingsStore::load(&path)
            .expect("rolled-back settings should reload")
            .global_brightness(),
        1.0
    );
}

#[tokio::test]
async fn explicit_resume_preempts_an_in_flight_session_fade() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let power = OutputPower::new(DeviceSettingsStore::new(
        tempdir.path().join("device-settings.json"),
    ));
    let event_bus = HypercolorBus::new();
    let generation = power.begin_session_transition();
    let mut updates = power.subscribe();
    let fading_power = power.clone();
    let fade = tokio::spawn(async move { fading_power.fade_session_to(0.0, 96, generation).await });

    loop {
        updates
            .changed()
            .await
            .expect("fade publication should arrive");
        if updates.borrow().session_brightness < 1.0 {
            break;
        }
    }

    power.clear_output_override(&event_bus).await;

    assert!(!fade.await.expect("fade task should join"));
    assert_eq!(power.snapshot().session_brightness, 1.0);
    assert!(!power.snapshot().sleeping());
}

#[tokio::test]
async fn session_wake_preserves_a_manual_pause_latch() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let power = OutputPower::new(DeviceSettingsStore::new(
        tempdir.path().join("device-settings.json"),
    ));
    let event_bus = HypercolorBus::new();
    assert_eq!(
        power.set_manual_pause(&event_bus, true, [4, 5, 6]).await,
        OutputPowerTransition::Paused
    );
    let generation = power.begin_session_transition();
    assert!(
        power
            .pause_for_session(&event_bus, generation, OffOutputBehavior::Static, [0, 0, 0],)
            .await
    );

    assert!(power.clear_session_sleep(&event_bus, generation).await);
    assert!(power.snapshot().manually_paused());
    assert!(power.snapshot().sleeping());
    assert_eq!(power.snapshot().effective_off_output_color(), [4, 5, 6]);
}
