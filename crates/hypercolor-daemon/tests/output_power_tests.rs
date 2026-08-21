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

    let write = tokio::spawn(async move { writer.set_global_brightness(0.42).await });
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

    power
        .set_global_brightness(0.25)
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
            .set_global_brightness(0.25)
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
