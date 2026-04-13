//! End-to-end daemon integration tests.
//!
//! Tests the full daemon lifecycle: initialization, subsystem wiring,
//! config loading, and graceful shutdown. Uses real subsystems (no mocks).

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::input::InputManager;
use hypercolor_daemon::runtime_state::{self, RuntimeSessionSnapshot};
use hypercolor_daemon::startup::{DaemonState, default_config, load_config};
use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::config::{CURRENT_SCHEMA_VERSION, RenderAccelerationMode};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{ControlBinding, ControlValue, EffectSource};
use hypercolor_types::sensor::SystemSnapshot;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, watch};

/// Minimal TOML that parses into a valid `HypercolorConfig`.
const MINIMAL_TOML: &str = "schema_version = 3\n";

static CONFIG_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct TestConfigDirGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    #[allow(dead_code)]
    config_dir: PathBuf,
}

impl TestConfigDirGuard {
    async fn new() -> Self {
        let lock = CONFIG_DIR_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let config_dir = dir.path().join("config");
        ConfigManager::set_config_dir_override(Some(config_dir.clone()));
        Self {
            _lock: lock,
            _dir: dir,
            config_dir,
        }
    }
}

impl Drop for TestConfigDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_config_dir_override(None);
    }
}

struct TestDataDirGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    #[allow(dead_code)]
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
}

impl Drop for TestDataDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
    }
}

fn temp_config_file() -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("failed to create temp file");
    f.write_all(MINIMAL_TOML.as_bytes())
        .expect("failed to write temp config");
    f.flush().expect("failed to flush temp config");
    f
}

fn test_input_manager() -> InputManager {
    let (_tx, rx) = watch::channel(Arc::new(SystemSnapshot::empty()));
    let mut input_manager = InputManager::new();
    input_manager.set_sensor_snapshot_receiver(rx);
    input_manager
}

fn make_device_info(name: &str, led_count: u32) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.to_string(),
        vendor: "TestCorp".to_string(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "main".to_string(),
            led_count,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("1.0.0".to_string()),
        capabilities: DeviceCapabilities {
            led_count,
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

// ═════════════════════════════════════════════════════════════════════════════
// DaemonState Lifecycle Tests
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn daemon_lifecycle_initialize_start_shutdown() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    // Verify initial state — all subsystems created but not started
    assert!(state.device_registry.is_empty().await);
    {
        let scenes = state.scene_manager.read().await;
        assert_eq!(scenes.scene_count(), 1);
        assert!(scenes.active_scene_id().is_some_and(|id| id.is_default()));
        assert!(scenes.active_render_groups().is_empty());
    }
    {
        let loop_guard = state.render_loop.read().await;
        assert!(!loop_guard.is_running());
    }

    // Start
    state.start().await.expect("start should succeed");

    // Verify render loop is running
    {
        let loop_guard = state.render_loop.read().await;
        assert!(loop_guard.is_running());
    }

    // Shutdown
    state.shutdown().await.expect("shutdown should succeed");

    // Verify render loop is stopped
    {
        let loop_guard = state.render_loop.read().await;
        assert!(!loop_guard.is_running());
    }

    // Verify scene-backed runtime state returns to the empty default scene
    {
        let scenes = state.scene_manager.read().await;
        assert!(scenes.active_scene_id().is_some_and(|id| id.is_default()));
        assert!(scenes.active_render_groups().is_empty());
    }
}

#[tokio::test]
async fn daemon_shutdown_publishes_events() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    let mut rx = state.event_bus.subscribe_all();

    state.start().await.expect("start");

    // Drain the DaemonStarted event
    let started = rx.recv().await.expect("should receive startup event");
    assert!(matches!(
        started.event,
        hypercolor_types::event::HypercolorEvent::DaemonStarted { .. }
    ));

    state.shutdown().await.expect("shutdown");

    // Discovery workers may emit additional events during shutdown; keep
    // receiving until the terminal DaemonShutdown event arrives.
    let shutdown = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.expect("should receive event");
            if matches!(
                event.event,
                hypercolor_types::event::HypercolorEvent::DaemonShutdown { .. }
            ) {
                break event;
            }
        }
    })
    .await
    .expect("timed out waiting for DaemonShutdown event");
    assert!(matches!(
        shutdown.event,
        hypercolor_types::event::HypercolorEvent::DaemonShutdown { .. }
    ));
}

#[tokio::test]
async fn daemon_double_shutdown_is_safe() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    state.start().await.expect("start");
    state.shutdown().await.expect("first shutdown");
    state
        .shutdown()
        .await
        .expect("second shutdown should also succeed");
}

#[tokio::test]
async fn daemon_start_restores_last_runtime_session() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.daemon.start_profile = "last".to_owned();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    let requested_speed = ControlValue::Float(7.0);
    let preset_id = hypercolor_types::library::PresetId::new();
    let effect_id = {
        let registry = state.effect_registry.read().await;
        let (_, entry) = registry
            .iter()
            .find(|(_, entry)| {
                matches!(entry.metadata.source, EffectSource::Native { .. })
                    && entry.metadata.control_by_id("speed").is_some()
            })
            .expect("expected at least one native effect with a speed control in registry");
        entry.metadata.id.to_string()
    };
    let snapshot = RuntimeSessionSnapshot {
        active_scene_id: Some(hypercolor_types::scene::SceneId::DEFAULT.to_string()),
        default_scene_groups: Vec::new(),
        active_effect_id: Some(effect_id.clone()),
        active_preset_id: Some(preset_id.to_string()),
        control_values: HashMap::from([("speed".to_owned(), requested_speed.clone())]),
        control_bindings: HashMap::from([(
            "speed".to_owned(),
            ControlBinding {
                sensor: "cpu_temp".to_owned(),
                sensor_min: 30.0,
                sensor_max: 100.0,
                target_min: 0.0,
                target_max: 1.0,
                deadband: 0.5,
                smoothing: 0.25,
            },
        )]),
        active_layout_id: None,
        global_brightness: 1.0,
        wled_probe_ips: Vec::new(),
        wled_probe_targets: Vec::new(),
    };
    runtime_state::save(&state.runtime_state_path, &snapshot).expect("persist runtime snapshot");

    state
        .start()
        .await
        .expect("start should restore runtime state");

    let scenes = state.scene_manager.read().await;
    let primary_group = scenes
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .expect("primary group should be restored on startup");
    assert_eq!(
        primary_group.effect_id.map(|id| id.to_string()),
        Some(effect_id)
    );
    assert_eq!(
        primary_group.preset_id.map(|preset| preset.to_string()),
        Some(preset_id.to_string())
    );
    assert_eq!(primary_group.controls.get("speed"), Some(&requested_speed));
    let binding = primary_group
        .control_bindings
        .get("speed")
        .expect("speed binding should be restored");
    assert_eq!(binding.sensor, "cpu_temp");
    drop(scenes);

    state.shutdown().await.expect("shutdown should succeed");
}

// ═════════════════════════════════════════════════════════════════════════════
// Config Loading Tests
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_loading_defaults_have_correct_schema() {
    let _guard = TestConfigDirGuard::new().await;
    let (config, _path) = load_config(None).await.expect("should load defaults");

    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(config.daemon.target_fps, 30);
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.listen_address, "127.0.0.1");
    assert_eq!(config.daemon.canvas_width, DEFAULT_CANVAS_WIDTH);
    assert_eq!(config.daemon.canvas_height, DEFAULT_CANVAS_HEIGHT);
    assert_eq!(config.daemon.max_devices, 32);
}

#[tokio::test]
async fn config_loading_all_sub_configs_have_defaults() {
    let _guard = TestConfigDirGuard::new().await;
    let (config, _path) = load_config(None).await.expect("should load defaults");

    // Audio config defaults
    assert!(config.audio.enabled);
    assert_eq!(config.audio.device, "default");
    assert_eq!(config.audio.fft_size, 1024);

    // Web config defaults
    assert!(config.web.enabled);
    assert_eq!(config.web.websocket_fps, 30);

    // Discovery config defaults
    assert!(config.discovery.mdns_enabled);
    assert!(config.discovery.wled_scan);
    assert!(config.discovery.hue_scan);
    assert!(config.discovery.nanoleaf_scan);

    // Feature flags default to false
    assert!(!config.features.wasm_plugins);
    assert!(!config.features.hue_entertainment);
    assert!(!config.features.midi_input);

    // Network backend config defaults
    assert!(config.hue.use_cie_xy);
    assert_eq!(config.nanoleaf.transition_time, 1);
    assert_eq!(
        config.effect_engine.render_acceleration_mode,
        RenderAccelerationMode::Cpu
    );

    // TUI config defaults
    assert_eq!(config.tui.theme, "silkcircuit");
    assert_eq!(config.tui.preview_fps, 15);

    // D-Bus config defaults
    assert!(config.dbus.enabled);
    assert_eq!(config.dbus.bus_name, "tech.hyperbliss.hypercolor1");
}

#[tokio::test]
async fn config_loading_from_custom_file() {
    let toml_str = r"
schema_version = 3

[daemon]
target_fps = 45
canvas_width = 640
canvas_height = 400
port = 8888

[audio]
enabled = false
fft_size = 2048

[features]
wasm_plugins = true
";

    let mut temp = NamedTempFile::new().expect("create temp file");
    temp.write_all(toml_str.as_bytes()).expect("write config");
    temp.flush().expect("flush");

    let (config, path) = load_config(Some(temp.path()))
        .await
        .expect("should load custom config");

    assert_eq!(path, temp.path());
    assert_eq!(config.daemon.target_fps, 45);
    assert_eq!(config.daemon.canvas_width, 640);
    assert_eq!(config.daemon.canvas_height, 400);
    assert_eq!(config.daemon.port, 8888);
    assert!(!config.audio.enabled);
    assert_eq!(config.audio.fft_size, 2048);
    assert!(config.features.wasm_plugins);
}

// ═════════════════════════════════════════════════════════════════════════════
// API + State Integration
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn api_state_device_list_starts_empty_and_grows() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    // Initially empty
    let devices = state.device_registry.list().await;
    assert!(devices.is_empty(), "device list should start empty");
    assert_eq!(state.device_registry.len().await, 0);

    // Add a device directly to the registry
    let device_info = make_device_info("WLED Living Room", 60);
    let id = state.device_registry.add(device_info).await;

    // Now should have 1 device
    let devices = state.device_registry.list().await;
    assert_eq!(devices.len(), 1, "device list should have 1 entry");
    assert_eq!(devices[0].info.name, "WLED Living Room");
    assert_eq!(devices[0].info.total_led_count(), 60);

    // Can look up by ID
    let found = state.device_registry.get(&id).await;
    assert!(found.is_some());
    assert_eq!(found.expect("device").info.name, "WLED Living Room");

    // Add another device
    let device_info2 = make_device_info("USB RGB Controller", 40);
    state.device_registry.add(device_info2).await;

    assert_eq!(state.device_registry.len().await, 2);
}

#[tokio::test]
async fn api_state_default_scene_starts_without_active_groups() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    // Verify the default scene is active and empty until something applies a group.
    {
        let scenes = state.scene_manager.read().await;
        assert!(scenes.active_scene_id().is_some_and(|id| id.is_default()));
        assert!(scenes.active_render_groups().is_empty());
    }
}

#[tokio::test]
async fn api_state_scene_manager_accessible_through_rwlock() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf())
        .expect("initialization should succeed");

    // Write lock: create a scene
    {
        let mut scenes = state.scene_manager.write().await;
        let scene = hypercolor_core::scene::make_scene("Test Scene");
        scenes.create(scene).expect("create scene");
    }

    // Read lock: verify scene exists
    {
        let scenes = state.scene_manager.read().await;
        assert_eq!(scenes.scene_count(), 2);
        let listed = scenes.list();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|scene| scene.id.is_default()));
        assert!(listed.iter().any(|scene| scene.name == "Test Scene"));
    }
}

#[tokio::test]
async fn api_state_config_snapshot_matches_init_config() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.daemon.target_fps = 45;

    let toml_str = "schema_version = 3\n[daemon]\ntarget_fps = 45\n";
    let mut temp = NamedTempFile::new().expect("create temp");
    temp.write_all(toml_str.as_bytes()).expect("write");
    temp.flush().expect("flush");

    let state = DaemonState::initialize(&config, temp.path().to_path_buf()).expect("initialize");

    let snapshot = state.config();
    assert_eq!(snapshot.daemon.target_fps, 45);
    assert_eq!(snapshot.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(
        snapshot.effect_engine.render_acceleration_mode,
        RenderAccelerationMode::Cpu
    );
}

#[tokio::test]
async fn api_state_event_bus_subscriber_works() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(&config, temp.path().to_path_buf()).expect("initialize");

    // Subscribe to events
    let mut rx = state.event_bus.subscribe_all();
    assert_eq!(state.event_bus.subscriber_count(), 1);

    // Publish custom event
    state.event_bus.publish(
        hypercolor_types::event::HypercolorEvent::BrightnessChanged {
            old: 100,
            new_value: 80,
        },
    );

    let event = rx.recv().await.expect("receive event");
    assert!(matches!(
        event.event,
        hypercolor_types::event::HypercolorEvent::BrightnessChanged {
            old: 100,
            new_value: 80,
        }
    ));
}

#[tokio::test]
async fn daemon_render_loop_uses_configured_fps() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.daemon.target_fps = 30;

    let toml_str = "schema_version = 3\n[daemon]\ntarget_fps = 30\n";
    let mut temp = NamedTempFile::new().expect("create temp");
    temp.write_all(toml_str.as_bytes()).expect("write");
    temp.flush().expect("flush");

    let state = DaemonState::initialize(&config, temp.path().to_path_buf()).expect("initialize");

    {
        let rl = state.render_loop.read().await;
        assert_eq!(
            rl.fps_controller().tier(),
            hypercolor_core::engine::FpsTier::Medium,
            "30fps should resolve to Medium tier"
        );
    }
}
