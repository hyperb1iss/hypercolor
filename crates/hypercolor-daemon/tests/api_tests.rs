//! Integration tests for the Hypercolor REST API.
//!
//! Tests use `axum::Router` directly with tower's `ServiceExt` and
//! `Request::builder()` — no TCP server needed.

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use anyhow::{Result, bail};
use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{BackendInfo, DeviceBackend};
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::logical_devices::{LogicalDevice, LogicalDeviceKind};
use tower::ServiceExt;
use uuid::Uuid;

use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

// ── Test Helpers ─────────────────────────────────────────────────────────

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> AppState {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    state
}

/// Build a test router with fresh state.
fn test_app() -> axum::Router {
    let state = Arc::new(isolated_state());
    api::build_router(state, None)
}

/// Build a test router with shared state (for multi-step tests).
fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
}

struct NoopBackend {
    info: BackendInfo,
}

impl NoopBackend {
    fn new(id: &str, name: &str) -> Self {
        Self {
            info: BackendInfo {
                id: id.to_owned(),
                name: name.to_owned(),
                description: "Test no-op backend".to_owned(),
            },
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for NoopBackend {
    fn info(&self) -> BackendInfo {
        self.info.clone()
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn write_colors(&mut self, _id: &DeviceId, _colors: &[[u8; 3]]) -> Result<()> {
        Ok(())
    }
}

struct DisconnectRecordingBackend {
    expected_device_id: DeviceId,
    disconnects: Arc<AtomicUsize>,
    connected: bool,
}

impl DisconnectRecordingBackend {
    fn new(expected_device_id: DeviceId, disconnects: Arc<AtomicUsize>) -> Self {
        Self {
            expected_device_id,
            disconnects,
            connected: false,
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for DisconnectRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "wled".to_owned(),
            name: "Disconnect Recording Backend".to_owned(),
            description: "Tracks lifecycle disconnects from the API".to_owned(),
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

async fn register_noop_backend(state: &Arc<AppState>, id: &str, name: &str) {
    let mut manager = state.backend_manager.lock().await;
    manager.register_backend(Box::new(NoopBackend::new(id, name)));
}

/// Extract the JSON body from a response.
async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

/// Extract UTF-8 text body from a response.
async fn body_text(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    String::from_utf8(bytes.to_vec()).expect("failed to decode UTF-8 body")
}

// ── Health / Status ──────────────────────────────────────────────────────

#[tokio::test]
async fn health_check_returns_200() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["checks"]["render_loop"], "idle");
    assert_eq!(json["checks"]["device_backends"], "idle");
    assert_eq!(json["checks"]["event_bus"], "idle");
    assert!(json["version"].is_string());
}

#[tokio::test]
async fn spa_fallback_serves_index_html_for_client_routes() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let index_path = tempdir.path().join("index.html");
    fs::write(&index_path, "<!doctype html><title>hypercolor</title>")
        .expect("index.html should be written");

    let app = api::build_router(Arc::new(isolated_state()), Some(tempdir.path()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/layout")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_text(response).await;
    assert!(body.contains("<title>hypercolor</title>"));
}

#[tokio::test]
async fn status_returns_200_with_envelope() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert!(
        json["data"]["running"]
            .as_bool()
            .expect("running should be bool")
    );
    assert!(
        json["data"]["global_brightness"].as_u64().is_some(),
        "global_brightness should be an integer percentage"
    );
    assert!(json["meta"]["api_version"].is_string());
    assert!(json["meta"]["request_id"].is_string());
    assert!(json["meta"]["timestamp"].is_string());

    // Request ID should start with "req_"
    let request_id = json["meta"]["request_id"]
        .as_str()
        .expect("request_id should be a string");
    assert!(
        request_id.starts_with("req_"),
        "request_id should start with req_"
    );
    assert_eq!(
        json["data"]["config_path"],
        serde_json::json!(default_config_path())
    );
    assert!(
        json["data"]["data_dir"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "data_dir should be a non-empty string"
    );
    assert!(
        json["data"]["cache_dir"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "cache_dir should be a non-empty string"
    );
    assert!(
        json["data"]["audio_available"].is_boolean(),
        "audio_available should be a bool"
    );
    assert_eq!(json["data"]["capture_available"], serde_json::json!(false));
}

#[tokio::test]
async fn status_prefers_live_config_manager_path() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let custom_config_path = tempdir.path().join("custom-settings.toml");
    let config_manager = Arc::new(
        ConfigManager::new(custom_config_path.clone()).expect("config manager should build"),
    );
    let mut state = isolated_state();
    state.config_manager = Some(config_manager);

    let app = test_app_with_state(Arc::new(state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(
        json["data"]["config_path"],
        serde_json::json!(custom_config_path.display().to_string())
    );
}

#[tokio::test]
async fn global_brightness_endpoint_updates_status_and_persistence() {
    let (state, tmp) = test_state_with_temp_output_store();
    let app = test_app_with_state(Arc::clone(&state));

    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings/brightness")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"brightness":42}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["brightness"], 42);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/settings/brightness")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["brightness"], 42);

    let status_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(status_response.status(), StatusCode::OK);
    let status_json = body_json(status_response).await;
    assert_eq!(status_json["data"]["global_brightness"], 42);

    let device_settings_raw = fs::read_to_string(tmp.path().join("device-settings.json"))
        .expect("device settings file should exist");
    let device_settings_json: serde_json::Value =
        serde_json::from_str(&device_settings_raw).expect("device settings file should be valid");
    assert_eq!(
        device_settings_json["global_brightness"],
        serde_json::json!(0.42)
    );

    let runtime_state_raw = fs::read_to_string(tmp.path().join("runtime-state.json"))
        .expect("runtime state file should exist");
    let runtime_state_json: serde_json::Value =
        serde_json::from_str(&runtime_state_raw).expect("runtime state file should be valid");
    assert_eq!(
        runtime_state_json["global_brightness"],
        serde_json::json!(0.42)
    );
}

#[tokio::test]
async fn audio_devices_returns_default_option_and_current_value() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/audio/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let devices = json["data"]["devices"]
        .as_array()
        .expect("devices should be an array");
    assert!(
        !devices.is_empty(),
        "devices should include the default option"
    );
    assert_eq!(devices[0]["id"], "default");
    assert_eq!(devices[0]["name"], "System Monitor (Auto)");
    assert_eq!(devices[1]["id"], "microphone");
    assert_eq!(devices[2]["id"], "none");
    assert_eq!(json["data"]["current"], "default");
}

#[tokio::test]
async fn audio_devices_normalize_legacy_aliases() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path).expect("config manager should build"));
    let mut config = HypercolorConfig::default();
    config.audio.device = "mic".to_owned();
    config_manager.update(config);

    let mut state = isolated_state();
    state.config_manager = Some(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/audio/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["current"], "microphone");
}

#[test]
fn audio_device_filter_hides_synthetic_outputs_from_named_input_list() {
    assert!(
        !hypercolor_daemon::api::settings::should_offer_named_audio_device("PipeWire Sound Server",)
    );
    assert!(
        !hypercolor_daemon::api::settings::should_offer_named_audio_device(
            "PulseAudio Sound Server",
        )
    );
    assert!(
        !hypercolor_daemon::api::settings::should_offer_named_audio_device(
            "Monitor of Built-in Audio Analog Stereo",
        )
    );
    assert!(
        !hypercolor_daemon::api::settings::should_offer_named_audio_device(
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
        )
    );
    assert!(
        hypercolor_daemon::api::settings::should_offer_named_audio_device(
            "Razer Seiren V3 Chroma, USB Audio",
        )
    );
    assert!(
        !hypercolor_daemon::api::settings::should_offer_named_audio_device(
            "Rate Converter Plugin Using Speex Resampler",
        )
    );
    assert!(
        !hypercolor_daemon::api::settings::should_offer_named_audio_device(
            "Discard all samples (playback) or generate zero samples (capture)",
        )
    );
}

#[tokio::test]
async fn config_set_audio_device_persists_without_live_rebuild_by_default() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let mut state = isolated_state();
    state.config_manager = Some(config_manager);
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/config/set")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"key":"audio.device","value":"\"microphone\""}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "microphone");
    assert_eq!(json["data"]["live"], false);

    {
        let input_manager = state.input_manager.lock().await;
        assert_eq!(input_manager.source_count(), 0);
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.audio.device, "microphone");
}

#[tokio::test]
async fn config_set_audio_device_rebuilds_live_input_manager_when_requested() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let mut state = isolated_state();
    state.config_manager = Some(config_manager);
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/config/set")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"key":"audio.device","value":"\"microphone\"","live":true}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "microphone");
    assert_eq!(json["data"]["live"], true);

    {
        let input_manager = state.input_manager.lock().await;
        assert_eq!(input_manager.source_count(), 1);
        assert!(
            input_manager
                .source_names()
                .iter()
                .any(|name| name == "AudioInput(microphone)"),
            "rebuilt input manager should include the selected audio source"
        );
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.audio.device, "microphone");
}

#[tokio::test]
async fn config_set_identical_audio_value_skips_live_rebuild() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let mut state = isolated_state();
    state.config_manager = Some(config_manager);
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/config/set")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"key":"audio.device","value":"\"default\"","live":true}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "default");
    assert_eq!(json["data"]["live"], false);

    {
        let input_manager = state.input_manager.lock().await;
        assert_eq!(input_manager.source_count(), 0);
    }

    assert!(
        !config_path.exists(),
        "no-op config writes should not persist a fresh config file"
    );
}

#[tokio::test]
async fn preview_page_returns_html() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/preview")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.contains("text/html"),
        "expected text/html content type, got {content_type}"
    );

    let body = body_text(response).await;
    assert!(body.contains("Hypercolor Live Preview"));
    assert!(body.contains("/api/v1/ws"));
    assert!(body.contains("show unavailable"));
    assert!(body.contains("run-preview-servo.sh"));
    assert!(body.contains("value=\"30\""));
}

async fn insert_test_effect(state: &Arc<AppState>, name: &str) {
    let mut registry = state.effect_registry.write().await;
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(5.0),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(0.5),
            labels: Vec::new(),
            group: Some("General".to_owned()),
            tooltip: Some("Animation speed".to_owned()),
        }],
        presets: Vec::new(),
        audio_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    };
    let entry = EffectEntry {
        metadata,
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = registry.register(entry);
}

fn default_config_path() -> String {
    ConfigManager::config_dir()
        .join("hypercolor.toml")
        .display()
        .to_string()
}

async fn insert_test_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 60,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

async fn insert_test_asus_smbus_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: name.to_owned(),
        vendor: "ASUS".to_owned(),
        family: DeviceFamily::Asus,
        model: Some("ROG STRIX Test".to_owned()),
        connection_type: ConnectionType::SmBus,
        zones: vec![ZoneInfo {
            name: "GPU".to_owned(),
            led_count: 24,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("AUMA0-E6K5-0107".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 24,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint("smbus:/dev/i2c-9:40".to_owned());
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("backend_id".to_owned(), "smbus".to_owned());
    metadata.insert("smbus_address".to_owned(), "0x40".to_owned());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
}

/// Set up a spatial layout with a zone targeting the given `layout_device_id`.
///
/// This ensures that `sync_active_layout_connectivity` won't disconnect the
/// device because the active layout has a zone referencing it.
async fn set_layout_targeting_device(state: &AppState, layout_device_id: &str, led_count: u32) {
    let layout = SpatialLayout {
        id: "test-layout".into(),
        name: "Test Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: "zone_main".into(),
            name: "Main".into(),
            device_id: layout_device_id.into(),
            zone_name: None,
            group_id: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 0.1),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: led_count,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
        }],
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let mut spatial = state.spatial_engine.write().await;
    spatial.update_layout(layout);
}

// ── Devices ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_devices_returns_empty_list() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("items should be an array");
    assert!(items.is_empty());
    assert_eq!(json["data"]["pagination"]["total"], 0);
}

#[tokio::test]
async fn list_devices_includes_structured_zone_topology_hints() {
    let state = Arc::new(isolated_state());
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: "Matrix Panel".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Panel".to_owned(),
            led_count: 96,
            topology: DeviceTopologyHint::Matrix { rows: 6, cols: 16 },
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 96,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["items"][0]["layout_device_id"],
        format!("device:{id}")
    );
    let zone = &json["data"]["items"][0]["zones"][0];
    assert_eq!(zone["name"], "Panel");
    assert_eq!(zone["topology_hint"]["type"], "matrix");
    assert_eq!(zone["topology_hint"]["rows"], 6);
    assert_eq!(zone["topology_hint"]["cols"], 16);
}

#[tokio::test]
async fn debug_output_queues_returns_empty_snapshot() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/debug/queues")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["queue_count"], 0);
    assert_eq!(json["data"]["mapped_device_count"], 0);
    assert_eq!(
        json["data"]["queues"]
            .as_array()
            .expect("queues should be an array")
            .len(),
        0
    );
}

#[tokio::test]
async fn debug_device_routing_returns_empty_snapshot() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/debug/routing")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["mapping_count"], 0);
    assert_eq!(json["data"]["queue_count"], 0);
    assert!(
        json["data"]["backend_ids"]
            .as_array()
            .expect("backend_ids should be an array")
            .is_empty()
    );
}

#[tokio::test]
async fn get_device_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/00000000-0000-0000-0000-000000000000")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "not_found");
    assert!(json["meta"]["request_id"].is_string());
}

#[tokio::test]
async fn get_device_by_unknown_name_returns_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/not-a-uuid")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "not_found");
}

#[tokio::test]
async fn delete_device_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/devices/00000000-0000-0000-0000-000000000000")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn discover_devices_returns_accepted() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"backends": ["wled"], "timeout_ms": 5000}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "scanning");
    assert!(
        json["data"]["scan_id"]
            .as_str()
            .expect("scan_id should be a string")
            .starts_with("scan_")
    );
}

#[tokio::test]
async fn discover_devices_wait_mode_returns_report() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"backends": ["wled"], "timeout_ms": 100, "wait": true}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "completed");
    assert!(
        json["data"]["scan_id"]
            .as_str()
            .expect("scan_id should be a string")
            .starts_with("scan_")
    );
    assert!(json["data"]["result"]["duration_ms"].is_number());
    assert!(json["data"]["result"]["scanners"].is_array());
}

// ── Effects ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_effects_returns_empty_list() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("items should be an array");
    assert!(items.is_empty());
}

#[tokio::test]
async fn list_effects_returns_items_sorted_by_name() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "zeta").await;
    insert_test_effect(&state, "Alpha").await;
    insert_test_effect(&state, "beta").await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("items should be an array");
    let names: Vec<&str> = items
        .iter()
        .map(|item| item["name"].as_str().expect("name should be a string"))
        .collect();
    assert_eq!(names, vec!["Alpha", "beta", "zeta"]);
}

#[tokio::test]
async fn get_effect_returns_controls() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/solid_color")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let controls = json["data"]["controls"]
        .as_array()
        .expect("controls should be an array");
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0]["id"], "speed");
    assert_eq!(controls[0]["kind"], "number");
}

#[tokio::test]
async fn get_active_effect_returns_not_found_when_none() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stop_effect_returns_not_found_when_none() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_current_controls_requires_active_effect() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/effects/current/controls")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"controls":{"speed":7.5}}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_current_controls_updates_active_effect() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let apply_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(apply_response.status(), StatusCode::OK);

    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/effects/current/controls")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"controls":{"speed":7.25}}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["applied"]["speed"]["float"], 7.5);

    let active_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["control_values"]["speed"]["float"], 7.5);
}

// ── Library ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn library_favorites_crud_lifecycle() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let add_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/favorites")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"effect":"solid_color"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(add_response.status(), StatusCode::OK);
    let add_json = body_json(add_response).await;
    assert_eq!(add_json["data"]["created"], true);

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/favorites")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["pagination"]["total"], 1);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/library/favorites/solid_color")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let list_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/favorites")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["pagination"]["total"], 0);
}

#[tokio::test]
async fn library_presets_create_and_get() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Warm Sweep",
                        "effect":"solid_color",
                        "controls":{"speed":7.25},
                        "tags":[" cozy ","test"]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    assert_eq!(create_json["data"]["name"], "Warm Sweep");
    assert_eq!(create_json["data"]["controls"]["speed"]["float"], 7.5);
    assert_eq!(create_json["data"]["tags"][0], "cozy");
    let preset_id = create_json["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/library/presets/{preset_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["id"], preset_id);
    assert_eq!(get_json["data"]["controls"]["speed"]["float"], 7.5);
}

#[tokio::test]
async fn library_preset_apply_activates_effect_with_controls() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Apply Me",
                        "effect":"solid_color",
                        "controls":{"speed":7.25}
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let preset_id = create_json["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let apply_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/presets/{preset_id}/apply"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(apply_response.status(), StatusCode::OK);
    let apply_json = body_json(apply_response).await;
    assert_eq!(apply_json["data"]["preset"]["id"], preset_id);
    assert_eq!(apply_json["data"]["effect"]["name"], "solid_color");
    assert_eq!(
        apply_json["data"]["applied_controls"]["speed"]["float"],
        7.5
    );

    let active_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["name"], "solid_color");
    assert_eq!(active_json["data"]["control_values"]["speed"]["float"], 7.5);
}

#[tokio::test]
async fn library_preset_apply_resolves_by_name() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"named_preset",
                        "effect":"solid_color",
                        "controls":{"speed":5}
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);

    let apply_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets/named_preset/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(apply_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn library_playlists_create_with_effect_and_preset_targets() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let preset_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Preset A",
                        "effect":"solid_color",
                        "controls":{"speed":5}
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(preset_response.status(), StatusCode::CREATED);
    let preset_json = body_json(preset_response).await;
    let preset_id = preset_json["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let playlist_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{
                        "name":"Night Rotation",
                        "loop_enabled":true,
                        "items":[
                            {{
                                "target":{{"type":"effect","effect":"solid_color"}},
                                "duration_ms":2000
                            }},
                            {{
                                "target":{{"type":"preset","preset_id":"{preset_id}"}},
                                "duration_ms":3000
                            }}
                        ]
                    }}"#
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(playlist_response.status(), StatusCode::CREATED);
    let playlist_json = body_json(playlist_response).await;
    assert_eq!(
        playlist_json["data"]["items"]
            .as_array()
            .map_or(0, Vec::len),
        2
    );
    let playlist_id = playlist_json["data"]["id"]
        .as_str()
        .expect("playlist id should be string")
        .to_owned();

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/library/playlists/{playlist_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["items"].as_array().map_or(0, Vec::len), 2);
}

#[tokio::test]
async fn library_playlist_activate_and_stop_lifecycle() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Runtime Playlist",
                        "loop_enabled":true,
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let playlist_id = create_json["data"]["id"]
        .as_str()
        .expect("playlist id should be string")
        .to_owned();

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{playlist_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);
    let activate_json = body_json(activate_response).await;
    assert_eq!(activate_json["data"]["playlist"]["id"], playlist_id);

    let active_playlist_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/playlists/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_playlist_response.status(), StatusCode::OK);
    let active_playlist_json = body_json(active_playlist_response).await;
    assert_eq!(active_playlist_json["data"]["playlist"]["id"], playlist_id);

    let active_effect_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_effect_response.status(), StatusCode::OK);
    let active_effect_json = body_json(active_effect_response).await;
    assert_eq!(active_effect_json["data"]["name"], "solid_color");

    let stop_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(stop_response.status(), StatusCode::OK);

    let active_playlist_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/playlists/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_playlist_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn library_playlist_activate_resolves_by_name() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"runtime_by_name",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/runtime_by_name/activate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);

    let stop_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(stop_response.status(), StatusCode::OK);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "test validates full playlist replacement lifecycle"
)]
async fn library_playlist_activate_replaces_previous_runtime() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let first_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"first_runtime",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_create.status(), StatusCode::CREATED);
    let first_json = body_json(first_create).await;
    let first_id = first_json["data"]["id"]
        .as_str()
        .expect("first playlist id should be string")
        .to_owned();

    let second_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"second_runtime",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_create.status(), StatusCode::CREATED);
    let second_json = body_json(second_create).await;
    let second_id = second_json["data"]["id"]
        .as_str()
        .expect("second playlist id should be string")
        .to_owned();

    let first_activate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{first_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_activate.status(), StatusCode::OK);

    let second_activate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{second_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_activate.status(), StatusCode::OK);

    let active_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/playlists/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["playlist"]["id"], second_id);

    let stop_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(stop_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn library_delete_active_playlist_stops_runtime() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"delete_me",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let playlist_id = create_json["data"]["id"]
        .as_str()
        .expect("playlist id should be string")
        .to_owned();

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{playlist_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/library/playlists/{playlist_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let active_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/playlists/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::NOT_FOUND);
}

// ── Scenes ───────────────────────────────────────────────────────────────

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "CRUD lifecycle test covers full create-read-update-delete flow"
)]
async fn scene_crud_lifecycle() {
    let state = Arc::new(isolated_state());

    // Create scene
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name": "Test Scene", "description": "A test scene"}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Test Scene");
    let scene_id = json["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

    // Get scene
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Test Scene");

    // List scenes
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["pagination"]["total"], 1);

    // Update scene
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name": "Updated Scene", "description": "Updated description"}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Updated Scene");

    // Activate scene
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/scenes/{scene_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["activated"], true);

    // Delete scene
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["deleted"], true);

    // Verify deletion
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ── Profiles ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn profile_crud_lifecycle() {
    let state = Arc::new(isolated_state());

    // Create profile
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Gaming Mode", "brightness": 100}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Gaming Mode");
    let profile_id = json["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

    // Get profile
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/profiles/{profile_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Gaming Mode");

    // List profiles
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/profiles")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["pagination"]["total"], 1);

    // Update profile
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/profiles/{profile_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Chill Mode", "brightness": 50}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Chill Mode");

    // Apply profile
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/profiles/{profile_id}/apply"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["applied"], true);

    // Delete profile
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/profiles/{profile_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["deleted"], true);

    // Verify deletion
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/profiles/{profile_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ── Layouts ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn layout_crud_lifecycle() {
    let state = Arc::new(isolated_state());

    // Create layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name": "Main Setup", "canvas_width": 320, "canvas_height": 200}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Main Setup");
    assert_eq!(json["data"]["canvas_width"], 320);
    assert_eq!(json["data"]["group_count"], 0);
    let layout_id = json["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

    // Get layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    // List layouts
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["pagination"]["total"], 1);
    assert_eq!(json["data"]["items"][0]["group_count"], 0);

    // Update layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Updated Setup"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Updated Setup");

    // Delete layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["deleted"], true);
}

#[tokio::test]
async fn layout_apply_updates_active_layout() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Studio Layout","canvas_width":640,"canvas_height":360}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let layout_id = create_json["data"]["id"]
        .as_str()
        .expect("id should be string")
        .to_owned();

    let apply_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/layouts/{layout_id}/apply"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(apply_response.status(), StatusCode::OK);
    let apply_json = body_json(apply_response).await;
    assert_eq!(apply_json["data"]["applied"], true);
    assert_eq!(apply_json["data"]["layout"]["id"], layout_id);

    let active_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["id"], layout_id);
    assert_eq!(active_json["data"]["name"], "Studio Layout");

    let list_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts?active=true")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["pagination"]["total"], 1);
    assert_eq!(list_json["data"]["items"][0]["id"], layout_id);
    assert_eq!(list_json["data"]["items"][0]["is_active"], true);

    let runtime_raw = std::fs::read_to_string(&state.runtime_state_path)
        .expect("runtime state file should exist after apply");
    let runtime_json: serde_json::Value =
        serde_json::from_str(&runtime_raw).expect("runtime state should be valid JSON");
    assert_eq!(runtime_json["active_layout_id"], layout_id);
}

#[tokio::test]
async fn layout_delete_active_falls_back_to_default_layout() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Cannot Delete Active"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let layout_id = create_json["data"]["id"]
        .as_str()
        .expect("id should be string")
        .to_owned();

    let apply_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/layouts/{layout_id}/apply"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(apply_response.status(), StatusCode::OK);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let active_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["id"], "default");
    assert_eq!(active_json["data"]["name"], "Default Layout");

    let runtime_raw = std::fs::read_to_string(&state.runtime_state_path)
        .expect("runtime state file should exist after delete");
    let runtime_json: serde_json::Value =
        serde_json::from_str(&runtime_raw).expect("runtime state should be valid JSON");
    assert_eq!(runtime_json["active_layout_id"], "default");
}

#[tokio::test]
async fn layout_create_validates_input() {
    let app = test_app();

    let empty_name_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from("{\"name\":\"   \"}"))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        empty_name_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_canvas_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Bad","canvas_width":0}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        invalid_canvas_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn layout_groups_roundtrip_persist_and_preview() {
    let (state, _tmp) = test_state_with_temp_layout_store();
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Grouped Layout"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let layout_id = create_json["data"]["id"]
        .as_str()
        .expect("layout id should be string")
        .to_owned();

    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .header("content-type", "application/json")
                .body(Body::from(grouped_layout_update_payload()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["group_count"], 1);
    assert_eq!(update_json["data"]["zone_count"], 1);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["groups"][0]["id"], "g1");
    assert_eq!(get_json["data"]["zones"][0]["group_id"], "g1");

    let persisted_raw =
        std::fs::read_to_string(&state.layouts_path).expect("layout persistence file should exist");
    let persisted: serde_json::Value =
        serde_json::from_str(&persisted_raw).expect("layout store should be valid JSON");
    assert_eq!(persisted[0]["groups"][0]["id"], "g1");
    assert_eq!(persisted[0]["zones"][0]["group_id"], "g1");

    let preview_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/layouts/active/preview")
                .header("content-type", "application/json")
                .body(Body::from(grouped_layout_preview_payload()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(preview_response.status(), StatusCode::OK);

    let active_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["groups"][0]["id"], "g1");
    assert_eq!(active_json["data"]["zones"][0]["group_id"], "g1");
}

#[tokio::test]
async fn layout_update_cleans_orphaned_group_ids() {
    let app = test_app();

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Cleanup Layout"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let layout_id = create_json["data"]["id"]
        .as_str()
        .expect("layout id should be string")
        .to_owned();

    let orphan_update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .header("content-type", "application/json")
                .body(Body::from(orphan_group_update_payload()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(orphan_update.status(), StatusCode::OK);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(
        get_json["data"]["zones"][0]["group_id"],
        serde_json::Value::Null
    );

    let delete_group_update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"groups":[]}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_group_update.status(), StatusCode::OK);
    let delete_group_json = body_json(delete_group_update).await;
    assert_eq!(delete_group_json["data"]["group_count"], 0);

    let final_get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(final_get_response.status(), StatusCode::OK);
    let final_get_json = body_json(final_get_response).await;
    assert_eq!(
        final_get_json["data"]["zones"][0]["group_id"],
        serde_json::Value::Null
    );
    assert_eq!(final_get_json["data"]["groups"], serde_json::json!([]));
}

// ── Effect Layout Associations ──────────────────────────────────────────

fn test_state_with_temp_effect_layout_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = isolated_state();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.effect_layout_links_path = dir.path().join("effect-layouts.json");
    (Arc::new(state), dir)
}

fn test_state_with_temp_layout_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = isolated_state();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.layouts_path = dir.path().join("layouts.json");
    (Arc::new(state), dir)
}

fn test_state_with_temp_layout_and_runtime_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = isolated_state();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.layouts_path = dir.path().join("layouts.json");
    state.runtime_state_path = dir.path().join("runtime-state.json");
    (Arc::new(state), dir)
}

fn test_state_with_temp_output_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = isolated_state();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.device_settings = Arc::new(tokio::sync::RwLock::new(DeviceSettingsStore::new(
        dir.path().join("device-settings.json"),
    )));
    state.runtime_state_path = dir.path().join("runtime-state.json");
    (Arc::new(state), dir)
}

fn grouped_layout_update_payload() -> &'static str {
    r##"{
        "groups":[{"id":"g1","name":"PC Case","color":"#e135ff"}],
        "zones":[{
            "id":"zone-1",
            "name":"Desk Strip",
            "device_id":"wled:desk",
            "zone_name":null,
            "group_id":"g1",
            "position":{"x":0.5,"y":0.5},
            "size":{"x":0.4,"y":0.1},
            "rotation":0.0,
            "scale":1.0,
            "orientation":null,
            "topology":{"type":"strip","count":30,"direction":"left_to_right"},
            "sampling_mode":null,
            "edge_behavior":null,
            "shape":null,
            "shape_preset":null
        }]
    }"##
}

fn grouped_layout_preview_payload() -> &'static str {
    r##"{
        "id":"preview-layout",
        "name":"Preview Layout",
        "description":null,
        "canvas_width":320,
        "canvas_height":200,
        "zones":[{
            "id":"zone-preview",
            "name":"Preview Strip",
            "device_id":"wled:preview",
            "zone_name":null,
            "group_id":"g1",
            "position":{"x":0.4,"y":0.6},
            "size":{"x":0.3,"y":0.1},
            "rotation":0.0,
            "scale":1.0,
            "orientation":null,
            "topology":{"type":"point"},
            "sampling_mode":null,
            "edge_behavior":null,
            "shape":null,
            "shape_preset":null
        }],
        "groups":[{"id":"g1","name":"Preview Group","color":"#80ffea"}],
        "default_sampling_mode":{"type":"bilinear"},
        "default_edge_behavior":"clamp",
        "spaces":null,
        "version":1
    }"##
}

fn orphan_group_update_payload() -> &'static str {
    r##"{
        "groups":[{"id":"valid","name":"Valid Group","color":"#ff6ac1"}],
        "zones":[{
            "id":"zone-1",
            "name":"Orphan Zone",
            "device_id":"wled:orphan",
            "zone_name":null,
            "group_id":"missing",
            "position":{"x":0.5,"y":0.5},
            "size":{"x":0.2,"y":0.2},
            "rotation":0.0,
            "scale":1.0,
            "orientation":null,
            "topology":{"type":"point"},
            "sampling_mode":null,
            "edge_behavior":null,
            "shape":null,
            "shape_preset":null
        }]
    }"##
}

#[tokio::test]
async fn effect_layout_association_crud_persists_to_disk() {
    let (state, _tmp) = test_state_with_temp_effect_layout_store();
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_layout_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Effect Bound Layout","canvas_width":640,"canvas_height":360}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_layout_response.status(), StatusCode::CREATED);
    let create_layout_json = body_json(create_layout_response).await;
    let layout_id = create_layout_json["data"]["id"]
        .as_str()
        .expect("layout id should be string")
        .to_owned();

    let link_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/effects/solid_color/layout")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"layout_id":"{layout_id}"}}"#)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(link_response.status(), StatusCode::OK);
    let link_json = body_json(link_response).await;
    assert_eq!(link_json["data"]["linked"], true);
    assert_eq!(link_json["data"]["layout"]["id"], layout_id);
    let effect_id = link_json["data"]["effect"]["id"]
        .as_str()
        .expect("effect id should be string")
        .to_owned();

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/solid_color/layout")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["layout_id"], layout_id);
    assert_eq!(get_json["data"]["resolved"], true);

    let persisted_raw = std::fs::read_to_string(&state.effect_layout_links_path)
        .expect("effect layout persistence file should exist");
    let persisted: serde_json::Value =
        serde_json::from_str(&persisted_raw).expect("effect layout map should be valid JSON");
    assert_eq!(persisted[&effect_id], layout_id);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/effects/solid_color/layout")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);
    let delete_json = body_json(delete_response).await;
    assert_eq!(delete_json["data"]["deleted"], true);

    let get_after_delete_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/solid_color/layout")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_after_delete_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn applying_effect_auto_applies_associated_layout() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let resp_layout_a = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Layout A"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(resp_layout_a.status(), StatusCode::CREATED);
    let json_layout_a = body_json(resp_layout_a).await;
    let first_layout_id = json_layout_a["data"]["id"]
        .as_str()
        .expect("layout A id should be string")
        .to_owned();

    let resp_layout_b = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Layout B"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(resp_layout_b.status(), StatusCode::CREATED);
    let json_layout_b = body_json(resp_layout_b).await;
    let layout_b_id = json_layout_b["data"]["id"]
        .as_str()
        .expect("layout B id should be string")
        .to_owned();

    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/effects/solid_color/layout")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"layout_id":"{layout_b_id}"}}"#)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/layouts/{first_layout_id}/apply"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    let apply_effect_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(apply_effect_response.status(), StatusCode::OK);
    let apply_effect_json = body_json(apply_effect_response).await;
    assert_eq!(apply_effect_json["data"]["layout"]["applied"], true);
    assert_eq!(
        apply_effect_json["data"]["layout"]["associated_layout_id"],
        layout_b_id
    );

    let active_layout_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_layout_response.status(), StatusCode::OK);
    let active_layout_json = body_json(active_layout_response).await;
    assert_eq!(active_layout_json["data"]["id"], layout_b_id);
}

#[tokio::test]
async fn apply_effect_rejects_unimplemented_transition_requests() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"transition":{"type":"crossfade","duration_ms":250}}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "bad_request");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("error message should be a string")
            .contains("only immediate cut applies"),
    );
}

// ── Error Envelope Format ────────────────────────────────────────────────

#[tokio::test]
async fn error_responses_have_correct_envelope() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/profiles/nonexistent")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;

    // Error envelope must have `error` and `meta` at top level.
    assert!(json["error"].is_object(), "error key should be an object");
    assert!(json["meta"].is_object(), "meta key should be an object");

    // Error object must have `code` and `message`.
    assert_eq!(json["error"]["code"], "not_found");
    assert!(
        json["error"]["message"].is_string(),
        "error.message should be a string"
    );

    // Meta must have `api_version`, `request_id`, and `timestamp`.
    assert_eq!(json["meta"]["api_version"], "1.0");
    assert!(
        json["meta"]["request_id"]
            .as_str()
            .expect("request_id should be string")
            .starts_with("req_"),
        "request_id should start with req_"
    );
    assert!(
        json["meta"]["timestamp"].is_string(),
        "timestamp should be a string"
    );
}

#[tokio::test]
async fn success_responses_have_correct_envelope() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;

    // Success envelope must have `data` and `meta` at top level.
    assert!(json["data"].is_object(), "data key should be an object");
    assert!(json["meta"].is_object(), "meta key should be an object");

    // Meta must have correct fields.
    assert_eq!(json["meta"]["api_version"], "1.0");
    assert!(json["meta"]["request_id"].is_string());
    assert!(json["meta"]["timestamp"].is_string());
}

// ── Device Discovery (no body) ──────────────────────────────────────────

#[tokio::test]
async fn discover_devices_without_body() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn discover_devices_rejects_unknown_backend() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"backends": ["mystery"], "timeout_ms": 5000}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
}

#[tokio::test]
async fn discover_devices_returns_conflict_when_scan_active() {
    let state = Arc::new(isolated_state());
    state.discovery_in_progress.store(true, Ordering::Release);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
}

// ── Device Identify ──────────────────────────────────────────────────────

#[tokio::test]
async fn identify_device_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/00000000-0000-0000-0000-000000000000/identify")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_device_persists_name_enabled_and_brightness_state() {
    let (state, tmp) = test_state_with_temp_output_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk Strip Renamed","enabled":false,"brightness":27}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["name"], "Desk Strip Renamed");
    assert_eq!(update_json["data"]["status"], "disabled");
    assert_eq!(update_json["data"]["brightness"], 27);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["name"], "Desk Strip Renamed");
    assert_eq!(get_json["data"]["status"], "disabled");
    assert_eq!(get_json["data"]["brightness"], 27);

    let persisted_raw = fs::read_to_string(tmp.path().join("device-settings.json"))
        .expect("device settings file should exist");
    let persisted_json: serde_json::Value =
        serde_json::from_str(&persisted_raw).expect("device settings file should be valid json");
    let persisted_device = &persisted_json["devices"][device_id.to_string()];
    assert_eq!(persisted_device["name"], "Desk Strip Renamed");
    assert_eq!(persisted_device["disabled"], true);
    assert_eq!(persisted_device["brightness"], serde_json::json!(0.27));

    let reenable_response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(reenable_response.status(), StatusCode::OK);
    let reenable_json = body_json(reenable_response).await;
    assert_eq!(reenable_json["data"]["status"], "known");
    assert_eq!(reenable_json["data"]["brightness"], 27);
}

#[tokio::test]
async fn update_device_disable_runs_lifecycle_disconnect_cleanup() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let disconnects = Arc::new(AtomicUsize::new(0));

    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Box::new(DisconnectRecordingBackend::new(
            device_id,
            Arc::clone(&disconnects),
        )));
    }

    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("device should exist");
    let layout_device_id = {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        let _actions = lifecycle.on_discovered(device_id, &tracked.info, "wled", None);
        lifecycle
            .layout_device_id_for(device_id)
            .expect("layout id should exist")
            .to_owned()
    };

    state
        .backend_manager
        .lock()
        .await
        .connect_device("wled", device_id, &layout_device_id)
        .await
        .expect("device should connect for disable flow");

    {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        lifecycle
            .on_connected(device_id)
            .expect("connect transition should succeed");
    }
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":false}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "disabled");
    assert_eq!(disconnects.load(Ordering::Relaxed), 1);
    assert_eq!(state.backend_manager.lock().await.mapped_device_count(), 0);
}

#[tokio::test]
async fn list_devices_supports_filters() {
    let state = Arc::new(isolated_state());
    let _first_id = insert_test_device(&state, "Desk Strip").await;
    let second_id = insert_test_device(&state, "Ceiling Panel").await;
    let _ = state
        .device_registry
        .set_state(&second_id, DeviceState::Disabled)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let disabled_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?status=disabled")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(disabled_response.status(), StatusCode::OK);
    let disabled_json = body_json(disabled_response).await;
    assert_eq!(disabled_json["data"]["pagination"]["total"], 1);
    assert_eq!(disabled_json["data"]["items"][0]["name"], "Ceiling Panel");

    let query_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?backend=wled&q=desk")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(query_response.status(), StatusCode::OK);
    let query_json = body_json(query_response).await;
    assert_eq!(query_json["data"]["pagination"]["total"], 1);
    assert_eq!(query_json["data"]["items"][0]["name"], "Desk Strip");
}

#[tokio::test]
async fn list_devices_includes_network_metadata_when_available() {
    let state = Arc::new(isolated_state());
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Desk Strip".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 60,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            features: DeviceFeatures::default(),
        },
    };
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("ip".to_owned(), "192.168.1.42".to_owned());
    metadata.insert("hostname".to_owned(), "wled-desk".to_owned());
    let _ = state
        .device_registry
        .add_with_fingerprint_and_metadata(
            info,
            DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".to_owned()),
            metadata,
        )
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["items"][0]["network_ip"], "192.168.1.42");
    assert_eq!(json["data"]["items"][0]["network_hostname"], "wled-desk");
}

#[tokio::test]
async fn list_devices_rejects_invalid_status_filter() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?status=invalid")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
}

#[tokio::test]
async fn list_devices_rejects_invalid_limit() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?limit=0")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn identify_device_validates_and_returns_canonical_id() {
    let state = Arc::new(isolated_state());
    register_noop_backend(&state, "wled", "WLED Test Backend").await;
    let device_id = insert_test_device(&state, "Keyboard").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let invalid_color_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Keyboard/identify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"color":"zzzzzz"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        invalid_color_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_duration_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Keyboard/identify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":0}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        invalid_duration_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let valid_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Keyboard/identify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":1500,"color":"ff00aa"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(valid_response.status(), StatusCode::OK);
    let valid_json = body_json(valid_response).await;
    assert_eq!(valid_json["data"]["device_id"], device_id.to_string());
    assert_eq!(valid_json["data"]["color"], "#FF00AA");
}

#[tokio::test]
async fn identify_device_requires_connected_state() {
    let state = Arc::new(isolated_state());
    let _device_id = insert_test_device(&state, "Known Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Known%20Strip/identify")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
}

#[tokio::test]
async fn identify_device_uses_discovered_smbus_backend_for_asus_devices() {
    let state = Arc::new(isolated_state());
    register_noop_backend(&state, "smbus", "SMBus Test Backend").await;
    let device_id = insert_test_asus_smbus_device(&state, "Aura GPU").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/identify"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["device_id"], device_id.to_string());
    assert_eq!(json["data"]["identifying"], true);
}

#[tokio::test]
async fn get_device_by_ambiguous_name_returns_conflict() {
    let state = Arc::new(isolated_state());
    let _ = insert_test_device(&state, "Same Name").await;
    let _ = insert_test_device(&state, "Same Name").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/Same%20Name")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
}

#[tokio::test]
async fn delete_device_by_name_returns_canonical_id() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Panel").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/devices/Panel")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], device_id.to_string());
}

fn test_state_with_temp_logical_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = isolated_state();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.logical_devices_path = dir.path().join("logical-devices.json");
    (Arc::new(state), dir)
}

#[tokio::test]
async fn logical_devices_crud_persists_user_segments() {
    let (state, _tmp) = test_state_with_temp_logical_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk Left","led_start":0,"led_count":20}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    assert_eq!(create_json["data"]["kind"], "segment");
    assert_eq!(
        create_json["data"]["physical_device_id"],
        device_id.to_string()
    );
    let logical_id = create_json["data"]["id"]
        .as_str()
        .expect("logical id should be string")
        .to_owned();

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["pagination"]["total"], 2);
    let default_entry = list_json["data"]["items"]
        .as_array()
        .expect("items should be array")
        .iter()
        .find(|item| item["kind"] == "default")
        .expect("default logical entry should exist");
    assert_eq!(default_entry["enabled"], false);

    let persisted_raw = std::fs::read_to_string(&state.logical_devices_path)
        .expect("logical device persistence file should exist");
    assert!(
        persisted_raw.contains(&logical_id),
        "persistence file should include the created logical segment"
    );

    let delete_response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/logical-devices/{logical_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn logical_devices_reject_overlapping_segments() {
    let (state, _tmp) = test_state_with_temp_logical_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let first_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk Left","led_start":0,"led_count":20}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_create.status(), StatusCode::CREATED);

    let overlapping_create = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk Mid","led_start":10,"led_count":20}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        overlapping_create.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let json = body_json(overlapping_create).await;
    assert_eq!(json["error"]["code"], "validation_error");
}

#[tokio::test]
async fn logical_device_endpoints_preserve_smbus_backend_metadata() {
    let (state, _tmp) = test_state_with_temp_logical_store();
    register_noop_backend(&state, "smbus", "SMBus Test").await;

    let device_id = insert_test_asus_smbus_device(&state, "Aura GPU").await;
    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("device should exist");

    let layout_device_id = {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        let fingerprint = DeviceFingerprint("smbus:/dev/i2c-9:40".to_owned());
        let _ = lifecycle.on_discovered(device_id, &tracked.info, "smbus", Some(&fingerprint));
        let layout_device_id = lifecycle
            .layout_device_id_for(device_id)
            .expect("layout id should exist")
            .to_owned();
        let _ = lifecycle
            .on_connected(device_id)
            .expect("connect transition should succeed");
        layout_device_id
    };

    state
        .backend_manager
        .lock()
        .await
        .connect_device("smbus", device_id, &layout_device_id)
        .await
        .expect("smbus test backend should connect");
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    // The active layout must reference the segment that will be created, so that
    // sync_active_layout_connectivity doesn't disconnect the device when
    // reconcile_default_enabled disables the default entry.
    let expected_segment_id = format!("{layout_device_id}:aura-segment");
    set_layout_targeting_device(&state, &expected_segment_id, 12).await;

    let app = test_app_with_state(Arc::clone(&state));
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Aura Segment","led_start":0,"led_count":12}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    assert_eq!(create_json["data"]["backend"], "smbus");
    let segment_id = create_json["data"]["id"]
        .as_str()
        .expect("segment id should be a string")
        .to_owned();

    let list_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    let items = list_json["data"]["items"]
        .as_array()
        .expect("logical items should be an array");
    assert!(
        items.iter().all(|item| item["backend"] == "smbus"),
        "every logical device summary should keep the smbus backend id"
    );

    let manager = state.backend_manager.lock().await;
    let routing = manager.routing_snapshot();
    assert!(
        routing.mappings.iter().any(|entry| {
            entry.backend_id == "smbus"
                && entry.device_id == device_id.to_string()
                && entry.layout_device_id == segment_id
        }),
        "logical segment routing should stay attached to the smbus backend"
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "this migration test validates legacy alias preservation, logical-device creation, and backend mapping state in one end-to-end flow"
)]
async fn logical_devices_migrate_legacy_default_ids_and_keep_legacy_aliases_mapped() {
    let (state, _tmp) = test_state_with_temp_logical_store();
    register_noop_backend(&state, "wled", "WLED Test").await;

    let fingerprint = DeviceFingerprint("net:00:11:22:33:44:55".to_owned());
    let canonical_layout_id = "wled:00:11:22:33:44:55".to_owned();

    let device_id = {
        let id = DeviceId::new();
        let info = DeviceInfo {
            id,
            name: "Desk Strip".to_owned(),
            vendor: "test-vendor".to_owned(),
            family: DeviceFamily::Wled,
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 60,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("0.1.0".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 60,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
                features: DeviceFeatures::default(),
            },
        };
        state
            .device_registry
            .add_with_fingerprint(info, fingerprint.clone())
            .await
    };
    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("device should exist");

    {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &tracked.info, "wled", Some(&fingerprint));
        let _ = lifecycle
            .on_connected(device_id)
            .expect("connect transition should succeed");
    }
    state
        .backend_manager
        .lock()
        .await
        .connect_device("wled", device_id, &canonical_layout_id)
        .await
        .expect("wled test backend should connect");
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    // The active layout must reference this device so that
    // sync_active_layout_connectivity doesn't disconnect it.
    set_layout_targeting_device(&state, &canonical_layout_id, 60).await;

    let legacy_layout_id = format!("device:{device_id}");
    {
        let mut store = state.logical_devices.write().await;
        store.insert(
            legacy_layout_id.clone(),
            LogicalDevice {
                id: legacy_layout_id.clone(),
                physical_device_id: device_id,
                name: "Desk Strip".to_owned(),
                led_start: 0,
                led_count: 60,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );
    }

    let app = test_app_with_state(Arc::clone(&state));
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk Left","led_start":0,"led_count":20}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let segment_id = create_json["data"]["id"]
        .as_str()
        .expect("segment id should be string")
        .to_owned();

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/logical-devices/{segment_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}/logical-devices"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    let items = list_json["data"]["items"]
        .as_array()
        .expect("items should be array");
    let default_entry = items
        .iter()
        .find(|item| item["kind"] == "default")
        .expect("default entry should exist");
    assert_eq!(default_entry["id"], canonical_layout_id);
    assert!(
        items.iter().all(|item| item["id"] != legacy_layout_id),
        "legacy default logical id should be migrated away"
    );

    let manager = state.backend_manager.lock().await;
    let routing = manager.routing_snapshot();
    let mapped_layout_ids = routing
        .mappings
        .into_iter()
        .filter(|entry| entry.backend_id == "wled" && entry.device_id == device_id.to_string())
        .map(|entry| entry.layout_device_id)
        .collect::<Vec<_>>();
    assert!(
        mapped_layout_ids.contains(&canonical_layout_id),
        "canonical layout id should be mapped"
    );
    assert!(
        mapped_layout_ids.contains(&legacy_layout_id),
        "legacy device:<uuid> alias should stay mapped for compatibility"
    );
    assert!(
        mapped_layout_ids.contains(&device_id.to_string()),
        "raw physical uuid alias should stay mapped"
    );
}
