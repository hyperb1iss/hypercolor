use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state_with_tempdir() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (Arc::new(state), tempdir)
}

fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
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
        .expect("response body should read");
    serde_json::from_slice(&bytes).expect("response body should be JSON")
}

fn json_request(method: &str, uri: impl AsRef<str>, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri.as_ref())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn empty_request(method: &str, uri: impl AsRef<str>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri.as_ref())
        .body(Body::empty())
        .expect("request should build")
}

#[tokio::test]
async fn status_advertises_multi_zone_backend_capabilities() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(&app, empty_request("GET", "/api/v1/status")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let capabilities = json["data"]["capabilities"]
        .as_array()
        .expect("capabilities should be an array")
        .iter()
        .map(|value| value.as_str().expect("capability should be string"))
        .collect::<Vec<_>>();
    assert!(capabilities.contains(&"multi-zone-sampling"));
    assert!(capabilities.contains(&"zone-crud"));
    assert!(capabilities.contains(&"zone-device-assignment"));
    assert!(capabilities.contains(&"zone-layout-edit"));
    assert!(capabilities.contains(&"zone-preview-frames"));
    assert!(capabilities.contains(&"scene-unassigned-behavior-write"));
}

#[tokio::test]
async fn created_scenes_are_born_with_a_default_zone() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scenes",
            serde_json::json!({ "name": "Studio Scene" }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let scene_id = body_json(response).await["data"]["id"]
        .as_str()
        .expect("scene id should be a string")
        .to_owned();

    let response = send(
        &app,
        empty_request("POST", format!("/api/v1/scenes/{scene_id}/activate")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = send(&app, empty_request("GET", "/api/v1/scene")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let zones = json["data"]["zones"].as_array().expect("zones array");
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0]["role"], "primary");
}
