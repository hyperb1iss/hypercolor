//! Integration tests for attachment template and profile endpoints.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};

use anyhow::Result;
use axum::body::Body;
use http::{Request, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tower::ServiceExt;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{BackendInfo, DeviceBackend};
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn write_colors(&mut self, _id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
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

async fn insert_prism_8_test_device(state: &Arc<AppState>) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: "PrismRGB Prism 8".to_owned(),
        vendor: "PrismRGB".to_owned(),
        family: DeviceFamily::PrismRgb,
        model: Some("prism_8".to_owned()),
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "Channel 1".to_owned(),
            led_count: 126,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Grb,
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

async fn set_active_layout_for_device(state: &Arc<AppState>, device_id: DeviceId) {
    let layout = SpatialLayout {
        id: "active-layout".to_owned(),
        name: "Active Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: "zone-main".to_owned(),
            name: "Desk Strip".to_owned(),
            device_id: format!("device:{device_id}"),
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

    let mut spatial = state.spatial_engine.write().await;
    spatial.update_layout(layout);
}

async fn register_recording_backend(
    state: &Arc<AppState>,
    writes: Arc<StdMutex<Vec<Vec<[u8; 3]>>>>,
) {
    let mut manager = state.backend_manager.lock().await;
    manager.register_backend(Box::new(RecordingBackend::new(writes)));
}

#[tokio::test]
async fn attachment_catalog_and_metadata_endpoints_work() {
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

    let detail_response = send_empty(
        &app,
        "GET",
        "/api/v1/attachments/templates/generic-argb-fan-6-leds",
    )
    .await;
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail_json = body_json(detail_response).await;
    assert_eq!(detail_json["data"]["origin"], "built_in");
    assert_eq!(
        detail_json["data"]["led_positions"]
            .as_array()
            .expect("led_positions should be an array")
            .len(),
        6
    );

    let categories_response = send_empty(&app, "GET", "/api/v1/attachments/categories").await;
    assert_eq!(categories_response.status(), StatusCode::OK);
    let categories_json = body_json(categories_response).await;
    assert!(
        categories_json["data"]["items"]
            .as_array()
            .expect("items should be an array")
            .iter()
            .any(|item| item["category"] == "fan"),
        "expected at least one fan category entry"
    );

    let vendors_response = send_empty(&app, "GET", "/api/v1/attachments/vendors").await;
    assert_eq!(vendors_response.status(), StatusCode::OK);
    let vendors_json = body_json(vendors_response).await;
    assert!(
        vendors_json["data"]["items"]
            .as_array()
            .expect("items should be an array")
            .iter()
            .any(|item| item["vendor"] == "Generic"),
        "expected Generic vendor to be present"
    );
}

#[tokio::test]
async fn user_template_crud_persists_to_overridden_data_dir() {
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

    let update_response = send_json(
        &app,
        "PUT",
        format!("/api/v1/attachments/templates/{template_id}"),
        json!({
            "id": template_id,
            "name": "Test Custom Strip Updated",
            "vendor": "Test Vendor",
            "category": "strip",
            "description": "Updated strip template",
            "default_size": {
                "width": 0.5,
                "height": 0.1
            },
            "topology": {
                "type": "strip",
                "count": 12,
                "direction": "left_to_right"
            },
            "compatible_slots": [],
            "tags": ["updated", "strip"]
        }),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);

    let get_response = send_empty(
        &app,
        "GET",
        format!("/api/v1/attachments/templates/{template_id}"),
    )
    .await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["name"], "Test Custom Strip Updated");
    assert_eq!(get_json["data"]["tags"], json!(["updated", "strip"]));

    let delete_response = send_empty(
        &app,
        "DELETE",
        format!("/api/v1/attachments/templates/{template_id}"),
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::OK);
    let delete_json = body_json(delete_response).await;
    assert_eq!(delete_json["data"]["deleted"], true);
    assert!(
        !template_path.exists(),
        "template file should be removed after delete"
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "end-to-end API flow covers preview, save, conflict, and delete behavior together"
)]
async fn device_attachment_profile_flow_persists_and_blocks_in_use_template_deletes() {
    let guard = TestDataDirGuard::new().await;
    let state = Arc::new(AppState::new());
    let app = test_app_with_state(Arc::clone(&state));
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let template_id = "profile-test-strip";

    create_template(&app, template_id, "Profile Test Strip", 12).await;
    set_active_layout_for_device(&state, device_id).await;

    let preview_body = json!({
        "bindings": [{
            "slot_id": "main",
            "template_id": template_id,
            "name": "Desk Edge",
            "instances": 2,
            "led_offset": 0
        }]
    });
    let preview_response = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/preview"),
        preview_body.clone(),
    )
    .await;
    assert_eq!(preview_response.status(), StatusCode::OK);
    let preview_json = body_json(preview_response).await;
    assert_eq!(
        preview_json["data"]["zones"]
            .as_array()
            .expect("zones should be an array")
            .len(),
        2
    );
    assert_eq!(preview_json["data"]["zones"][0]["led_start"], 0);
    assert_eq!(preview_json["data"]["zones"][1]["led_start"], 12);
    assert_eq!(preview_json["data"]["zones"][0]["led_count"], 12);
    assert!(
        preview_json["data"]["zones"][0]["name"]
            .as_str()
            .expect("zone name should be a string")
            .contains("Desk Edge"),
        "zone name should include the binding name"
    );

    let overlap_response = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/preview"),
        json!({
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
        preview_body,
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

    let delete_template_while_bound = send_empty(
        &app,
        "DELETE",
        format!("/api/v1/attachments/templates/{template_id}"),
    )
    .await;
    assert_eq!(delete_template_while_bound.status(), StatusCode::CONFLICT);
    let conflict_json = body_json(delete_template_while_bound).await;
    assert_eq!(conflict_json["error"]["code"], "conflict");

    let delete_profile_response = send_empty(
        &app,
        "DELETE",
        format!("/api/v1/devices/{device_id}/attachments"),
    )
    .await;
    assert_eq!(delete_profile_response.status(), StatusCode::OK);
    let delete_profile_json = body_json(delete_profile_response).await;
    assert_eq!(delete_profile_json["data"]["deleted"], true);

    let delete_template_response = send_empty(
        &app,
        "DELETE",
        format!("/api/v1/attachments/templates/{template_id}"),
    )
    .await;
    assert_eq!(delete_template_response.status(), StatusCode::OK);
    let delete_template_json = body_json(delete_template_response).await;
    assert_eq!(delete_template_json["data"]["deleted"], true);
    assert!(
        !guard
            .attachments_dir()
            .join(format!("{template_id}.toml"))
            .exists(),
        "template should be removed after the profile is cleared"
    );
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

    let preview_response = send_json(
        &app,
        "POST",
        format!("/api/v1/devices/{device_id}/attachments/preview"),
        body.clone(),
    )
    .await;
    assert_eq!(preview_response.status(), StatusCode::OK);
    let preview_json = body_json(preview_response).await;
    let preview_zones = preview_json["data"]["zones"]
        .as_array()
        .expect("zones should be an array");
    assert_eq!(preview_zones.len(), 2);
    assert_eq!(preview_zones[0]["led_start"], 0);
    assert_eq!(preview_zones[1]["led_start"], 12);
    assert_eq!(preview_zones[0]["name"], "Stacked Strip 1");
    assert_eq!(preview_zones[1]["name"], "Stacked Strip 2");

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
