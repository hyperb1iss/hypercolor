//! Mounting rotation is a device setting: set through the device route,
//! reported on both the device and the display summaries, and refused for
//! hardware without a panel.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Method, Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::simulators::SimulatedDisplayExt;
use hypercolor_daemon::simulators::{SimulatedDisplayConfig, activate_simulated_displays};
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, SegmentInfo,
};
use hypercolor_types::scene::DisplayRotation;
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = Arc::new(AppState::new());
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

async fn register_display(state: &Arc<AppState>, name: &str) -> DeviceId {
    let config = SimulatedDisplayConfig {
        id: DeviceId::from_uuid(Uuid::now_v7()),
        name: name.to_owned(),
        width: 480,
        height: 480,
        circular: true,
        enabled: true,
    }
    .normalized();
    state
        .simulated_displays
        .write()
        .await
        .upsert(config.clone());
    activate_simulated_displays(
        &state.driver_host().discovery_runtime(),
        &state.simulated_displays,
    )
    .await
    .expect("simulated display should activate");
    config.id
}

async fn register_led_strip(state: &Arc<AppState>) -> DeviceId {
    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Studio Strip".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
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
        .add_discovered(DiscoveredDevice {
            fingerprint: DeviceFingerprint::from_persisted("wled:rotation-strip".to_owned()),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            info,
            metadata: HashMap::new(),
            claim: None,
        })
        .await
}

fn json_request(method: Method, uri: String, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn get_request(uri: String) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("request should build")
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("request should succeed")
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

#[tokio::test]
async fn setting_the_mount_rotation_sticks_to_the_device() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Inverted Fan").await;
    let app = api::build_router(Arc::clone(&state), None);

    let response = send(
        &app,
        json_request(
            Method::PUT,
            format!("/api/v1/devices/{device_id}"),
            serde_json::json!({ "display_rotation": "deg180" }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload = body_json(response).await;
    assert_eq!(payload["data"]["display_rotation"], "deg180");

    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("display should stay registered");
    assert_eq!(
        tracked.user_settings.display_rotation,
        DisplayRotation::Deg180
    );

    // The display summary reports the same mount.
    let displays = body_json(send(&app, get_request("/api/v1/displays".to_owned())).await).await;
    let display = displays["data"]
        .as_array()
        .expect("displays should be a bare array")
        .iter()
        .find(|display| display["id"] == device_id.to_string())
        .expect("the display should be listed");
    assert_eq!(display["rotation"], "deg180");

    // A later brightness-only update leaves the mount alone.
    let response = send(
        &app,
        json_request(
            Method::PUT,
            format!("/api/v1/devices/{device_id}"),
            serde_json::json!({ "brightness": 40 }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload = body_json(response).await;
    assert_eq!(payload["data"]["brightness"], 40);
    assert_eq!(payload["data"]["display_rotation"], "deg180");
}

#[tokio::test]
async fn mount_rotation_is_refused_for_hardware_without_a_panel() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_led_strip(&state).await;
    let app = api::build_router(Arc::clone(&state), None);

    let response = send(
        &app,
        json_request(
            Method::PUT,
            format!("/api/v1/devices/{device_id}"),
            serde_json::json!({ "display_rotation": "deg90" }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Non-display devices never report a mount at all.
    let devices = body_json(send(&app, get_request("/api/v1/devices".to_owned())).await).await;
    let strip = devices["data"]["items"]
        .as_array()
        .expect("devices should list items")
        .iter()
        .find(|device| device["id"] == device_id.to_string())
        .expect("the strip should be listed");
    assert!(strip.get("display_rotation").is_none());
}
