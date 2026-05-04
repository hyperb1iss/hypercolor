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
use hypercolor_daemon::{
    api::{self, AppState, cloud},
    cloud_entitlements,
};
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
    );

    assert_eq!(
        serde_json::to_value(status.state).expect("serialize state"),
        "ready"
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

#[test]
fn cloud_connection_status_blocks_when_signed_out() {
    let mut config = HypercolorConfig::default().cloud;
    config.enabled = true;
    let session = CloudSessionStatusFixture::signed_out().into_status();

    let status = cloud::connection_status_from_parts(&config, &session, None);

    assert_eq!(
        serde_json::to_value(status.state).expect("serialize state"),
        "signed_out"
    );
    assert!(!status.can_connect);
    assert!(!status.entitlement_cached);
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
