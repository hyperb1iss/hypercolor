//! Integration tests for daemon startup orchestration.

use std::io::Write;
use std::path::PathBuf;

use hypercolor_daemon::startup::{
    DaemonState, default_config, install_signal_handlers, load_config, parse_config_toml,
};
use tempfile::NamedTempFile;

/// Minimal TOML content that `ConfigManager` can parse.
const MINIMAL_TOML: &str = "schema_version = 3\n";

/// Create a temp file pre-populated with valid minimal TOML config.
fn temp_config_file() -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("failed to create temp file");
    f.write_all(MINIMAL_TOML.as_bytes())
        .expect("failed to write temp config");
    f.flush().expect("failed to flush temp config");
    f
}

// ── Config Loading ──────────────────────────────────────────────────────────

#[tokio::test]
async fn load_config_falls_back_to_defaults_when_no_file() {
    // When no explicit path is provided and no file exists at the default
    // location, load_config should succeed with defaults.
    let (config, _path) = load_config(None).await.expect("default config should load");
    assert_eq!(config.schema_version, 3);
    assert_eq!(config.daemon.target_fps, 60);
    assert_eq!(config.daemon.port, 9420);
}

#[tokio::test]
async fn load_config_reads_toml_file() {
    let toml_content = r#"
schema_version = 3

[daemon]
target_fps = 30
port = 8080
listen_address = "0.0.0.0"
"#;

    let mut temp = NamedTempFile::new().expect("failed to create temp file");
    temp.write_all(toml_content.as_bytes())
        .expect("failed to write temp config");

    let (config, path) = load_config(Some(temp.path()))
        .await
        .expect("config should load from file");

    assert_eq!(config.daemon.target_fps, 30);
    assert_eq!(config.daemon.port, 8080);
    assert_eq!(config.daemon.listen_address, "0.0.0.0");
    assert_eq!(path, temp.path());
}

#[tokio::test]
async fn load_config_errors_on_explicit_missing_path() {
    let missing = PathBuf::from("/tmp/hypercolor_does_not_exist_xyz.toml");
    let result = load_config(Some(&missing)).await;
    assert!(
        result.is_err(),
        "should error when explicit path is missing"
    );
}

// ── Config Parsing ──────────────────────────────────────────────────────────

#[test]
fn parse_config_toml_minimal() {
    let config = parse_config_toml(MINIMAL_TOML).expect("minimal config should parse");
    assert_eq!(config.schema_version, 3);
    // All sections should have serde defaults.
    assert_eq!(config.daemon.target_fps, 60);
    assert!(config.audio.enabled);
}

#[test]
fn parse_config_toml_with_overrides() {
    let toml_str = r"
schema_version = 3

[daemon]
target_fps = 45
canvas_width = 640
canvas_height = 400

[audio]
enabled = false
fft_size = 2048

[features]
wasm_plugins = true
";

    let config = parse_config_toml(toml_str).expect("config with overrides should parse");
    assert_eq!(config.daemon.target_fps, 45);
    assert_eq!(config.daemon.canvas_width, 640);
    assert_eq!(config.daemon.canvas_height, 400);
    assert!(!config.audio.enabled);
    assert_eq!(config.audio.fft_size, 2048);
    assert!(config.features.wasm_plugins);
}

#[test]
fn parse_config_toml_rejects_invalid_toml() {
    let bad_toml = "this is not valid toml {{{}}}";
    let result = parse_config_toml(bad_toml);
    assert!(result.is_err(), "invalid TOML should fail");
}

// ── Default Config ──────────────────────────────────────────────────────────

#[test]
fn default_config_has_sane_values() {
    let config = default_config();
    assert_eq!(config.schema_version, 3);
    assert_eq!(config.daemon.target_fps, 60);
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.listen_address, "127.0.0.1");
    assert_eq!(config.daemon.canvas_width, 320);
    assert_eq!(config.daemon.canvas_height, 200);
    assert!(config.include.is_empty());
}

// ── DaemonState Initialization ──────────────────────────────────────────────

#[test]
fn daemon_state_initializes_with_default_config() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf());
    assert!(state.is_ok(), "initialization should succeed with defaults");
}

#[tokio::test]
async fn daemon_state_start_and_shutdown() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    // Start all subsystems.
    state.start().await.expect("start should succeed");

    // Verify the render loop is running.
    {
        let loop_guard = state.render_loop.read().await;
        assert!(
            loop_guard.is_running(),
            "render loop should be running after start"
        );
    }

    // Shutdown should complete cleanly.
    state.shutdown().await.expect("shutdown should succeed");

    // Verify the render loop is stopped.
    {
        let loop_guard = state.render_loop.read().await;
        assert!(
            !loop_guard.is_running(),
            "render loop should be stopped after shutdown"
        );
    }
}

#[tokio::test]
async fn daemon_state_device_registry_starts_empty() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    assert!(
        state.device_registry.is_empty().await,
        "device registry should start empty"
    );
}

#[tokio::test]
async fn daemon_state_effect_engine_starts_idle() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let engine = state.effect_engine.lock().await;
    assert!(
        !engine.is_running(),
        "effect engine should not be running initially"
    );
}

#[tokio::test]
async fn daemon_state_scene_manager_starts_empty() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let scenes = state.scene_manager.read().await;
    assert_eq!(
        scenes.scene_count(),
        0,
        "scene manager should start with no scenes"
    );
}

#[tokio::test]
async fn daemon_state_config_accessor_returns_loaded_config() {
    let mut config = default_config();
    config.daemon.target_fps = 45;
    let temp = temp_config_file();
    // Write specific config content so ConfigManager can load it.
    std::fs::write(
        temp.path(),
        "schema_version = 3\n[daemon]\ntarget_fps = 45\n",
    )
    .expect("failed to write config");
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let snapshot = state.config();
    assert_eq!(snapshot.daemon.target_fps, 45);
}

// ── Signal Handler ──────────────────────────────────────────────────────────

#[tokio::test]
async fn signal_handler_channel_starts_false() {
    let rx = install_signal_handlers();
    assert!(!*rx.borrow(), "shutdown signal should start as false");
}

// ── Shutdown Sequence ───────────────────────────────────────────────────────

#[tokio::test]
async fn shutdown_is_idempotent() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    state.start().await.expect("start should succeed");

    // Shutdown twice — second call should not panic or error.
    state
        .shutdown()
        .await
        .expect("first shutdown should succeed");
    state
        .shutdown()
        .await
        .expect("second shutdown should succeed");
}

#[tokio::test]
async fn event_bus_receives_startup_event() {
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    // Subscribe before starting so we catch the DaemonStarted event.
    let mut rx = state.event_bus.subscribe_all();

    state.start().await.expect("start should succeed");

    // The startup event should be receivable.
    let event = rx.recv().await.expect("should receive startup event");
    assert!(
        matches!(
            event.event,
            hypercolor_types::event::HypercolorEvent::DaemonStarted { .. }
        ),
        "first event should be DaemonStarted"
    );
}
