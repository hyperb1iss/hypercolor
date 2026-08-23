//! End-to-end daemon integration tests.
//!
//! Tests the full daemon lifecycle: initialization, subsystem wiring,
//! config loading, and graceful shutdown. Uses real subsystems (no mocks).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use hypercolor_core::config::{BootConfig, ConfigManager};
use hypercolor_core::input::InputManager;
use hypercolor_daemon::extensions::DaemonLifecycleExtension;
use hypercolor_daemon::startup::{DaemonState, config_sources, default_config};
use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::config::{
    CURRENT_SCHEMA_VERSION, HypercolorConfig, RenderAccelerationMode, ServoGpuImportMode,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, SegmentInfo,
};
use hypercolor_types::scene::SceneId;
use hypercolor_types::sensor::SystemSnapshot;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, watch};

/// Minimal TOML that parses into a valid `HypercolorConfig`.
const MINIMAL_TOML: &str = "schema_version = 5\n";

static PATH_OVERRIDE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct TestConfigDirGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    #[allow(dead_code)]
    config_dir: PathBuf,
}

impl TestConfigDirGuard {
    async fn new() -> Self {
        let lock = PATH_OVERRIDE_LOCK.lock().await;
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
    _config_dir: PathBuf,
    _state_dir: PathBuf,
}

impl TestDataDirGuard {
    async fn new() -> Self {
        let lock = PATH_OVERRIDE_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let data_dir = dir.path().join("data");
        let config_dir = dir.path().join("config");
        let state_dir = dir.path().join("state");
        ConfigManager::set_data_dir_override(Some(data_dir.clone()));
        ConfigManager::set_config_dir_override(Some(config_dir.clone()));
        ConfigManager::set_state_dir_override(Some(state_dir.clone()));
        Self {
            _lock: lock,
            _dir: dir,
            data_dir,
            _config_dir: config_dir,
            _state_dir: state_dir,
        }
    }
}

impl Drop for TestDataDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
        ConfigManager::set_config_dir_override(None);
        ConfigManager::set_state_dir_override(None);
    }
}

/// A boot config for a test that synthesized its own settings.
///
/// Initialization consumes the boot config, so each call mints one.
fn boot_config(config: &HypercolorConfig) -> BootConfig {
    BootConfig::from_config_unchecked(config.clone())
}

/// A config manager whose live snapshot is exactly the config under test.
///
/// The daemon's own manager comes from the load pipeline; tests that
/// synthesize a config in memory materialize one over the same path the
/// daemon would persist to.
fn config_manager_for(config: &HypercolorConfig, path: &Path) -> Arc<ConfigManager> {
    Arc::new(ConfigManager::from_config_unchecked(
        path.to_path_buf(),
        config.clone(),
    ))
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

struct FailingStartupExtension {
    shutdowns: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl DaemonLifecycleExtension for FailingStartupExtension {
    fn name(&self) -> &'static str {
        "failing-startup-test"
    }

    async fn start(&self, _daemon: &DaemonState) -> anyhow::Result<()> {
        anyhow::bail!("intentional startup failure")
    }

    async fn shutdown(&self, _daemon: &DaemonState) -> anyhow::Result<()> {
        self.shutdowns.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn make_device_info(name: &str, led_count: u32) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.to_string(),
        vendor: "TestCorp".to_string(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "main".to_string(),
            led_count,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
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
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    // Verify initial state — all subsystems created but not started
    assert!(state.device_registry.is_empty().await);
    {
        let scenes = state.scene_manager.snapshot().await;
        assert_eq!(scenes.scene_count(), 1);
        assert!(scenes.active_scene_id().is_some_and(SceneId::is_default));
        assert_eq!(scenes.resolved_zones().len(), 1);
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

    // Verify scene-backed runtime state returns to the default zone.
    {
        let scenes = state.scene_manager.snapshot().await;
        assert!(scenes.active_scene_id().is_some_and(SceneId::is_default));
        assert_eq!(scenes.resolved_zones().len(), 1);
    }
}

#[tokio::test]
async fn daemon_shutdown_publishes_events() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    let mut rx = state.event_bus.subscribe_all();

    state.start().await.expect("start");

    let started = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.expect("should receive event");
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
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
async fn daemon_start_rolls_back_partial_startup() {
    let _data_guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.audio.enabled = false;
    config.capture.enabled = false;
    config.input.enabled = false;
    config.session.enabled = false;
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Cpu;
    config.rendering.servo_gpu_import.mode = ServoGpuImportMode::Off;
    config.effect_engine.watch_effects = false;
    config.discovery.background_enabled = false;
    config.daemon.start_scene = "none".to_owned();

    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    let shutdowns = Arc::new(AtomicUsize::new(0));
    state.register_lifecycle_extension(Arc::new(FailingStartupExtension {
        shutdowns: Arc::clone(&shutdowns),
    }));

    let error = state
        .start()
        .await
        .expect_err("extension start should fail");

    assert!(error.to_string().contains("failing-startup-test"));
    assert_eq!(shutdowns.load(Ordering::Relaxed), 1);
    assert!(state.input_publication_demands().is_none());
    assert!(!state.render_loop.read().await.is_running());
}

#[tokio::test]
async fn removed_runtime_effect_fields_are_rejected_on_startup() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.daemon.start_scene = "last".to_owned();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");
    *state.input_manager.lock().await = test_input_manager();

    let effect_id = {
        let registry = state.effect_registry.read().await;
        let (_, entry) = registry
            .iter()
            .find(|(_, entry)| entry.metadata.control_by_id("speed").is_some())
            .expect("expected at least one effect with a speed control in registry");
        entry.metadata.id.to_string()
    };
    std::fs::write(
        &state.runtime_state_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "active_scene_id": hypercolor_types::scene::SceneId::DEFAULT.to_string(),
            "default_scene_groups": [],
            "active_effect_id": effect_id,
            "control_values": {
                "speed": { "kind": "float", "value": 7.0 }
            }
        }))
        .expect("runtime snapshot json should serialize"),
    )
    .expect("runtime snapshot json should write");

    state
        .start()
        .await
        .expect("start should ignore invalid runtime state");

    let scenes = state.scene_manager.snapshot().await;
    let primary = scenes
        .active_scene()
        .and_then(|scene| scene.primary_zone())
        .expect("startup should keep the seeded Default zone");
    assert!(
        primary.layers.is_empty(),
        "startup should not hydrate removed runtime fields"
    );
    drop(scenes);

    state.shutdown().await.expect("shutdown should succeed");
}

// ═════════════════════════════════════════════════════════════════════════════
// Config Loading Tests
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_loading_defaults_have_correct_schema() {
    let _guard = TestConfigDirGuard::new().await;
    let config = ConfigManager::load_with_sources(config_sources(None, None, None))
        .expect("should load defaults")
        .boot;

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
    let config = ConfigManager::load_with_sources(config_sources(None, None, None))
        .expect("should load defaults")
        .boot;

    // Audio config defaults
    assert!(config.audio.enabled);
    assert_eq!(config.audio.device, "default");
    assert_eq!(config.audio.fft_size, 1024);

    // Web config defaults
    assert!(config.web.enabled);
    assert_eq!(config.web.websocket_fps, 30);

    // Discovery config defaults
    assert!(config.discovery.mdns_enabled);
    assert!(config.drivers["wled"].enabled);
    assert!(config.drivers["hue"].enabled);
    assert!(config.drivers["nanoleaf"].enabled);

    // Feature flags default to false
    assert!(!config.features.wasm_plugins);
    assert!(!config.features.hue_entertainment);
    assert!(!config.features.midi_input);

    assert_eq!(
        config.effect_engine.compositor_acceleration_mode,
        RenderAccelerationMode::Auto
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
schema_version = 5

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

    let loaded = ConfigManager::load_with_sources(config_sources(
        Some(temp.path().to_path_buf()),
        None,
        None,
    ))
    .expect("should load custom config");
    let config = loaded.boot;

    assert_eq!(loaded.manager.path(), temp.path());
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
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
async fn api_state_default_scene_starts_with_default_zone() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    // Verify the default scene is active with a selectable Default zone.
    {
        let scenes = state.scene_manager.snapshot().await;
        assert!(scenes.active_scene_id().is_some_and(SceneId::is_default));
        assert_eq!(scenes.resolved_zones().len(), 1);
    }
}

#[tokio::test]
async fn daemon_scene_service_returns_owned_snapshots() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    let mut snapshot = state.scene_manager.snapshot().await;
    snapshot
        .create(hypercolor_core::scene::make_scene("Detached Scene"))
        .expect("owned snapshot should remain mutable");

    let current = state.scene_manager.snapshot().await;
    assert_eq!(current.scene_count(), 1);
    assert!(current.list().iter().all(|scene| scene.id.is_default()));
}

#[tokio::test]
async fn api_state_config_snapshot_matches_init_config() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.daemon.target_fps = 45;

    let toml_str =
        format!("schema_version = {CURRENT_SCHEMA_VERSION}\n[daemon]\ntarget_fps = 45\n");
    let mut temp = NamedTempFile::new().expect("create temp");
    temp.write_all(toml_str.as_bytes()).expect("write");
    temp.flush().expect("flush");

    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialize");

    let snapshot = state.config();
    assert_eq!(snapshot.daemon.target_fps, 45);
    assert_eq!(snapshot.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(
        snapshot.effect_engine.compositor_acceleration_mode,
        RenderAccelerationMode::Auto
    );
}

#[tokio::test]
async fn api_state_event_bus_subscriber_works() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialize");

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

    let toml_str = "schema_version = 5\n[daemon]\ntarget_fps = 30\n";
    let mut temp = NamedTempFile::new().expect("create temp");
    temp.write_all(toml_str.as_bytes()).expect("write");
    temp.flush().expect("flush");

    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialize");

    {
        let rl = state.render_loop.read().await;
        assert_eq!(
            rl.fps_controller().tier(),
            hypercolor_core::engine::FpsTier::Medium,
            "30fps should resolve to Medium tier"
        );
    }
}
