//! Integration tests for daemon security middleware and CORS defaults.

use std::convert::Infallible;
use std::sync::{Arc, LazyLock, Mutex};
use std::{net::Ipv4Addr, net::SocketAddr};

use axum::body::Body;
use axum::extract::ConnectInfo;
use http::{Method, Request, StatusCode, header};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_types::config::HypercolorConfig;
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone)]
struct TestApp {
    router: axum::Router,
    _data_dir: Arc<tempfile::TempDir>,
}

impl TestApp {
    async fn oneshot(self, request: Request<Body>) -> Result<http::Response<Body>, Infallible> {
        self.router.oneshot(request).await
    }
}

fn isolated_state() -> (AppState, tempfile::TempDir) {
    let (tempdir, builder) = isolated_state_builder();
    (builder.build(), tempdir)
}

fn isolated_state_builder() -> (tempfile::TempDir, AppStateBuilder) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (tempdir, AppStateBuilder::new(data_dir))
}

fn test_app() -> TestApp {
    let (state, tempdir) = isolated_state();
    TestApp {
        router: api::build_router(Arc::new(state), None),
        _data_dir: Arc::new(tempdir),
    }
}

fn test_app_with_config(config: HypercolorConfig) -> TestApp {
    let (tempdir, builder) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    manager.update(config);
    let state = builder.with_config_manager(manager).build();
    TestApp {
        router: api::build_router(Arc::new(state), None),
        _data_dir: Arc::new(tempdir),
    }
}

fn request_from(ip: Ipv4Addr, method: Method, path: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .expect("request should build");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((ip, 9420))));
    request
}

#[tokio::test]
async fn loopback_origin_receives_cors_headers() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
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
async fn exact_bundled_tauri_origins_receive_cors_headers() {
    for origin in [
        "tauri://localhost",
        "http://tauri.localhost",
        "https://tauri.localhost",
    ] {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system")
                    .header(header::ORIGIN, origin)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            origin
        );
    }

    for origin in ["tauri://attacker.example", "https://tauri.localhost.evil"] {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system")
                    .header(header::ORIGIN, origin)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }
}

#[tokio::test]
async fn public_origin_does_not_receive_cors_headers() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
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

#[tokio::test]
async fn configured_public_origin_is_ignored_without_api_auth() {
    let mut config = HypercolorConfig::default();
    config.web.cors_origins = vec!["https://studio.example".to_owned()];

    let response = test_app_with_config(config)
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .header(header::ORIGIN, "https://studio.example")
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

#[tokio::test]
async fn protected_capture_routes_reject_remote_clients_before_dispatch() {
    let app = test_app();
    for (method, path) in [
        (Method::POST, "/api/v1/input/authorize"),
        (Method::POST, "/api/v1/capture/authorize"),
        (Method::PUT, "/api/v1/capture/source"),
        (Method::GET, "/api/v1/capture/monitors"),
    ] {
        let mut request = request_from(Ipv4Addr::new(203, 0, 113, 9), method.clone(), path);
        request.headers_mut().insert(
            "x-forwarded-for",
            "127.0.0.1".parse().expect("header should parse"),
        );
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("protected request should complete");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");

        let mut proxied = request_from(Ipv4Addr::LOCALHOST, method, path);
        proxied.headers_mut().insert(
            "x-forwarded-for",
            "203.0.113.9".parse().expect("header should parse"),
        );
        let response = app
            .clone()
            .oneshot(proxied)
            .await
            .expect("proxied protected request should complete");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "proxied {path}");
    }
}

#[tokio::test]
async fn protected_capture_routes_reject_malformed_forwarded_clients() {
    let app = test_app();
    for (method, path) in [
        (Method::POST, "/api/v1/input/authorize"),
        (Method::POST, "/api/v1/capture/authorize"),
        (Method::PUT, "/api/v1/capture/source"),
        (Method::GET, "/api/v1/capture/monitors"),
    ] {
        let mut request = request_from(Ipv4Addr::LOCALHOST, method, path);
        request.headers_mut().insert(
            "x-forwarded-for",
            "not-an-ip".parse().expect("header should parse"),
        );
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("malformed forwarded request should complete");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn protected_capture_routes_reject_unauthenticated_loopback_clients() {
    let app = test_app();
    for (method, path) in [
        (Method::POST, "/api/v1/input/authorize"),
        (Method::POST, "/api/v1/capture/authorize"),
        (Method::PUT, "/api/v1/capture/source"),
        (Method::GET, "/api/v1/capture/monitors"),
    ] {
        let response = app
            .clone()
            .oneshot(request_from(Ipv4Addr::LOCALHOST, method, path))
            .await
            .expect("local protected request should complete");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn privacy_bearing_config_and_diagnose_reject_unauthenticated_loopback_clients() {
    let app = test_app();
    for (method, path, body) in [
        (Method::PUT, "/api/v1/config/keys/capture.enabled", "true"),
        (Method::POST, "/api/v1/config/reset", "{}"),
        (
            Method::POST,
            "/api/v1/diagnose",
            r#"{"checks":["macos_screen_parity"]}"#,
        ),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("request should build");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((Ipv4Addr::LOCALHOST, 9420))));

        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("local privacy-bearing request should complete");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn protected_capture_routes_accept_trusted_in_process_control() {
    let (state, _data_dir) = isolated_state();
    let api = api::local::TrustedLocalApi::new(Arc::new(state));
    for (method, path) in [
        (Method::POST, "/api/v1/input/authorize"),
        (Method::POST, "/api/v1/capture/authorize"),
        (Method::PUT, "/api/v1/capture/source"),
        (Method::GET, "/api/v1/capture/monitors"),
    ] {
        let response = api
            .execute(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .expect("trusted request should build"),
            )
            .await
            .expect("trusted protected request should complete");

        assert_ne!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}
