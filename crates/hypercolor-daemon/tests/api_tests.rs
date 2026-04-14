//! Integration tests for the Hypercolor REST API.
//!
//! Tests use `axum::Router` directly with tower's `ServiceExt` and
//! `Request::builder()` — no TCP server needed.

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::net::Credentials;
use hypercolor_core::device::{BackendInfo, DeviceBackend};
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::logical_devices::{LogicalDevice, LogicalDeviceKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;
use uuid::Uuid;

use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::profile_store::{Profile, ProfilePrimary};
use hypercolor_daemon::runtime_state;
use hypercolor_daemon::scene_transactions::SceneTransaction;
use hypercolor_daemon::session::{current_global_brightness, set_global_brightness};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory,
    EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectStopReason, HypercolorEvent, RenderGroupChangeKind, SceneChangeReason,
};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, RenderGroup, RenderGroupRole, Scene,
    SceneId, SceneKind, SceneMutationMode, ScenePriority, SceneScope, TransitionSpec,
    UnassignedBehavior,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

// ── Test Helpers ─────────────────────────────────────────────────────────

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> AppState {
    isolated_state_with_tempdir().0
}

fn isolated_state_with_tempdir() -> (AppState, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
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
    assert_eq!(json["checks"]["device_backends"], "ok");
    assert_eq!(json["checks"]["event_bus"], "idle");
    assert!(json["version"].is_string());
}

#[tokio::test]
async fn health_check_reports_stopped_render_loop_as_degraded() {
    let state = Arc::new(isolated_state());
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.stop();
    }

    let app = test_app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let json = body_json(response).await;
    assert_eq!(json["status"], "degraded");
    assert_eq!(json["checks"]["render_loop"], "degraded");
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
    assert!(
        json["data"]["active_scene"].is_string(),
        "active_scene should be a string"
    );
    assert!(
        json["data"]["active_scene_snapshot_locked"].is_boolean(),
        "active_scene_snapshot_locked should be a bool"
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
    assert_eq!(
        json["data"]["capture_available"],
        serde_json::json!(
            cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
        )
    );
}

#[tokio::test]
async fn status_reports_stopped_render_loop_as_not_running() {
    let state = Arc::new(isolated_state());
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.stop();
    }

    let app = test_app_with_state(state);
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
    assert_eq!(json["data"]["running"], serde_json::json!(false));
    assert_eq!(json["data"]["render_loop"]["state"], "stopped");
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
async fn config_set_render_canvas_updates_active_layout_dimensions() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let mut state = isolated_state();
    state.config_manager = Some(config_manager);

    let active_layout = {
        let spatial = state
            .spatial_engine
            .try_read()
            .expect("spatial engine should not be contended");
        spatial.layout().as_ref().clone()
    };
    {
        let mut layouts = state
            .layouts
            .try_write()
            .expect("layout store should not be contended");
        layouts.insert(active_layout.id.clone(), active_layout.clone());
    }

    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));

    for (key, value) in [
        ("daemon.canvas_width", "1024"),
        ("daemon.canvas_height", "768"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/config/set")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"key":"{key}","value":"{value}"}}"#
                    )))
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["data"]["key"], key);
        assert_eq!(json["data"]["live"], true);
    }

    {
        let spatial = state.spatial_engine.read().await;
        assert_eq!(spatial.layout().canvas_width, 1024);
        assert_eq!(spatial.layout().canvas_height, 768);
    }

    {
        let layouts = state.layouts.read().await;
        let saved = layouts
            .get("default")
            .expect("active layout should remain persisted");
        assert_eq!(saved.canvas_width, 1024);
        assert_eq!(saved.canvas_height, 768);
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.daemon.canvas_width, 1024);
    assert_eq!(config.daemon.canvas_height, 768);

    let transactions = state.scene_transactions.drain();
    assert!(transactions.len() >= 4);
    assert!(transactions.iter().any(|transaction| {
        matches!(
            transaction,
            SceneTransaction::ReplaceLayout(layout)
                if layout.id == "default"
                    && layout.canvas_width == 1024
                    && layout.canvas_height == 768
        )
    }));
    assert!(transactions.iter().any(|transaction| {
        matches!(
            transaction,
            SceneTransaction::ResizeCanvas { width, height }
                if *width == 1024 && *height == 768
        )
    }));
    assert!(transactions.iter().any(|transaction| {
        matches!(
            transaction,
            SceneTransaction::ResizeCanvas { width, .. } if *width == 1024
        )
    }));
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
    assert!(body.contains("/api/v1/simulators/displays"));
    assert!(body.contains("id=\"previewMode\""));
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
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
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

fn test_html_effect_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} html effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned(), "html".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html {
            path: format!("/tmp/{name}.html").into(),
        },
        license: None,
    }
}

fn test_display_face_effect_metadata(name: &str) -> EffectMetadata {
    let mut metadata = test_html_effect_metadata(name);
    metadata.category = EffectCategory::Display;
    metadata
}

async fn insert_test_html_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = test_html_effect_metadata(name);
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let mut registry = state.effect_registry.write().await;
    let _ = registry.register(entry);
    metadata
}

async fn activate_test_html_scene_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = insert_test_html_effect(state, name).await;
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let mut scene_manager = state.scene_manager.write().await;
    scene_manager
        .upsert_primary_group(&metadata, HashMap::new(), None, layout)
        .expect("html test effect should populate the primary scene group");
    metadata
}

async fn insert_test_display_face_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = test_display_face_effect_metadata(name);
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let mut registry = state.effect_registry.write().await;
    let _ = registry.register(entry);
    metadata
}

async fn activate_empty_test_scene(state: &Arc<AppState>, name: &str) -> SceneId {
    activate_empty_test_scene_with_mode(state, name, SceneMutationMode::Live).await
}

async fn activate_empty_test_scene_with_mode(
    state: &Arc<AppState>,
    name: &str,
    mutation_mode: SceneMutationMode,
) -> SceneId {
    let scene = Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: Vec::new(),
        transition: TransitionSpec {
            duration_ms: 0,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode,
    };

    let mut manager = state.scene_manager.write().await;
    manager
        .create(scene.clone())
        .expect("test scene should be created");
    manager
        .activate(&scene.id, None)
        .expect("test scene should activate");
    scene.id
}

async fn activate_display_face_test_scene(
    state: &Arc<AppState>,
    name: &str,
    effect_id: EffectId,
    device_id: DeviceId,
) -> SceneId {
    let scene = Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: vec![RenderGroup {
            id: hypercolor_types::scene::RenderGroupId::new(),
            name: "Display Face".to_owned(),
            description: None,
            effect_id: Some(effect_id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: SpatialLayout {
                id: "display-face-layout".to_owned(),
                name: "Display Face Layout".to_owned(),
                description: None,
                canvas_width: 320,
                canvas_height: 320,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(DisplayFaceTarget { device_id }),
            role: RenderGroupRole::Display,
        }],
        transition: TransitionSpec {
            duration_ms: 0,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    };

    let mut manager = state.scene_manager.write().await;
    manager
        .create(scene.clone())
        .expect("display face scene should be created");
    manager
        .activate(&scene.id, None)
        .expect("display face scene should activate");
    scene.id
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
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

async fn insert_test_display_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::Wled,
        model: Some("LCD".to_owned()),
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "LCD".to_owned(),
            led_count: 320 * 320,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
            },
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 320 * 320,
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((320, 320)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

#[cfg(feature = "hue")]
async fn insert_test_hue_bridge_device(
    state: &Arc<AppState>,
    name: &str,
    bridge_id: &str,
    ip: &str,
    api_port: u16,
) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "Philips Hue".to_owned(),
        family: DeviceFamily::Hue,
        model: Some("Bridge".to_owned()),
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Bridge".to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Point,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("1.0.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 1,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint(format!("hue:{bridge_id}"));
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("backend_id".to_owned(), "hue".to_owned());
    metadata.insert("bridge_id".to_owned(), bridge_id.to_owned());
    metadata.insert("ip".to_owned(), ip.to_owned());
    metadata.insert("api_port".to_owned(), api_port.to_string());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
}

#[cfg(feature = "nanoleaf")]
async fn insert_test_nanoleaf_device(
    state: &Arc<AppState>,
    name: &str,
    device_key: &str,
    ip: &str,
    api_port: u16,
) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "Nanoleaf".to_owned(),
        family: DeviceFamily::Nanoleaf,
        model: Some("Shapes".to_owned()),
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Panel".to_owned(),
            led_count: 12,
            topology: DeviceTopologyHint::Matrix { rows: 3, cols: 4 },
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("12.0.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 12,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint(format!("nanoleaf:{device_key}"));
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("backend_id".to_owned(), "nanoleaf".to_owned());
    metadata.insert("device_key".to_owned(), device_key.to_owned());
    metadata.insert("ip".to_owned(), ip.to_owned());
    metadata.insert("api_port".to_owned(), api_port.to_string());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
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
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
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
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
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
    assert_eq!(
        json["data"]["backend_ids"],
        serde_json::json!(["simulator"])
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
async fn get_active_effect_returns_idle_payload_when_none() {
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

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["state"], "idle");
    assert!(json["data"]["id"].is_null());
    assert!(json["data"]["name"].is_null());
    assert!(json["data"]["render_group_id"].is_null());
}

#[tokio::test]
async fn apply_effect_upserts_primary_group() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let manager = state.scene_manager.read().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_group)
        .expect("active scene should contain a primary group");
    assert_eq!(primary.role, RenderGroupRole::Primary);
    assert!(primary.effect_id.is_some());
}

#[tokio::test]
async fn get_active_effect_returns_primary_group_info() {
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

    let primary_group_id = {
        let manager = state.scene_manager.read().await;
        manager
            .active_scene()
            .and_then(Scene::primary_group)
            .map(|group| (group.id.to_string(), group.effect_id))
            .expect("active scene should expose a primary group")
    };

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["id"],
        primary_group_id
            .1
            .expect("primary group should have an effect id")
            .to_string()
    );
    assert_eq!(json["data"]["name"], "solid_color");
    assert_eq!(json["data"]["state"], "running");
    assert_eq!(json["data"]["render_group_id"], primary_group_id.0);
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
async fn stop_current_clears_primary_effect_id_but_keeps_scene() {
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

    let stop_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(stop_response.status(), StatusCode::OK);

    let manager = state.scene_manager.read().await;
    let active_scene = manager.active_scene().expect("active scene should remain");
    assert_eq!(active_scene.id, SceneId::DEFAULT);
    let primary = active_scene
        .primary_group()
        .expect("primary group shell should remain after stop");
    assert!(primary.effect_id.is_none());
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
async fn patch_controls_updates_primary_group_controls() {
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

#[tokio::test]
async fn apply_effect_swap_replaces_primary_effect_id() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;
    insert_test_effect(&state, "Sunset").await;
    let app = test_app_with_state(Arc::clone(&state));

    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Aurora/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_primary_effect_id = {
        let manager = state.scene_manager.read().await;
        manager
            .active_scene()
            .and_then(Scene::primary_group)
            .and_then(|group| group.effect_id)
            .expect("first effect apply should populate the primary group")
    };

    let second_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Sunset/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_response.status(), StatusCode::OK);

    let manager = state.scene_manager.read().await;
    let active_scene = manager.active_scene().expect("active scene should remain");
    assert_eq!(active_scene.groups.len(), 1);
    let primary = active_scene
        .primary_group()
        .expect("primary group should exist after effect swap");
    assert_ne!(primary.effect_id, Some(first_primary_effect_id));
    assert!(primary.effect_id.is_some());
}

#[tokio::test]
async fn put_current_control_binding_updates_active_effect_schema() {
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

    let binding = ControlBinding {
        sensor: " cpu_temp ".to_owned(),
        sensor_min: 30.0,
        sensor_max: 100.0,
        target_min: 0.0,
        target_max: 1.0,
        deadband: -0.5,
        smoothing: 1.2,
    };
    let binding_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/effects/current/controls/speed/binding")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&binding).expect("binding should serialize"),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(binding_response.status(), StatusCode::OK);
    let binding_json = body_json(binding_response).await;
    assert_eq!(binding_json["data"]["control"], "speed");
    assert_eq!(binding_json["data"]["binding"]["sensor"], "cpu_temp");
    assert_eq!(binding_json["data"]["binding"]["deadband"], 0.0);
    assert!(
        (binding_json["data"]["binding"]["smoothing"]
            .as_f64()
            .expect("smoothing should be numeric")
            - 0.99)
            .abs()
            < 1.0e-6
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
    assert_eq!(
        active_json["data"]["controls"][0]["binding"]["sensor"],
        "cpu_temp"
    );
    assert_eq!(
        active_json["data"]["controls"][0]["binding"]["target_max"],
        1.0
    );
    assert_eq!(active_json["data"]["control_values"]["speed"]["float"], 5.0);

    let persisted =
        runtime_state::load(&state.runtime_state_path).expect("runtime state should load");
    let persisted = persisted.expect("runtime state should exist");
    let primary = persisted
        .default_scene_groups
        .iter()
        .find(|group| group.role == RenderGroupRole::Primary)
        .expect("primary group should be persisted");
    assert_eq!(
        primary
            .control_bindings
            .get("speed")
            .expect("binding should be persisted")
            .sensor,
        "cpu_temp"
    );
}

#[tokio::test]
async fn rest_effect_lifecycle_publishes_started_and_stopped_events() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let mut events = state.event_bus.subscribe_all();
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

    let started = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::EffectStarted {
                        effect,
                        trigger,
                        previous,
                        transition,
                    } = timestamped.event
                    {
                        break (effect, trigger, previous, transition);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before effect start event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for effect start event");

    assert_eq!(started.0.name, "solid_color");
    assert_eq!(started.1, ChangeTrigger::Api);
    assert!(
        started.2.is_none(),
        "first activation should have no previous effect"
    );
    assert!(
        started.3.is_none(),
        "REST effect apply should mirror MCP transition semantics"
    );

    let stop_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(stop_response.status(), StatusCode::OK);

    let stopped = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::EffectStopped { effect, reason } = timestamped.event {
                        break (effect, reason);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before effect stop event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for effect stop event");

    assert_eq!(stopped.0.name, "solid_color");
    assert_eq!(stopped.1, EffectStopReason::Stopped);
}

#[tokio::test]
async fn scene_activate_and_deactivate_publish_active_scene_events() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));
    let mut events = state.event_bus.subscribe_all();

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Studio"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    let create_json = body_json(create_response).await;
    let scene_id = create_json["data"]["id"]
        .as_str()
        .expect("scene id should be present")
        .parse::<uuid::Uuid>()
        .map(SceneId)
        .expect("scene id should parse");

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/scenes/{scene_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);

    let activated = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ActiveSceneChanged {
                        previous,
                        current,
                        current_name,
                        current_snapshot_locked,
                        reason,
                        ..
                    } = timestamped.event
                    {
                        break (
                            previous,
                            current,
                            current_name,
                            current_snapshot_locked,
                            reason,
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before scene activation event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for scene activation event");

    assert_eq!(activated.0, Some(SceneId::DEFAULT));
    assert_eq!(activated.1, scene_id);
    assert_eq!(activated.2, "Studio");
    assert!(!activated.3);
    assert_eq!(activated.4, SceneChangeReason::UserActivate);

    let deactivate_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes/deactivate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(deactivate_response.status(), StatusCode::OK);

    let deactivated = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ActiveSceneChanged {
                        previous,
                        current,
                        current_name,
                        current_snapshot_locked,
                        reason,
                        ..
                    } = timestamped.event
                    {
                        break (
                            previous,
                            current,
                            current_name,
                            current_snapshot_locked,
                            reason,
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before scene deactivation event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for scene deactivation event");

    assert_eq!(deactivated.0, Some(scene_id));
    assert_eq!(deactivated.1, SceneId::DEFAULT);
    assert_eq!(deactivated.2, "Default");
    assert!(!deactivated.3);
    assert_eq!(deactivated.4, SceneChangeReason::UserDeactivate);
}

#[tokio::test]
async fn patch_current_controls_publishes_render_group_and_control_events() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));
    let mut events = state.event_bus.subscribe_all();

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

    let patch_response = app
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
    assert_eq!(patch_response.status(), StatusCode::OK);

    let mut saw_render_group_change = false;
    let mut saw_control_change = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !saw_render_group_change || !saw_control_change {
            match events.recv().await {
                Ok(timestamped) => match timestamped.event {
                    HypercolorEvent::RenderGroupChanged {
                        scene_id,
                        role,
                        kind,
                        ..
                    } => {
                        if scene_id == SceneId::DEFAULT
                            && role == RenderGroupRole::Primary
                            && kind == RenderGroupChangeKind::ControlsPatched
                        {
                            saw_render_group_change = true;
                        }
                    }
                    HypercolorEvent::EffectControlChanged {
                        control_id,
                        old_value,
                        new_value,
                        trigger,
                        ..
                    } => {
                        if control_id == "speed"
                            && old_value == hypercolor_types::event::EventControlValue::Number(5.0)
                            && new_value == hypercolor_types::event::EventControlValue::Number(7.5)
                            && trigger == ChangeTrigger::Api
                        {
                            saw_control_change = true;
                        }
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before control change events arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for control patch events");
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
    let (state, tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let scenes_path = tempdir.path().join("data/scenes.json");

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
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&scenes_path).expect("scene store should be written after create"),
    )
    .expect("scene store should parse");
    assert_eq!(persisted[scene_id.as_str()]["name"], "Test Scene");

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
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&scenes_path).expect("scene store should be written after update"),
    )
    .expect("scene store should parse");
    assert_eq!(persisted[scene_id.as_str()]["name"], "Updated Scene");

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

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], scene_id);
    assert_eq!(json["data"]["kind"], "named");
    assert!(
        json["data"]["groups"]
            .as_array()
            .expect("groups should serialize as an array")
            .is_empty()
    );

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
    let persisted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&scenes_path).expect("scene store should be written after delete"),
    )
    .expect("scene store should parse");
    assert!(
        persisted.get(scene_id.as_str()).is_none(),
        "deleted scene should be removed from the scene store"
    );

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

#[tokio::test]
async fn scene_deactivate_returns_to_default_scene() {
    let state = Arc::new(isolated_state());

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Work"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    let json = body_json(response).await;
    let scene_id = json["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

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

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes/deactivate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["scene"]["name"], "Default");

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Default");
    assert_eq!(json["data"]["kind"], "ephemeral");
    assert!(
        json["data"]["groups"]
            .as_array()
            .expect("groups should serialize as an array")
            .is_empty()
    );
}

#[tokio::test]
async fn list_scenes_excludes_default_scene() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .clone()
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
    assert_eq!(json["data"]["pagination"]["total"], 0);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Movie Night"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::CREATED);

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
    let items = json["data"]["items"]
        .as_array()
        .expect("scene list should serialize as an array");
    assert_eq!(items[0]["name"], "Movie Night");
    assert!(
        items.iter().all(|item| item["name"] != "Default"),
        "default scene must stay hidden from the scenes list"
    );
}

#[tokio::test]
async fn delete_default_returns_409_or_422() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/scenes/default")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("cannot be deleted"),
    );
}

#[tokio::test]
async fn scene_deactivate_on_default_is_noop() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes/deactivate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["deactivated"], true);
    assert_eq!(json["data"]["scene"]["name"], "Default");
    assert_eq!(json["data"]["previous_scene"]["name"], "Default");
}

// ── Profiles ─────────────────────────────────────────────────────────────

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "profile lifecycle coverage is clearer as one end-to-end integration test"
)]
async fn profile_crud_lifecycle() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let profile_layout = SpatialLayout {
        id: "layout_profile".to_owned(),
        name: "Profile Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let alternate_layout = SpatialLayout {
        id: "layout_alternate".to_owned(),
        name: "Alternate Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    {
        let mut layouts = state.layouts.write().await;
        layouts.insert(profile_layout.id.clone(), profile_layout.clone());
        layouts.insert(alternate_layout.id.clone(), alternate_layout.clone());
    }
    {
        let mut spatial = state.spatial_engine.write().await;
        spatial.update_layout(profile_layout.clone());
    }
    set_global_brightness(&state.power_state, 0.72);

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"controls":{"speed":12.5}}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"effect_id":"{}"}}"#, face.id)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    // Create profile
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Gaming Mode"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Gaming Mode");
    assert_eq!(json["data"]["brightness"], 72);
    assert_eq!(json["data"]["layout_id"], profile_layout.id);
    assert_eq!(json["data"]["primary"]["controls"]["speed"]["float"], 12.5);
    assert_eq!(
        json["data"]["displays"][0]["device_id"],
        display_id.to_string()
    );
    assert_eq!(
        json["data"]["displays"][0]["effect_id"],
        face.id.to_string()
    );
    let primary_effect_id = json["data"]["primary"]["effect_id"]
        .as_str()
        .expect("primary effect id should be present")
        .to_owned();
    let profile_id = json["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

    // Get profile
    let response = app
        .clone()
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
    assert_eq!(json["data"]["primary"]["controls"]["speed"]["float"], 12.5);
    assert_eq!(
        json["data"]["displays"][0]["effect_id"],
        face.id.to_string()
    );
    assert_eq!(json["data"]["layout_id"], profile_layout.id);

    // List profiles
    let response = app
        .clone()
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
    let response = app
        .clone()
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
    assert_eq!(json["data"]["brightness"], 50);

    {
        let mut spatial = state.spatial_engine.write().await;
        spatial.update_layout(alternate_layout);
    }
    set_global_brightness(&state.power_state, 0.05);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/stop")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    // Apply profile
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
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
    assert_eq!(
        json["data"]["profile"]["primary"]["effect_id"],
        primary_effect_id
    );
    assert_eq!(
        json["data"]["profile"]["primary"]["controls"]["speed"]["float"],
        12.5
    );
    assert_eq!(
        json["data"]["profile"]["displays"][0]["device_id"],
        display_id.to_string()
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "solid_color");
    assert_eq!(json["data"]["control_values"]["speed"]["float"], 12.5);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["effect"]["id"], face.id.to_string());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], profile_layout.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/settings/brightness")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["brightness"], 50);

    // Delete profile
    let response = app
        .clone()
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

#[tokio::test]
async fn pre_final_profile_shape_is_rejected_on_load() {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    fs::create_dir_all(&data_dir).expect("temp data dir should be created");

    let effect_id = EffectId::new(Uuid::now_v7());
    let preset_id = PresetId(Uuid::now_v7());
    let profiles_path = data_dir.join("profiles.json");
    fs::write(
        &profiles_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "prof_evening": {
                "id": "prof_evening",
                "name": "Evening",
                "effect_id": effect_id,
                "effect_name": "solid_color",
                "active_preset_id": preset_id,
                "controls": {
                    "speed": { "float": 12.5 }
                }
            }
        }))
        .expect("pre-final profile json should serialize"),
    )
    .expect("pre-final profile json should be written");

    ConfigManager::set_data_dir_override(Some(data_dir.clone()));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);

    {
        let profiles = state.profiles.read().await;
        assert!(
            profiles.get("prof_evening").is_none(),
            "invalid pre-final profiles should be dropped on load"
        );
    }
}

#[tokio::test]
async fn create_profile_rejects_duplicate_names_without_force_and_force_overwrites() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Gaming Mode","brightness":72}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_response.status(), StatusCode::CREATED);
    let first_json = body_json(first_response).await;
    let first_id = first_json["data"]["id"]
        .as_str()
        .expect("profile id should be present")
        .to_owned();

    let duplicate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"gaming mode","brightness":15}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(duplicate_response.status(), StatusCode::CONFLICT);
    let duplicate_json = body_json(duplicate_response).await;
    assert_eq!(duplicate_json["error"]["code"], "conflict");

    let forced_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"gaming mode","brightness":15,"force":true}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(forced_response.status(), StatusCode::OK);
    let forced_json = body_json(forced_response).await;
    assert_eq!(forced_json["data"]["id"], first_id);
    assert_eq!(forced_json["data"]["brightness"], 15);

    let list_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/profiles")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["pagination"]["total"], 1);
}

#[tokio::test]
async fn profile_lookup_returns_conflict_for_ambiguous_name() {
    let state = Arc::new(isolated_state());
    {
        let mut profiles = state.profiles.write().await;
        profiles.insert(Profile::named("prof_alpha", "Evening"));
        profiles.insert(Profile::named("prof_beta", "evening"));
    }

    let app = test_app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/profiles/evening")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("ambiguous"),
    );
}

#[tokio::test]
async fn apply_profile_rejects_unimplemented_transition_requests() {
    let state = Arc::new(isolated_state());
    {
        let mut profiles = state.profiles.write().await;
        profiles.insert(Profile::named("prof_evening", "Evening"));
    }

    let app = test_app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles/prof_evening/apply")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"transition_ms":250}"#))
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
            .expect("message should be a string")
            .contains("only immediate apply is supported"),
    );
}

#[tokio::test]
async fn apply_profile_conflicts_when_snapshot_scene_is_active() {
    let state = Arc::new(isolated_state());
    {
        let mut profiles = state.profiles.write().await;
        let mut profile = Profile::named("prof_evening", "Evening");
        profile.brightness = Some(40);
        profiles.insert(profile);
    }
    activate_empty_test_scene_with_mode(&state, "Focus", SceneMutationMode::Snapshot).await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles/prof_evening/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("snapshot mode"),
    );
}

#[tokio::test]
async fn failed_profile_apply_does_not_mutate_layout_or_brightness() {
    let state = Arc::new(isolated_state());
    let current_layout = SpatialLayout {
        id: "layout_current".to_owned(),
        name: "Current Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    let profile_layout = SpatialLayout {
        id: "layout_profile".to_owned(),
        name: "Profile Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };
    {
        let mut layouts = state.layouts.write().await;
        layouts.insert(current_layout.id.clone(), current_layout.clone());
        layouts.insert(profile_layout.id.clone(), profile_layout);
    }
    {
        let mut spatial = state.spatial_engine.write().await;
        spatial.update_layout(current_layout.clone());
    }
    set_global_brightness(&state.power_state, 0.8);
    {
        let mut profiles = state.profiles.write().await;
        let mut profile = Profile::named("prof_broken", "Broken");
        profile.brightness = Some(25);
        profile.primary = Some(ProfilePrimary {
            effect_id: EffectId::new(Uuid::now_v7()),
            controls: HashMap::new(),
            active_preset_id: None,
        });
        profile.layout_id = Some("layout_profile".to_owned());
        profiles.insert(profile);
    }

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profiles/prof_broken/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = body_json(response).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("profile effect not found"),
    );

    let active_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().id.clone()
    };
    assert_eq!(active_layout, current_layout.id);
    assert!((current_global_brightness(&state.power_state) - 0.8).abs() < f32::EPSILON);
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
async fn layout_create_defaults_canvas_to_active_layout_dimensions() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let active_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Canvas Follower"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["canvas_width"], active_layout.canvas_width);
    assert_eq!(json["data"]["canvas_height"], active_layout.canvas_height);
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

    let transactions = state.scene_transactions.drain();
    assert!(matches!(
        transactions.first(),
        Some(SceneTransaction::ReplaceLayout(layout))
            if layout.id == layout_id && layout.canvas_width == 640 && layout.canvas_height == 360
    ));
    assert!(transactions.iter().any(|transaction| {
        matches!(
            transaction,
            SceneTransaction::ResizeCanvas { width, height }
                if *width == 640 && *height == 360
        )
    }));

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

// ── Effect Layout Associations ──────────────────────────────────────────

fn test_state_with_temp_effect_layout_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = isolated_state();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.effect_layout_links_path = dir.path().join("effect-layouts.json");
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

#[tokio::test]
async fn apply_effect_rejects_display_face_effects() {
    let state = Arc::new(isolated_state());
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/effects/{}/apply", face.id))
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
async fn apply_effect_mutates_active_scene_not_default_if_named_active() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;
    insert_test_effect(&state, "Sunset").await;
    let app = test_app_with_state(Arc::clone(&state));

    let default_apply = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Aurora/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(default_apply.status(), StatusCode::OK);

    let default_effect_id = {
        let manager = state.scene_manager.read().await;
        manager
            .get(&SceneId::DEFAULT)
            .and_then(Scene::primary_group)
            .and_then(|group| group.effect_id)
            .expect("default scene should retain its primary effect")
            .to_string()
    };

    let named_scene_id = activate_empty_test_scene(&state, "Focus").await;

    let named_apply = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Sunset/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(named_apply.status(), StatusCode::OK);

    let named_primary_group_id = {
        let manager = state.scene_manager.read().await;
        let default_scene = manager
            .get(&SceneId::DEFAULT)
            .expect("default scene should still exist");
        assert_eq!(
            default_scene
                .primary_group()
                .and_then(|group| group.effect_id)
                .map(|effect_id| effect_id.to_string()),
            Some(default_effect_id.clone())
        );

        let active_scene = manager
            .active_scene()
            .expect("named scene should stay active");
        assert_eq!(active_scene.id, named_scene_id);
        let primary = active_scene
            .primary_group()
            .expect("named scene should gain a primary group");
        assert_ne!(
            primary.effect_id.map(|effect_id| effect_id.to_string()),
            Some(default_effect_id.clone())
        );
        primary.id.to_string()
    };

    let active_named = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_named.status(), StatusCode::OK);
    let active_named_json = body_json(active_named).await;
    assert_eq!(active_named_json["data"]["name"], "Sunset");
    assert_eq!(
        active_named_json["data"]["render_group_id"],
        named_primary_group_id
    );

    let deactivate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes/deactivate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(deactivate.status(), StatusCode::OK);

    let active_default = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_default.status(), StatusCode::OK);
    let active_default_json = body_json(active_default).await;
    assert_eq!(active_default_json["data"]["name"], "Aurora");
    assert_ne!(
        active_default_json["data"]["render_group_id"],
        named_primary_group_id
    );
}

#[tokio::test]
async fn activating_named_scene_then_applying_effect_mutates_named_scene() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Sunset").await;
    let app = test_app_with_state(Arc::clone(&state));
    let named_scene_id = activate_empty_test_scene(&state, "Focus").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Sunset/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let manager = state.scene_manager.read().await;
    let default_scene = manager
        .get(&SceneId::DEFAULT)
        .expect("default scene should still exist");
    assert!(
        default_scene.primary_group().is_none(),
        "default scene should not be mutated while a named scene is active"
    );

    let active_scene = manager
        .active_scene()
        .expect("named scene should stay active");
    assert_eq!(active_scene.id, named_scene_id);
    assert!(
        active_scene
            .primary_group()
            .and_then(|group| group.effect_id)
            .is_some()
    );
}

#[tokio::test]
async fn apply_effect_conflicts_when_snapshot_scene_is_active() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;
    activate_empty_test_scene_with_mode(&state, "Focus", SceneMutationMode::Snapshot).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Aurora/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("snapshot mode"),
    );

    let manager = state.scene_manager.read().await;
    assert!(
        manager
            .active_scene()
            .and_then(Scene::primary_group)
            .is_none(),
        "snapshot scene should not be rewritten by effect apply",
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
async fn list_displays_only_returns_display_capable_devices() {
    let state = Arc::new(isolated_state());
    let _ = insert_test_device(&state, "Desk Strip").await;
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/displays")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let items = json["data"]
        .as_array()
        .expect("display list should be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], display_id.to_string());
    assert_eq!(items[0]["name"], "Pump LCD");
    assert_eq!(items[0]["width"], 320);
    assert_eq!(items[0]["height"], 320);
    assert_eq!(items[0]["circular"], true);
}

#[tokio::test]
async fn delete_face_idempotent_when_no_group_present() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/displays/{display_id}/face"))
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["data"]["device_id"], display_id.to_string());
        assert_eq!(json["data"]["deleted"], true);
    }
}

#[tokio::test]
async fn get_face_returns_null_when_no_display_group() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn patch_face_controls_updates_display_group() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let mut face = test_display_face_effect_metadata("System Monitor");
    face.controls = vec![ControlDefinition {
        id: "label".to_owned(),
        name: "Label".to_owned(),
        kind: ControlKind::Text,
        control_type: ControlType::TextInput,
        default_value: ControlValue::Text("cpu".to_owned()),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some("General".to_owned()),
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    }];
    {
        let mut registry = state.effect_registry.write().await;
        let _ = registry.register(EffectEntry {
            metadata: face.clone(),
            source_path: format!("/tmp/{}.html", face.name).into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        });
    }
    let app = test_app_with_state(Arc::clone(&state));

    let assign_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"effect_id":"{}"}}"#, face.id)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_response.status(), StatusCode::OK);

    let patch_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/controls"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"controls":{"label":"gpu"}}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(patch_response.status(), StatusCode::OK);
    let patch_json = body_json(patch_response).await;
    assert_eq!(
        patch_json["data"]["group"]["controls"]["label"]["text"],
        "gpu"
    );

    let manager = state.scene_manager.read().await;
    let display_group = manager
        .active_scene()
        .and_then(|scene| scene.display_group_for(display_id))
        .expect("display face should remain assigned");
    assert_eq!(
        display_group.controls.get("label"),
        Some(&ControlValue::Text("gpu".to_owned()))
    );
}

#[tokio::test]
async fn put_face_conflicts_when_snapshot_scene_is_active() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    activate_empty_test_scene_with_mode(&state, "Focus", SceneMutationMode::Snapshot).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"effect_id":"{}"}}"#, face.id)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("snapshot mode"),
    );

    let manager = state.scene_manager.read().await;
    assert!(
        manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(display_id))
            .is_none(),
        "snapshot scene should not be rewritten by face assignment",
    );
}

#[tokio::test]
async fn display_face_endpoints_assign_get_and_delete_face() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let scene_id = activate_empty_test_scene(&state, "Desk Scene").await;
    let app = test_app_with_state(Arc::clone(&state));

    let put_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"effect_id":"{}"}}"#, face.id)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(put_response.status(), StatusCode::OK);
    let put_json = body_json(put_response).await;
    assert_eq!(put_json["data"]["device_id"], display_id.to_string());
    assert_eq!(put_json["data"]["scene_id"], scene_id.to_string());
    assert_eq!(put_json["data"]["effect"]["id"], face.id.to_string());
    assert_eq!(put_json["data"]["effect"]["category"], "display");
    assert_eq!(
        put_json["data"]["group"]["display_target"]["device_id"],
        display_id.to_string()
    );
    assert_eq!(put_json["data"]["group"]["layout"]["canvas_width"], 320);
    assert_eq!(put_json["data"]["group"]["layout"]["canvas_height"], 320);
    assert!(
        put_json["data"]["group"]["layout"]["zones"]
            .as_array()
            .expect("zones should serialize as an array")
            .is_empty()
    );

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["effect"]["id"], face.id.to_string());
    assert_eq!(
        get_json["data"]["group"]["display_target"]["device_id"],
        display_id.to_string()
    );

    {
        let manager = state.scene_manager.read().await;
        let active_scene = manager.active_scene().expect("scene should be active");
        assert_eq!(active_scene.id, scene_id);
        assert_eq!(active_scene.groups.len(), 1);
    }

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);
    let delete_json = body_json(delete_response).await;
    assert_eq!(delete_json["data"]["deleted"], true);

    let missing_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(missing_response.status(), StatusCode::OK);
    let missing_json = body_json(missing_response).await;
    assert!(missing_json["data"].is_null());

    let manager = state.scene_manager.read().await;
    let active_scene = manager.active_scene().expect("scene should remain active");
    assert!(active_scene.groups.is_empty());
}

#[tokio::test]
async fn face_survives_effect_swap() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;
    insert_test_effect(&state, "Sunset").await;
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let scene_id = activate_empty_test_scene(&state, "Desk Scene").await;
    let app = test_app_with_state(Arc::clone(&state));

    let assign_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"effect_id":"{}"}}"#, face.id)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_response.status(), StatusCode::OK);
    let assign_json = body_json(assign_response).await;
    let face_group_id = assign_json["data"]["group"]["id"]
        .as_str()
        .expect("face group id should be present")
        .to_owned();

    for effect_name in ["Aurora", "Sunset"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/effects/{effect_name}/apply"))
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");
        assert_eq!(response.status(), StatusCode::OK);
    }

    let active_effect = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_effect.status(), StatusCode::OK);
    let active_effect_json = body_json(active_effect).await;
    assert_eq!(active_effect_json["data"]["name"], "Sunset");
    assert_ne!(active_effect_json["data"]["render_group_id"], face_group_id);

    let face_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(face_response.status(), StatusCode::OK);
    let face_json = body_json(face_response).await;
    assert_eq!(face_json["data"]["scene_id"], scene_id.to_string());
    assert_eq!(face_json["data"]["effect"]["id"], face.id.to_string());
    assert_eq!(face_json["data"]["group"]["id"], face_group_id);

    let manager = state.scene_manager.read().await;
    let active_scene = manager.active_scene().expect("scene should remain active");
    assert_eq!(active_scene.id, scene_id);
    assert_eq!(active_scene.groups.len(), 2);
    let primary = active_scene
        .primary_group()
        .expect("primary group should exist after effect apply");
    let display_group = active_scene
        .display_group_for(display_id)
        .expect("display face should remain assigned");
    assert_eq!(display_group.id.to_string(), face_group_id);
    assert_eq!(display_group.effect_id, Some(face.id));
    assert_ne!(primary.id, display_group.id);
}

#[tokio::test]
async fn put_face_from_cold_start_succeeds_no_409() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"effect_id":"{}"}}"#, face.id)))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["scene_id"],
        hypercolor_types::scene::SceneId::DEFAULT.to_string()
    );
    assert_eq!(json["data"]["effect"]["id"], face.id.to_string());
}

#[tokio::test]
async fn display_face_endpoint_rejects_non_display_effects() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    insert_test_effect(&state, "Rainbow").await;
    activate_empty_test_scene(&state, "Desk Scene").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"effect_id":"Rainbow"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
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
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
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

#[cfg(feature = "hue")]
#[tokio::test]
async fn list_devices_includes_hue_auth_summary_when_pairing_required() {
    let state = Arc::new(isolated_state());
    let _device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "10.0.0.5", 80).await;
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
    let auth = &json["data"]["items"][0]["auth"];
    assert_eq!(auth["state"], "required");
    assert_eq!(auth["can_pair"], true);
    assert_eq!(auth["descriptor"]["kind"], "physical_action");
    assert_eq!(auth["descriptor"]["action_label"], "Pair Bridge");
}

#[cfg(feature = "hue")]
#[tokio::test]
async fn list_devices_includes_hue_auth_summary_when_configured() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let _device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "10.0.0.5", 80).await;
    state
        .credential_store
        .store(
            "hue:test-bridge",
            Credentials::HueBridge {
                api_key: "api-key".to_owned(),
                client_key: "client-key".to_owned(),
            },
        )
        .await
        .expect("store credentials");
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
    assert_eq!(json["data"]["items"][0]["auth"]["state"], "configured");
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

#[tokio::test]
async fn deleting_display_device_prunes_scene_display_groups_and_persists_cleanup() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    {
        let mut manager = state.scene_manager.write().await;
        manager
            .upsert_display_group(
                display_id,
                "Pump LCD",
                &face,
                HashMap::new(),
                SpatialLayout {
                    id: "default-display-layout".to_owned(),
                    name: "Default Display Layout".to_owned(),
                    description: None,
                    canvas_width: 320,
                    canvas_height: 320,
                    zones: Vec::new(),
                    default_sampling_mode: SamplingMode::Bilinear,
                    default_edge_behavior: EdgeBehavior::Clamp,
                    spaces: None,
                    version: 1,
                },
            )
            .expect("default scene face should be assigned");
    }
    let named_scene_id =
        activate_display_face_test_scene(&state, "Desk Scene", face.id, display_id).await;
    {
        let mut manager = state.scene_manager.write().await;
        manager.deactivate_current();
    }

    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{display_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], display_id.to_string());
    assert!(state.device_registry.get(&display_id).await.is_none());

    let mut removed_scene_ids = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while removed_scene_ids.len() < 2 {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::RenderGroupChanged {
                        scene_id,
                        role,
                        kind,
                        ..
                    } = timestamped.event
                        && role == RenderGroupRole::Display
                        && kind == RenderGroupChangeKind::Removed
                    {
                        removed_scene_ids.push(scene_id);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before display-group removal events arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for display-group removal events");
    assert!(removed_scene_ids.contains(&SceneId::DEFAULT));
    assert!(removed_scene_ids.contains(&named_scene_id));

    {
        let manager = state.scene_manager.read().await;
        let default_scene = manager
            .active_scene()
            .expect("default scene should remain active");
        assert!(default_scene.display_group_for(display_id).is_none());
        let named_scene = manager
            .get(&named_scene_id)
            .expect("named scene should remain present");
        assert!(named_scene.display_group_for(display_id).is_none());
    }

    let persisted =
        runtime_state::load(&state.runtime_state_path).expect("runtime state should load");
    let persisted = persisted.expect("runtime state should exist");
    assert!(
        persisted.default_scene_groups.iter().all(|group| {
            group
                .display_target
                .as_ref()
                .is_none_or(|target| target.device_id != display_id)
        }),
        "deleted device should not survive in the persisted default scene"
    );

    let scene_store = state.scene_store.read().await;
    let named_scene = scene_store
        .list()
        .find(|scene| scene.id == named_scene_id)
        .expect("named scene should be persisted");
    assert!(
        named_scene.groups.iter().all(|group| {
            group
                .display_target
                .as_ref()
                .is_none_or(|target| target.device_id != display_id)
        }),
        "deleted device should not survive in persisted named scenes"
    );
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
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
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

#[cfg(feature = "hue")]
#[tokio::test]
async fn pair_device_route_pairs_hue_by_device_id() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Hue mock server");
    let port = listener.local_addr().expect("local addr").port();
    let device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "127.0.0.1", port)
            .await;
    let app = test_app_with_state(Arc::clone(&state));

    let server_task = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_pairing_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("POST /api HTTP/1.1") {
                pairing_json_response(
                    r#"[{"success":{"username":"test-api-key","clientkey":"00112233445566778899aabbccddeeff"}}]"#,
                )
            } else if request.starts_with("GET /api/config HTTP/1.1") {
                pairing_json_response(
                    r#"{"bridgeid":"test-bridge","name":"Studio Bridge","modelid":"BSB002","swversion":"1968096020"}"#,
                )
            } else {
                pairing_not_found_response()
            };
            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"activate_after_pair":true}"#))
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "paired");
    assert_eq!(json["data"]["activated"], false);
    assert_eq!(json["data"]["device"]["auth"]["state"], "configured");

    assert_eq!(
        state.credential_store.get("hue:test-bridge").await,
        Some(Credentials::HueBridge {
            api_key: "test-api-key".to_owned(),
            client_key: "00112233445566778899aabbccddeeff".to_owned(),
        })
    );

    server_task.await.expect("Hue mock task should finish");
}

#[cfg(feature = "hue")]
#[tokio::test]
async fn pair_device_route_returns_action_required_for_hue_without_button() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Hue mock server");
    let port = listener.local_addr().expect("local addr").port();
    let device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "127.0.0.1", port)
            .await;
    let app = test_app_with_state(Arc::clone(&state));

    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let _request = read_pairing_http_request(&mut stream)
            .await
            .expect("read HTTP request");
        stream
            .write_all(
                pairing_json_response(
                    r#"[{"error":{"type":101,"description":"link button not pressed"}}]"#,
                )
                .as_slice(),
            )
            .await
            .expect("write HTTP response");
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "action_required");
    assert_eq!(json["data"]["activated"], false);
    assert_eq!(json["data"]["device"]["auth"]["state"], "required");

    server_task.await.expect("Hue mock task should finish");
}

#[cfg(feature = "nanoleaf")]
#[tokio::test]
async fn pair_device_route_pairs_nanoleaf_by_device_id() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Nanoleaf mock server");
    let port = listener.local_addr().expect("local addr").port();
    let device_id =
        insert_test_nanoleaf_device(&state, "Living Room Shapes", "serial42", "127.0.0.1", port)
            .await;
    let app = test_app_with_state(Arc::clone(&state));

    let server_task = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_pairing_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("POST /api/v1/new HTTP/1.1") {
                pairing_json_response(r#"{"auth_token":"nanoleaf-token"}"#)
            } else if request.starts_with("GET /api/v1/nanoleaf-token HTTP/1.1") {
                pairing_json_response(
                    r#"{"name":"Living Room Shapes","model":"Shapes","serialNo":"SERIAL42","firmwareVersion":"12.0.0"}"#,
                )
            } else {
                pairing_not_found_response()
            };
            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"activate_after_pair":true}"#))
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "paired");
    assert_eq!(json["data"]["activated"], false);
    assert_eq!(json["data"]["device"]["auth"]["state"], "configured");

    assert_eq!(
        state.credential_store.get("nanoleaf:serial42").await,
        Some(Credentials::Nanoleaf {
            auth_token: "nanoleaf-token".to_owned(),
        })
    );

    server_task.await.expect("Nanoleaf mock task should finish");
}

#[cfg(feature = "hue")]
#[tokio::test]
async fn delete_pairing_removes_hue_credentials() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "10.0.0.5", 80).await;
    state
        .credential_store
        .store(
            "hue:test-bridge",
            Credentials::HueBridge {
                api_key: "api-key".to_owned(),
                client_key: "client-key".to_owned(),
            },
        )
        .await
        .expect("store Hue credentials");
    state
        .credential_store
        .store(
            "hue:ip:10.0.0.5",
            Credentials::HueBridge {
                api_key: "api-key".to_owned(),
                client_key: "client-key".to_owned(),
            },
        )
        .await
        .expect("store Hue IP credentials");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "unpaired");
    assert_eq!(json["data"]["device"]["auth"]["state"], "required");
    assert_eq!(state.credential_store.get("hue:test-bridge").await, None);
    assert_eq!(state.credential_store.get("hue:ip:10.0.0.5").await, None);
}

#[cfg(feature = "nanoleaf")]
#[tokio::test]
async fn delete_pairing_removes_nanoleaf_credentials() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let device_id =
        insert_test_nanoleaf_device(&state, "Living Room Shapes", "serial42", "10.0.0.8", 16021)
            .await;
    state
        .credential_store
        .store(
            "nanoleaf:serial42",
            Credentials::Nanoleaf {
                auth_token: "auth-token".to_owned(),
            },
        )
        .await
        .expect("store Nanoleaf credentials");
    state
        .credential_store
        .store(
            "nanoleaf:ip:10.0.0.8",
            Credentials::Nanoleaf {
                auth_token: "auth-token".to_owned(),
            },
        )
        .await
        .expect("store Nanoleaf IP credentials");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "unpaired");
    assert_eq!(json["data"]["device"]["auth"]["state"], "required");
    assert_eq!(state.credential_store.get("nanoleaf:serial42").await, None);
    assert_eq!(
        state.credential_store.get("nanoleaf:ip:10.0.0.8").await,
        None
    );
}

fn pairing_json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

fn pairing_not_found_response() -> Vec<u8> {
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

async fn read_pairing_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = vec![0_u8; 4096];
    let mut total = 0_usize;
    let mut header_end = None;

    loop {
        let read = stream.read(&mut buf[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;

        if header_end.is_none() {
            header_end = buf[..total]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4);
        }

        if let Some(header_end) = header_end {
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if total >= header_end + content_length {
                break;
            }
        }

        if total == buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }

    Ok(String::from_utf8_lossy(&buf[..total]).into_owned())
}
