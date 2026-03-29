//! Integration tests for daemon security middleware and CORS defaults.

use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode, header};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use tower::ServiceExt;

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

fn test_app() -> axum::Router {
    api::build_router(Arc::new(isolated_state()), None)
}

#[tokio::test]
async fn loopback_origin_receives_cors_headers() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .header(header::ORIGIN, "http://localhost:9430")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "http://localhost:9430"
    );
    assert!(response.headers().contains_key(header::VARY));
}

#[tokio::test]
async fn public_origin_does_not_receive_cors_headers() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}
