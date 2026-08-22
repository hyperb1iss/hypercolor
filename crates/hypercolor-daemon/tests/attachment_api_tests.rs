//! Integration tests for attachment template and profile endpoints.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};

use axum::body::Body;
use http::{Request, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tower::ServiceExt;

use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_driver_api::{BackendInfo, DeviceBackend};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    SegmentInfo,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

type Result<T> = std::result::Result<T, DeviceError>;

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

    fn attachments_dir(&self) -> PathBuf {
        self.data_dir.join("attachments")
    }

    fn attachment_profiles_path(&self) -> PathBuf {
        self.data_dir.join("attachment-profiles.json")
    }
}

impl Drop for TestDataDirGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
    }
}

fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
}

struct RecordingBackend {
    writes: Arc<StdMutex<Vec<Vec<[u8; 3]>>>>,
}

impl RecordingBackend {
    fn new(writes: Arc<StdMutex<Vec<Vec<[u8; 3]>>>>) -> Self {
        Self { writes }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for RecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "wled".to_owned(),
            name: "Recording Backend".to_owned(),
            description: "Captures attachment identify writes".to_owned(),
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
        self.writes
            .lock()
            .expect("recording backend mutex should not be poisoned")
            .push(colors.to_vec());
        Ok(())
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

async fn insert_test_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
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

async fn insert_prism_8_test_device(state: &Arc<AppState>) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: "PrismRGB Prism 8".to_owned(),
        vendor: "PrismRGB".to_owned(),
        family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
        model: Some("prism_8".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("nollie", "usb", ConnectionType::Usb)
            .with_protocol_id("nollie/prism-8"),
        segments: vec![SegmentInfo {
            name: "Channel 1".to_owned(),
            led_count: 126,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Grb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 126,
            supports_direct: true,
            supports_brightness: false,
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

async fn insert_nollie32_test_device(state: &Arc<AppState>) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: "Nollie 32".to_owned(),
        vendor: "Nollie".to_owned(),
        family: DeviceFamily::new_static("nollie", "Nollie"),
        model: Some("nollie_32".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("nollie", "usb", ConnectionType::Usb)
            .with_protocol_id("nollie/nollie-32"),
        segments: (1..=20)
            .map(|index| SegmentInfo {
                name: format!("Channel {index}"),
                led_count: 256,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Grb,
                layout_hint: None,
            })
            .collect(),
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 5_120,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

async fn send_json(
    app: &axum::Router,
    method: &str,
    uri: impl Into<String>,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri.into())
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
}

async fn send_empty(
    app: &axum::Router,
    method: &str,
    uri: impl Into<String>,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri.into())
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
}

fn user_strip_template(template_id: &str, name: &str, count: u32) -> Value {
    json!({
        "id": template_id,
        "name": name,
        "vendor": "Test Vendor",
        "category": "strip",
        "description": "Custom strip template for attachment API tests",
        "default_size": {
            "width": 0.35,
            "height": 0.08
        },
        "topology": {
            "type": "strip",
            "count": count,
            "direction": "left_to_right"
        },
        "compatible_slots": [],
        "tags": ["test", "strip"]
    })
}

async fn create_template(app: &axum::Router, template_id: &str, name: &str, count: u32) {
    let response = send_json(
        app,
        "POST",
        "/api/v1/attachments/templates",
        user_strip_template(template_id, name, count),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

fn set_active_layout_for_device(state: &Arc<AppState>, device_id: DeviceId) {
    let layout = SpatialLayout {
        id: "active-layout".to_owned(),
        name: "Active Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: "zone-main".to_owned(),
            name: "Desk Strip".to_owned(),
            device_id: device_id.to_string(),
            zone_name: Some("Main".to_owned()),

            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.4, 0.1),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 12,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    state.spatial_engine.update_layout(layout);
}

async fn register_recording_backend(
    state: &Arc<AppState>,
    writes: Arc<StdMutex<Vec<Vec<[u8; 3]>>>>,
) {
    let mut manager = state.backend_manager.lock().await;
    manager.register_backend(Arc::new(RecordingBackend::new(writes)));
}

#[tokio::test]
async fn attachment_template_collection_lists_builtin_metadata() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(state);

    let list_response = send_empty(
        &app,
        "GET",
        "/api/v1/attachments/templates?origin=built_in&q=generic-argb-fan-6-leds",
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(
        list_json["data"]["items"][0]["id"],
        "generic-argb-fan-6-leds"
    );
    assert_eq!(list_json["data"]["items"][0]["vendor"], "Generic");
}

#[tokio::test]
async fn user_template_create_persists_to_overridden_data_dir() {
    let guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(state);
    let template_id = "test-custom-strip";
    let template_path = guard.attachments_dir().join(format!("{template_id}.toml"));

    let create_response = send_json(
        &app,
        "POST",
        "/api/v1/attachments/templates",
        user_strip_template(template_id, "Test Custom Strip", 12),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    assert_eq!(create_json["data"]["origin"], "user");
    assert!(template_path.exists(), "template file should be persisted");

    let list_response = send_empty(
        &app,
        "GET",
        format!("/api/v1/attachments/templates?q={template_id}"),
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["items"][0]["id"], template_id);
    assert_eq!(list_json["data"]["items"][0]["name"], "Test Custom Strip");
}

#[tokio::test]
async fn attachment_template_item_and_facet_routes_are_absent() {
    let _guard = TestDataDirGuard::new().await;
    let app = test_app_with_state(Arc::new(AppState::new()));

    for (method, path) in [
        (
            "GET",
            "/api/v1/attachments/templates/generic-argb-fan-6-leds",
        ),
        (
            "PUT",
            "/api/v1/attachments/templates/generic-argb-fan-6-leds",
        ),
        (
            "DELETE",
            "/api/v1/attachments/templates/generic-argb-fan-6-leds",
        ),
        ("GET", "/api/v1/attachments/categories"),
        ("GET", "/api/v1/attachments/vendors"),
    ] {
        let response = send_empty(&app, method, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "end-to-end API flow covers validation, save, fetch, and delete together"
)]
async fn device_attachment_profile_flow_persists_and_clears() {
    let guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let template_id = "profile-test-strip";

    create_template(&app, template_id, "Profile Test Strip", 12).await;
    set_active_layout_for_device(&state, device_id);

    let update_body = json!({
        "bindings": [{
            "slot_id": "main",
            "template_id": template_id,
            "name": "Desk Edge",
            "instances": 2,
            "led_offset": 0
        }]
    });
    let logical_device_count = state.logical_devices.read().await.len();
    let layout_before = state.spatial_engine.snapshot().layout().clone();
    let mut events = state.event_bus.subscribe_all();
    let validation_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": update_body["bindings"].clone(),
            "validate_only": true
        }),
    )
    .await;
    assert_eq!(validation_response.status(), StatusCode::OK);
    let validation_json = body_json(validation_response).await;
    assert_eq!(
        validation_json["data"]["suggested_zones"]
            .as_array()
            .expect("suggested_zones should be an array")
            .len(),
        2
    );
    assert_eq!(
        validation_json["data"]["suggested_zones"][0]["led_start"],
        0
    );
    assert_eq!(
        validation_json["data"]["suggested_zones"][1]["led_start"],
        12
    );
    assert_eq!(
        validation_json["data"]["suggested_zones"][0]["led_count"],
        12
    );
    assert!(
        validation_json["data"]["suggested_zones"][0]["name"]
            .as_str()
            .expect("zone name should be a string")
            .contains("Desk Edge"),
        "zone name should include the binding name"
    );
    assert_eq!(validation_json["data"]["needs_layout_update"], true);
    assert!(!guard.attachment_profiles_path().exists());
    assert_eq!(
        state.logical_devices.read().await.len(),
        logical_device_count
    );
    assert_eq!(state.spatial_engine.snapshot().layout(), layout_before);
    assert!(state.usb_protocol_configs.config(device_id).await.is_none());
    assert!(events.try_recv().is_err());

    let unpersisted_response = send_empty(
        &app,
        "GET",
        format!("/api/v1/devices/{device_id}/attachments"),
    )
    .await;
    assert_eq!(unpersisted_response.status(), StatusCode::OK);
    let unpersisted_json = body_json(unpersisted_response).await;
    assert_eq!(unpersisted_json["data"]["bindings"], json!([]));

    let overlap_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "validate_only": true,
            "bindings": [
                {
                    "slot_id": "main",
                    "template_id": template_id,
                    "instances": 1,
                    "led_offset": 0
                },
                {
                    "slot_id": "main",
                    "template_id": template_id,
                    "instances": 1,
                    "led_offset": 6
                }
            ]
        }),
    )
    .await;
    assert_eq!(overlap_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let overlap_json = body_json(overlap_response).await;
    assert_eq!(overlap_json["error"]["code"], "validation_error");

    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        update_body,
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(
        update_json["data"]["bindings"][0]["effective_led_count"],
        24
    );
    assert_eq!(
        update_json["data"]["suggested_zones"]
            .as_array()
            .expect("suggested_zones should be an array")
            .len(),
        2
    );
    assert_eq!(
        update_json["data"]["suggested_zones"][0]["template_id"],
        template_id
    );
    assert_eq!(update_json["data"]["needs_layout_update"], true);
    assert!(
        guard.attachment_profiles_path().exists(),
        "attachment profile store should be written"
    );

    let get_response = send_empty(
        &app,
        "GET",
        format!("/api/v1/devices/{device_id}/attachments"),
    )
    .await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["slots"][0]["id"], "main");
    assert_eq!(get_json["data"]["bindings"][0]["template_id"], template_id);
    assert_eq!(
        get_json["data"]["suggested_zones"]
            .as_array()
            .expect("suggested_zones should be an array")
            .len(),
        2
    );
    assert!(
        get_json["data"]["suggested_zones"][0]["name"]
            .as_str()
            .expect("suggested zone name should be a string")
            .contains("Desk Edge"),
        "suggested zone name should preserve the binding name"
    );

    let delete_profile_response = send_empty(
        &app,
        "DELETE",
        format!("/api/v1/devices/{device_id}/attachments"),
    )
    .await;
    assert_eq!(delete_profile_response.status(), StatusCode::OK);
    let delete_profile_json = body_json(delete_profile_response).await;
    assert_eq!(delete_profile_json["data"]["deleted"], true);
}

#[tokio::test]
async fn multiple_same_slot_bindings_are_named_and_suggested_distinctly() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(state.clone());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let template_id = "stacked-strip";

    create_template(&app, template_id, "Stacked Strip", 12).await;

    let body = json!({
        "bindings": [
            {
                "slot_id": "main",
                "template_id": template_id,
                "instances": 1,
                "led_offset": 0
            },
            {
                "slot_id": "main",
                "template_id": template_id,
                "instances": 1,
                "led_offset": 12
            }
        ]
    });

    let validation_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": body["bindings"].clone(),
            "validate_only": true
        }),
    )
    .await;
    assert_eq!(validation_response.status(), StatusCode::OK);
    let validation_json = body_json(validation_response).await;
    let validation_zones = validation_json["data"]["suggested_zones"]
        .as_array()
        .expect("suggested_zones should be an array");
    assert_eq!(validation_zones.len(), 2);
    assert_eq!(validation_zones[0]["led_start"], 0);
    assert_eq!(validation_zones[1]["led_start"], 12);
    assert_eq!(validation_zones[0]["name"], "Stacked Strip 1");
    assert_eq!(validation_zones[1]["name"], "Stacked Strip 2");

    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        body,
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    let suggested_zones = update_json["data"]["suggested_zones"]
        .as_array()
        .expect("suggested_zones should be an array");
    assert_eq!(suggested_zones.len(), 2);
    assert_eq!(suggested_zones[0]["led_start"], 0);
    assert_eq!(suggested_zones[1]["led_start"], 12);
    assert_eq!(suggested_zones[0]["name"], "Stacked Strip 1");
    assert_eq!(suggested_zones[1]["name"], "Stacked Strip 2");
}

#[tokio::test]
async fn prism_8_channel_slots_accept_fan_templates() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(state.clone());
    let device_id = insert_prism_8_test_device(&state).await;

    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "channel-1",
                "template_id": "generic-argb-fan-16-leds",
                "instances": 1,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(
        update_json["data"]["bindings"][0]["template_id"],
        "generic-argb-fan-16-leds"
    );
}

#[tokio::test]
async fn prism_8_accepts_driver_scoped_templates() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_prism_8_test_device(&state).await;
    let template_id = "nollie-scoped-prism-fan";
    let mut template = user_strip_template(template_id, "Nollie Scoped Prism Fan", 16);
    template["category"] = json!("fan");
    template["compatible_slots"] = json!([{
        "controller_ids": ["nollie"],
        "models": ["prism_8"],
        "slots": ["channel-1"]
    }]);
    let create_response = send_json(&app, "POST", "/api/v1/attachments/templates", template).await;
    assert_eq!(create_response.status(), StatusCode::CREATED);

    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "channel-1",
                "template_id": template_id,
                "instances": 1,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn nollie32_channel_slots_accept_fan_profiles() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_nollie32_test_device(&state).await;

    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "channel-1",
                "template_id": "lian-li-sl-infinity-fan",
                "instances": 1,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(
        update_json["data"]["bindings"][0]["template_id"],
        "lian-li-sl-infinity-fan"
    );
    assert_eq!(update_json["data"]["suggested_zones"][0]["led_start"], 0);
    assert_eq!(update_json["data"]["suggested_zones"][0]["led_count"], 20);
}

#[tokio::test]
async fn nollie32_attachment_slots_support_cable_profiles() {
    let guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_nollie32_test_device(&state).await;

    let get_response = send_empty(
        &app,
        "GET",
        format!("/api/v1/devices/{device_id}/attachments"),
    )
    .await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    let slots = get_json["data"]["slots"]
        .as_array()
        .expect("slots should be an array");
    assert!(slots.iter().any(|slot| slot["id"] == "atx-strimer"));
    assert!(slots.iter().any(|slot| slot["id"] == "gpu-strimer"));

    let logical_device_count = state.logical_devices.read().await.len();
    let mut events = state.event_bus.subscribe_all();
    let gpu_only_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "validate_only": true,
            "bindings": [{
                "slot_id": "gpu-strimer",
                "template_id": "lian-li-gpu-strimer-4x27",
                "instances": 1,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(gpu_only_response.status(), StatusCode::OK);
    let gpu_only_json = body_json(gpu_only_response).await;
    assert_eq!(
        gpu_only_json["data"]["suggested_zones"][0]["led_start"],
        5_120
    );
    assert_eq!(
        gpu_only_json["data"]["suggested_zones"][0]["led_count"],
        108
    );
    assert!(state.usb_protocol_configs.config(device_id).await.is_none());
    assert_eq!(
        state.logical_devices.read().await.len(),
        logical_device_count
    );
    assert!(!guard.attachment_profiles_path().exists());
    assert!(events.try_recv().is_err());

    let full_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [
                {
                    "slot_id": "atx-strimer",
                    "template_id": "lian-li-atx-strimer",
                    "instances": 1,
                    "led_offset": 0
                },
                {
                    "slot_id": "gpu-strimer",
                    "template_id": "lian-li-gpu-strimer-6x27",
                    "instances": 1,
                    "led_offset": 0
                }
            ]
        }),
    )
    .await;
    assert_eq!(full_response.status(), StatusCode::OK);
    let full_json = body_json(full_response).await;
    assert_eq!(
        full_json["data"]["suggested_zones"][0]["template_id"],
        "lian-li-atx-strimer"
    );
    assert_eq!(
        full_json["data"]["suggested_zones"][1]["template_id"],
        "lian-li-gpu-strimer-6x27"
    );
    assert_eq!(full_json["data"]["suggested_zones"][0]["led_start"], 5_120);
    assert_eq!(full_json["data"]["suggested_zones"][1]["led_start"], 5_240);
    let config = state
        .usb_protocol_configs
        .config(device_id)
        .await
        .expect("saved attachment profile should update USB protocol config");
    assert_eq!(config.atx_attachment_leds(), 120);
    assert_eq!(config.gpu_attachment_leds(), 162);
    assert_eq!(config.build_protocol().total_leds(), 5_402);
    assert!(guard.attachment_profiles_path().exists());

    let delete_response = send_empty(
        &app,
        "DELETE",
        format!("/api/v1/devices/{device_id}/attachments"),
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::OK);
    assert!(state.usb_protocol_configs.config(device_id).await.is_none());
}

#[tokio::test]
async fn attachment_identify_indexes_multi_instance_rows_individually() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let writes = Arc::new(StdMutex::new(Vec::new()));
    register_recording_backend(&state, Arc::clone(&writes)).await;
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    create_template(&app, "identify-test-fan", "Identify Test Fan", 6).await;
    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "main",
                "template_id": "identify-test-fan",
                "instances": 3,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);

    let identify_response = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/main/identify"),
        json!({
            "binding_index": 1,
            "duration_ms": 2000,
            "color": "80FFEA"
        }),
    )
    .await;
    assert_eq!(identify_response.status(), StatusCode::OK);
    let identify_json = body_json(identify_response).await;
    assert_eq!(identify_json["data"]["binding_index"], 1);
    assert_eq!(identify_json["data"]["instance"], Value::Null);

    let recorded = writes
        .lock()
        .expect("recording backend mutex should not be poisoned");
    let frame = recorded
        .first()
        .expect("identify should issue an immediate on-frame");
    assert_eq!(frame.len(), 60);

    let lit_indices = frame
        .iter()
        .enumerate()
        .filter_map(|(index, color)| (*color != [0, 0, 0]).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(lit_indices, (6..12).collect::<Vec<_>>());

    let lit_color = frame[6];
    assert_ne!(lit_color, [0, 0, 0]);
    assert!(frame[6..12].iter().all(|color| *color == lit_color));
}

/// The identify route separates an addressed resource that is absent from
/// a supplied value the profile cannot satisfy.
///
/// An unknown slot is a missing sub-resource and answers `404`. A slot
/// that exists but holds no enabled bindings, and a binding index past
/// the end of the ones it does hold, are values the caller chose against
/// a profile that resolved, so both answer `422` and name the field.
#[tokio::test]
async fn attachment_identify_separates_missing_slots_from_unusable_selections() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let writes = Arc::new(StdMutex::new(Vec::new()));
    register_recording_backend(&state, Arc::clone(&writes)).await;
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    create_template(&app, "identify-bounds-fan", "Identify Bounds Fan", 6).await;
    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "main",
                "template_id": "identify-bounds-fan",
                "instances": 1,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);

    let missing_slot = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/no-such-slot/identify"),
        json!({ "binding_index": 0 }),
    )
    .await;
    assert_eq!(missing_slot.status(), StatusCode::NOT_FOUND);
    let missing_slot_json = body_json(missing_slot).await;
    assert_eq!(missing_slot_json["error"]["code"], "not_found");
    assert_eq!(
        missing_slot_json["error"]["message"],
        "attachment slot not found: no-such-slot"
    );

    let out_of_range = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/main/identify"),
        json!({ "binding_index": 42 }),
    )
    .await;
    assert_eq!(out_of_range.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let out_of_range_json = body_json(out_of_range).await;
    assert_eq!(out_of_range_json["error"]["code"], "validation_error");
    assert_eq!(
        out_of_range_json["error"]["details"]["field"],
        "binding_index"
    );

    // A slot that exists but holds only disabled bindings: the slot
    // lookup resolves, so this is the caller's selection failing, not a
    // missing resource.
    let disable_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "main",
                "template_id": "identify-bounds-fan",
                "instances": 1,
                "led_offset": 0,
                "enabled": false
            }]
        }),
    )
    .await;
    assert_eq!(disable_response.status(), StatusCode::OK);

    let no_enabled = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/main/identify"),
        json!({ "binding_index": 0 }),
    )
    .await;
    assert_eq!(no_enabled.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let no_enabled_json = body_json(no_enabled).await;
    assert_eq!(no_enabled_json["error"]["code"], "validation_error");
    assert_eq!(no_enabled_json["error"]["details"]["field"], "slot_id");
    assert_eq!(
        no_enabled_json["error"]["message"],
        "No enabled bindings in slot 'main'"
    );
}

#[tokio::test]
async fn device_list_embeds_attachments_only_when_requested() {
    let _guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_test_device(&state, "Desk Strip").await;

    create_template(&app, "embedded-list-strip", "Embedded List Strip", 12).await;
    let update = send_json(
        &app,
        "PUT",
        format!("/api/v1/devices/{device_id}/attachments"),
        json!({
            "bindings": [{
                "slot_id": "main",
                "template_id": "embedded-list-strip",
                "instances": 1,
                "led_offset": 0
            }]
        }),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);

    let default_response = send_empty(&app, "GET", "/api/v1/devices").await;
    assert_eq!(default_response.status(), StatusCode::OK);
    let default_json = body_json(default_response).await;
    assert!(
        default_json["data"]["items"][0]
            .get("attachments")
            .is_none()
    );

    let expanded_response = send_empty(&app, "GET", "/api/v1/devices?include=attachments").await;
    assert_eq!(expanded_response.status(), StatusCode::OK);
    let expanded_json = body_json(expanded_response).await;
    let device = &expanded_json["data"]["items"][0];
    assert_eq!(device["attachments"]["device_id"], device_id.to_string());
    assert_eq!(
        device["attachments"]["bindings"][0]["template_id"],
        "embedded-list-strip"
    );
}
