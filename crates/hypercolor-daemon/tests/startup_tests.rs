//! Integration tests for daemon startup orchestration.

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::{Router, body::to_bytes, routing::get};
use hypercolor_core::config::{BootConfig, ConfigManager};
use hypercolor_core::device::manager::{
    BackendRoutingDebugSnapshot, LayoutRoutingDebugEntry, OrphanedQueueDebugEntry,
};
use hypercolor_core::engine::RenderLoopState;
use hypercolor_daemon::api::system::get_status;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::daemon::{
    DaemonRunOptions, bind_api_listener, effective_bind_target, effective_bind_targets,
    effective_startup_bind_targets, serve_api_listeners_with_shutdown_timeout,
    validate_network_bind_auth,
};
use hypercolor_daemon::display_preferences::{DisplayPreference, DisplayPreferencesStore};
use hypercolor_daemon::library::{JsonLibraryStore, LibraryStore};
use hypercolor_daemon::startup::{
    DaemonState, collect_unmapped_driver_layout_targets, collect_unmapped_prefixed_layout_targets,
    config_sources, default_config, install_signal_handlers, parse_config_toml,
};
use hypercolor_daemon::{layout_store, runtime_state};
use hypercolor_driver_api::{BackendInfo, DeviceBackend, OutputCadence};
use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::config::{
    CURRENT_SCHEMA_VERSION, EffectErrorFallbackPolicy, HypercolorConfig, InteractionRoutePolicy,
    NetworkAccessMode, RenderAccelerationMode,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    SegmentInfo, SegmentLayoutHint,
};
use hypercolor_types::effect::{EffectId, EffectSource};
use hypercolor_types::event::{EffectStopReason, HypercolorEvent};
use hypercolor_types::identity::LayoutId;
use hypercolor_types::layer::{BlendMode, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection, ZoneShape,
};
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::sync::Mutex;

type Result<T> = std::result::Result<T, DeviceError>;

macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(DeviceError::protocol("test backend", format!($($arg)*)))
    };
}

/// Minimal TOML content that `ConfigManager` can parse.
const MINIMAL_TOML: &str = "schema_version = 5\n";

fn write_scene_store(
    path: &Path,
    scenes: impl IntoIterator<Item = hypercolor_types::scene::Scene>,
) {
    std::fs::create_dir_all(
        path.parent()
            .expect("scene store path should have a parent"),
    )
    .expect("scene store directory should build");
    let scenes = scenes
        .into_iter()
        .map(|scene| (scene.id.to_string(), scene))
        .collect::<std::collections::HashMap<_, _>>();
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2,
            "scenes": scenes,
        }))
        .expect("scene store should serialize"),
    )
    .expect("scene store should write");
}

struct SeededEffectIdentity {
    effect_root: PathBuf,
    legacy_id: EffectId,
    device_id: DeviceId,
    scene_id: SceneId,
}

fn deterministic_html_effect_id(path: &Path) -> EffectId {
    let mut hash: u128 = 0x6c62_69f0_7bb0_14d9_8d4f_1283_7ec6_3b8b;
    for byte in format!("hypercolor:html:{}", path.display()).bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    EffectId::new(uuid::Uuid::from_bytes(bytes))
}

async fn seed_effect_identity_stores(
    guard: &TestDataDirGuard,
    builtin_id: &str,
) -> SeededEffectIdentity {
    let effect_root = guard.data_dir.join("startup-effects");
    let effect_path = effect_root.join(format!("{builtin_id}.html"));
    std::fs::create_dir_all(&effect_root).expect("effect root should build");
    std::fs::write(
        &effect_path,
        format!("<head><title>{builtin_id}</title><meta builtin-id=\"{builtin_id}\" /></head>"),
    )
    .expect("effect port should write");
    let effect_path = std::fs::canonicalize(effect_path).expect("effect path should canonicalize");
    let legacy_id = deterministic_html_effect_id(&effect_path);

    let mut named_scene = hypercolor_core::scene::make_scene("Legacy identity");
    let scene_id = named_scene.id;
    let mut zone = hypercolor_core::scene::default_primary_zone(SpatialLayout {
        id: "identity-test".into(),
        name: "Identity Test".to_owned(),
        description: None,
        canvas_width: DEFAULT_CANVAS_WIDTH,
        canvas_height: DEFAULT_CANVAS_HEIGHT,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    });
    zone.layers = vec![SceneLayer::from_effect(
        SceneLayerId::new(),
        legacy_id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];
    named_scene.zones.push(zone);
    write_scene_store(&guard.scenes_path(), [named_scene.clone()]);

    runtime_state::save(
        &guard.legacy_state_path("runtime-state.json"),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_zones: named_scene.zones.clone(),
            active_layout_id: None,
            manual_paused: false,
        },
    )
    .expect("runtime identity should persist");

    let device_id = DeviceId::new();
    let mut display =
        DisplayPreferencesStore::new(guard.legacy_state_path("display-preferences.json"))
            .expect("display store should open");
    display
        .set(
            device_id,
            DisplayPreference {
                effect_id: legacy_id,
                controls: HashMap::new(),
                blend_mode: BlendMode::Alpha,
                opacity: 1.0,
            },
        )
        .expect("display identity should persist");

    let library = JsonLibraryStore::open(guard.data_dir.join("library.json"))
        .expect("library store should open");
    library
        .upsert_favorite(legacy_id, 1)
        .await
        .expect("library identity should persist");

    SeededEffectIdentity {
        effect_root,
        legacy_id,
        device_id,
        scene_id,
    }
}

async fn registry_effect_id(state: &DaemonState, source_stem: &str) -> EffectId {
    state
        .effect_registry
        .read()
        .await
        .iter()
        .find(|(_, entry)| entry.metadata.source.source_stem() == Some(source_stem))
        .map(|(effect_id, _)| *effect_id)
        .unwrap_or_else(|| panic!("registry should contain {source_stem}"))
}

async fn assert_effect_identity_everywhere(
    state: &DaemonState,
    guard: &TestDataDirGuard,
    seeded: &SeededEffectIdentity,
    expected_id: EffectId,
) {
    let manager = state.scene_manager.snapshot().await;
    let scene_ids = manager
        .get(&seeded.scene_id)
        .expect("seeded scene should load")
        .zones
        .iter()
        .flat_map(Zone::effect_ids)
        .collect::<Vec<_>>();
    assert_eq!(scene_ids, vec![expected_id]);

    let runtime = runtime_state::load(&guard.runtime_state_path())
        .expect("runtime state should load")
        .expect("runtime state should exist");
    assert_eq!(
        runtime
            .default_scene_zones
            .iter()
            .flat_map(Zone::effect_ids)
            .collect::<Vec<_>>(),
        vec![expected_id]
    );

    let scenes = hypercolor_daemon::scene_store::load(&guard.scenes_path())
        .expect("scene store should load");
    assert_eq!(
        scenes
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(Zone::effect_ids)
            .collect::<Vec<_>>(),
        vec![expected_id]
    );

    assert_eq!(
        state
            .display_preferences
            .read()
            .await
            .get(seeded.device_id)
            .map(|preference| preference.effect_id),
        Some(expected_id)
    );
    let display = DisplayPreferencesStore::load(&guard.state_path("display-preferences.json"))
        .expect("display store should reopen");
    assert_eq!(
        display
            .get(seeded.device_id)
            .map(|preference| preference.effect_id),
        Some(expected_id)
    );

    assert_eq!(
        state.library_store.list_favorites().await[0].effect_id,
        expected_id
    );
    let library = JsonLibraryStore::open(guard.data_dir.join("library.json"))
        .expect("library store should reopen");
    assert_eq!(library.list_favorites().await[0].effect_id, expected_id);
}

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static CONFIG_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn write_layout_store_fixture(
    path: &Path,
    layouts: &std::collections::HashMap<String, SpatialLayout>,
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("layout fixture directory should exist");
    }
    let mut entries = layouts.values().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    let payload = serde_json::to_vec_pretty(&entries).expect("layout fixture should serialize");
    std::fs::write(path, payload).expect("layout fixture should write");
}

#[derive(Clone)]
struct StuckHandlerState {
    entered: Arc<tokio::sync::Notify>,
}

struct ShutdownCleanupBackend {
    expected_device_id: DeviceId,
    disconnects: Arc<AtomicUsize>,
    connected: AtomicBool,
}

struct StaticHoldRecordingBackend {
    writes: Arc<AtomicUsize>,
    write_notify: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl DeviceBackend for StaticHoldRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "static-hold-test".to_owned(),
            name: "Static Hold Test Backend".to_owned(),
            description: "Records paused late-connect output".to_owned(),
        }
    }

    fn adopt_device(
        &self,
        _discovered: &hypercolor_driver_api::DiscoveredDevice,
    ) -> std::result::Result<(), hypercolor_types::device::DeviceError> {
        Ok(())
    }

    async fn connect(&self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn write_colors(&self, _id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if colors.iter().any(|color| *color != [0, 0, 0]) {
            bail!("static hold emitted a non-black color");
        }
        self.writes.fetch_add(1, Ordering::Release);
        self.write_notify.notify_waiters();
        Ok(())
    }

    fn output_cadence(&self, _id: &DeviceId) -> Option<OutputCadence> {
        Some(OutputCadence::from_fps(60).with_max_frame_silence(Duration::from_millis(20)))
    }
}

impl ShutdownCleanupBackend {
    fn new(expected_device_id: DeviceId, disconnects: Arc<AtomicUsize>) -> Self {
        Self {
            expected_device_id,
            disconnects,
            connected: AtomicBool::new(false),
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

    fn adopt_device(
        &self,
        _discovered: &hypercolor_driver_api::DiscoveredDevice,
    ) -> std::result::Result<(), hypercolor_types::device::DeviceError> {
        Ok(())
    }

    async fn connect(&self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        self.connected.store(true, Ordering::Release);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected.load(Ordering::Acquire) {
            bail!("disconnect called while backend was not connected");
        }
        self.connected.store(false, Ordering::Release);
        self.disconnects.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn write_colors(&self, _id: &DeviceId, _colors: &[[u8; 3]]) -> Result<()> {
        Ok(())
    }
}

struct TestDataDirGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    state_dir: PathBuf,
}

impl TestDataDirGuard {
    async fn new() -> Self {
        let lock = DATA_DIR_LOCK.lock().await;
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let data_dir = dir.path().join("data");
        let state_dir = dir.path().join("state");
        ConfigManager::set_data_dir_override(Some(data_dir.clone()));
        ConfigManager::set_state_dir_override(Some(state_dir.clone()));
        Self {
            _lock: lock,
            _dir: dir,
            data_dir,
            state_dir,
        }
    }

    fn layouts_path(&self) -> PathBuf {
        self.data_dir.join("layouts.json")
    }

    fn runtime_state_path(&self) -> PathBuf {
        self.state_dir.join("runtime-state.json")
    }

    fn legacy_state_path(&self, file_name: &str) -> PathBuf {
        self.data_dir.join(file_name)
    }

    fn state_path(&self, file_name: &str) -> PathBuf {
        self.state_dir.join(file_name)
    }

    fn scenes_path(&self) -> PathBuf {
        self.data_dir.join("scenes.json")
    }
}

impl Drop for TestDataDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
        ConfigManager::set_state_dir_override(None);
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

#[tokio::test]
async fn daemon_initialization_relocates_machine_state_out_of_data() {
    let guard = TestDataDirGuard::new().await;
    std::fs::create_dir_all(&guard.data_dir).expect("legacy data directory should be created");

    let legacy_documents = [
        (
            "driver-inventory.json",
            serde_json::json!({"schema_version": 1, "drivers": {}}),
        ),
        ("display-preferences.json", serde_json::json!({})),
        (
            "device-settings.json",
            serde_json::json!({
                "schema_version": 3,
                "global_brightness": 0.42,
                "devices": {},
                "driver_controls": {},
            }),
        ),
        (
            "runtime-state.json",
            serde_json::to_value(runtime_state::RuntimeSessionSnapshot::default())
                .expect("runtime snapshot should serialize"),
        ),
        (
            "device-aliases.json",
            serde_json::json!({
                "schema_version": 2,
                "aliases": {},
                "quarantined_keys": [],
                "collisions": [],
            }),
        ),
    ];
    for (file_name, document) in &legacy_documents {
        std::fs::write(
            guard.legacy_state_path(file_name),
            serde_json::to_vec_pretty(document).expect("legacy document should serialize"),
        )
        .expect("legacy document should be written");
    }

    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("state migration should succeed");

    assert_eq!(state.runtime_state_path, guard.runtime_state_path());
    assert_eq!(
        state.device_aliases_path,
        guard.state_path("device-aliases.json")
    );
    assert_eq!(
        state.driver_host.driver_inventory().path(),
        guard.state_path("driver-inventory.json")
    );
    assert_eq!(state.output_power.global_brightness(), 0.42);
    for (file_name, _) in &legacy_documents {
        assert!(guard.state_path(file_name).exists());
        assert!(!guard.legacy_state_path(file_name).exists());
    }
}

/// Create a temp file pre-populated with valid minimal TOML config.
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

async fn stuck_handler(State(state): State<StuckHandlerState>) -> &'static str {
    state.entered.notify_one();
    std::future::pending::<&'static str>().await
}

fn compact_perimeter_layout_hint() -> SegmentLayoutHint {
    SegmentLayoutHint::custom_grid(
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

fn shutdown_cleanup_device_info(id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id,
        name: "Shutdown Device".to_owned(),
        vendor: "TestVendor".to_owned(),
        family: DeviceFamily::named("cleanup"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("cleanup", "cleanup", ConnectionType::Network),
        segments: vec![SegmentInfo {
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
    // location, the pipeline succeeds with defaults.
    let config = ConfigManager::load_with_sources(config_sources(None, None, None))
        .expect("default config should load")
        .boot;
    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(config.daemon.target_fps, 30);
    assert_eq!(config.daemon.port, 9420);
}

#[tokio::test]
async fn load_config_reads_toml_file() {
    let toml_content = r#"
schema_version = 5

[daemon]
target_fps = 30
port = 8080
listen_address = "0.0.0.0"
"#;

    let mut temp = NamedTempFile::new().expect("failed to create temp file");
    temp.write_all(toml_content.as_bytes())
        .expect("failed to write temp config");

    let loaded = ConfigManager::load_with_sources(config_sources(
        Some(temp.path().to_path_buf()),
        None,
        None,
    ))
    .expect("config should load from file");

    assert_eq!(loaded.boot.daemon.target_fps, 30);
    assert_eq!(loaded.boot.daemon.port, 8080);
    assert_eq!(loaded.boot.daemon.listen_address, "0.0.0.0");
    assert_eq!(loaded.manager.path(), temp.path());
}

#[cfg(not(feature = "wgpu"))]
#[tokio::test]
async fn initialize_rejects_explicit_gpu_render_acceleration_without_wgpu_feature() {
    let _guard = TestDataDirGuard::new().await;
    let temp = temp_config_file();
    let mut config = default_config();
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Gpu;

    let Err(error) = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    ) else {
        panic!("gpu render acceleration should fail explicitly without wgpu support");
    };

    assert!(format!("{error:#}").contains("rebuild hypercolor-daemon with the `wgpu` feature"));
}

#[tokio::test]
async fn initialize_rejects_a_corrupt_scene_store_without_overwriting_it() {
    let guard = TestDataDirGuard::new().await;
    std::fs::create_dir_all(&guard.data_dir).expect("test data directory should exist");
    let corrupt = "{ definitely not scene json";
    std::fs::write(guard.scenes_path(), corrupt).expect("corrupt scene store should write");
    let temp = temp_config_file();
    let config = default_config();

    let Err(error) = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    ) else {
        panic!("corrupt scene persistence must prevent startup");
    };

    assert!(
        format!("{error:#}").contains("failed to load scenes"),
        "the startup error identifies scene persistence: {error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(guard.scenes_path())
            .expect("corrupt scene store should remain readable"),
        corrupt,
        "startup must not replace corrupt scene persistence"
    );
}

#[cfg(not(feature = "wgpu"))]
#[tokio::test]
async fn status_reports_auto_render_acceleration_cpu_fallback_without_wgpu_feature() {
    let _guard = TestDataDirGuard::new().await;
    let temp = temp_config_file();
    let mut config = default_config();
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Auto;

    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("auto render acceleration should initialize with CPU fallback");
    state
        .start()
        .await
        .expect("subsystems should start before the API reads live state");
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

    match DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    ) {
        Ok(daemon) => drop(daemon),
        Err(error) => {
            assert!(format!("{error:#}").contains("gpu compositor acceleration is unavailable"));
        }
    }
}

#[test]
fn load_config_errors_on_explicit_missing_path() {
    let missing = PathBuf::from("/tmp/hypercolor_does_not_exist_xyz.toml");
    let result = ConfigManager::load_with_sources(config_sources(Some(missing), None, None));
    assert!(
        result.is_err(),
        "should error when explicit path is missing"
    );
}

// ── Config Parsing ──────────────────────────────────────────────────────────

#[test]
fn parse_config_toml_minimal() {
    let config = parse_config_toml(MINIMAL_TOML).expect("minimal config should parse");
    // Parsing a string runs the same migrate and normalize a file load
    // runs, so an old schema comes back migrated forward.
    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    // All sections should have serde defaults.
    assert_eq!(config.daemon.target_fps, 30);
    assert!(config.audio.enabled);
}

#[test]
fn parse_config_toml_with_overrides() {
    let toml_str = r#"
schema_version = 5

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
fn parse_config_toml_runs_the_shared_normalize_and_seed() {
    let config = parse_config_toml("schema_version = 5\n[audio]\ndevice = \"Auto\"\n")
        .expect("current-schema config should parse");

    // Normalization: audio device aliases canonicalize.
    assert_eq!(config.audio.device, "default");
    // Seeding: the builtin driver entries are installed.
    assert!(config.drivers.contains_key("wled"));
    assert_eq!(config.input.preview_route, InteractionRoutePolicy::Browser);
}

#[test]
fn parse_config_toml_refuses_an_outdated_schema() {
    let error = parse_config_toml("schema_version = 3\n")
        .expect_err("an outdated schema must be refused, not migrated");
    let rendered = format!("{error:#}");

    assert!(rendered.contains("schema_version 3"), "{rendered}");
    assert!(rendered.contains("schema_version = 5"), "{rendered}");
    assert!(rendered.contains(r#"daemon_route = "merge""#), "{rendered}");
    assert!(
        rendered.contains(r#"preview_route = "browser""#),
        "{rendered}"
    );
}

#[test]
fn parse_config_toml_refuses_a_newer_schema() {
    let error = parse_config_toml("schema_version = 6\n")
        .expect_err("a future schema must be refused, not read");
    let rendered = format!("{error:#}");

    assert!(rendered.contains("schema_version 6"), "{rendered}");
    assert!(rendered.contains("newer hypercolor"), "{rendered}");
}

#[test]
fn loaded_config_and_manager_agree_after_one_load() {
    let dir = tempfile::tempdir().expect("tempdir should be created");
    let path = dir.path().join("hypercolor.toml");
    std::fs::write(
        &path,
        "schema_version = 5\n[audio]\ndevice = \"Microphone\"\n[daemon]\nport = 9421\n",
    )
    .expect("config file should write");

    let loaded = ConfigManager::load_with_sources(config_sources(Some(path), None, None))
        .expect("config should load");
    let live = loaded.manager.live();

    // One materialization: the boot config and the retained manager are
    // the same normalized, seeded document.
    assert_eq!(loaded.boot.audio.device, "microphone");
    assert_eq!(live.audio.device, "microphone");
    assert_eq!(live.daemon.port, 9421);
    assert!(live.drivers.contains_key("wled"));
    assert_eq!(loaded.boot.drivers, live.drivers);
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
    assert_eq!(config.schema_version, 5);
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
    config.network.access_mode = NetworkAccessMode::Custom;
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

    validate_network_bind_auth(bind, false, false).expect("localhost should not require API key");
}

#[test]
fn network_bind_auth_allows_ipv6_loopback_without_control_key() {
    let bind = "[::1]:9420"
        .parse::<SocketAddr>()
        .expect("IPv6 loopback bind target should parse as a socket address");

    validate_network_bind_auth(bind, false, false)
        .expect("IPv6 localhost should not require API key");
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
        validate_network_bind_auth(bind, false, false).expect_err("listen-all should require auth");
    let message = error.to_string();
    assert!(message.contains("0.0.0.0:9420"));
    assert!(message.contains("HYPERCOLOR_API_KEY"));
}

#[test]
fn network_bind_auth_rejects_ipv6_all_without_control_key() {
    let bind = "[::]:9420"
        .parse::<SocketAddr>()
        .expect("IPv6 all-interface bind target should parse as a socket address");

    let error = validate_network_bind_auth(bind, false, false)
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

    let error = validate_network_bind_auth(bind, false, false)
        .expect_err("remote access should require auth");
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

    validate_network_bind_auth(bind, true, false)
        .expect("control API key should allow network bind");
}

#[test]
fn network_bind_auth_allows_network_bind_with_explicit_unauthenticated_access() {
    let bind = "0.0.0.0:9420"
        .parse::<SocketAddr>()
        .expect("all-interface bind target should parse as a socket address");

    validate_network_bind_auth(bind, false, true)
        .expect("explicit unauthenticated access should allow network bind");
}

#[test]
fn startup_bind_targets_fall_back_to_loopback_for_config_remote_access_without_control_key() {
    let mut config = default_config();
    config.network.remote_access = true;
    let options = DaemonRunOptions::default();

    let (targets, fell_back) = effective_startup_bind_targets(&options, &config, false, false);

    assert!(fell_back);
    assert_eq!(targets, vec!["127.0.0.1:9420", "[::1]:9420"]);
}

#[test]
fn startup_bind_targets_keep_config_remote_access_with_control_key() {
    let mut config = default_config();
    config.network.remote_access = true;
    let options = DaemonRunOptions::default();

    let (targets, fell_back) = effective_startup_bind_targets(&options, &config, true, false);

    assert!(!fell_back);
    assert_eq!(targets, vec!["0.0.0.0:9420", "[::]:9420"]);
}

#[test]
fn startup_bind_targets_keep_config_remote_access_with_explicit_unauthenticated_access() {
    let mut config = default_config();
    config.network.remote_access = true;
    config.network.allow_unauthenticated_remote_access = true;
    let options = DaemonRunOptions::default();

    let (targets, fell_back) = effective_startup_bind_targets(&options, &config, false, true);

    assert!(!fell_back);
    assert_eq!(targets, vec!["0.0.0.0:9420", "[::]:9420"]);
}

#[test]
fn startup_bind_targets_keep_explicit_listen_all_for_auth_validation() {
    let config = default_config();
    let options = DaemonRunOptions {
        listen_all: true,
        ..DaemonRunOptions::default()
    };

    let (targets, fell_back) = effective_startup_bind_targets(&options, &config, false, false);

    assert!(!fell_back);
    assert_eq!(targets, vec!["0.0.0.0:9420", "[::]:9420"]);
}

#[test]
fn startup_bind_targets_force_loopback_for_local_only_mode() {
    let mut config = default_config();
    config.daemon.listen_address = "all".to_owned();
    config.network.access_mode = NetworkAccessMode::LocalOnly;
    let options = DaemonRunOptions::default();

    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["127.0.0.1:9420", "[::1]:9420"]
    );
}

#[test]
fn startup_bind_targets_expand_lan_modes_to_all_interfaces() {
    let mut config = default_config();
    config.network.access_mode = NetworkAccessMode::LanTrusted;
    let options = DaemonRunOptions::default();

    assert_eq!(
        effective_bind_targets(&options, &config),
        vec!["0.0.0.0:9420", "[::]:9420"]
    );
}

#[test]
fn startup_bind_targets_keep_lan_trusted_without_control_key() {
    let mut config = default_config();
    config.network.access_mode = NetworkAccessMode::LanTrusted;
    let options = DaemonRunOptions::default();

    let (targets, fell_back) = effective_startup_bind_targets(
        &options,
        &config,
        false,
        config.network.unauthenticated_remote_access_allowed(),
    );

    assert!(!fell_back);
    assert_eq!(targets, vec!["0.0.0.0:9420", "[::]:9420"]);
}

#[test]
fn startup_bind_targets_fall_back_for_lan_protected_without_control_key() {
    let mut config = default_config();
    config.network.access_mode = NetworkAccessMode::LanProtected;
    let options = DaemonRunOptions::default();

    let (targets, fell_back) = effective_startup_bind_targets(
        &options,
        &config,
        false,
        config.network.unauthenticated_remote_access_allowed(),
    );

    assert!(fell_back);
    assert_eq!(targets, vec!["127.0.0.1:9420", "[::1]:9420"]);
}

#[tokio::test]
async fn api_listener_drop_releases_bound_port() {
    let first = bind_api_listener(
        "127.0.0.1:0"
            .parse()
            .expect("ephemeral loopback address should parse"),
    )
    .expect("first listener should bind");
    let bound = first.local_addr().expect("listener address should resolve");

    bind_api_listener(bound).expect_err("port should reject a second live listener");

    drop(first);
    let reopened = bind_api_listener(bound).expect("port should reopen after listener drop");
    drop(reopened);
}

#[tokio::test]
async fn api_shutdown_timeout_forces_stuck_connections_to_close() {
    let listener = bind_api_listener(
        "127.0.0.1:0"
            .parse()
            .expect("ephemeral loopback address should parse"),
    )
    .expect("listener should bind");
    let addr = listener
        .local_addr()
        .expect("listener address should resolve");
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let entered = Arc::new(tokio::sync::Notify::new());
    let router = Router::new()
        .route("/stuck", get(stuck_handler))
        .with_state(StuckHandlerState {
            entered: Arc::clone(&entered),
        });

    let server = tokio::spawn(serve_api_listeners_with_shutdown_timeout(
        vec![listener],
        router,
        shutdown_rx,
        Duration::from_millis(25),
    ));
    let client = tokio::spawn(async move {
        let _ = reqwest::get(format!("http://{addr}/stuck")).await;
    });

    // Upper bound only — notified() returns the moment the handler is
    // entered. Generous because connection setup + dispatch can exceed a
    // second when the host is saturated (e.g. parallel workspace builds
    // during `just verify`).
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("stuck request should enter handler");
    shutdown_tx
        .send(true)
        .expect("shutdown signal should send to server");

    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("server should stop before test timeout")
        .expect("server task should join")
        .expect("server shutdown should succeed");

    client.abort();
}

// ── DaemonState Initialization ──────────────────────────────────────────────

#[tokio::test]
async fn startup_migrates_registered_builtin_ports_across_every_durable_store() {
    let guard = TestDataDirGuard::new().await;
    let seeded = seed_effect_identity_stores(&guard, "breathing").await;
    let mut config = default_config();
    config.effect_engine.extra_effect_dirs = vec![seeded.effect_root.clone()];
    let temp = temp_config_file();

    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("daemon should migrate the registered builtin port");
    let canonical_id = registry_effect_id(&state, "breathing").await;

    assert_ne!(canonical_id, seeded.legacy_id);
    assert_effect_identity_everywhere(&state, &guard, &seeded, canonical_id).await;
}

#[cfg(not(feature = "servo"))]
#[tokio::test]
async fn startup_rejects_screen_cast_port_migration_without_servo() {
    let guard = TestDataDirGuard::new().await;
    let seeded = seed_effect_identity_stores(&guard, "screen_cast").await;
    let mut config = default_config();
    config.effect_engine.extra_effect_dirs = vec![seeded.effect_root.clone()];
    let temp = temp_config_file();

    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("daemon should skip an unavailable HTML screen cast port");
    let native_id = registry_effect_id(&state, "screen_cast").await;
    let effect_path = seeded.effect_root.join("screen_cast.html");

    assert_ne!(native_id, seeded.legacy_id);
    assert!(state.effect_registry.read().await.iter().all(|(_, entry)| {
        !matches!(
                    &entry.metadata.source,
                    EffectSource::Html { path } if path == &effect_path
        )
    }));
    assert_effect_identity_everywhere(&state, &guard, &seeded, seeded.legacy_id).await;
}

#[tokio::test]
async fn daemon_state_initializes_with_default_config() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    );
    assert!(state.is_ok(), "initialization should succeed with defaults");
}

#[tokio::test]
async fn daemon_state_start_and_shutdown() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    let device_id = DeviceId::new();
    let disconnects = Arc::new(AtomicUsize::new(0));
    let info = shutdown_cleanup_device_info(device_id);

    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Arc::new(ShutdownCleanupBackend::new(
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
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    assert!(
        state.device_registry.is_empty().await,
        "device registry should start empty"
    );
}

#[tokio::test]
async fn daemon_state_default_scene_starts_with_default_zone() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    let scenes = state.scene_manager.snapshot().await;
    assert!(
        scenes.active_scene_id().is_some_and(SceneId::is_default),
        "default scene should be active initially"
    );
    let groups = scenes.resolved_zones();
    assert_eq!(groups.len(), 1, "default scene should start with a zone");
    assert_eq!(groups[0].name, "Default zone");
    assert_eq!(groups[0].role, ZoneRole::Primary);
}

#[tokio::test]
async fn daemon_state_scene_manager_starts_with_default_scene() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    let scenes = state.scene_manager.snapshot().await;
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
    let named_scene = hypercolor_core::scene::make_scene("Movie Night");
    let named_scene_id = named_scene.id;
    write_scene_store(&guard.scenes_path(), [named_scene]);

    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    let scenes = state.scene_manager.snapshot().await;
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
    // The manager is built over the same config the daemon boots with;
    // the file only has to agree with it for the accessor to be honest.
    std::fs::write(
        temp.path(),
        "schema_version = 5\n[daemon]\ntarget_fps = 45\n",
    )
    .expect("failed to write config");
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
async fn a_stale_runtime_snapshot_never_blocks_startup() {
    let guard = TestDataDirGuard::new().await;
    // A snapshot written before `driver_runtime_cache` was retired. The
    // snapshot denies unknown fields, so this one no longer parses; the
    // daemon must log it and start fresh rather than refuse to boot.
    std::fs::create_dir_all(
        guard
            .runtime_state_path()
            .parent()
            .expect("runtime state path has a parent"),
    )
    .expect("data directory should be created");
    std::fs::write(
        guard.runtime_state_path(),
        serde_json::json!({
            "active_scene_id": null,
            "default_scene_zones": [],
            "active_layout_id": "layout_gone",
            "global_brightness": 0.25,
            "manual_paused": true,
            "driver_runtime_cache": {},
        })
        .to_string(),
    )
    .expect("stale snapshot should be written");

    let mut config = default_config();
    config.daemon.start_scene = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("a stale snapshot must not block initialization");

    state
        .start()
        .await
        .expect("a stale snapshot must not block startup");

    // Nothing from the unreadable snapshot was restored.
    assert!((state.output_power.global_brightness() - 1.0).abs() < f32::EPSILON);

    state.shutdown().await.expect("shutdown should succeed");
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
    write_layout_store_fixture(&guard.layouts_path(), &layouts);
    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_zones: Vec::new(),
            active_layout_id: Some(restored_layout.id.clone()),
            manual_paused: false,
        },
    )
    .expect("runtime state should save");

    let mut config = default_config();
    config.daemon.start_scene = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    assert_eq!(state.runtime_state_path, guard.runtime_state_path());

    state.start().await.expect("start should succeed");

    let active_layout = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };
    assert_eq!(active_layout.id, restored_layout.id);
    assert_eq!(active_layout.name, restored_layout.name);

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn daemon_start_discards_legacy_runtime_brightness_and_restores_pause() {
    let guard = TestDataDirGuard::new().await;
    let runtime_path = guard.runtime_state_path();
    std::fs::create_dir_all(
        runtime_path
            .parent()
            .expect("runtime state path should have a parent"),
    )
    .expect("runtime state directory should build");
    std::fs::write(
        &runtime_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "active_scene_id": SceneId::DEFAULT.to_string(),
            "default_scene_zones": [],
            "active_layout_id": null,
            "global_brightness": 0.42,
            "manual_paused": true,
        }))
        .expect("legacy runtime snapshot should serialize"),
    )
    .expect("legacy runtime snapshot should write");

    let mut config = default_config();
    config.daemon.start_scene = "default".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    state.start().await.expect("start should succeed");

    assert!(state.output_power.snapshot().manually_paused());
    assert_eq!(state.output_power.global_brightness(), 1.0);
    let rewritten: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&runtime_path).expect("rewritten runtime snapshot should read"),
    )
    .expect("rewritten runtime snapshot should parse");
    assert!(rewritten.get("global_brightness").is_none());
    assert_eq!(
        state.render_loop.read().await.state(),
        RenderLoopState::Paused
    );

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
    write_layout_store_fixture(&guard.layouts_path(), &layouts);

    let config = default_config();
    let temp = temp_config_file();
    let state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    let persisted = layout_store::load(&guard.layouts_path()).expect("layout store should load");
    assert!(persisted.contains_key("default"));
    assert!(persisted.contains_key("layout_custom"));
    let default_layout = state
        .domains
        .layout
        .resolve("default")
        .await
        .expect("default layout should be present in memory");
    assert_eq!(default_layout.name, "Default Layout");
    assert_eq!(default_layout.canvas_width, config.daemon.canvas_width);
    assert_eq!(default_layout.canvas_height, config.daemon.canvas_height);
}

#[tokio::test]
async fn runtime_state_and_driver_inventory_persist_independently() {
    let guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
            let spatial = state.spatial_engine.snapshot();
            spatial.layout().as_ref().clone()
        };
        let api_state = AppState::from_daemon_state(&state);
        let mut mutation = api_state.scene_manager.begin_mutation().await;
        mutation
            .upsert_primary_zone(
                &metadata,
                std::collections::HashMap::new(),
                Some(preset_id),
                layout,
                hypercolor_types::event::ChangeTrigger::System,
                None,
            )
            .expect("native effect should activate");
        hypercolor_daemon::domain::scene::commit_scene(&api_state.domains.scene, mutation)
            .await
            .expect("native effect should commit");
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
                segments: vec![SegmentInfo {
                    name: "Main".to_owned(),
                    led_count: 30,
                    topology: DeviceTopologyHint::Strip,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                }],
                firmware_version: Some("0.15.3".to_owned()),
                capabilities: DeviceCapabilities::default(),
            },
            DeviceFingerprint::from_persisted("net:aa:bb:cc:dd:ee:ff".to_owned()),
            wled_metadata,
        )
        .await;

    state.shutdown().await.expect("shutdown should succeed");

    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state snapshot should exist");
    assert_eq!(snapshot.active_scene_id, Some(SceneId::DEFAULT.to_string()));
    assert_eq!(snapshot.default_scene_zones.len(), 1);
    assert!(matches!(
        snapshot.default_scene_zones[0]
            .layers
            .first()
            .map(|layer| &layer.source),
        Some(LayerSource::Effect {
            effect_id,
            preset_id: Some(candidate),
            ..
        }) if *effect_id == metadata.id && *candidate == preset_id
    ));
    let wled_cache = state.driver_host.driver_inventory().driver_cache("wled");
    let probe_ips: Vec<std::net::IpAddr> = serde_json::from_value(wled_cache["probe_ips"].clone())
        .expect("probe IP inventory should deserialize");
    assert_eq!(
        probe_ips,
        vec!["10.0.0.42".parse::<std::net::IpAddr>().expect("valid IP"),]
    );
}

#[tokio::test]
async fn daemon_start_restores_named_active_scene_and_default_groups() {
    let guard = TestDataDirGuard::new().await;
    let named_scene = hypercolor_core::scene::make_scene("Focus");
    let named_scene_id = named_scene.id;
    write_scene_store(&guard.scenes_path(), [named_scene]);

    let default_group = Zone {
        id: ZoneId::new(),
        name: "Saved Default Group".to_owned(),
        description: None,
        layers: Vec::new(),
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
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    };
    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(named_scene_id.to_string()),
            default_scene_zones: vec![default_group.clone()],
            active_layout_id: None,
            manual_paused: false,
        },
    )
    .expect("runtime state should save");

    let mut config = default_config();
    config.daemon.start_scene = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    state.start().await.expect("start should succeed");

    let scenes = state.scene_manager.snapshot().await;
    assert_eq!(scenes.active_scene_id(), Some(&named_scene_id));
    let default_scene = scenes
        .get(&SceneId::DEFAULT)
        .expect("default scene should exist");
    assert_eq!(default_scene.zones, vec![default_group]);
    drop(scenes);

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn daemon_start_activates_configured_scene_name_without_runtime_snapshot() {
    let guard = TestDataDirGuard::new().await;
    let selected_layout = SpatialLayout {
        id: "startup_evening".into(),
        name: "Startup Evening".into(),
        description: None,
        canvas_width: 512,
        canvas_height: 256,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    write_layout_store_fixture(
        &guard.layouts_path(),
        &std::collections::HashMap::from([(selected_layout.id.clone(), selected_layout.clone())]),
    );
    let mut named_scene = hypercolor_core::scene::make_scene("Evening");
    let named_scene_id = named_scene.id;
    named_scene.layout_id = Some(LayoutId::new(&selected_layout.id).expect("valid layout id"));
    named_scene.activation_brightness = Some(0.35);
    write_scene_store(&guard.scenes_path(), [named_scene]);

    let mut config = default_config();
    config.daemon.start_scene = "evening".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    assert!(!guard.runtime_state_path().exists());
    state.start().await.expect("start should succeed");

    assert_eq!(
        state.scene_manager.snapshot().await.active_scene_id(),
        Some(&named_scene_id)
    );
    assert!((state.output_power.global_brightness() - 0.35).abs() < f32::EPSILON);
    assert_eq!(
        state.spatial_engine.snapshot().layout().id,
        selected_layout.id
    );

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn daemon_start_activates_configured_scene_id() {
    let guard = TestDataDirGuard::new().await;
    let named_scene = hypercolor_core::scene::make_scene("Focus");
    let named_scene_id = named_scene.id;
    write_scene_store(&guard.scenes_path(), [named_scene]);

    let mut config = default_config();
    config.daemon.start_scene = named_scene_id.to_string();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");

    state.start().await.expect("start should succeed");

    assert_eq!(
        state.scene_manager.snapshot().await.active_scene_id(),
        Some(&named_scene_id)
    );

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn default_scene_contents_restore_on_restart() {
    let guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.daemon.start_scene = "last".into();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");
    let effect_id = {
        let registry = state.effect_registry.read().await;
        registry
            .iter()
            .find_map(|(_, entry)| {
                (entry.metadata.source.source_stem() == Some("breathing"))
                    .then_some(entry.metadata.id)
            })
            .expect("breathing effect should be registered")
    };
    let zone_id = ZoneId::new();
    let controls = std::collections::HashMap::from([(
        "speed".to_owned(),
        hypercolor_types::control::ControlValue::Float(4.5),
    )]);

    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_zones: vec![Zone {
                id: zone_id,
                name: "Saved Default Group".to_owned(),
                description: Some("Restored from runtime snapshot".to_owned()),
                layers: vec![SceneLayer::from_effect(
                    SceneLayerId::new(),
                    effect_id,
                    controls,
                    std::collections::HashMap::new(),
                    None,
                )],
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
                role: ZoneRole::Primary,
                controls_version: 0,
                layers_version: 0,
            }],
            active_layout_id: None,
            manual_paused: false,
        },
    )
    .expect("runtime state should save");

    state.start().await.expect("start should succeed");

    let scenes = state.scene_manager.snapshot().await;
    assert_eq!(scenes.active_scene_id(), Some(&SceneId::DEFAULT));
    let default_scene = scenes
        .get(&SceneId::DEFAULT)
        .expect("default scene should exist");
    assert_eq!(default_scene.zones.len(), 1);
    assert_eq!(default_scene.zones[0].name, "Saved Default Group");
    assert!(matches!(
        default_scene.zones[0]
            .layers
            .first()
            .map(|layer| &layer.source),
        Some(LayerSource::Effect {
            effect_id: candidate,
            controls,
            ..
        }) if *candidate == effect_id
            && controls.get("speed")
                == Some(&hypercolor_types::control::ControlValue::Float(4.5))
    ));
    assert_eq!(default_scene.zones[0].brightness, 0.75);
    drop(scenes);

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn paused_startup_seeds_and_reasserts_late_connected_device_output() {
    let guard = TestDataDirGuard::new().await;
    runtime_state::save(
        &guard.runtime_state_path(),
        &runtime_state::RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            manual_paused: true,
            ..runtime_state::RuntimeSessionSnapshot::default()
        },
    )
    .expect("runtime state should save");

    let mut config = default_config();
    config.daemon.start_scene = "last".into();
    config.discovery.background_enabled = false;
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
    .expect("initialization should succeed");
    state.start().await.expect("start should succeed");

    let device_id = DeviceId::new();
    let layout_device_id = device_id.to_string();
    let writes = Arc::new(AtomicUsize::new(0));
    let write_notify = Arc::new(tokio::sync::Notify::new());
    state
        .device_registry
        .add(DeviceInfo {
            id: device_id,
            name: "Late Paused Strip".to_owned(),
            vendor: "TestVendor".to_owned(),
            family: DeviceFamily::new_static("static-hold-test", "Static Hold Test"),
            model: None,
            connection_type: ConnectionType::Usb,
            origin: DeviceOrigin::native(
                "static-hold-test",
                "static-hold-test",
                ConnectionType::Usb,
            ),
            segments: vec![SegmentInfo {
                name: "Main".to_owned(),
                led_count: 30,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 30,
                supports_direct: true,
                max_fps: 60,
                ..DeviceCapabilities::default()
            },
        })
        .await;
    assert!(
        state
            .device_registry
            .set_state(&device_id, hypercolor_types::device::DeviceState::Connected,)
            .await,
        "late device should enter connected state"
    );
    let api_state = Arc::new(AppState::from_daemon_state(&state));
    let response = hypercolor_daemon::api::layouts::preview_layout(
        State(api_state),
        axum::Json(SpatialLayout {
            id: "late-paused-layout".to_owned(),
            name: "Late Paused Layout".to_owned(),
            description: None,
            canvas_width: 32,
            canvas_height: 18,
            zones: vec![test_zone("late-paused-zone", &layout_device_id)],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }),
    )
    .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Arc::new(StaticHoldRecordingBackend {
            writes: Arc::clone(&writes),
            write_notify: Arc::clone(&write_notify),
        }));
        manager
            .connect_device("static-hold-test", device_id, &layout_device_id)
            .await
            .expect("late device should connect while output is paused");
    }

    state.event_bus.publish(HypercolorEvent::DeviceConnected {
        device_id: device_id.to_string(),
        name: "Late Paused Strip".to_owned(),
        origin: DeviceOrigin::native("static-hold-test", "static-hold-test", ConnectionType::Usb),
        led_count: 30,
        zones: Vec::new(),
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let notified = write_notify.notified();
            if writes.load(Ordering::Acquire) >= 2 {
                break;
            }
            notified.await;
        }
    })
    .await
    .expect("late-connected device should receive black and a repeated hold");
    let scene_canvas = state.event_bus.scene_canvas_receiver();
    let scene_canvas = scene_canvas.borrow();
    assert!(scene_canvas.width > 0);
    assert!(
        scene_canvas
            .rgba_bytes()
            .chunks_exact(4)
            .all(|pixel| { pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 && pixel[3] == 255 })
    );

    state.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn event_bus_receives_startup_event() {
    let _guard = TestDataDirGuard::new().await;
    let config = default_config();
    let temp = temp_config_file();
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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

fn test_zone(id: &str, device_id: &str) -> Output {
    Output {
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

#[tokio::test]
async fn effect_error_fallback_worker_clears_active_zones_when_configured() {
    let _guard = TestDataDirGuard::new().await;
    let mut config = default_config();
    config.effect_engine.effect_error_fallback = EffectErrorFallbackPolicy::ClearZones;
    let temp = temp_config_file();
    std::fs::write(
        temp.path(),
        toml::to_string(&config).expect("serialize test config"),
    )
    .expect("write test config");
    let mut state = DaemonState::initialize(
        boot_config(&config),
        config_manager_for(&config, temp.path()),
    )
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
            let spatial = state.spatial_engine.snapshot();
            spatial.layout().as_ref().clone()
        };
        let api_state = AppState::from_daemon_state(&state);
        let mut mutation = api_state.scene_manager.begin_mutation().await;
        let group_id = mutation
            .upsert_primary_zone(
                &metadata,
                std::collections::HashMap::new(),
                None,
                layout,
                hypercolor_types::event::ChangeTrigger::System,
                None,
            )
            .expect("native effect should activate")
            .id;
        hypercolor_daemon::domain::scene::commit_scene(&api_state.domains.scene, mutation)
            .await
            .expect("native effect should commit");
        group_id
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
                HypercolorEvent::EffectStopped { effect, reason, .. }
                    if effect.id == expected_effect_id && reason == EffectStopReason::Error =>
                {
                    saw_stopped = true;
                }
                HypercolorEvent::EffectError {
                    effect_id,
                    fallback,
                    ..
                } if effect_id == expected_effect_id
                    && fallback.as_deref() == Some("clear_zones") =>
                {
                    saw_fallback_event = true;
                }
                HypercolorEvent::ZoneChanged {
                    zone_id: changed, ..
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
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.zones.iter().find(|group| group.id == group_id))
            .and_then(|group| group.effect_ids().next())
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
