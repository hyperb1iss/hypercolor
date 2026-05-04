#![cfg(feature = "cloud")]

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_daemon::api::{self, AppState};
use tower::ServiceExt;

#[tokio::test]
async fn cloud_status_reports_compiled_config_without_keyring_access() {
    let app = api::build_router(Arc::new(AppState::new()), None);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/cloud/status")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("body should be json");
    let data = &body["data"];

    assert_eq!(data["compiled"], true);
    assert_eq!(data["enabled"], false);
    assert_eq!(data["connect_on_start"], true);
    assert_eq!(data["base_url"], "https://api.hypercolor.lighting");
    assert_eq!(data["auth_base_url"], "https://hypercolor.lighting");
    assert_eq!(data["app_base_url"], "https://app.hypercolor.lighting");
    assert_eq!(data["identity_storage"], "os_keyring");
}
