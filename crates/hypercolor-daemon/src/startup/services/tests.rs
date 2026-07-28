use std::sync::Arc;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::input::screen::ResolvedCaptureSource;

use super::{
    CaptureConfigPersistenceGate, screen_capture_config_from, windows_capture_source_sink,
};

#[test]
fn resolved_windows_capture_source_survives_daemon_restart() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:1".to_owned());
    manager.save().expect("legacy source is persisted");

    windows_capture_source_sink(CaptureConfigPersistenceGate::new(
        Arc::clone(&manager),
        true,
    ))(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stable".to_owned(),
    });
    drop(manager);

    let restarted = ConfigManager::new(path).expect("config manager reopens after restart");
    assert_eq!(restarted.get().capture.source, "monitor:display:stable");
}

#[test]
fn resolved_windows_capture_source_does_not_overwrite_a_newer_selection() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:new-choice".to_owned());
    manager.save().expect("new source is persisted");

    windows_capture_source_sink(CaptureConfigPersistenceGate::new(
        Arc::clone(&manager),
        true,
    ))(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stale".to_owned(),
    });
    drop(manager);

    let restarted = ConfigManager::new(path).expect("config manager reopens after restart");
    assert_eq!(restarted.get().capture.source, "monitor:new-choice");
}

#[test]
fn resolved_windows_capture_source_waits_for_graph_commit() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:1".to_owned());
    let persistence = CaptureConfigPersistenceGate::new(Arc::clone(&manager), false);

    windows_capture_source_sink(persistence.clone())(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stable".to_owned(),
    });

    assert_eq!(manager.get().capture.source, "monitor:1");
    assert!(!path.exists());
    persistence.commit();
    assert_eq!(manager.get().capture.source, "monitor:display:stable");
    assert!(path.exists());
}

#[test]
fn screen_capture_config_conversion_preserves_validated_values_exactly() {
    let capture = hypercolor_types::config::CaptureConfig {
        capture_fps: 240,
        grid_cols: 64,
        grid_rows: 1,
        smoothing: 1.0,
        gamma: 5.0,
        ..hypercolor_types::config::CaptureConfig::default()
    };

    let runtime = screen_capture_config_from(&capture).expect("boundary config should validate");

    assert_eq!(runtime.target_fps, 240);
    assert_eq!(runtime.grid_cols, 64);
    assert_eq!(runtime.grid_rows, 1);
    assert!((runtime.smoothing_alpha - 1.0).abs() < f32::EPSILON);
    assert!((runtime.tuning.gamma - 5.0).abs() < f32::EPSILON);
}

#[test]
fn screen_capture_config_conversion_rejects_instead_of_clamping() {
    let capture = hypercolor_types::config::CaptureConfig {
        capture_fps: 241,
        ..hypercolor_types::config::CaptureConfig::default()
    };

    let error = screen_capture_config_from(&capture)
        .expect_err("unsupported cadence must not be silently clamped");

    assert!(format!("{error:#}").contains("1..=240"));
}

#[test]
fn daemon_initialization_rejects_invalid_capture_config_before_startup() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let config = hypercolor_types::config::HypercolorConfig {
        capture: hypercolor_types::config::CaptureConfig {
            capture_fps: 0,
            ..hypercolor_types::config::CaptureConfig::default()
        },
        ..hypercolor_types::config::HypercolorConfig::default()
    };

    let result = super::DaemonState::initialize(&config, directory.path().join("hypercolor.toml"));
    let error = match result {
        Ok(_) => panic!("invalid capture config must stop daemon initialization"),
        Err(error) => error,
    };

    assert!(format!("{error:#}").contains("capture.capture_fps"));
}
