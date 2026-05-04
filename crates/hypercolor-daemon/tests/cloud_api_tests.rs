#![cfg(feature = "cloud")]

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::StatusCode as AxumStatusCode;
use axum::routing::post;
use http::{Request, StatusCode};
use hypercolor_cloud_client::{
    CloudClientError, CloudSecretKey, DeviceAuthorizationSession, RefreshTokenOwner, SecretStore,
    api as cloud_api, load_or_create_identity, load_refresh_token, store_refresh_token,
};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState, cloud};
use hypercolor_types::config::HypercolorConfig;
use tempfile::TempDir;
use tokio::sync::oneshot;
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

fn cloud_test_state(auth_base_url: &str) -> Arc<AppState> {
    let tempdir = TempDir::new().expect("temp dir should be created");
    let manager = ConfigManager::new(tempdir.path().join("config.toml"))
        .expect("config manager should initialize");
    let mut config = HypercolorConfig::default();
    auth_base_url.clone_into(&mut config.cloud.base_url);
    auth_base_url.clone_into(&mut config.cloud.auth_base_url);
    manager.update(config);

    let mut state = AppState::new();
    state.config_manager = Some(Arc::new(manager));
    Arc::new(state)
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
