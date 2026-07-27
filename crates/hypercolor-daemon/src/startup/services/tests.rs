use std::sync::Arc;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::input::screen::ResolvedCaptureSource;

use super::windows_capture_source_sink;

#[test]
fn resolved_windows_capture_source_survives_daemon_restart() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:1".to_owned());
    manager.save().expect("legacy source is persisted");

    windows_capture_source_sink(Arc::clone(&manager))(ResolvedCaptureSource {
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

    windows_capture_source_sink(Arc::clone(&manager))(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stale".to_owned(),
    });
    drop(manager);

    let restarted = ConfigManager::new(path).expect("config manager reopens after restart");
    assert_eq!(restarted.get().capture.source, "monitor:new-choice");
}
