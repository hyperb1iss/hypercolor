#![cfg(feature = "cloud")]

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::StatusCode as AxumStatusCode;
use axum::routing::post;
use futures_util::{SinkExt, StreamExt};
use http::{Request, StatusCode};
use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, CloudClientError, CloudSecretKey, DeviceAuthorizationSession,
    RefreshTokenOwner, SecretStore, api as cloud_api,
    daemon_link::{
        HEADER_AUTHORIZATION, HEADER_WEBSOCKET_PROTOCOL, IdentityNonce, UpgradeNonce,
        WEBSOCKET_PROTOCOL, WelcomeFrame,
    },
    load_or_create_identity, load_refresh_token, store_refresh_token,
};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::{
    api::{self, AppState, cloud},
    cloud_connection::{
        CloudConnectionPrepareInput, CloudConnectionRuntime, CloudConnectionRuntimeState,
        CloudConnectionSnapshot,
    },
    cloud_entitlements,
    cloud_socket::CloudSocketStartError,
};
use hypercolor_types::config::HypercolorConfig;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse as WsErrorResponse, Request as WsRequest, Response as WsResponse,
};
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

#[tokio::test]
async fn cloud_login_start_stores_session_without_returning_device_code() {
    let (auth_base_url, shutdown_tx, task) = spawn_auth_server().await;
    let app = api::build_router(cloud_test_state(&auth_base_url), None);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cloud/login/start")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response_json(response).await;
    let data = &body["data"];

    assert!(
        data["login_id"]
            .as_str()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
    );
    assert_eq!(data["user_code"], "HC-1234");
    assert_eq!(data["interval"], 1);
    assert_eq!(data["retry_after_ms"], 1000);
    assert_eq!(data["device_code"], serde_json::Value::Null);

    shutdown_auth_server(shutdown_tx, task).await;
}

#[tokio::test]
async fn cloud_login_poll_keeps_pending_session_retryable() {
    let (auth_base_url, shutdown_tx, task) = spawn_auth_server().await;
    let app = api::build_router(cloud_test_state(&auth_base_url), None);

    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cloud/login/start")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("start request should succeed");
    let start = response_json(start).await;
    let login_id = start["data"]["login_id"]
        .as_str()
        .expect("login id should be a string");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/cloud/login/{login_id}/poll"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("poll request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let data = &body["data"];

    assert_eq!(data["login_id"], login_id);
    assert_eq!(data["status"], "pending");
    assert_eq!(data["retry_after_ms"], 1000);
    assert_eq!(data["refresh_token_stored"], false);
    assert_eq!(data["device_registered"], false);
    assert_eq!(data["error"]["code"], "authorization_pending");

    shutdown_auth_server(shutdown_tx, task).await;
}

#[tokio::test]
async fn cloud_login_poll_rejects_unknown_session() {
    let app = api::build_router(Arc::new(AppState::new()), None);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cloud/login/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1/poll")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cloud_login_prunes_expired_pending_sessions() {
    let state = AppState::new();
    let expired_id = uuid::Uuid::new_v4();
    let live_id = uuid::Uuid::new_v4();
    state.cloud_login_sessions.lock().await.insert(
        expired_id,
        DeviceAuthorizationSession::new(device_code_fixture(0)),
    );
    state.cloud_login_sessions.lock().await.insert(
        live_id,
        DeviceAuthorizationSession::new(device_code_fixture(900)),
    );

    assert_eq!(cloud::prune_expired_login_sessions(&state).await, 1);
    let sessions = state.cloud_login_sessions.lock().await;
    assert!(!sessions.contains_key(&expired_id));
    assert!(sessions.contains_key(&live_id));
}

#[tokio::test]
async fn cloud_entitlement_cache_round_trips_and_deletes_token() {
    let tempdir = TempDir::new().expect("temp cache dir should be created");
    let path = tempdir.path().join("entitlement.json");
    let token = entitlement_token_fixture();

    let cached = cloud_entitlements::store_entitlement_response(&path, &token)
        .await
        .expect("entitlement should cache");
    let loaded = cloud_entitlements::load_cached_entitlement(&path)
        .await
        .expect("entitlement should load")
        .expect("entitlement cache should exist");

    assert_eq!(cached.jwt, "header.payload.signature");
    assert_eq!(loaded.claims.tier, "free");
    assert!(!loaded.is_stale_at_unix(1_999_999_999));
    assert!(
        cloud_entitlements::delete_cached_entitlement(&path)
            .await
            .expect("entitlement should delete")
    );
    assert!(
        !cloud_entitlements::delete_cached_entitlement(&path)
            .await
            .expect("missing entitlement should not fail")
    );
}

#[tokio::test]
async fn cloud_entitlement_status_summarizes_cache_without_jwt() {
    let (_tempdir, _guard) = set_temp_data_dir();
    cloud_entitlements::store_entitlement_response(
        cloud_entitlements::entitlement_cache_path(),
        &entitlement_token_fixture(),
    )
    .await
    .expect("entitlement should cache");
    let app = api::build_router(Arc::new(AppState::new()), None);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/cloud/entitlement")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let data = &body["data"];
    assert_eq!(data["cached"], true);
    assert_eq!(data["jwt_present"], true);
    assert_eq!(data["stale"], false);
    assert_eq!(data["jwt"], serde_json::Value::Null);
    assert_eq!(data["tier"], "free");
    assert_eq!(data["features"][0], "hc.cloud_sync");
    assert_eq!(data["channels"][0], "stable");
}

#[test]
fn cloud_session_status_reports_auth_and_identity_without_creating_identity() {
    let store = MemorySecretStore::default();

    let empty = cloud::session_status_from_store(&store).expect("status should read");
    assert!(!empty.authenticated);
    assert!(!empty.refresh_token_present);
    assert!(!empty.identity_present);
    assert!(empty.daemon_id.is_none());
    assert!(
        store
            .get_secret(CloudSecretKey::DaemonIdentityKey)
            .expect("identity key should read")
            .is_none()
    );

    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh")
        .expect("refresh token should store");
    let identity = load_or_create_identity(&store).expect("identity should create");
    let ready = cloud::session_status_from_store(&store).expect("status should read");

    assert!(ready.authenticated);
    assert!(ready.refresh_token_present);
    assert!(ready.identity_present);
    assert_eq!(
        ready.daemon_id,
        Some(identity.daemon_id().hyphenated().to_string())
    );
    assert_eq!(
        ready.identity_pubkey,
        Some(identity.keypair().public_key().as_str().to_owned())
    );
}

#[test]
fn cloud_logout_deletes_refresh_token_and_preserves_identity() {
    let store = MemorySecretStore::default();
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh")
        .expect("refresh token should store");
    let identity = load_or_create_identity(&store).expect("identity should create");

    let logout = cloud::logout_from_store(&store, 2).expect("logout should clear local token");

    assert!(!logout.authenticated);
    assert!(logout.refresh_token_deleted);
    assert!(!logout.entitlement_cache_deleted);
    assert!(logout.identity_preserved);
    assert_eq!(
        logout.daemon_id,
        Some(identity.daemon_id().hyphenated().to_string())
    );
    assert_eq!(logout.pending_login_sessions_cleared, 2);
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("refresh token should read"),
        None
    );
    assert!(
        store
            .get_secret(CloudSecretKey::DaemonIdentityKey)
            .expect("identity key should read")
            .is_some()
    );
}

#[test]
fn cloud_connection_status_reports_ready_prerequisites_without_live_socket() {
    let config = HypercolorConfig::default().cloud;
    let session = CloudSessionStatusFixture::ready();
    let entitlement = cloud_entitlements::CachedCloudEntitlement::from_response(
        &entitlement_token_fixture(),
        std::time::SystemTime::UNIX_EPOCH,
    );
    let mut enabled_config = config.clone();
    enabled_config.enabled = true;

    let status = cloud::connection_status_from_parts(
        &enabled_config,
        &session.into_status(),
        Some(&entitlement),
        &CloudConnectionSnapshot::default(),
    );

    assert_eq!(
        serde_json::to_value(status.state).expect("serialize state"),
        "ready"
    );
    assert_eq!(
        serde_json::to_value(status.runtime_state).expect("serialize runtime state"),
        "idle"
    );
    assert!(!status.connected);
    assert!(status.can_connect);
    assert!(status.authenticated);
    assert!(status.identity_present);
    assert!(status.entitlement_cached);
    assert_eq!(
        status.connect_url.as_deref(),
        Some("wss://api.hypercolor.lighting/v1/daemon/connect")
    );
}

#[tokio::test]
async fn cloud_connection_prepare_rejects_disabled_cloud_without_keyring() {
    let app = api::build_router(Arc::new(AppState::new()), None);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cloud/connection/prepare")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn cloud_connection_connect_rejects_disabled_cloud_without_keyring() {
    let app = api::build_router(Arc::new(AppState::new()), None);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cloud/connection/connect")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn cloud_connection_prepare_stages_signed_request_without_returning_secret() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test server address should resolve")
    );
    let state = cloud_test_state_with_cloud(&base_url, true);
    let store = MemorySecretStore::default();
    let identity = load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should store");
    let server = spawn_connect_preparation_server(
        listener,
        "access-for-registration",
        "refresh-next",
        "daemon-connect-token",
        identity.daemon_id(),
        identity.keypair().public_key().as_str().to_owned(),
    );
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(&base_url, &base_url)
            .expect("base urls should parse"),
    );

    let status = cloud::prepare_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([3_u8; 32], [7_u8; 16]),
    )
    .await
    .expect("connection prepare should succeed");

    tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
    assert_eq!(
        serde_json::to_value(status.state).expect("serialize state"),
        "ready"
    );
    assert_eq!(status.runtime_state, CloudConnectionRuntimeState::Prepared);
    assert!(!status.connected);
    assert!(status.can_connect);
    assert!(status.last_error.is_none());

    let request = state
        .cloud_connection
        .write()
        .await
        .take_prepared_connect()
        .expect("runtime should stage signed request");
    assert_eq!(
        header_value(&request.headers.pairs(), HEADER_AUTHORIZATION),
        "Bearer daemon-connect-token"
    );
}

#[tokio::test]
async fn cloud_connection_prepare_reports_missing_identity_and_refresh_token() {
    let state = cloud_test_state_with_cloud("http://127.0.0.1:1", true);
    let store = MemorySecretStore::default();
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url("http://127.0.0.1:1", "http://127.0.0.1:1")
            .expect("base urls should parse"),
    );

    let missing_identity = cloud::prepare_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([4; 32], [8; 16]),
    )
    .await
    .expect_err("missing identity should fail with a conflict classification");
    assert!(matches!(
        missing_identity,
        cloud::CloudConnectionPrepareError::MissingIdentity
    ));

    load_or_create_identity(&store).expect("identity should create");
    let missing_refresh = cloud::prepare_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([5; 32], [9; 16]),
    )
    .await
    .expect_err("missing refresh token should fail with a conflict classification");
    assert!(matches!(
        missing_refresh,
        cloud::CloudConnectionPrepareError::MissingRefreshToken
    ));
    let snapshot = state.cloud_connection.read().await.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("missing cloud refresh token")
    );
}

#[tokio::test]
async fn cloud_connection_prepare_records_network_failure_as_backoff() {
    let state = cloud_test_state_with_cloud("http://127.0.0.1:1", true);
    let store = MemorySecretStore::default();
    load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should store");
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url("http://127.0.0.1:1", "http://127.0.0.1:1")
            .expect("base urls should parse"),
    );

    let error = cloud::prepare_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([6; 32], [10; 16]),
    )
    .await
    .expect_err("network failure should surface as prepare error");
    assert!(matches!(
        error,
        cloud::CloudConnectionPrepareError::Prepare(_)
    ));
    let snapshot = state.cloud_connection.read().await.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert!(snapshot.last_error.is_some());
}

#[tokio::test]
async fn cloud_connection_connect_starts_socket_task_after_prepare() {
    let (_tempdir, _guard) = set_temp_data_dir();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test server address should resolve")
    );
    let state = cloud_test_state_with_cloud(&base_url, true);
    let store = MemorySecretStore::default();
    let identity = load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should store");
    let (welcome_tx, welcome_rx) = oneshot::channel();
    let (close_tx, close_rx) = oneshot::channel();
    let server = spawn_connect_start_server(
        listener,
        "access-for-registration",
        "refresh-next",
        "daemon-connect-token",
        identity.daemon_id(),
        identity.keypair().public_key().as_str().to_owned(),
        welcome_tx,
        close_rx,
    );
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(&base_url, &base_url)
            .expect("base urls should parse"),
    );

    let status = cloud::connect_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([7; 32], [11; 16]),
    )
    .await
    .expect("cloud connection should start");

    assert_eq!(
        status.runtime_state,
        CloudConnectionRuntimeState::Connecting
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), welcome_rx)
        .await
        .expect("welcome should arrive")
        .expect("welcome signal should send");
    wait_for_cloud_runtime_state(&state, CloudConnectionRuntimeState::Connected).await;
    let snapshot = state.cloud_connection.read().await.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connected
    );
    assert!(snapshot.connected);
    assert_eq!(
        snapshot.session_id.as_deref(),
        Some("00000000000000000000000000")
    );

    let prepare_while_running = cloud::prepare_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([8; 32], [12; 16]),
    )
    .await
    .expect_err("prepare should be rejected while the socket task is running");
    assert!(matches!(
        prepare_while_running,
        cloud::CloudConnectionPrepareError::AlreadyRunning
    ));

    let second = cloud::connect_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([9; 32], [13; 16]),
    )
    .await
    .expect_err("duplicate connection should be rejected");
    assert!(matches!(
        second,
        cloud::CloudConnectionStartError::Socket(CloudSocketStartError::AlreadyRunning)
    ));

    let _ = close_tx.send(());
    tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
    wait_for_cloud_runtime_state(&state, CloudConnectionRuntimeState::Backoff).await;
    let snapshot = state.cloud_connection.read().await.snapshot();
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("cloud websocket closed")
    );
    state
        .cloud_socket
        .lock()
        .await
        .shutdown(&state.cloud_connection)
        .await;
    let snapshot = state.cloud_connection.read().await.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("cloud websocket closed")
    );
}

#[tokio::test]
async fn cloud_connection_disconnect_clears_runtime_snapshot() {
    let (_tempdir, _guard) = set_temp_data_dir();
    let state = cloud_test_state_with_cloud("http://127.0.0.1:1", true);
    let store = MemorySecretStore::default();
    load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should store");
    state
        .cloud_connection
        .write()
        .await
        .mark_backoff("stale cloud error");

    let status = cloud::disconnect_connection_from_store(&state, &store)
        .await
        .expect("disconnect should report status");

    assert_eq!(status.runtime_state, CloudConnectionRuntimeState::Idle);
    assert!(!status.connected);
    assert!(status.last_error.is_none());
    assert_eq!(
        state.cloud_connection.read().await.snapshot().runtime_state,
        CloudConnectionRuntimeState::Idle
    );
}

#[tokio::test]
async fn cloud_connection_disconnect_stops_running_socket_task() {
    let (_tempdir, _guard) = set_temp_data_dir();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test server address should resolve")
    );
    let state = cloud_test_state_with_cloud(&base_url, true);
    let store = MemorySecretStore::default();
    let identity = load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should store");
    let (welcome_tx, welcome_rx) = oneshot::channel();
    let (close_tx, close_rx) = oneshot::channel();
    let server = spawn_connect_start_server(
        listener,
        "access-for-registration",
        "refresh-next",
        "daemon-connect-token",
        identity.daemon_id(),
        identity.keypair().public_key().as_str().to_owned(),
        welcome_tx,
        close_rx,
    );
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(&base_url, &base_url)
            .expect("base urls should parse"),
    );

    cloud::connect_connection_from_store(
        &state,
        &client,
        &store,
        prepare_input([10; 32], [14; 16]),
    )
    .await
    .expect("cloud connection should start");
    tokio::time::timeout(std::time::Duration::from_secs(2), welcome_rx)
        .await
        .expect("welcome should arrive")
        .expect("welcome signal should send");
    wait_for_cloud_runtime_state(&state, CloudConnectionRuntimeState::Connected).await;

    let status = cloud::disconnect_connection_from_store(&state, &store)
        .await
        .expect("disconnect should report status");

    assert_eq!(status.runtime_state, CloudConnectionRuntimeState::Idle);
    assert!(!status.connected);
    assert!(status.last_error.is_none());
    let _ = close_tx.send(());
    tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
}

async fn wait_for_cloud_runtime_state(state: &AppState, expected: CloudConnectionRuntimeState) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if state.cloud_connection.read().await.snapshot().runtime_state == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for cloud runtime state {expected:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[test]
fn cloud_connection_status_reports_live_runtime_channels() {
    let mut config = HypercolorConfig::default().cloud;
    config.enabled = true;
    let session = CloudSessionStatusFixture::ready().into_status();
    let mut runtime = CloudConnectionRuntime::default();
    runtime.mark_connected(&welcome_fixture());

    let status = cloud::connection_status_from_parts(&config, &session, None, &runtime.snapshot());

    assert_eq!(
        serde_json::to_value(status.runtime_state).expect("serialize runtime state"),
        "connected"
    );
    assert!(status.connected);
    assert!(status.session_id.is_some());
    assert_eq!(status.available_channels, vec!["control"]);
    assert_eq!(status.denied_channels[0].name, "relay.http");
    assert_eq!(status.denied_channels[0].reason, "entitlement_missing");
    assert_eq!(
        status.denied_channels[0].feature.as_deref(),
        Some("hc.remote")
    );
}

#[test]
fn cloud_connection_status_blocks_when_signed_out() {
    let mut config = HypercolorConfig::default().cloud;
    config.enabled = true;
    let session = CloudSessionStatusFixture::signed_out().into_status();

    let status = cloud::connection_status_from_parts(
        &config,
        &session,
        None,
        &CloudConnectionSnapshot::default(),
    );

    assert_eq!(
        serde_json::to_value(status.state).expect("serialize state"),
        "signed_out"
    );
    assert!(!status.can_connect);
    assert!(!status.entitlement_cached);
}

#[test]
fn cloud_connection_status_reports_runtime_backoff_error() {
    let mut config = HypercolorConfig::default().cloud;
    config.enabled = true;
    let mut runtime = CloudConnectionRuntime::default();
    runtime.mark_backoff("websocket refused");

    let status = cloud::connection_status_from_parts(
        &config,
        &CloudSessionStatusFixture::ready().into_status(),
        None,
        &runtime.snapshot(),
    );

    assert_eq!(
        serde_json::to_value(status.runtime_state).expect("serialize runtime state"),
        "backoff"
    );
    assert!(!status.connected);
    assert!(status.can_connect);
    assert_eq!(status.last_error.as_deref(), Some("websocket refused"));
}

#[test]
fn cloud_connection_status_blocks_when_missing_identity_or_disabled() {
    let mut enabled = HypercolorConfig::default().cloud;
    enabled.enabled = true;
    let missing_identity = CloudSessionStatusFixture {
        refresh_token_present: true,
        identity_present: false,
    }
    .into_status();

    let missing = cloud::connection_status_from_parts(
        &enabled,
        &missing_identity,
        None,
        &CloudConnectionSnapshot::default(),
    );
    assert_eq!(
        serde_json::to_value(missing.state).expect("serialize state"),
        "missing_identity"
    );
    assert!(!missing.can_connect);

    let disabled = cloud::connection_status_from_parts(
        &HypercolorConfig::default().cloud,
        &CloudSessionStatusFixture::ready().into_status(),
        None,
        &CloudConnectionSnapshot::default(),
    );
    assert_eq!(
        serde_json::to_value(disabled.state).expect("serialize state"),
        "disabled"
    );
    assert!(!disabled.can_connect);
}

fn set_temp_data_dir() -> (TempDir, DataDirOverrideGuard) {
    let tempdir = TempDir::new().expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(tempdir.path().join("data")));
    (tempdir, DataDirOverrideGuard)
}

struct DataDirOverrideGuard;

impl Drop for DataDirOverrideGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
    }
}

struct CloudSessionStatusFixture {
    refresh_token_present: bool,
    identity_present: bool,
}

impl CloudSessionStatusFixture {
    const fn ready() -> Self {
        Self {
            refresh_token_present: true,
            identity_present: true,
        }
    }

    const fn signed_out() -> Self {
        Self {
            refresh_token_present: false,
            identity_present: true,
        }
    }

    fn into_status(self) -> cloud::CloudSessionStatus {
        cloud::CloudSessionStatus {
            authenticated: self.refresh_token_present && self.identity_present,
            refresh_token_present: self.refresh_token_present,
            identity_present: self.identity_present,
            daemon_id: None,
            identity_pubkey: None,
            credential_storage: "memory".into(),
        }
    }
}

fn cloud_test_state(auth_base_url: &str) -> Arc<AppState> {
    cloud_test_state_with_cloud(auth_base_url, false)
}

fn cloud_test_state_with_cloud(auth_base_url: &str, enabled: bool) -> Arc<AppState> {
    let tempdir = TempDir::new().expect("temp dir should be created");
    let manager = ConfigManager::new(tempdir.path().join("config.toml"))
        .expect("config manager should initialize");
    let mut config = HypercolorConfig::default();
    auth_base_url.clone_into(&mut config.cloud.base_url);
    auth_base_url.clone_into(&mut config.cloud.auth_base_url);
    config.cloud.enabled = enabled;
    manager.update(config);

    let mut state = AppState::new();
    state.config_manager = Some(Arc::new(manager));
    Arc::new(state)
}

fn prepare_input(
    identity_nonce: [u8; 32],
    upgrade_nonce: [u8; 16],
) -> CloudConnectionPrepareInput<'static> {
    CloudConnectionPrepareInput {
        install_name: "desk-mac",
        os: "macos",
        arch: "aarch64",
        daemon_version: "1.4.2",
        identity_nonce: IdentityNonce::from_bytes(identity_nonce),
        timestamp: "2026-05-15T17:00:00Z",
        upgrade_nonce: UpgradeNonce::from_bytes(upgrade_nonce),
    }
}

fn entitlement_token_fixture() -> cloud_api::EntitlementTokenResponse {
    cloud_api::EntitlementTokenResponse {
        jwt: "header.payload.signature".into(),
        claims: cloud_api::EntitlementClaims {
            iss: "https://api.hypercolor.lighting".into(),
            sub: uuid::Uuid::nil().to_string(),
            aud: vec!["hypercolor-daemon".into(), "hypercolor-updater".into()],
            iat: 1_999_999_000,
            exp: 2_000_000_000,
            jti: "01JTEST".into(),
            kid: "ent-2026-01".into(),
            token_version: 1,
            device_install_id: uuid::Uuid::nil(),
            tier: "free".into(),
            features: vec![cloud_api::FeatureKey::CloudSync],
            channels: vec![cloud_api::ReleaseChannel::Stable],
            rate_limits: cloud_api::RateLimits {
                remote_bandwidth_gb_month: 10,
                remote_concurrent_tunnels: 5,
                studio_sessions_month: 5,
                studio_max_session_seconds: 30,
                studio_max_session_tokens: 100_000,
                studio_default_model: "claude-haiku-4-5".into(),
            },
            update_until: 2_100_000_000,
        },
    }
}

#[derive(Debug, Default)]
struct MemorySecretStore {
    values: std::sync::Mutex<std::collections::HashMap<CloudSecretKey, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get_secret(&self, key: CloudSecretKey) -> Result<Option<String>, CloudClientError> {
        Ok(self
            .values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .get(&key)
            .cloned())
    }

    fn put_secret(&self, key: CloudSecretKey, value: &str) -> Result<(), CloudClientError> {
        self.values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .insert(key, value.to_owned());
        Ok(())
    }

    fn delete_secret(&self, key: CloudSecretKey) -> Result<(), CloudClientError> {
        self.values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .remove(&key);
        Ok(())
    }
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    serde_json::from_slice(&body).expect("body should be json")
}

fn welcome_fixture() -> WelcomeFrame {
    serde_json::from_value(serde_json::json!({
        "session_id": "00000000000000000000000000",
        "available_channels": ["control"],
        "denied_channels": [
            {
                "name": "relay.http",
                "reason": "entitlement_missing",
                "feature": "hc.remote"
            }
        ],
        "server_capabilities": {
            "tunnel_resume": false,
            "compression": [],
            "max_frame_bytes": 65536
        },
        "heartbeat_interval_s": 25
    }))
    .expect("welcome fixture should deserialize")
}

async fn spawn_auth_server() -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let router = Router::new()
        .route("/api/auth/device/code", post(device_code))
        .route("/api/auth/device/token", post(device_token_pending));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("auth server listener should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("auth server address should resolve")
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    (base_url, shutdown_tx, task)
}

async fn shutdown_auth_server(shutdown_tx: oneshot::Sender<()>, task: tokio::task::JoinHandle<()>) {
    let _ = shutdown_tx.send(());
    task.await.expect("auth server task should join");
}

fn spawn_connect_preparation_server(
    listener: tokio::net::TcpListener,
    access_token: &'static str,
    refresh_token: &'static str,
    registration_token: &'static str,
    daemon_id: uuid::Uuid,
    identity_pubkey: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut token_socket, _) = listener
            .accept()
            .await
            .expect("token request should connect");
        let mut buffer = vec![0_u8; 4096];
        let read = token_socket
            .read(&mut buffer)
            .await
            .expect("token request should read");
        let token_request = String::from_utf8_lossy(&buffer[..read]);

        assert!(token_request.starts_with("POST /api/auth/oauth2/token HTTP/1.1"));
        assert!(token_request.contains(r#""refresh_token":"refresh-old""#));
        assert!(token_request.contains(r#""client_id":"hypercolor-daemon""#));

        write_json_response(
            &mut token_socket,
            &serde_json::json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "refresh_token": refresh_token,
                "expires_in": 900,
                "scope": "openid profile email"
            }),
        )
        .await;
        drop(token_socket);

        let (mut registration_socket, _) = listener
            .accept()
            .await
            .expect("registration request should connect");
        let mut buffer = vec![0_u8; 8192];
        let read = registration_socket
            .read(&mut buffer)
            .await
            .expect("registration request should read");
        let registration_request = String::from_utf8_lossy(&buffer[..read]);

        assert!(registration_request.starts_with("POST /v1/me/devices HTTP/1.1"));
        assert!(registration_request.contains(&format!("authorization: Bearer {access_token}")));
        assert!(registration_request.contains(&format!(r#""daemon_id":"{daemon_id}""#)));
        assert!(registration_request.contains(r#""install_name":"desk-mac""#));
        assert!(
            registration_request.contains(&format!(r#""identity_pubkey":"{identity_pubkey}""#))
        );

        write_json_response(
            &mut registration_socket,
            &serde_json::json!({
                "device": {
                    "id": "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
                    "user_id": "018f4c36-4a44-7cc9-9f57-0d2e9224d2f2",
                    "daemon_id": daemon_id,
                    "install_name": "desk-mac",
                    "os": "macos",
                    "arch": "aarch64",
                    "daemon_version": "1.4.2",
                    "identity_pubkey": identity_pubkey,
                    "last_seen_at": "2026-05-15T17:00:00Z",
                    "created_at": "2026-05-15T17:00:00Z"
                },
                "registration_token": registration_token
            }),
        )
        .await;
        drop(registration_socket);
    })
}

fn spawn_connect_start_server(
    listener: tokio::net::TcpListener,
    access_token: &'static str,
    refresh_token: &'static str,
    registration_token: &'static str,
    daemon_id: uuid::Uuid,
    identity_pubkey: String,
    welcome_tx: oneshot::Sender<()>,
    close_rx: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut token_socket, _) = listener
            .accept()
            .await
            .expect("token request should connect");
        let mut buffer = vec![0_u8; 4096];
        let read = token_socket
            .read(&mut buffer)
            .await
            .expect("token request should read");
        let token_request = String::from_utf8_lossy(&buffer[..read]);

        assert!(token_request.starts_with("POST /api/auth/oauth2/token HTTP/1.1"));
        assert!(token_request.contains(r#""refresh_token":"refresh-old""#));
        write_json_response(
            &mut token_socket,
            &serde_json::json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "refresh_token": refresh_token,
                "expires_in": 900,
                "scope": "openid profile email"
            }),
        )
        .await;
        drop(token_socket);

        let (mut registration_socket, _) = listener
            .accept()
            .await
            .expect("registration request should connect");
        let mut buffer = vec![0_u8; 8192];
        let read = registration_socket
            .read(&mut buffer)
            .await
            .expect("registration request should read");
        let registration_request = String::from_utf8_lossy(&buffer[..read]);

        assert!(registration_request.starts_with("POST /v1/me/devices HTTP/1.1"));
        assert!(registration_request.contains(&format!("authorization: Bearer {access_token}")));
        assert!(registration_request.contains(&format!(r#""daemon_id":"{daemon_id}""#)));
        assert!(
            registration_request.contains(&format!(r#""identity_pubkey":"{identity_pubkey}""#))
        );
        write_json_response(
            &mut registration_socket,
            &serde_json::json!({
                "device": {
                    "id": "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
                    "user_id": "018f4c36-4a44-7cc9-9f57-0d2e9224d2f2",
                    "daemon_id": daemon_id,
                    "install_name": "desk-mac",
                    "os": "macos",
                    "arch": "aarch64",
                    "daemon_version": "1.4.2",
                    "identity_pubkey": identity_pubkey,
                    "last_seen_at": "2026-05-15T17:00:00Z",
                    "created_at": "2026-05-15T17:00:00Z"
                },
                "registration_token": registration_token
            }),
        )
        .await;
        drop(registration_socket);

        let (stream, _) = listener
            .accept()
            .await
            .expect("websocket request should connect");
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, assert_cloud_handshake)
            .await
            .expect("websocket should accept");
        let hello = socket
            .next()
            .await
            .expect("daemon should send hello")
            .expect("hello should read");
        let Message::Text(hello) = hello else {
            panic!("hello should be text");
        };
        let hello: hypercolor_cloud_client::daemon_link::HelloFrame =
            serde_json::from_str(&hello).expect("hello should deserialize");
        assert!(hello.daemon_capabilities.sync);
        assert_eq!(hello.entitlement_jwt, None);

        socket
            .send(Message::Text(welcome_fixture_json().into()))
            .await
            .expect("welcome should send");
        let _ = welcome_tx.send(());
        let _ = close_rx.await;
        let _ = socket.send(Message::Close(None)).await;
    })
}

async fn write_json_response(socket: &mut tokio::net::TcpStream, body: &serde_json::Value) {
    let body = serde_json::to_string(body).expect("response should serialize");
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    socket
        .write_all(response.as_bytes())
        .await
        .expect("response should write");
}

#[expect(
    clippy::result_large_err,
    clippy::unnecessary_wraps,
    reason = "Tungstenite's Callback trait fixes the response error type"
)]
fn assert_cloud_handshake(
    request: &WsRequest,
    mut response: WsResponse,
) -> Result<WsResponse, WsErrorResponse> {
    assert_eq!(
        request
            .headers()
            .get(HEADER_AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer daemon-connect-token")
    );
    assert_eq!(
        request
            .headers()
            .get(HEADER_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(WEBSOCKET_PROTOCOL)
    );
    response.headers_mut().insert(
        HEADER_WEBSOCKET_PROTOCOL,
        WEBSOCKET_PROTOCOL
            .parse()
            .expect("websocket protocol should parse"),
    );

    Ok(response)
}

fn welcome_fixture_json() -> String {
    serde_json::to_string(&serde_json::json!({
        "session_id": "00000000000000000000000000",
        "available_channels": ["control", "sync.notifications"],
        "denied_channels": [],
        "server_capabilities": {
            "tunnel_resume": true,
            "compression": [],
            "max_frame_bytes": 1_048_576
        },
        "heartbeat_interval_s": 25
    }))
    .expect("welcome should serialize")
}

fn header_value<'a>(pairs: &'a [(&'static str, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| value.as_str())
        .expect("header should be present")
}

async fn device_code() -> Json<cloud_api::DeviceCodeResponse> {
    Json(device_code_fixture(900))
}

fn device_code_fixture(expires_in: u64) -> cloud_api::DeviceCodeResponse {
    cloud_api::DeviceCodeResponse {
        device_code: "device-code-secret".to_owned(),
        user_code: "HC-1234".to_owned(),
        verification_uri: "https://hypercolor.lighting/activate".to_owned(),
        verification_uri_complete: Some("https://hypercolor.lighting/activate?code=HC-1234".into()),
        expires_in,
        interval: Some(1),
    }
}

async fn device_token_pending() -> (AxumStatusCode, Json<cloud_api::DeviceTokenError>) {
    (
        AxumStatusCode::BAD_REQUEST,
        Json(cloud_api::DeviceTokenError {
            error: cloud_api::DeviceTokenErrorCode::AuthorizationPending,
            error_description: None,
        }),
    )
}
