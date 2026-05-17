use std::io::Cursor;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::event::{AssetChangeKind, HypercolorEvent};
use image::{ImageBuffer, ImageFormat, Rgba};
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

fn png_bytes(color: [u8; 4]) -> Vec<u8> {
    let image = ImageBuffer::from_pixel(2, 2, Rgba(color));
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode test png");
    bytes.into_inner()
}

fn multipart_upload_request(
    uri: &str,
    file_name: &str,
    file_bytes: &[u8],
    fields: &[(&str, &str)],
) -> Request<Body> {
    let boundary = "hypercolor-test-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request should build")
}

fn json_request(method: &str, uri: String, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn empty_request(method: &str, uri: String) -> Request<Body> {
    Request::builder()
        .method(method)
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

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body")
        .to_vec()
}

#[tokio::test]
async fn asset_upload_list_metadata_blob_and_thumbnail_work() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let mut events = state.event_bus.subscribe_all();
    let bytes = png_bytes([255, 0, 160, 255]);

    let upload = send(
        &app,
        multipart_upload_request(
            "/api/v1/assets",
            "ignored.png",
            &bytes,
            &[("name", "panel.png"), ("tags", "[\"panel\",\"pink\"]")],
        ),
    )
    .await;
    assert_eq!(upload.status(), StatusCode::CREATED);
    let upload_json = body_json(upload).await;
    let asset_id = upload_json["data"]["id"]
        .as_str()
        .expect("asset id should serialize as string")
        .to_owned();
    assert_eq!(upload_json["data"]["name"], "panel.png");
    assert_eq!(upload_json["data"]["mime_type"], "image/png");
    assert_eq!(upload_json["data"]["duplicate"], false);
    assert_eq!(upload_json["data"]["tags"][0], "panel");
    assert!(matches!(
        events.recv().await.expect("asset added event").event,
        HypercolorEvent::AssetChanged {
            kind: AssetChangeKind::Added,
            ..
        }
    ));

    let list = send(&app, empty_request("GET", "/api/v1/assets".to_owned())).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_json = body_json(list).await;
    assert_eq!(list_json["data"]["total"], 1);

    let get = send(
        &app,
        empty_request("GET", format!("/api/v1/assets/{asset_id}")),
    )
    .await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_json = body_json(get).await;
    assert_eq!(get_json["data"]["id"], asset_id);

    let blob = send(
        &app,
        empty_request("GET", format!("/api/v1/assets/{asset_id}/blob")),
    )
    .await;
    assert_eq!(blob.status(), StatusCode::OK);
    assert_eq!(
        blob.headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(body_bytes(blob).await, bytes);

    let thumbnail = send(
        &app,
        empty_request("GET", format!("/api/v1/assets/{asset_id}/thumbnail")),
    )
    .await;
    assert_eq!(thumbnail.status(), StatusCode::OK);
    assert_eq!(
        thumbnail
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/webp")
    );
    assert!(!body_bytes(thumbnail).await.is_empty());
}

#[tokio::test]
async fn duplicate_upload_update_and_delete_publish_asset_events() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let app = test_app_with_state(Arc::clone(&state));
    let bytes = png_bytes([0, 200, 255, 255]);

    let first = send(
        &app,
        multipart_upload_request("/api/v1/assets", "first.png", &bytes, &[]),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_json = body_json(first).await;
    let asset_id = first_json["data"]["id"].as_str().expect("asset id");
    let asset_id = asset_id.to_owned();

    let duplicate = send(
        &app,
        multipart_upload_request("/api/v1/assets", "second.png", &bytes, &[]),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    let duplicate_json = body_json(duplicate).await;
    assert_eq!(duplicate_json["data"]["id"], asset_id);
    assert_eq!(duplicate_json["data"]["name"], "first.png");
    assert_eq!(duplicate_json["data"]["duplicate"], true);

    let mut events = state.event_bus.subscribe_all();
    let update = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/assets/{asset_id}"),
            serde_json::json!({ "name": "renamed.png", "tags": ["logo"] }),
        ),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let update_json = body_json(update).await;
    assert_eq!(update_json["data"]["name"], "renamed.png");
    assert!(matches!(
        events.recv().await.expect("asset modified event").event,
        HypercolorEvent::AssetChanged {
            kind: AssetChangeKind::Modified,
            ..
        }
    ));

    let delete = send(
        &app,
        empty_request("DELETE", format!("/api/v1/assets/{asset_id}")),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::OK);
    assert!(matches!(
        events.recv().await.expect("asset removed event").event,
        HypercolorEvent::AssetChanged {
            kind: AssetChangeKind::Removed,
            ..
        }
    ));

    let missing = send(
        &app,
        empty_request("GET", format!("/api/v1/assets/{asset_id}")),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unsupported_upload_returns_415() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let app = test_app_with_state(state);

    let response = send(
        &app,
        multipart_upload_request("/api/v1/assets", "notes.txt", b"not media", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "unsupported_media_type");
}
