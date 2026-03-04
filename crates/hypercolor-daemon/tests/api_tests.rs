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
    assert_eq!(apply_json["data"]["applied_controls"]["speed"]["float"], 7.5);

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
    assert_eq!(playlist_json["data"]["items"].as_array().map_or(0, Vec::len), 2);
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
