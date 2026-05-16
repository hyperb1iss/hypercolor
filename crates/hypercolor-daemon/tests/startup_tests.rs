//! Integration tests for daemon startup orchestration.

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use axum::body::to_bytes;
use axum::extract::State;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::manager::{
    BackendRoutingDebugSnapshot, LayoutRoutingDebugEntry, OrphanedQueueDebugEntry,
};
use hypercolor_daemon::api::{AppState, system::get_status};
use hypercolor_daemon::daemon::{
    DaemonRunOptions, effective_bind_target, effective_bind_targets, validate_network_bind_auth,
};
use hypercolor_daemon::discovery;
use hypercolor_daemon::startup::{
    DaemonState, collect_unmapped_driver_layout_targets, collect_unmapped_prefixed_layout_targets,
    default_config, install_signal_handlers, load_config, parse_config_toml,
};
use hypercolor_daemon::{layout_store, runtime_state, scene_store::SceneStore};
use hypercolor_driver_api::{BackendInfo, DeviceBackend};
use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::config::{
    CURRENT_SCHEMA_VERSION, EffectErrorFallbackPolicy, RenderAccelerationMode,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
    ZoneLayoutHint,
};
use hypercolor_types::effect::EffectSource;
use hypercolor_types::event::{EffectStopReason, HypercolorEvent};
use hypercolor_types::scene::{RenderGroup, RenderGroupId, RenderGroupRole, SceneId};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection, ZoneShape,
};
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::sync::Mutex;

/// Minimal TOML content that `ConfigManager` can parse.
const MINIMAL_TOML: &str = "schema_version = 3\n";

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static CONFIG_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct ShutdownCleanupBackend {
    expected_device_id: DeviceId,
    disconnects: Arc<AtomicUsize>,
    connected: bool,
}

impl ShutdownCleanupBackend {
    fn new(expected_device_id: DeviceId, disconnects: Arc<AtomicUsize>) -> Self {
        Self {
            expected_device_id,
            disconnects,
            connected: false,
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for ShutdownCleanupBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "cleanup".to_owned(),
            name: "Shutdown Cleanup Backend".to_owned(),
            description: "Tracks daemon shutdown disconnect cleanup".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("disconnect called while backend was not connected");
        }
        self.connected = false;
        self.disconnects.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn write_colors(&mut self, _id: &DeviceId, _colors: &[[u8; 3]]) -> Result<()> {
        Ok(())
    }
}

struct TestDataDirGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
}

impl TestDataDirGuard {
    async fn new() -> Self {
        let lock = DATA_DIR_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let data_dir = dir.path().join("data");
        ConfigManager::set_data_dir_override(Some(data_dir.clone()));
        Self {
            _lock: lock,
            _dir: dir,
            data_dir,
        }
    }

    fn layouts_path(&self) -> PathBuf {
        self.data_dir.join("layouts.json")
    }

    fn runtime_state_path(&self) -> PathBuf {
        self.data_dir.join("runtime-state.json")
    }

    fn scenes_path(&self) -> PathBuf {
        self.data_dir.join("scenes.json")
    }
}

impl Drop for TestDataDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
    }
}

struct TestConfigDirGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
}

impl TestConfigDirGuard {
    async fn new() -> Self {
        let lock = CONFIG_DIR_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let config_dir = dir.path().join("config");
        ConfigManager::set_config_dir_override(Some(config_dir));
        Self {
            _lock: lock,
            _dir: dir,
        }
    }
}

impl Drop for TestConfigDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_config_dir_override(None);
    }
}

/// Create a temp file pre-populated with valid minimal TOML config.
fn temp_config_file() -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("failed to create temp file");
    f.write_all(MINIMAL_TOML.as_bytes())
        .expect("failed to write temp config");
    f.flush().expect("failed to flush temp config");
    f
}

fn shutdown_cleanup_device_info(id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id,
        name: "Shutdown Device".to_owned(),
        vendor: "TestVendor".to_owned(),
        family: DeviceFamily::named("cleanup"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("cleanup", "cleanup", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 8,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: Some(compact_perimeter_layout_hint()),
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 8,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

// ── Config Loading ──────────────────────────────────────────────────────────

#[tokio::test]
async fn load_config_falls_back_to_defaults_when_no_file() {
    let _guard = TestConfigDirGuard::new().await;

    // When no explicit path is provided and no file exists at the default
    // location, load_config should succeed with defaults.
    let (config, _path) = load_config(None).await.expect("default config should load");
    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(config.daemon.target_fps, 30);
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

#[cfg(not(feature = "wgpu"))]
#[tokio::test]
async fn initialize_rejects_explicit_gpu_render_acceleration_without_wgpu_feature() {
    let _guard = TestDataDirGuard::new().await;
    let temp = temp_config_file();
    let mut config = default_config();
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Gpu;

    let Err(error) = DaemonState::initialize(&config, temp.path().to_path_buf()) else {
        panic!("gpu render acceleration should fail explicitly without wgpu support");
    };

    assert!(format!("{error:#}").contains("rebuild hypercolor-daemon with the `wgpu` feature"));
}

#[cfg(not(feature = "wgpu"))]
#[tokio::test]
async fn status_reports_auto_render_acceleration_cpu_fallback_without_wgpu_feature() {
    let _guard = TestDataDirGuard::new().await;
    let temp = temp_config_file();
    let mut config = default_config();
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Auto;

    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("auto render acceleration should initialize with CPU fallback");
    let response = get_status(State(Arc::new(AppState::from_daemon_state(&state)))).await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("status body should read");
    let json: Value = serde_json::from_slice(&body).expect("status should serialize");

    assert_eq!(
        json["data"]["compositor_acceleration"]["requested_mode"],
        "auto"
    );
    assert_eq!(
        json["data"]["compositor_acceleration"]["effective_mode"],
        "cpu"
    );
    assert!(
        json["data"]["compositor_acceleration"]["fallback_reason"]
            .as_str()
            .expect("auto fallback reason should be present")
            .contains("built without the `wgpu` feature")
    );
    assert!(json["data"]["compositor_acceleration"]["gpu_probe"].is_null());
}

#[cfg(feature = "wgpu")]
#[tokio::test]
async fn initialize_handles_explicit_gpu_render_acceleration_when_wgpu_is_enabled() {
    let _guard = TestDataDirGuard::new().await;
    let temp = temp_config_file();
    let mut config = default_config();
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Gpu;

    match DaemonState::initialize(&config, temp.path().to_path_buf()) {
        Ok(daemon) => drop(daemon),
        Err(error) => {
            assert!(
                format!("{error:#}").contains("gpu compositor acceleration is not available yet")
            );
        }
    }
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
    assert_eq!(config.daemon.target_fps, 30);
    assert!(config.audio.enabled);
}

#[test]
fn parse_config_toml_with_overrides() {
    let toml_str = r#"
schema_version = 3

[daemon]
target_fps = 45
canvas_width = 640
canvas_height = 400

[audio]
enabled = false
fft_size = 2048

[drivers.wled]
default_protocol = "e131"
known_ips = ["192.168.1.50"]
realtime_http_enabled = false
dedup_threshold = 0

[features]
wasm_plugins = true
"#;

    let config = parse_config_toml(toml_str).expect("config with overrides should parse");
    assert_eq!(config.daemon.target_fps, 45);
    assert_eq!(config.daemon.canvas_width, 640);
    assert_eq!(config.daemon.canvas_height, 400);
    assert!(!config.audio.enabled);
    assert_eq!(config.audio.fft_size, 2048);
    assert_eq!(config.drivers["wled"].settings["default_protocol"], "e131");
    assert_eq!(
        config.drivers["wled"].settings["known_ips"],
        serde_json::json!(["192.168.1.50"])
    );
    assert_eq!(
        config.drivers["wled"].settings["realtime_http_enabled"],
        false
    );
    assert_eq!(config.drivers["wled"].settings["dedup_threshold"], 0);
    assert!(config.drivers["nollie"].enabled);
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
    assert_eq!(config.schema_version, 4);
    assert_eq!(config.daemon.target_fps, 30);
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.listen_address, "127.0.0.1");
    assert_eq!(config.daemon.canvas_width, DEFAULT_CANVAS_WIDTH);
    assert_eq!(config.daemon.canvas_height, DEFAULT_CANVAS_HEIGHT);
    assert!(config.drivers["wled"].enabled);
    assert!(config.drivers["wled"].settings.is_empty());
    assert!(config.drivers["asus"].enabled);
    assert!(config.drivers["nollie"].enabled);
    assert!(config.include.is_empty());
}

#[test]
fn effective_bind_target_keeps_localhost_default() {
    let config = default_config();
    let options = DaemonRunOptions::default();

    assert_eq!(effective_bind_target(&options, &config), "127.0.0.1:9420");
}

#[test]
fn effective_bind_targets_include_ipv6_loopback_default() {
    let config = default_config();
    let options = DaemonRunOptions::default();

    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["127.0.0.1:9420", "[::1]:9420"]
    );
}

#[test]
fn effective_bind_target_accepts_all_interface_aliases() {
    let mut config = default_config();
    config.daemon.listen_address = "all".to_owned();
    config.daemon.port = 9431;
    let options = DaemonRunOptions::default();

    assert_eq!(effective_bind_target(&options, &config), "0.0.0.0:9431");
    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["0.0.0.0:9431", "[::]:9431"]
    );
}

#[test]
fn effective_bind_target_supports_cli_listen_shortcuts() {
    let mut config = default_config();
    config.daemon.port = 9432;

    let all = DaemonRunOptions {
        listen_all: true,
        ..DaemonRunOptions::default()
    };
    assert_eq!(effective_bind_target(&all, &config), "0.0.0.0:9432");
    assert_eq!(
        effective_bind_targets(&all, &config),
        vec!["0.0.0.0:9432", "[::]:9432"]
    );

    let custom = DaemonRunOptions {
        listen_address: Some("192.168.1.42".to_owned()),
        ..DaemonRunOptions::default()
    };
    assert_eq!(effective_bind_target(&custom, &config), "192.168.1.42:9432");

    let ipv6_loopback = DaemonRunOptions {
        listen_address: Some("::1".to_owned()),
        ..DaemonRunOptions::default()
    };
    assert_eq!(effective_bind_target(&ipv6_loopback, &config), "[::1]:9432");

    let bracketed_ipv6_loopback = DaemonRunOptions {
        listen_address: Some("[::1]".to_owned()),
        ..DaemonRunOptions::default()
    };
    assert_eq!(
        effective_bind_target(&bracketed_ipv6_loopback, &config),
        "[::1]:9432"
    );
}

#[test]
fn effective_bind_target_normalizes_bind_alias_with_port() {
    let config = default_config();
    let options = DaemonRunOptions {
        bind: Some("all:9444".to_owned()),
        ..DaemonRunOptions::default()
    };

    assert_eq!(effective_bind_target(&options, &config), "0.0.0.0:9444");
    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["0.0.0.0:9444", "[::]:9444"]
    );
}

#[test]
fn effective_bind_targets_expand_ipv4_loopback_bind_with_port() {
    let config = default_config();
    let options = DaemonRunOptions {
        bind: Some("127.0.0.1:9444".to_owned()),
        ..DaemonRunOptions::default()
    };

    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["127.0.0.1:9444", "[::1]:9444"]
    );
}

#[test]
fn effective_bind_targets_expand_localhost_bind_with_port() {
    let config = default_config();
    let options = DaemonRunOptions {
        bind: Some("localhost:9444".to_owned()),
        ..DaemonRunOptions::default()
    };

    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["127.0.0.1:9444", "[::1]:9444"]
    );
}

#[test]
fn effective_bind_target_brackets_ipv6_bind_with_port() {
    let config = default_config();
    let options = DaemonRunOptions {
        bind: Some("[::1]:9444".to_owned()),
        ..DaemonRunOptions::default()
    };

    assert_eq!(effective_bind_target(&options, &config), "[::1]:9444");
}

#[test]
fn network_bind_auth_allows_localhost_without_control_key() {
    let config = default_config();
    let options = DaemonRunOptions::default();
    let bind = effective_bind_target(&options, &config)
        .parse::<SocketAddr>()
        .expect("default bind target should parse as a socket address");

    validate_network_bind_auth(bind, false).expect("localhost should not require API key");
}

#[test]
fn network_bind_auth_allows_ipv6_loopback_without_control_key() {
    let bind = "[::1]:9420"
        .parse::<SocketAddr>()
        .expect("IPv6 loopback bind target should parse as a socket address");

    validate_network_bind_auth(bind, false).expect("IPv6 localhost should not require API key");
}

#[test]
fn network_bind_auth_rejects_listen_all_without_control_key() {
    let config = default_config();
    let options = DaemonRunOptions {
        listen_all: true,
        ..DaemonRunOptions::default()
    };
    let bind = effective_bind_target(&options, &config)
        .parse::<SocketAddr>()
        .expect("listen-all bind target should parse as a socket address");

    let error =
        validate_network_bind_auth(bind, false).expect_err("listen-all should require auth");
    let message = error.to_string();
    assert!(message.contains("0.0.0.0:9420"));
    assert!(message.contains("HYPERCOLOR_API_KEY"));
}

#[test]
fn network_bind_auth_rejects_ipv6_all_without_control_key() {
    let bind = "[::]:9420"
        .parse::<SocketAddr>()
        .expect("IPv6 all-interface bind target should parse as a socket address");

    let error = validate_network_bind_auth(bind, false)
        .expect_err("IPv6 all-interface bind should require auth");
    let message = error.to_string();
    assert!(message.contains("[::]:9420"));
    assert!(message.contains("HYPERCOLOR_API_KEY"));
}

#[test]
fn network_bind_auth_rejects_remote_access_without_control_key() {
    let mut config = default_config();
    config.network.remote_access = true;
    let options = DaemonRunOptions::default();
    let bind = effective_bind_target(&options, &config)
        .parse::<SocketAddr>()
        .expect("remote-access bind target should parse as a socket address");

    let error =
        validate_network_bind_auth(bind, false).expect_err("remote access should require auth");
    assert!(error.to_string().contains("HYPERCOLOR_API_KEY"));
}

#[test]
fn network_bind_auth_allows_network_bind_with_control_key() {
    let config = default_config();
    let options = DaemonRunOptions {
        listen_address: Some("192.168.1.42".to_owned()),
        ..DaemonRunOptions::default()
    };
    let bind = effective_bind_target(&options, &config)
        .parse::<SocketAddr>()
        .expect("custom bind target should parse as a socket address");

    validate_network_bind_auth(bind, true).expect("control API key should allow network bind");
}

// ── DaemonState Initialization ──────────────────────────────────────────────

#[tokio::test]
async fn daemon_state_initializes_with_default_config() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf());
    assert!(state.is_ok(), "initialization should succeed with defaults");
}

#[tokio::test]
async fn daemon_state_start_and_shutdown() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
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
async fn daemon_shutdown_disconnects_renderable_devices() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let device_id = DeviceId::new();
    let disconnects = Arc::new(AtomicUsize::new(0));
    let info = shutdown_cleanup_device_info(device_id);

    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Box::new(ShutdownCleanupBackend::new(
            device_id,
            Arc::clone(&disconnects),
        )));
    }

    let _ = state.device_registry.add(info.clone()).await;
    let layout_device_id = {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        let _actions = lifecycle.on_discovered(device_id, &info, None);
        lifecycle
            .layout_device_id_for(device_id)
            .expect("layout id should exist")
            .to_owned()
    };

    state
        .backend_manager
        .lock()
        .await
        .connect_device("cleanup", device_id, &layout_device_id)
        .await
        .expect("device should connect for shutdown cleanup");

    {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        lifecycle
            .on_connected(device_id)
            .expect("connect transition should succeed");
    }
    let _ = state
        .device_registry
        .set_state(&device_id, hypercolor_types::device::DeviceState::Connected)
        .await;

    state.shutdown().await.expect("shutdown should succeed");

    assert_eq!(disconnects.load(Ordering::Relaxed), 1);
    assert_eq!(state.backend_manager.lock().await.mapped_device_count(), 0);
}

#[tokio::test]
async fn daemon_state_device_registry_starts_empty() {
    let _guard = TestDataDirGuard::new().await;
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
async fn daemon_state_default_scene_starts_without_render_groups() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let scenes = state.scene_manager.read().await;
    assert!(
        scenes.active_scene_id().is_some_and(SceneId::is_default),
        "default scene should be active initially"
    );
    assert!(
        scenes.active_render_groups().is_empty(),
        "default scene should start without any active render groups"
    );
}

#[tokio::test]
async fn daemon_state_scene_manager_starts_with_default_scene() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let scenes = state.scene_manager.read().await;
    assert_eq!(
        scenes.scene_count(),
        1,
        "scene manager should synthesize the default scene"
    );
    assert_eq!(
        scenes.active_scene_id(),
        Some(&hypercolor_types::scene::SceneId::DEFAULT)
    );
}

#[tokio::test]
async fn named_scenes_persist_across_restart() {
    let guard = TestDataDirGuard::new().await;
    let mut store = SceneStore::new(guard.scenes_path());
    let named_scene = hypercolor_core::scene::make_scene("Movie Night");
    let named_scene_id = named_scene.id;
    store.replace_named_scenes([named_scene]);
    store.save().expect("scene store should save");

    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let scenes = state.scene_manager.read().await;
    assert_eq!(scenes.scene_count(), 2);
    assert_eq!(scenes.active_scene_id(), Some(&SceneId::DEFAULT));
    assert_eq!(
        scenes.get(&named_scene_id).map(|scene| scene.name.as_str()),
        Some("Movie Night")
    );
}

#[tokio::test]
async fn daemon_state_config_accessor_returns_loaded_config() {
    let _guard = TestDataDirGuard::new().await;
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
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
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
async fn daemon_start_restores_persisted_active_layout_from_disk() {
    let guard = TestDataDirGuard::new().await;
    let mut layouts = std::collections::HashMap::new();
    let restored_layout = SpatialLayout {
        id: "layout_restored".into(),
        name: "Restored Layout".into(),
        description: Some("Persisted layout".into()),
        canvas_width: 640,
        canvas_height: 360,
        zones: vec![],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    layouts.insert(restored_layout.id.clone(), restored_layout.clone());
    layout_store::save(&guard.layouts_path(), &layouts).expect("layout store should save");
    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_groups: Vec::new(),
            active_layout_id: Some(restored_layout.id.clone()),
            global_brightness: 1.0,
            driver_runtime_cache: std::collections::BTreeMap::new(),
        },
    )
    .expect("runtime state should save");

    let mut config = default_config();
    config.daemon.start_profile = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    assert_eq!(state.layouts_path, guard.layouts_path());
    assert_eq!(state.runtime_state_path, guard.runtime_state_path());

    state.start().await.expect("start should succeed");

    let active_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    assert_eq!(active_layout.id, restored_layout.id);
    assert_eq!(active_layout.name, restored_layout.name);

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn daemon_initialize_inserts_missing_default_layout_into_store() {
    let guard = TestDataDirGuard::new().await;
    let mut layouts = std::collections::HashMap::new();
    let custom_layout = SpatialLayout {
        id: "layout_custom".into(),
        name: "Custom Layout".into(),
        description: Some("Persisted custom layout".into()),
        canvas_width: 640,
        canvas_height: 360,
        zones: vec![],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    layouts.insert(custom_layout.id.clone(), custom_layout);
    layout_store::save(&guard.layouts_path(), &layouts).expect("layout store should save");

    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    let persisted = layout_store::load(&guard.layouts_path()).expect("layout store should load");
    assert!(persisted.contains_key("default"));
    assert!(persisted.contains_key("layout_custom"));
    assert_eq!(state.layouts_path, guard.layouts_path());

    let in_memory = state.layouts.read().await;
    let default_layout = in_memory
        .get("default")
        .expect("default layout should be present in memory");
    assert_eq!(default_layout.name, "Default Layout");
    assert_eq!(default_layout.canvas_width, config.daemon.canvas_width);
    assert_eq!(default_layout.canvas_height, config.daemon.canvas_height);
}

#[tokio::test]
async fn runtime_state_captures_default_scene_groups() {
    let guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    assert_eq!(state.runtime_state_path, guard.runtime_state_path());
    state.start().await.expect("start should succeed");

    let metadata = {
        let registry = state.effect_registry.read().await;
        let (_, entry) = registry
            .iter()
            .find(|(_, entry)| matches!(entry.metadata.source, EffectSource::Native { .. }))
            .expect("expected at least one native effect in registry");
        entry.metadata.clone()
    };
    let preset_id = hypercolor_types::library::PresetId::new();

    {
        let layout = {
            let spatial = state.spatial_engine.read().await;
            spatial.layout().as_ref().clone()
        };
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .upsert_primary_group(
                &metadata,
                std::collections::HashMap::new(),
                Some(preset_id),
                layout,
            )
            .expect("native effect should activate");
    }

    let mut wled_metadata = std::collections::HashMap::new();
    wled_metadata.insert("ip".to_owned(), "10.0.0.42".to_owned());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(
            DeviceInfo {
                id: DeviceId::new(),
                name: "Desk Strip".to_owned(),
                vendor: "WLED".to_owned(),
                family: DeviceFamily::new_static("wled", "WLED"),
                model: None,
                connection_type: ConnectionType::Network,
                origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
                zones: vec![ZoneInfo {
                    name: "Main".to_owned(),
                    led_count: 30,
                    topology: DeviceTopologyHint::Strip,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                }],
                firmware_version: Some("0.15.3".to_owned()),
                capabilities: DeviceCapabilities::default(),
            },
            DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".to_owned()),
            wled_metadata,
        )
        .await;

    state.shutdown().await.expect("shutdown should succeed");

    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state snapshot should exist");
    assert_eq!(snapshot.active_scene_id, Some(SceneId::DEFAULT.to_string()));
    assert_eq!(snapshot.default_scene_groups.len(), 1);
    assert_eq!(
        snapshot.default_scene_groups[0].effect_id,
        Some(metadata.id)
    );
    assert_eq!(snapshot.default_scene_groups[0].preset_id, Some(preset_id));
    let wled_cache = snapshot
        .driver_runtime_cache
        .get("wled")
        .expect("WLED runtime cache should be persisted");
    let probe_ips: Vec<std::net::IpAddr> = serde_json::from_value(wled_cache["probe_ips"].clone())
        .expect("probe IP cache should deserialize");
    assert_eq!(
        probe_ips,
        vec!["10.0.0.42".parse::<std::net::IpAddr>().expect("valid IP"),]
    );
}

#[tokio::test]
async fn daemon_start_restores_named_active_scene_and_default_groups() {
    let guard = TestDataDirGuard::new().await;
    let mut store = SceneStore::new(guard.scenes_path());
    let named_scene = hypercolor_core::scene::make_scene("Focus");
    let named_scene_id = named_scene.id;
    store.replace_named_scenes([named_scene]);
    store.save().expect("scene store should save");

    let default_group = RenderGroup {
        id: RenderGroupId::new(),
        name: "Saved Default Group".to_owned(),
        description: None,
        effect_id: None,
        controls: std::collections::HashMap::new(),
        control_bindings: std::collections::HashMap::new(),
        preset_id: None,
        layout: SpatialLayout {
            id: "default_saved".to_owned(),
            name: "Saved Default Layout".to_owned(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        },
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: RenderGroupRole::Primary,
        controls_version: 0,
    };
    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(named_scene_id.to_string()),
            default_scene_groups: vec![default_group.clone()],
            active_layout_id: None,
            global_brightness: 1.0,
            driver_runtime_cache: std::collections::BTreeMap::new(),
        },
    )
    .expect("runtime state should save");

    let mut config = default_config();
    config.daemon.start_profile = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    state.start().await.expect("start should succeed");

    let scenes = state.scene_manager.read().await;
    assert_eq!(scenes.active_scene_id(), Some(&named_scene_id));
    let default_scene = scenes
        .get(&SceneId::DEFAULT)
        .expect("default scene should exist");
    assert_eq!(default_scene.groups, vec![default_group]);
    drop(scenes);

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn default_scene_contents_restore_on_restart() {
    let guard = TestDataDirGuard::new().await;
    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_groups: vec![RenderGroup {
                id: RenderGroupId::new(),
                name: "Saved Default Group".to_owned(),
                description: Some("Restored from runtime snapshot".to_owned()),
                effect_id: None,
                controls: std::collections::HashMap::from([(
                    "speed".to_owned(),
                    hypercolor_types::effect::ControlValue::Float(4.5),
                )]),
                control_bindings: std::collections::HashMap::new(),
                preset_id: None,
                layout: SpatialLayout {
                    id: "default_saved".to_owned(),
                    name: "Saved Default Layout".to_owned(),
                    description: None,
                    canvas_width: 320,
                    canvas_height: 200,
                    zones: Vec::new(),
                    default_sampling_mode: SamplingMode::Bilinear,
                    default_edge_behavior: EdgeBehavior::Clamp,
                    spaces: None,
                    version: 1,
                },
                brightness: 0.75,
                enabled: true,
                color: None,
                display_target: None,
                role: RenderGroupRole::Primary,
                controls_version: 0,
            }],
            active_layout_id: None,
            global_brightness: 1.0,
            driver_runtime_cache: std::collections::BTreeMap::new(),
        },
    )
    .expect("runtime state should save");

    let mut config = default_config();
    config.daemon.start_profile = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    state.start().await.expect("start should succeed");

    let scenes = state.scene_manager.read().await;
    assert_eq!(scenes.active_scene_id(), Some(&SceneId::DEFAULT));
    let default_scene = scenes
        .get(&SceneId::DEFAULT)
        .expect("default scene should exist");
    assert_eq!(default_scene.groups.len(), 1);
    assert_eq!(default_scene.groups[0].name, "Saved Default Group");
    assert_eq!(
        default_scene.groups[0].controls.get("speed"),
        Some(&hypercolor_types::effect::ControlValue::Float(4.5))
    );
    assert_eq!(default_scene.groups[0].brightness, 0.75);
    drop(scenes);

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn event_bus_receives_startup_event() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    // Subscribe before starting so we catch the DaemonStarted event.
    let mut rx = state.event_bus.subscribe_all();

    state.start().await.expect("start should succeed");

    // Runtime restoration may publish scene events first; keep receiving until
    // the startup marker arrives.
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.expect("should receive startup event");
            if matches!(
                event.event,
                hypercolor_types::event::HypercolorEvent::DaemonStarted { .. }
            ) {
                break event;
            }
        }
    })
    .await
    .expect("timed out waiting for DaemonStarted event");
    assert!(
        matches!(
            event.event,
            hypercolor_types::event::HypercolorEvent::DaemonStarted { .. }
        ),
        "first event should be DaemonStarted"
    );
}

#[test]
fn collect_unmapped_prefixed_layout_targets_returns_only_missing_matching_prefixes() {
    let layout = SpatialLayout {
        id: "layout_test".to_owned(),
        name: "Test".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            test_zone("zone_usb", "usb:laptop"),
            test_zone("zone_alpha_mapped", "driver-alpha:desk"),
            test_zone("zone_alpha_missing", "driver-alpha:wall"),
            test_zone("zone_alpha_missing_dup", "driver-alpha:wall"),
            test_zone("zone_beta", "driver-beta:bridge"),
        ],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let routing = BackendRoutingDebugSnapshot {
        backend_ids: vec!["usb".to_owned(), "driver-alpha".to_owned()],
        mapping_count: 2,
        queue_count: 2,
        mappings: vec![
            LayoutRoutingDebugEntry {
                layout_device_id: "usb:laptop".to_owned(),
                backend_id: "usb".to_owned(),
                device_id: "device_usb".to_owned(),
                backend_registered: true,
                queue_active: true,
            },
            LayoutRoutingDebugEntry {
                layout_device_id: "driver-alpha:desk".to_owned(),
                backend_id: "driver-alpha".to_owned(),
                device_id: "device_alpha".to_owned(),
                backend_registered: true,
                queue_active: true,
            },
        ],
        orphaned_queues: Vec::<OrphanedQueueDebugEntry>::new(),
    };

    let unmapped = collect_unmapped_prefixed_layout_targets(&layout, &routing, "driver-alpha:");
    assert_eq!(unmapped, vec!["driver-alpha:wall".to_owned()]);
}

#[test]
fn collect_unmapped_driver_layout_targets_groups_missing_registered_driver_prefixes() {
    let layout = SpatialLayout {
        id: "layout_test".to_owned(),
        name: "Test".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            test_zone("zone_usb", "usb:laptop"),
            test_zone("zone_alpha_mapped", "driver-alpha:desk"),
            test_zone("zone_alpha_missing", "driver-alpha:wall"),
            test_zone("zone_alpha_missing_dup", "driver-alpha:wall"),
            test_zone("zone_beta_missing", "driver-beta:bridge"),
            test_zone("zone_gamma_ignored", "driver-gamma:panels"),
        ],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let routing = BackendRoutingDebugSnapshot {
        backend_ids: vec!["usb".to_owned(), "driver-alpha".to_owned()],
        mapping_count: 2,
        queue_count: 2,
        mappings: vec![
            LayoutRoutingDebugEntry {
                layout_device_id: "usb:laptop".to_owned(),
                backend_id: "usb".to_owned(),
                device_id: "device_usb".to_owned(),
                backend_registered: true,
                queue_active: true,
            },
            LayoutRoutingDebugEntry {
                layout_device_id: "driver-alpha:desk".to_owned(),
                backend_id: "driver-alpha".to_owned(),
                device_id: "device_alpha".to_owned(),
                backend_registered: true,
                queue_active: true,
            },
        ],
        orphaned_queues: Vec::<OrphanedQueueDebugEntry>::new(),
    };
    let driver_ids = vec!["driver-alpha".to_owned(), "driver-beta".to_owned()];

    let unmapped = collect_unmapped_driver_layout_targets(&layout, &routing, &driver_ids);

    assert_eq!(unmapped.len(), 2);
    assert_eq!(
        unmapped["driver-alpha"],
        vec!["driver-alpha:wall".to_owned()]
    );
    assert_eq!(
        unmapped["driver-beta"],
        vec!["driver-beta:bridge".to_owned()]
    );
}

#[test]
fn collect_unmapped_prefixed_layout_targets_ignores_unmatched_prefixes() {
    let layout = SpatialLayout {
        id: "layout_test".to_owned(),
        name: "Test".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            test_zone("zone_usb", "usb:laptop"),
            test_zone("zone_beta", "driver-beta:bridge"),
        ],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let routing = BackendRoutingDebugSnapshot {
        backend_ids: vec!["usb".to_owned()],
        mapping_count: 1,
        queue_count: 1,
        mappings: vec![LayoutRoutingDebugEntry {
            layout_device_id: "usb:laptop".to_owned(),
            backend_id: "usb".to_owned(),
            device_id: "device_usb".to_owned(),
            backend_registered: true,
            queue_active: true,
        }],
        orphaned_queues: Vec::<OrphanedQueueDebugEntry>::new(),
    };

    let unmapped = collect_unmapped_prefixed_layout_targets(&layout, &routing, "driver-alpha:");
    assert!(unmapped.is_empty());
}

fn test_zone(id: &str, device_id: &str) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,
        position: NormalizedPosition { x: 0.5, y: 0.5 },
        size: NormalizedPosition { x: 0.25, y: 0.1 },
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 30,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
        led_mapping: None,
    }
}

#[test]
fn append_auto_layout_zones_for_device_adds_default_strip_zone() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Desk Strip".to_owned(),
        vendor: "Test".to_owned(),
        family: DeviceFamily::new_static("fixture-strip", "Fixture Strip"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("fixture-strip", "fixture-output", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 30,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let added = discovery::append_auto_layout_zones_for_device(
        &mut layout,
        "fixture-strip:desk-strip",
        &info,
    );

    assert_eq!(added, 1);
    assert_eq!(layout.zones.len(), 1);
    assert_eq!(layout.zones[0].device_id, "fixture-strip:desk-strip");
    assert_eq!(layout.zones[0].zone_name, Some("Main".to_owned()));
    assert_eq!(layout.zones[0].name, "Desk Strip");
    assert_eq!(
        layout.zones[0].topology,
        LedTopology::Strip {
            count: 30,
            direction: StripDirection::LeftToRight,
        }
    );
}

#[test]
fn append_auto_layout_zones_for_device_skips_display_only_devices() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "LCD Panel".to_owned(),
        vendor: "Test".to_owned(),
        family: DeviceFamily::named("display"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("display", "usb", ConnectionType::Usb),
        zones: vec![ZoneInfo {
            name: "Screen".to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            has_display: true,
            display_resolution: Some((320, 320)),
            ..DeviceCapabilities::default()
        },
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let added = discovery::append_auto_layout_zones_for_device(&mut layout, "usb:lcd-panel", &info);

    assert_eq!(added, 0);
    assert!(layout.zones.is_empty());
}

fn compact_perimeter_layout_hint() -> ZoneLayoutHint {
    ZoneLayoutHint::custom_grid(
        6,
        2,
        &[
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 1),
        ],
    )
    .with_size(NormalizedPosition::new(0.2, 0.08))
    .with_shape(ZoneShape::Rectangle)
}

fn asymmetric_pointer_layout_hint() -> ZoneLayoutHint {
    ZoneLayoutHint::custom_grid(
        7,
        8,
        &[
            (3, 5),
            (3, 1),
            (1, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (2, 6),
            (4, 6),
            (5, 3),
            (6, 2),
            (6, 1),
        ],
    )
    .with_size(NormalizedPosition::new(0.16, 0.18))
    .with_shape(ZoneShape::Rectangle)
}

fn outer_ring_layout_hint() -> ZoneLayoutHint {
    ZoneLayoutHint::custom_grid(
        13,
        13,
        &[
            (12, 6),
            (11, 8),
            (10, 10),
            (8, 11),
            (6, 12),
            (4, 11),
            (2, 10),
            (1, 8),
            (0, 6),
            (1, 4),
            (2, 2),
            (4, 1),
            (6, 0),
            (8, 1),
            (10, 2),
            (11, 4),
            (8, 6),
            (6, 8),
            (4, 6),
            (6, 4),
        ],
    )
    .with_size(NormalizedPosition::new(0.16, 0.16))
    .with_shape(ZoneShape::Ring)
    .co_located()
}

fn inner_ring_layout_hint() -> ZoneLayoutHint {
    ZoneLayoutHint::custom_grid(
        11,
        11,
        &[
            (10, 5),
            (9, 6),
            (9, 7),
            (8, 8),
            (7, 9),
            (6, 9),
            (5, 10),
            (4, 9),
            (3, 9),
            (2, 8),
            (1, 7),
            (1, 6),
            (0, 5),
            (1, 4),
            (1, 3),
            (2, 2),
            (3, 1),
            (4, 1),
            (5, 0),
            (6, 1),
            (7, 1),
            (8, 2),
            (9, 3),
            (9, 4),
        ],
    )
    .with_size(NormalizedPosition::new(0.19, 0.19))
    .with_shape(ZoneShape::Ring)
    .co_located()
}

#[test]
fn append_auto_layout_zones_uses_device_declared_compact_custom_geometry() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Compact Custom Device".to_owned(),
        vendor: "Layout Driver".to_owned(),
        family: DeviceFamily::new_static("layout-driver", "Layout Driver"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("layout-driver", "usb", ConnectionType::Usb),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 10,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: Some(compact_perimeter_layout_hint()),
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let added = discovery::append_auto_layout_zones_for_device(
        &mut layout,
        "usb:driver:compact:test",
        &info,
    );

    assert_eq!(added, 1);
    match &layout.zones[0].topology {
        LedTopology::Custom { positions } => {
            assert_eq!(positions.len(), 10);
            assert!((positions[0].x - 0.2).abs() < 0.001);
            assert!((positions[0].y - 0.0).abs() < 0.001);
            assert!((positions[9].x - 1.0).abs() < 0.001);
            assert!((positions[9].y - 1.0).abs() < 0.001);
        }
        other => panic!("expected custom topology, got {other:?}"),
    }
    assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.2, 0.08));
}

#[test]
fn append_auto_layout_zones_uses_device_declared_asymmetric_custom_geometry() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Asymmetric Custom Device".to_owned(),
        vendor: "Layout Driver".to_owned(),
        family: DeviceFamily::new_static("layout-driver", "Layout Driver"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("layout-driver", "usb", ConnectionType::Usb),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 11,
            topology: DeviceTopologyHint::Matrix { rows: 1, cols: 11 },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: Some(asymmetric_pointer_layout_hint()),
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let added = discovery::append_auto_layout_zones_for_device(
        &mut layout,
        "usb:driver:asymmetric:test",
        &info,
    );

    assert_eq!(added, 1);
    match &layout.zones[0].topology {
        LedTopology::Custom { positions } => {
            assert_eq!(positions.len(), 11);
            assert!((positions[0].x - 0.5).abs() < 0.001);
            assert!((positions[0].y - (5.0 / 7.0)).abs() < 0.001);
            assert!((positions[10].x - 1.0).abs() < 0.001);
            assert!((positions[10].y - (1.0 / 7.0)).abs() < 0.001);
        }
        other => panic!("expected custom topology, got {other:?}"),
    }
    assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.16, 0.18));
}

#[test]
fn append_auto_layout_zones_preserves_device_declared_colocated_ring_geometry() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Stacked Ring Controller".to_owned(),
        vendor: "Layout Driver".to_owned(),
        family: DeviceFamily::new_static("layout-driver", "Layout Driver"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("layout-driver", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "Outer Ring".to_owned(),
                led_count: 20,
                topology: DeviceTopologyHint::Ring { count: 20 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(outer_ring_layout_hint()),
            },
            ZoneInfo {
                name: "Inner Ring".to_owned(),
                led_count: 24,
                topology: DeviceTopologyHint::Ring { count: 24 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(inner_ring_layout_hint()),
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let added = discovery::append_auto_layout_zones_for_device(
        &mut layout,
        "usb:driver:stacked-rings:test",
        &info,
    );

    assert_eq!(added, 2);
    let outer = layout
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Outer Ring"))
        .expect("expected outer ring auto-layout zone");
    let inner = layout
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Inner Ring"))
        .expect("expected inner ring auto-layout zone");

    assert_eq!(outer.position, inner.position);
    match &outer.topology {
        LedTopology::Custom { positions } => {
            assert_eq!(positions.len(), 20);
            assert!((positions[0].x - 1.0).abs() < 0.001);
            assert!((positions[0].y - 0.5).abs() < 0.001);
            assert!((positions[16].x - (8.0 / 12.0)).abs() < 0.001);
            assert!((positions[19].y - (4.0 / 12.0)).abs() < 0.001);
        }
        other => panic!("expected custom topology, got {other:?}"),
    }
    match &inner.topology {
        LedTopology::Custom { positions } => {
            assert_eq!(positions.len(), 24);
            assert!((positions[0].x - 1.0).abs() < 0.001);
            assert!((positions[0].y - 0.5).abs() < 0.001);
            assert!((positions[12].x - 0.0).abs() < 0.001);
            assert!((positions[12].y - 0.5).abs() < 0.001);
        }
        other => panic!("expected custom topology, got {other:?}"),
    }
    assert_eq!(outer.size, NormalizedPosition::new(0.16, 0.16));
    assert_eq!(inner.size, NormalizedPosition::new(0.19, 0.19));
    assert_eq!(
        outer.shape,
        Some(hypercolor_types::spatial::ZoneShape::Ring)
    );
    assert_eq!(
        inner.shape,
        Some(hypercolor_types::spatial::ZoneShape::Ring)
    );
}

#[test]
fn append_auto_layout_zones_for_dense_matrix_device_clamps_height_without_panicking() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Ableton Push 2".to_owned(),
        vendor: "Ableton".to_owned(),
        family: DeviceFamily::named("Ableton"),
        model: Some("push2".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("ableton", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Buttons Above".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Buttons Below".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Scene Launch".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Transport".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Touch Strip".to_owned(),
                led_count: 31,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let added =
        discovery::append_auto_layout_zones_for_device(&mut layout, "usb:2982:1967:test", &info);

    assert_eq!(added, 6);
    assert_eq!(layout.zones.len(), 6);
    assert_eq!(layout.zones[0].name, "Ableton Push 2: Pads");
    assert!((layout.zones[0].size.x - 0.18).abs() < 0.001);
    assert!((layout.zones[0].size.y - 0.03).abs() < 0.001);
    assert_eq!(
        layout.zones[0].topology,
        LedTopology::Matrix {
            width: 8,
            height: 8,
            serpentine: false,
            start_corner: hypercolor_types::spatial::Corner::TopLeft,
        }
    );
}

#[test]
fn reconcile_auto_layout_zones_for_device_updates_existing_custom_auto_zone() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Compact Custom Device".to_owned(),
        vendor: "Layout Driver".to_owned(),
        family: DeviceFamily::new_static("layout-driver", "Layout Driver"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("layout-driver", "usb", ConnectionType::Usb),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 10,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: Some(compact_perimeter_layout_hint()),
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: "auto-usb-driver-compact-test-main".to_owned(),
            name: "Compact Custom Device".to_owned(),
            device_id: "usb:driver:compact:test".to_owned(),
            zone_name: Some("Main".to_owned()),

            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.26, 0.1),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 10,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: Some(hypercolor_types::spatial::ZoneShape::Rectangle),
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
            led_mapping: None,
        }],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let repaired = discovery::reconcile_auto_layout_zones_for_device(
        &mut layout,
        "usb:driver:compact:test",
        &info,
    );

    assert_eq!(repaired, 1);
    match &layout.zones[0].topology {
        LedTopology::Custom { positions } => assert_eq!(positions.len(), 10),
        other => panic!("expected custom topology, got {other:?}"),
    }
    assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.2, 0.08));
}

#[test]
fn reconcile_auto_layout_zones_repairs_device_declared_geometry_without_touching_rotation() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Stacked Ring Controller".to_owned(),
        vendor: "Layout Driver".to_owned(),
        family: DeviceFamily::new_static("layout-driver", "Layout Driver"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("layout-driver", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "Outer Ring".to_owned(),
                led_count: 20,
                topology: DeviceTopologyHint::Ring { count: 20 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(outer_ring_layout_hint()),
            },
            ZoneInfo {
                name: "Inner Ring".to_owned(),
                led_count: 24,
                topology: DeviceTopologyHint::Ring { count: 24 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(inner_ring_layout_hint()),
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            DeviceZone {
                id: "auto-usb-driver-stacked-rings-test-outer-ring".to_owned(),
                name: "Stacked Ring Controller: Outer Ring".to_owned(),
                device_id: "usb:driver:stacked-rings:test".to_owned(),
                zone_name: Some("Outer Ring".to_owned()),
                position: NormalizedPosition::new(0.42, 0.55),
                size: NormalizedPosition::new(0.08, 0.08),
                rotation: 0.25,
                scale: 1.0,
                display_order: 0,
                orientation: None,
                topology: LedTopology::Ring {
                    count: 20,
                    start_angle: 0.0,
                    direction: hypercolor_types::spatial::Winding::Clockwise,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(SamplingMode::Bilinear),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: Some(hypercolor_types::spatial::ZoneShape::Ring),
                shape_preset: None,
                attachment: None,
                brightness: None,
            },
            DeviceZone {
                id: "auto-usb-driver-stacked-rings-test-inner-ring".to_owned(),
                name: "Stacked Ring Controller: Inner Ring".to_owned(),
                device_id: "usb:driver:stacked-rings:test".to_owned(),
                zone_name: Some("Inner Ring".to_owned()),
                position: NormalizedPosition::new(0.42, 0.47),
                size: NormalizedPosition::new(0.08, 0.08),
                rotation: 3.0,
                scale: 1.0,
                display_order: 0,
                orientation: None,
                topology: LedTopology::Ring {
                    count: 24,
                    start_angle: 0.0,
                    direction: hypercolor_types::spatial::Winding::Clockwise,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(SamplingMode::Bilinear),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: Some(hypercolor_types::spatial::ZoneShape::Ring),
                shape_preset: None,
                attachment: None,
                brightness: None,
            },
        ],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let repaired = discovery::reconcile_auto_layout_zones_for_device(
        &mut layout,
        "usb:driver:stacked-rings:test",
        &info,
    );

    assert_eq!(repaired, 2);
    let outer = layout
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Outer Ring"))
        .expect("expected repaired outer ring zone");
    let inner = layout
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Inner Ring"))
        .expect("expected repaired inner ring zone");

    assert!((outer.rotation - 0.25).abs() < f32::EPSILON);
    assert!((inner.rotation - 3.0).abs() < f32::EPSILON);
    assert_eq!(outer.size, NormalizedPosition::new(0.16, 0.16));
    assert_eq!(inner.size, NormalizedPosition::new(0.19, 0.19));
    match &outer.topology {
        LedTopology::Custom { positions } => assert_eq!(positions.len(), 20),
        other => panic!("expected custom topology, got {other:?}"),
    }
    match &inner.topology {
        LedTopology::Custom { positions } => assert_eq!(positions.len(), 24),
        other => panic!("expected custom topology, got {other:?}"),
    }
}

#[test]
fn reconcile_auto_layout_zones_for_device_removes_stale_auto_zones() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "PrismRGB Prism S".to_owned(),
        vendor: "PrismRGB".to_owned(),
        family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
        model: Some("prism_s".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("prismrgb", "usb", ConnectionType::Usb)
            .with_protocol_id("prismrgb/prism-s"),
        zones: vec![ZoneInfo {
            name: "GPU Strimer".to_owned(),
            led_count: 108,
            topology: DeviceTopologyHint::Matrix { rows: 4, cols: 27 },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 108,
            ..DeviceCapabilities::default()
        },
    };
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            DeviceZone {
                id: "auto-usb-prism-s-test-atx-strimer".to_owned(),
                name: "PrismRGB Prism S: ATX Strimer".to_owned(),
                device_id: "usb:prism-s:test".to_owned(),
                zone_name: Some("ATX Strimer".to_owned()),

                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(0.25, 0.1),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Matrix {
                    width: 20,
                    height: 6,
                    serpentine: false,
                    start_corner: hypercolor_types::spatial::Corner::TopLeft,
                },
                led_positions: Vec::new(),
                sampling_mode: Some(SamplingMode::Bilinear),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: Some(hypercolor_types::spatial::ZoneShape::Rectangle),
                shape_preset: None,
                display_order: 0,
                attachment: None,
                brightness: None,
                led_mapping: None,
            },
            DeviceZone {
                id: "auto-usb-prism-s-test-gpu-strimer".to_owned(),
                name: "PrismRGB Prism S: GPU Strimer".to_owned(),
                device_id: "usb:prism-s:test".to_owned(),
                zone_name: Some("GPU Strimer".to_owned()),

                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(0.25, 0.1),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Matrix {
                    width: 27,
                    height: 6,
                    serpentine: false,
                    start_corner: hypercolor_types::spatial::Corner::TopLeft,
                },
                led_positions: Vec::new(),
                sampling_mode: Some(SamplingMode::Bilinear),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: Some(hypercolor_types::spatial::ZoneShape::Rectangle),
                shape_preset: None,
                display_order: 0,
                attachment: None,
                brightness: None,
                led_mapping: None,
            },
        ],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let repaired =
        discovery::reconcile_auto_layout_zones_for_device(&mut layout, "usb:prism-s:test", &info);

    assert_eq!(repaired, 2);
    assert_eq!(layout.zones.len(), 1);
    assert_eq!(layout.zones[0].zone_name.as_deref(), Some("GPU Strimer"));
    assert_eq!(
        layout.zones[0].topology,
        LedTopology::Matrix {
            width: 27,
            height: 4,
            serpentine: false,
            start_corner: hypercolor_types::spatial::Corner::TopLeft,
        }
    );
}

#[tokio::test]
async fn effect_error_fallback_worker_clears_active_groups_when_configured() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.effect_engine.effect_error_fallback = EffectErrorFallbackPolicy::ClearGroups;
    let temp = temp_config_file();
    std::fs::write(
        temp.path(),
        toml::to_string(&config).expect("serialize test config"),
    )
    .expect("write test config");
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("daemon state should initialize");
    state.start().await.expect("start should succeed");

    let metadata = {
        let registry = state.effect_registry.read().await;
        let (_, entry) = registry
            .iter()
            .find(|(_, entry)| matches!(entry.metadata.source, EffectSource::Native { .. }))
            .expect("expected at least one native effect in registry");
        entry.metadata.clone()
    };

    let group_id = {
        let layout = {
            let spatial = state.spatial_engine.read().await;
            spatial.layout().as_ref().clone()
        };
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .upsert_primary_group(&metadata, std::collections::HashMap::new(), None, layout)
            .expect("native effect should activate")
            .id
    };

    let mut rx = state.event_bus.subscribe_all();
    state.event_bus.publish(HypercolorEvent::EffectError {
        effect_id: metadata.id.to_string(),
        error: "render exploded".to_owned(),
        fallback: None,
    });

    let mut saw_stopped = false;
    let mut saw_fallback_event = false;
    let mut saw_group_update = false;
    let expected_effect_id = metadata.id.to_string();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !(saw_stopped && saw_fallback_event && saw_group_update) {
            let event = rx.recv().await.expect("effect-error fallback event");
            match event.event {
                HypercolorEvent::EffectStopped { effect, reason }
                    if effect.id == expected_effect_id && reason == EffectStopReason::Error =>
                {
                    saw_stopped = true;
                }
                HypercolorEvent::EffectError {
                    effect_id,
                    fallback,
                    ..
                } if effect_id == expected_effect_id
                    && fallback.as_deref() == Some("clear_groups") =>
                {
                    saw_fallback_event = true;
                }
                HypercolorEvent::RenderGroupChanged {
                    group_id: changed, ..
                } if changed == group_id => {
                    saw_group_update = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("effect-error fallback worker should react");

    let cleared_effect = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.groups.iter().find(|group| group.id == group_id))
            .and_then(|group| group.effect_id)
    };
    assert_eq!(cleared_effect, None);

    let response = get_status(State(Arc::new(AppState::from_daemon_state(&state)))).await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("status body should read");
    let json: Value = serde_json::from_slice(&body).expect("status should serialize");
    assert_eq!(json["data"]["effect_health"]["errors_total"], 1);
    assert_eq!(json["data"]["effect_health"]["fallbacks_applied_total"], 1);

    state.shutdown().await.expect("shutdown should succeed");
}
