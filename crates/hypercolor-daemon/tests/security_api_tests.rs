//! Integration tests for daemon security middleware and CORS defaults.

use std::sync::{Arc, LazyLock, Mutex};
use std::{net::Ipv4Addr, net::SocketAddr};

use axum::body::Body;
use axum::extract::ConnectInfo;
use http::{Method, Request, StatusCode, header};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::config::HypercolorConfig;
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

fn test_app_with_config(config: HypercolorConfig) -> axum::Router {
    let mut state = isolated_state();
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    manager.update(config);
    state.config_manager = Some(manager);
    api::build_router(Arc::new(state), None)
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

#[tokio::test]
async fn configured_public_origin_is_ignored_without_api_auth() {
    let mut config = HypercolorConfig::default();
    config.web.cors_origins = vec!["https://studio.example".to_owned()];

    let response = test_app_with_config(config)
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
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
        (Method::POST, "/api/v1/capture/source/pick"),
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
        (Method::POST, "/api/v1/capture/source/pick"),
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
async fn protected_capture_routes_accept_trustworthy_loopback_clients() {
    let app = test_app();
    let monitors = app
        .clone()
        .oneshot(request_from(
            Ipv4Addr::LOCALHOST,
            Method::GET,
            "/api/v1/capture/monitors",
        ))
        .await
        .expect("local monitor request should complete");
    let authorization = app
        .oneshot(request_from(
            Ipv4Addr::LOCALHOST,
            Method::POST,
            "/api/v1/input/authorize",
        ))
        .await
        .expect("local authorization request should complete");

    assert_eq!(monitors.status(), StatusCode::OK);
    assert_ne!(authorization.status(), StatusCode::FORBIDDEN);
}
