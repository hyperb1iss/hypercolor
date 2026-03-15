//! Tests for the configuration manager and path resolution.

use std::fs;

use hypercolor_core::config::ConfigManager;

// ─── TOML Parsing ───────────────────────────────────────────────────────────

#[test]
fn load_minimal_toml() {
    let toml = r"
        schema_version = 3
    ";

    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), toml).expect("failed to write temp file");

    let config = ConfigManager::load(tmp.path()).expect("minimal TOML should parse without error");

    assert_eq!(config.schema_version, 3);
    // Sections should fall back to their serde defaults
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.target_fps, 30);
    assert!(config.web.enabled);
    assert!(!config.features.wasm_plugins);
}

#[test]
fn load_full_toml_with_overrides() {
    let toml = r#"
        schema_version = 3
        include = ["local.toml"]

        [daemon]
        listen_address = "0.0.0.0"
        port = 8080
        target_fps = 120
        canvas_width = 640
        canvas_height = 400

        [web]
        enabled = false
        websocket_fps = 15

        [audio]
        device = "pulse-monitor"
        fft_size = 2048

        [features]
        wasm_plugins = true
        midi_input = true
    "#;

    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), toml).expect("failed to write temp file");

    let config = ConfigManager::load(tmp.path()).expect("full TOML should parse without error");

    assert_eq!(config.daemon.listen_address, "0.0.0.0");
    assert_eq!(config.daemon.port, 8080);
    assert_eq!(config.daemon.target_fps, 120);
    assert_eq!(config.daemon.canvas_width, 640);
    assert_eq!(config.daemon.canvas_height, 400);
    assert!(!config.web.enabled);
    assert_eq!(config.web.websocket_fps, 15);
    assert_eq!(config.audio.device, "pulse-monitor");
    assert_eq!(config.audio.fft_size, 2048);
    assert!(config.features.wasm_plugins);
    assert!(config.features.midi_input);
    assert!(!config.features.hue_entertainment);
    assert!(config.hue.use_cie_xy);
    assert_eq!(config.nanoleaf.transition_time, 1);
    assert_eq!(config.include, vec!["local.toml"]);
}

#[test]
fn load_invalid_toml_returns_error() {
    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), "not valid { toml [[[").expect("failed to write temp file");

    let result = ConfigManager::load(tmp.path());
    assert!(result.is_err());
}

#[test]
fn load_nonexistent_file_returns_error() {
    let result = ConfigManager::load(std::path::Path::new("/tmp/hypercolor_does_not_exist.toml"));
    assert!(result.is_err());
}

// ─── ConfigManager Lifecycle ────────────────────────────────────────────────

#[test]
fn new_with_missing_file_uses_defaults() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("nonexistent.toml");

    let manager =
        ConfigManager::new(path).expect("ConfigManager should fall back to defaults gracefully");
    let config = manager.get();

    assert_eq!(config.schema_version, 3);
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.target_fps, 30);
    assert!(config.web.enabled);
}

#[test]
fn new_with_valid_file_loads_it() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    fs::write(
        &path,
        r"
        schema_version = 3

        [daemon]
        port = 7777
    ",
    )
    .expect("failed to write config file");

    let manager = ConfigManager::new(path).expect("ConfigManager should load the file");
    let config = manager.get();

    assert_eq!(config.daemon.port, 7777);
}

#[test]
fn new_with_invalid_file_returns_error() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("broken.toml");
    fs::write(&path, "{{{{broken").expect("failed to write config file");

    let result = ConfigManager::new(path);
    assert!(result.is_err());
}

// ─── Reload ─────────────────────────────────────────────────────────────────

#[test]
fn reload_picks_up_file_changes() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");

    // Write initial config
    fs::write(
        &path,
        r"
        schema_version = 3

        [daemon]
        port = 9420
    ",
    )
    .expect("failed to write initial config");

    let manager = ConfigManager::new(path.clone()).expect("initial load should succeed");
    assert_eq!(manager.get().daemon.port, 9420);

    // Overwrite with new port
    fs::write(
        &path,
        r"
        schema_version = 3

        [daemon]
        port = 1234
    ",
    )
    .expect("failed to write updated config");

    manager.reload().expect("reload should succeed");
    assert_eq!(manager.get().daemon.port, 1234);
}

#[test]
fn reload_preserves_old_config_on_parse_error() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");

    fs::write(
        &path,
        r"
        schema_version = 3

        [daemon]
        port = 5555
    ",
    )
    .expect("failed to write initial config");

    let manager = ConfigManager::new(path.clone()).expect("initial load should succeed");
    assert_eq!(manager.get().daemon.port, 5555);

    // Corrupt the file
    fs::write(&path, "{{not valid toml").expect("failed to corrupt config");

    let result = manager.reload();
    assert!(result.is_err());

    // Old config should still be live
    assert_eq!(manager.get().daemon.port, 5555);
}

// ─── Path Resolution ────────────────────────────────────────────────────────

#[test]
fn config_dir_ends_with_hypercolor() {
    let dir = ConfigManager::config_dir();
    assert_eq!(
        dir.file_name().and_then(|n| n.to_str()),
        Some("hypercolor"),
        "config dir should end with 'hypercolor', got: {dir:?}"
    );
}

#[test]
fn data_dir_ends_with_hypercolor() {
    let dir = ConfigManager::data_dir();
    // On Windows the last component is "hypercolor" (under LocalAppData).
    // On Linux it's also "hypercolor" (under ~/.local/share).
    assert!(
        dir.to_string_lossy().contains("hypercolor"),
        "data dir should contain 'hypercolor', got: {dir:?}"
    );
}

#[test]
fn data_dir_override_replaces_default_resolution() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let override_path = dir.path().join("override-data");

    ConfigManager::set_data_dir_override(Some(override_path.clone()));
    assert_eq!(ConfigManager::data_dir(), override_path);
    ConfigManager::set_data_dir_override(None);
}

#[test]
fn cache_dir_contains_hypercolor() {
    let dir = ConfigManager::cache_dir();
    assert!(
        dir.to_string_lossy().contains("hypercolor"),
        "cache dir should contain 'hypercolor', got: {dir:?}"
    );
}

#[test]
fn all_dirs_are_absolute() {
    assert!(ConfigManager::config_dir().is_absolute());
    assert!(ConfigManager::data_dir().is_absolute());
    assert!(ConfigManager::cache_dir().is_absolute());
}
