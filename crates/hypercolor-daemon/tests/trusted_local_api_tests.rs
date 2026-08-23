use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ws::Message;
use axum::http::{Request, StatusCode};
use hypercolor_daemon::api::local::TrustedLocalApi;
use hypercolor_daemon::app_state::AppState;
use tempfile::TempDir;

fn isolated_api() -> (TrustedLocalApi, TempDir) {
    let tempdir = TempDir::new().expect("trusted local state directory should be created");
    let state = AppState::new_with_data_dir(tempdir.path().to_path_buf());
    (TrustedLocalApi::new(Arc::new(state)), tempdir)
}

#[tokio::test]
async fn public_trusted_local_http_surface_executes_the_daemon_router() {
    let (api, _tempdir) = isolated_api();
    let response = api
        .execute(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("trusted local status request should build"),
        )
        .await
        .expect("trusted local status path should execute");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("trusted local status body should be bounded");
    let body: serde_json::Value =
        serde_json::from_slice(&body).expect("trusted local status should return JSON");
    assert_eq!(body["data"]["status"]["running"], true);
}

#[tokio::test]
async fn public_trusted_local_websocket_surface_joins_its_session() {
    let (api, _tempdir) = isolated_api();
    let mut socket = api
        .open_websocket("/api/v1/ws")
        .expect("canonical trusted local websocket path should open");

    let hello = socket
        .recv()
        .await
        .expect("trusted local websocket should emit hello");
    let Message::Text(hello) = hello else {
        panic!("trusted local websocket hello should be text");
    };
    let hello: serde_json::Value =
        serde_json::from_str(hello.as_str()).expect("trusted local hello should be JSON");
    assert_eq!(hello["type"], "hello");

    socket.shutdown().await;
}
