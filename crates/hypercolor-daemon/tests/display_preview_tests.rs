use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use axum::body::Body;
use http::{Method, Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::display_frames::DisplayFrameSnapshot;
use hypercolor_daemon::simulators::{SimulatedDisplayConfig, activate_simulated_displays};
use hypercolor_types::device::DeviceId;
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = Arc::new(AppState::new());
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

fn simulator_config() -> SimulatedDisplayConfig {
    SimulatedDisplayConfig {
        id: DeviceId::from_uuid(Uuid::now_v7()),
        name: "Preview Test Display".to_owned(),
        width: 240,
        height: 160,
        circular: false,
        enabled: true,
    }
}

async fn register_display(state: &Arc<AppState>) -> DeviceId {
    let config = simulator_config().normalized();
    state
        .simulated_displays
        .write()
        .await
        .upsert(config.clone());
    activate_simulated_displays(
        &state.driver_host.discovery_runtime(),
        &state.simulated_displays,
    )
    .await
    .expect("simulated display should activate");
    config.id
}

async fn publish_frame(
    state: &Arc<AppState>,
    device_id: DeviceId,
    jpeg: Vec<u8>,
    frame_number: u64,
    captured_at: SystemTime,
) {
    state.display_frames.write().await.set_frame(
        device_id,
        DisplayFrameSnapshot {
            jpeg_data: Arc::new(jpeg),
            width: 240,
            height: 160,
            circular: false,
            frame_number,
            captured_at,
        },
    );
}

async fn send(app: axum::Router, request: Request<Body>) -> axum::response::Response {
    app.oneshot(request).await.expect("request should succeed")
}

async fn body_bytes(response: axum::response::Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body")
}

fn preview_request(device_id: DeviceId) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/displays/{device_id}/preview.jpg"))
        .body(Body::empty())
        .expect("request should build")
}

#[tokio::test]
async fn display_preview_returns_captured_frame_with_cache_headers() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state).await;
    let jpeg_bytes: Vec<u8> = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
    let captured_at = SystemTime::now();
    publish_frame(&state, device_id, jpeg_bytes.clone(), 42, captured_at).await;

    let app = api::build_router(Arc::clone(&state), None);
    let response = send(app.clone(), preview_request(device_id)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers
            .get(http::header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap_or("").to_owned()),
        Some("image/jpeg".to_owned())
    );
    let etag = headers
        .get(http::header::ETAG)
        .expect("etag header should be present")
        .to_str()
        .expect("etag should be valid utf-8")
        .to_owned();
    assert!(etag.contains("42"));
    assert!(headers.contains_key(http::header::LAST_MODIFIED));
    assert_eq!(
        headers.get("X-Display-Width").and_then(|v| v.to_str().ok()),
        Some("240")
    );
    assert_eq!(
        headers
            .get("X-Display-Height")
            .and_then(|v| v.to_str().ok()),
        Some("160")
    );
    assert_eq!(
        headers
            .get("X-Display-Circular")
            .and_then(|v| v.to_str().ok()),
        Some("0")
    );
    assert_eq!(
        headers
            .get("X-Display-Frame-Number")
            .and_then(|v| v.to_str().ok()),
        Some("42")
    );

    let body = body_bytes(response).await;
    assert_eq!(body.as_ref(), jpeg_bytes.as_slice());
}

#[tokio::test]
async fn display_preview_honors_if_none_match_with_304() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state).await;
    let jpeg_bytes: Vec<u8> = vec![1, 2, 3, 4];
    publish_frame(&state, device_id, jpeg_bytes, 7, SystemTime::now()).await;

    let app = api::build_router(Arc::clone(&state), None);
    let first = send(app.clone(), preview_request(device_id)).await;
    let etag = first
        .headers()
        .get(http::header::ETAG)
        .expect("first response should carry an etag")
        .to_str()
        .expect("etag should be valid utf-8")
        .to_owned();
    let _ = body_bytes(first).await;

    let conditional = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/displays/{device_id}/preview.jpg"))
        .header(http::header::IF_NONE_MATCH, etag.clone())
        .body(Body::empty())
        .expect("conditional request should build");
    let second = send(app, conditional).await;
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        second
            .headers()
            .get(http::header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some(etag.as_str())
    );
    let empty_body = body_bytes(second).await;
    assert!(empty_body.is_empty());
}

#[tokio::test]
async fn display_preview_serves_fresh_body_when_frame_advances() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state).await;
    let original = vec![9u8; 16];
    let refreshed = vec![7u8; 32];
    let baseline = SystemTime::now();
    publish_frame(&state, device_id, original, 1, baseline).await;

    let app = api::build_router(Arc::clone(&state), None);
    let first = send(app.clone(), preview_request(device_id)).await;
    let stale_etag = first
        .headers()
        .get(http::header::ETAG)
        .expect("response should carry an etag")
        .to_str()
        .expect("etag should be valid utf-8")
        .to_owned();
    let _ = body_bytes(first).await;

    publish_frame(
        &state,
        device_id,
        refreshed.clone(),
        2,
        baseline + Duration::from_secs(1),
    )
    .await;

    let conditional = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/displays/{device_id}/preview.jpg"))
        .header(http::header::IF_NONE_MATCH, stale_etag)
        .body(Body::empty())
        .expect("conditional request should build");
    let second = send(app, conditional).await;
    assert_eq!(second.status(), StatusCode::OK);
    let body = body_bytes(second).await;
    assert_eq!(body.as_ref(), refreshed.as_slice());
}

#[tokio::test]
async fn display_preview_returns_404_when_no_frame_captured() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state).await;

    let app = api::build_router(Arc::clone(&state), None);
    let response = send(app, preview_request(device_id)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn display_preview_returns_404_for_unknown_device() {
    let (state, _tempdir) = isolated_state();
    let unknown_id = DeviceId::from_uuid(Uuid::now_v7());

    let app = api::build_router(Arc::clone(&state), None);
    let response = send(app, preview_request(unknown_id)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn overlay_runtime_batch_endpoint_matches_per_slot_status() {
    use hypercolor_types::overlay::{
        Anchor, ClockConfig, ClockStyle, DisplayOverlayConfig, HourFormat, OverlayBlendMode,
        OverlayPosition, OverlaySlot, OverlaySlotId, OverlaySource,
    };

    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state).await;

    let slot_one = OverlaySlot {
        id: OverlaySlotId::generate(),
        name: "Clock".into(),
        source: OverlaySource::Clock(ClockConfig {
            style: ClockStyle::Digital,
            hour_format: HourFormat::TwentyFour,
            show_seconds: true,
            show_date: false,
            date_format: None,
            font_family: None,
            color: "#ffffff".into(),
            secondary_color: None,
            template: None,
        }),
        position: OverlayPosition::Anchored {
            anchor: Anchor::Center,
            offset_x: 0,
            offset_y: 0,
            width: 160,
            height: 60,
        },
        blend_mode: OverlayBlendMode::Normal,
        opacity: 1.0,
        enabled: true,
    };
    let slot_two = OverlaySlot {
        id: OverlaySlotId::generate(),
        enabled: false,
        ..slot_one.clone()
    };

    state
        .display_overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![slot_one.clone(), slot_two.clone()],
            },
        )
        .await;

    let app = api::build_router(Arc::clone(&state), None);
    let response = send(
        app,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/api/v1/displays/{device_id}/overlays/runtime"))
            .body(Body::empty())
            .expect("runtime request should build"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
    let entries = envelope["data"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2);
    let by_id: std::collections::HashMap<String, String> = entries
        .iter()
        .map(|entry| {
            (
                entry["slot_id"].as_str().unwrap().to_owned(),
                entry["runtime"]["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(by_id.get(&slot_one.id.to_string()).map(String::as_str), Some("active"));
    assert_eq!(by_id.get(&slot_two.id.to_string()).map(String::as_str), Some("disabled"));
}

// RFC 7232 §6: when both `If-None-Match` and `If-Modified-Since` are sent
// and the etag does not match, the server must NOT fall through to the
// date check. This catches the case where frames advance multiple times
// within the same HTTP-date second (round-tripped to seconds by httpdate).
#[tokio::test]
async fn display_preview_if_none_match_beats_if_modified_since() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state).await;
    let baseline = SystemTime::now();
    publish_frame(&state, device_id, vec![1, 2, 3], 1, baseline).await;

    let app = api::build_router(Arc::clone(&state), None);
    let first = send(app.clone(), preview_request(device_id)).await;
    let last_modified = first
        .headers()
        .get(http::header::LAST_MODIFIED)
        .expect("first response should carry Last-Modified")
        .to_str()
        .expect("last-modified should be ASCII")
        .to_owned();
    let _ = body_bytes(first).await;

    // New frame in the same second — same Last-Modified string but a
    // brand-new etag. A client that relies on the etag must not get a 304.
    publish_frame(&state, device_id, vec![9, 9, 9, 9], 2, baseline).await;

    let conditional = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/displays/{device_id}/preview.jpg"))
        .header(http::header::IF_NONE_MATCH, "\"stale-etag\"")
        .header(http::header::IF_MODIFIED_SINCE, last_modified)
        .body(Body::empty())
        .expect("conditional request should build");
    let response = send(app, conditional).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_bytes(response).await;
    assert_eq!(body.as_ref(), &[9, 9, 9, 9]);
}
