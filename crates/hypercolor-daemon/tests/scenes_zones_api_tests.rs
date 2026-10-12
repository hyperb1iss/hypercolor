use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
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

    let response = send(&app, empty_request("GET", "/api/v1/system")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let capabilities = json["data"]["status"]["capabilities"]
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
        json_request(
            "POST",
            format!("/api/v1/scenes/{scene_id}/activate"),
            serde_json::json!({}),
        ),
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

#[tokio::test]
async fn scene_activation_accepts_an_empty_request_body() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/scenes",
            serde_json::json!({ "name": "Bare Activate" }),
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
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], scene_id);
}

#[tokio::test]
async fn scene_activation_applies_the_requested_transition_duration() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let mut scene_ids = Vec::new();

    for name in ["First Scene", "Second Scene"] {
        let response = send(
            &app,
            json_request(
                "POST",
                "/api/v1/scenes",
                serde_json::json!({ "name": name }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        scene_ids.push(
            body_json(response).await["data"]["id"]
                .as_str()
                .expect("scene id should be a string")
                .to_owned(),
        );
    }

    for (scene_id, transition_ms) in scene_ids.iter().zip([None, Some(250)]) {
        let response = send(
            &app,
            json_request(
                "POST",
                format!("/api/v1/scenes/{scene_id}/activate"),
                serde_json::json!({ "transition_ms": transition_ms }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let manager = state.scene_manager.snapshot().await;
    let transition = manager
        .transition_plan()
        .expect("the duration override should start a transition");
    assert_eq!(transition.spec.duration_ms, 250);
}

async fn create_named_scene(app: &axum::Router, name: &str) -> String {
    let response = send(
        app,
        json_request(
            "POST",
            "/api/v1/scenes",
            serde_json::json!({ "name": name }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await["data"]["id"]
        .as_str()
        .expect("scene id should be a string")
        .to_owned()
}

/// The raw `data` of `GET /api/v1/scenes/{id}`, exactly as served.
async fn read_scene(app: &axum::Router, scene_id: &str) -> serde_json::Value {
    let response = send(
        app,
        empty_request("GET", format!("/api/v1/scenes/{scene_id}")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["data"].clone()
}

fn put_scene(scene_id: &str, body: &serde_json::Value, if_match: Option<u64>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/scenes/{scene_id}"))
        .header("content-type", "application/json");
    if let Some(revision) = if_match {
        builder = builder.header(http::header::IF_MATCH, format!("\"{revision}\""));
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn revision_of(document: &serde_json::Value) -> u64 {
    document["revision"]
        .as_u64()
        .expect("scene documents carry a numeric revision")
}

#[tokio::test]
async fn a_scene_read_can_be_put_back_with_edits() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let scene_id = create_named_scene(&app, "Round Trip").await;

    let mut document = read_scene(&app, &scene_id).await;
    let read_revision = revision_of(&document);
    assert_eq!(document["is_default"], false);
    document["name"] = serde_json::json!("Round Trip, Edited");
    document["description"] = serde_json::json!("written from a read-back body");

    let response = send(&app, put_scene(&scene_id, &document, None)).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET -> PUT must round-trip"
    );
    let replaced = body_json(response).await;
    assert_eq!(replaced["data"]["name"], "Round Trip, Edited");
    assert_eq!(
        replaced["data"]["description"],
        "written from a read-back body"
    );
    assert!(revision_of(&replaced["data"]) > read_revision);

    let reread = read_scene(&app, &scene_id).await;
    assert_eq!(reread["name"], "Round Trip, Edited");
    assert_eq!(reread["zones"], document["zones"], "zones survive the trip");
}

#[tokio::test]
async fn a_stale_body_revision_is_a_precondition_failure() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let scene_id = create_named_scene(&app, "Fenced").await;

    let stale = read_scene(&app, &scene_id).await;
    let stale_revision = revision_of(&stale);
    let mut first = stale.clone();
    first["name"] = serde_json::json!("First Writer");
    let response = send(&app, put_scene(&scene_id, &first, None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let current_revision = revision_of(&body_json(response).await["data"]);

    let mut second = stale;
    second["name"] = serde_json::json!("Late Writer");
    let response = send(&app, put_scene(&scene_id, &second, None)).await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        response
            .headers()
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some(format!("\"{current_revision}\"").as_str())
    );
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "precondition_failed");
    assert_eq!(json["error"]["details"]["expected"], stale_revision);
    assert_eq!(json["error"]["details"]["current"], current_revision);
    assert_eq!(read_scene(&app, &scene_id).await["name"], "First Writer");
}

#[tokio::test]
async fn body_revision_and_if_match_must_both_be_current() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let scene_id = create_named_scene(&app, "Two Fences").await;

    let stale = read_scene(&app, &scene_id).await;
    let stale_revision = revision_of(&stale);
    let response = send(&app, put_scene(&scene_id, &stale, None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fresh = body_json(response).await["data"].clone();
    let fresh_revision = revision_of(&fresh);

    let response = send(&app, put_scene(&scene_id, &fresh, Some(stale_revision))).await;
    assert_eq!(
        response.status(),
        StatusCode::PRECONDITION_FAILED,
        "a stale If-Match fails even beside a current body revision"
    );
    let response = send(&app, put_scene(&scene_id, &stale, Some(fresh_revision))).await;
    assert_eq!(
        response.status(),
        StatusCode::PRECONDITION_FAILED,
        "a stale body revision fails even beside a current If-Match"
    );
    let response = send(&app, put_scene(&scene_id, &fresh, Some(fresh_revision))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn is_default_is_checked_against_the_stored_scene_not_written() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let scene_id = create_named_scene(&app, "Not The Default").await;

    let mut document = read_scene(&app, &scene_id).await;
    document["is_default"] = serde_json::json!(true);
    let response = send(&app, put_scene(&scene_id, &document, None)).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(json["error"]["details"]["field"], "is_default");
    assert_eq!(read_scene(&app, &scene_id).await["is_default"], false);
}
