//! Integration tests for daemon startup orchestration.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::manager::{
    BackendRoutingDebugSnapshot, LayoutRoutingDebugEntry, OrphanedQueueDebugEntry,
};
use hypercolor_core::device::{BackendInfo, DeviceBackend};
use hypercolor_daemon::discovery;
use hypercolor_daemon::startup::{
    DaemonState, collect_unmapped_prefixed_layout_targets, default_config, install_signal_handlers,
    load_config, parse_config_toml,
};
use hypercolor_daemon::{layout_store, runtime_state};
use hypercolor_types::config::{RenderAccelerationMode, WledProtocolConfig};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::EffectSource;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
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
        family: DeviceFamily::Custom("cleanup".to_owned()),
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 8,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
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
    assert_eq!(config.schema_version, 3);
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

#[tokio::test]
async fn initialize_rejects_explicit_gpu_render_acceleration_until_supported() {
    let _guard = TestDataDirGuard::new().await;
    let temp = temp_config_file();
    let mut config = default_config();
    config.effect_engine.render_acceleration_mode = RenderAccelerationMode::Gpu;

    let error = match DaemonState::initialize(&config, temp.path().to_path_buf()) {
        Ok(_) => panic!("gpu render acceleration should fail explicitly"),
        Err(error) => error,
    };

    assert!(format!("{error:#}").contains("gpu compositor acceleration is not available yet"));
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

[wled]
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
    assert_eq!(config.wled.default_protocol, WledProtocolConfig::E131);
    assert_eq!(config.wled.known_ips.len(), 1);
    assert!(!config.wled.realtime_http_enabled);
    assert_eq!(config.wled.dedup_threshold, 0);
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
    assert_eq!(config.daemon.target_fps, 30);
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.listen_address, "127.0.0.1");
    assert_eq!(config.daemon.canvas_width, 320);
    assert_eq!(config.daemon.canvas_height, 200);
    assert_eq!(config.wled.default_protocol, WledProtocolConfig::Ddp);
    assert!(config.wled.realtime_http_enabled);
    assert!(config.wled.known_ips.is_empty());
    assert_eq!(config.wled.dedup_threshold, 2);
    assert!(config.include.is_empty());
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
        let _actions = lifecycle.on_discovered(device_id, &info, "cleanup", None);
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
async fn daemon_state_effect_engine_starts_idle() {
    let _guard = TestDataDirGuard::new().await;
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
    let _guard = TestDataDirGuard::new().await;
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
            active_effect_id: None,
            active_preset_id: None,
            control_values: std::collections::HashMap::new(),
            active_layout_id: Some(restored_layout.id.clone()),
            global_brightness: 1.0,
            wled_probe_ips: Vec::new(),
            wled_probe_targets: Vec::new(),
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
async fn daemon_shutdown_persists_active_runtime_session() {
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

    {
        let mut engine = state.effect_engine.lock().await;
        engine
            .activate_metadata(metadata.clone())
            .expect("native effect should activate");
        engine.set_active_preset_id("shutdown-preset".to_owned());
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
                family: DeviceFamily::Wled,
                model: None,
                connection_type: ConnectionType::Network,
                zones: vec![ZoneInfo {
                    name: "Main".to_owned(),
                    led_count: 30,
                    topology: DeviceTopologyHint::Strip,
                    color_format: DeviceColorFormat::Rgb,
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
    assert_eq!(snapshot.active_effect_id, Some(metadata.id.to_string()));
    assert_eq!(
        snapshot.active_preset_id,
        Some("shutdown-preset".to_owned())
    );
    assert_eq!(
        snapshot.wled_probe_ips,
        vec!["10.0.0.42".parse::<std::net::IpAddr>().expect("valid IP"),]
    );
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
            test_zone("zone_wled_mapped", "wled:desk"),
            test_zone("zone_wled_missing", "wled:wall"),
            test_zone("zone_wled_missing_dup", "wled:wall"),
            test_zone("zone_hue", "hue:bridge"),
        ],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let routing = BackendRoutingDebugSnapshot {
        backend_ids: vec!["usb".to_owned(), "wled".to_owned()],
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
                layout_device_id: "wled:desk".to_owned(),
                backend_id: "wled".to_owned(),
                device_id: "device_wled".to_owned(),
                backend_registered: true,
                queue_active: true,
            },
        ],
        orphaned_queues: Vec::<OrphanedQueueDebugEntry>::new(),
    };

    let unmapped = collect_unmapped_prefixed_layout_targets(&layout, &routing, "wled:");
    assert_eq!(unmapped, vec!["wled:wall".to_owned()]);
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
            test_zone("zone_hue", "hue:bridge"),
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

    let unmapped = collect_unmapped_prefixed_layout_targets(&layout, &routing, "wled:");
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
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 30,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
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

    let added =
        discovery::append_auto_layout_zones_for_device(&mut layout, "wled:desk-strip", &info);

    assert_eq!(added, 1);
    assert_eq!(layout.zones.len(), 1);
    assert_eq!(layout.zones[0].device_id, "wled:desk-strip");
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
        family: DeviceFamily::Custom("display".to_owned()),
        model: None,
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "Screen".to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
            },
            color_format: DeviceColorFormat::Rgb,
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

#[test]
fn append_auto_layout_zones_for_seiren_v3_uses_custom_mic_geometry() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Razer Seiren V3 Chroma".to_owned(),
        vendor: "Razer".to_owned(),
        family: DeviceFamily::Razer,
        model: None,
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 10,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
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

    let added =
        discovery::append_auto_layout_zones_for_device(&mut layout, "usb:1532:056f:test", &info);

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
fn append_auto_layout_zones_for_basilisk_v3_uses_custom_mouse_geometry() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Razer Basilisk V3".to_owned(),
        vendor: "Razer".to_owned(),
        family: DeviceFamily::Razer,
        model: None,
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 11,
            topology: DeviceTopologyHint::Matrix { rows: 1, cols: 11 },
            color_format: DeviceColorFormat::Rgb,
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

    let added =
        discovery::append_auto_layout_zones_for_device(&mut layout, "usb:1532:0099:test", &info);

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
fn append_auto_layout_zones_for_dense_matrix_device_clamps_height_without_panicking() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Ableton Push 2".to_owned(),
        vendor: "Ableton".to_owned(),
        family: DeviceFamily::Custom("Ableton".to_owned()),
        model: Some("push2".to_owned()),
        connection_type: ConnectionType::Usb,
        zones: vec![
            ZoneInfo {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Buttons Above".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Buttons Below".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Scene Launch".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Transport".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Touch Strip".to_owned(),
                led_count: 31,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
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
fn reconcile_auto_layout_zones_for_device_updates_existing_seiren_auto_zone() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Razer Seiren V3 Chroma".to_owned(),
        vendor: "Razer".to_owned(),
        family: DeviceFamily::Razer,
        model: None,
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 10,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
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
            id: "auto-usb-1532-056f-test-main".to_owned(),
            name: "Razer Seiren V3 Chroma".to_owned(),
            device_id: "usb:1532:056f:test".to_owned(),
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
            led_mapping: None,
        }],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let repaired =
        discovery::reconcile_auto_layout_zones_for_device(&mut layout, "usb:1532:056f:test", &info);

    assert_eq!(repaired, 1);
    match &layout.zones[0].topology {
        LedTopology::Custom { positions } => assert_eq!(positions.len(), 10),
        other => panic!("expected custom topology, got {other:?}"),
    }
    assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.2, 0.08));
}

#[test]
fn reconcile_auto_layout_zones_for_device_removes_stale_auto_zones() {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "PrismRGB Prism S".to_owned(),
        vendor: "PrismRGB".to_owned(),
        family: DeviceFamily::PrismRgb,
        model: Some("prism_s".to_owned()),
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "GPU Strimer".to_owned(),
            led_count: 108,
            topology: DeviceTopologyHint::Matrix { rows: 4, cols: 27 },
            color_format: DeviceColorFormat::Rgb,
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
