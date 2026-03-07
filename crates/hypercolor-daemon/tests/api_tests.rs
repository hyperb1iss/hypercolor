//! Integration tests for the Hypercolor REST API.
//!
//! Tests use `axum::Router` directly with tower's `ServiceExt` and
//! `Request::builder()` — no TCP server needed.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};

// ── Test Helpers ─────────────────────────────────────────────────────────

/// Build a test router with fresh state.
fn test_app() -> axum::Router {
    let state = Arc::new(AppState::new());
    api::build_router(state, None)
}

/// Build a test router with shared state (for multi-step tests).
fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
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
    assert!(json["version"].is_string());
    assert!(json["checks"]["render_loop"].is_string());
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
            max_fps: 60,
        },
    };
    let _ = state.device_registry.add(info).await;
    id
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
    let state = Arc::new(AppState::new());
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: "Matrix Panel".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::OpenRgb,
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
            max_fps: 60,
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
                    r#"{"backends": ["openrgb"], "timeout_ms": 100, "wait": true}"#,
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());

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
    let state = Arc::new(AppState::new());

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
    let state = Arc::new(AppState::new());

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
async fn layout_delete_active_returns_conflict() {
    let state = Arc::new(AppState::new());
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

    let _ = app
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

    let delete_response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::CONFLICT);
    let delete_json = body_json(delete_response).await;
    assert_eq!(delete_json["error"]["code"], "conflict");
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
    let mut state = AppState::new();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.effect_layout_links_path = dir.path().join("effect-layouts.json");
    (Arc::new(state), dir)
}

fn test_state_with_temp_layout_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = AppState::new();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.layouts_path = dir.path().join("layouts.json");
    (Arc::new(state), dir)
}

fn test_state_with_temp_layout_and_runtime_store() -> (Arc<AppState>, tempfile::TempDir) {
    let mut state = AppState::new();
    let dir = tempfile::tempdir().expect("tempdir should be created");
    state.layouts_path = dir.path().join("layouts.json");
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
async fn update_device_persists_name_and_enabled_state() {
    let state = Arc::new(AppState::new());
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
                    r#"{"name":"Desk Strip Renamed","enabled":false}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["name"], "Desk Strip Renamed");
    assert_eq!(update_json["data"]["status"], "disabled");

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
}

#[tokio::test]
async fn list_devices_supports_filters() {
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
async fn get_device_by_ambiguous_name_returns_conflict() {
    let state = Arc::new(AppState::new());
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
    let state = Arc::new(AppState::new());
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
    let mut state = AppState::new();
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
