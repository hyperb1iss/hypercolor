//! Hypercolor Cloud endpoints.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::extract::{Path, State};
use axum::response::Response;
use hypercolor_cloud_client::api as cloud_api;
use hypercolor_cloud_client::daemon_link::{IdentityNonce, UpgradeNonce};
use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, DeviceAuthorizationStatus, DeviceRegistrationInput,
    KeyringSecretStore, RefreshTokenOwner, SecretStore, delete_refresh_token, load_identity,
    load_or_create_identity, load_refresh_token, persist_device_token, signed_device_registration,
};
use hypercolor_types::config::CloudConfig;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse, iso8601_system_time};
use crate::cloud_connection::{
    CloudConnectionPrepareInput, CloudConnectionPrepareResult, CloudConnectionRuntime,
    CloudConnectionRuntimeState, CloudConnectionSnapshot, CloudDeniedChannelStatus,
};
use crate::cloud_entitlements::{
    CachedCloudEntitlement, delete_cached_entitlement, entitlement_cache_path,
    iso8601_from_unix_seconds, load_cached_entitlement, store_entitlement_response,
    unix_now_seconds,
};
use crate::cloud_socket::{CloudSocketHelloInput, CloudSocketStartError};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudStatus {
    pub compiled: bool,
    pub enabled: bool,
    pub connect_on_start: bool,
    pub base_url: String,
    pub auth_base_url: String,
    pub app_base_url: String,
    pub device_client_id: String,
    pub device_scope: String,
    pub identity_storage: String,
}

impl CloudStatus {
    #[must_use]
    pub fn from_config(config: &CloudConfig) -> Self {
        Self {
            compiled: true,
            enabled: config.enabled,
            connect_on_start: config.connect_on_start,
            base_url: config.base_url.clone(),
            auth_base_url: config.auth_base_url.clone(),
            app_base_url: config.app_base_url.clone(),
            device_client_id: config.device_client_id.clone(),
            device_scope: config.device_scope.clone(),
            identity_storage: "os_keyring".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudIdentityStatus {
    pub daemon_id: String,
    pub identity_pubkey: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudSessionStatus {
    pub authenticated: bool,
    pub refresh_token_present: bool,
    pub identity_present: bool,
    pub daemon_id: Option<String>,
    pub identity_pubkey: Option<String>,
    pub credential_storage: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudConnectionStatus {
    pub state: CloudConnectionState,
    pub runtime_state: CloudConnectionRuntimeState,
    pub connected: bool,
    pub can_connect: bool,
    pub connect_on_start: bool,
    pub connect_url: Option<String>,
    pub authenticated: bool,
    pub identity_present: bool,
    pub entitlement_cached: bool,
    pub entitlement_stale: Option<bool>,
    pub session_id: Option<String>,
    pub available_channels: Vec<String>,
    pub denied_channels: Vec<CloudDeniedChannelStatus>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudConnectionState {
    Disabled,
    SignedOut,
    MissingIdentity,
    Ready,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudLogoutStatus {
    pub authenticated: bool,
    pub refresh_token_deleted: bool,
    pub entitlement_cache_deleted: bool,
    pub identity_preserved: bool,
    pub daemon_id: Option<String>,
    pub pending_login_sessions_cleared: usize,
    pub credential_storage: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudLoginStart {
    pub login_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub retry_after_ms: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudLoginState {
    Pending,
    Authorized,
    Expired,
    Rejected,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudLoginError {
    pub code: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudLoginPoll {
    pub login_id: String,
    pub status: CloudLoginState,
    pub retry_after_ms: Option<u64>,
    pub refresh_token_stored: bool,
    pub daemon_id: Option<String>,
    pub identity_pubkey: Option<String>,
    pub device_install_id: Option<String>,
    pub device_registered: bool,
    pub registration_token_issued: bool,
    pub entitlement_cached: bool,
    pub entitlement_error: Option<String>,
    pub error: Option<CloudLoginError>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudEntitlementStatus {
    pub cached: bool,
    pub jwt_present: bool,
    pub stale: bool,
    pub cached_at: Option<String>,
    pub expires_at: Option<String>,
    pub update_until: Option<String>,
    pub tier: Option<String>,
    pub device_install_id: Option<String>,
    pub features: Vec<String>,
    pub channels: Vec<String>,
}

pub async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    let cloud_config = cloud_config(&state);
    ApiResponse::ok(CloudStatus::from_config(&cloud_config))
}

pub async fn ensure_identity() -> Response {
    match KeyringSecretStore::new_native().and_then(|store| load_or_create_identity(&store)) {
        Ok(identity) => ApiResponse::ok(CloudIdentityStatus {
            daemon_id: identity.daemon_id().hyphenated().to_string(),
            identity_pubkey: identity.keypair().public_key().as_str().to_owned(),
        }),
        Err(error) => ApiError::internal(format!("failed to initialize cloud identity: {error}")),
    }
}

pub async fn get_session() -> Response {
    match KeyringSecretStore::new_native().and_then(|store| session_status_from_store(&store)) {
        Ok(status) => ApiResponse::ok(status),
        Err(error) => ApiError::internal(format!("failed to read cloud session: {error}")),
    }
}

pub async fn get_entitlement_cache() -> Response {
    match load_cached_entitlement(entitlement_cache_path()).await {
        Ok(Some(entitlement)) => ApiResponse::ok(CloudEntitlementStatus::from_cached(&entitlement)),
        Ok(None) => ApiResponse::ok(CloudEntitlementStatus::empty()),
        Err(error) => {
            ApiError::internal(format!("failed to read cloud entitlement cache: {error}"))
        }
    }
}

pub async fn get_connection(State(state): State<Arc<AppState>>) -> Response {
    let store = match KeyringSecretStore::new_native() {
        Ok(store) => store,
        Err(error) => return ApiError::internal(format!("failed to open cloud keyring: {error}")),
    };

    match connection_status_from_store(&state, &store).await {
        Ok(status) => ApiResponse::ok(status),
        Err(error) => ApiError::internal(error),
    }
}

pub async fn prepare_connection(State(state): State<Arc<AppState>>) -> Response {
    let cloud_config = cloud_config(&state);
    if !cloud_config.enabled {
        return ApiError::conflict("cloud connection is disabled");
    }

    let client = match CloudClientConfig::try_from(&cloud_config).map(CloudClient::new) {
        Ok(client) => client,
        Err(error) => return ApiError::internal(format!("invalid cloud configuration: {error}")),
    };
    let store = match KeyringSecretStore::new_native() {
        Ok(store) => store,
        Err(error) => return ApiError::internal(format!("failed to open cloud keyring: {error}")),
    };
    let timestamp = iso8601_system_time(SystemTime::now());
    let input = CloudConnectionPrepareInput {
        install_name: &state.server_identity.instance_name,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        daemon_version: &state.server_identity.version,
        identity_nonce: IdentityNonce::generate(),
        timestamp: &timestamp,
        upgrade_nonce: UpgradeNonce::generate(),
    };

    match prepare_connection_from_store(&state, &client, &store, input).await {
        Ok(status) => ApiResponse::ok(status),
        Err(error) => prepare_error_response(error),
    }
}

pub async fn connect_connection(State(state): State<Arc<AppState>>) -> Response {
    let cloud_config = cloud_config(&state);
    if !cloud_config.enabled {
        return ApiError::conflict("cloud connection is disabled");
    }
    let client = match CloudClientConfig::try_from(&cloud_config).map(CloudClient::new) {
        Ok(client) => client,
        Err(error) => return ApiError::internal(format!("invalid cloud configuration: {error}")),
    };
    let store = match KeyringSecretStore::new_native() {
        Ok(store) => store,
        Err(error) => return ApiError::internal(format!("failed to open cloud keyring: {error}")),
    };
    let timestamp = iso8601_system_time(SystemTime::now());
    let input = CloudConnectionPrepareInput {
        install_name: &state.server_identity.instance_name,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        daemon_version: &state.server_identity.version,
        identity_nonce: IdentityNonce::generate(),
        timestamp: &timestamp,
        upgrade_nonce: UpgradeNonce::generate(),
    };

    match connect_connection_from_store(&state, &client, &store, input).await {
        Ok(status) => ApiResponse::ok(status),
        Err(error) => connect_error_response(error),
    }
}

fn prepare_error_response(error: CloudConnectionPrepareError) -> Response {
    match error {
        CloudConnectionPrepareError::MissingIdentity => {
            ApiError::conflict("missing cloud identity")
        }
        CloudConnectionPrepareError::MissingRefreshToken => {
            ApiError::conflict("missing cloud refresh token")
        }
        CloudConnectionPrepareError::AlreadyRunning => {
            ApiError::conflict("cloud connection is already running")
        }
        CloudConnectionPrepareError::Prepare(error) => {
            ApiError::internal(format!("failed to prepare cloud connection: {error}"))
        }
        CloudConnectionPrepareError::Status(error) => ApiError::internal(error),
    }
}

fn connect_error_response(error: CloudConnectionStartError) -> Response {
    match error {
        CloudConnectionStartError::Prepare(error) => prepare_error_response(error),
        CloudConnectionStartError::Entitlement(error) => {
            ApiError::internal(format!("failed to read cloud entitlement cache: {error}"))
        }
        CloudConnectionStartError::Socket(CloudSocketStartError::AlreadyRunning) => {
            ApiError::conflict("cloud connection is already running")
        }
        CloudConnectionStartError::Socket(CloudSocketStartError::Connect(error)) => {
            ApiError::internal(format!("failed to start cloud connection: {error}"))
        }
        CloudConnectionStartError::Status(error) => ApiError::internal(error),
    }
}

#[derive(Debug)]
pub enum CloudConnectionPrepareError {
    MissingIdentity,
    MissingRefreshToken,
    AlreadyRunning,
    Prepare(hypercolor_cloud_client::CloudClientError),
    Status(String),
}

#[derive(Debug)]
pub enum CloudConnectionStartError {
    Prepare(CloudConnectionPrepareError),
    Entitlement(String),
    Socket(CloudSocketStartError),
    Status(String),
}

pub async fn connect_connection_from_store(
    state: &AppState,
    client: &CloudClient,
    store: &impl SecretStore,
    input: CloudConnectionPrepareInput<'_>,
) -> Result<CloudConnectionStatus, CloudConnectionStartError> {
    reject_running_cloud_socket(state).await?;
    let _prepare_guard = state.cloud_connection_prepare_lock.lock().await;
    reject_running_cloud_socket(state).await?;

    prepare_connection_from_store_locked(state, client, store, input)
        .await
        .map_err(CloudConnectionStartError::Prepare)?;
    let entitlement = load_cached_entitlement(entitlement_cache_path())
        .await
        .map_err(|error| CloudConnectionStartError::Entitlement(error.to_string()))?;
    let hello = CloudSocketHelloInput {
        entitlement_jwt: entitlement.map(|entitlement| entitlement.jwt),
        tunnel_resume: None,
        studio_preview: false,
    };
    let mut cloud_socket = state.cloud_socket.lock().await;
    if cloud_socket.is_running() {
        return Err(CloudConnectionStartError::Socket(
            CloudSocketStartError::AlreadyRunning,
        ));
    }
    cloud_socket
        .spawn_prepared_session(Arc::clone(&state.cloud_connection), hello)
        .await
        .map_err(CloudConnectionStartError::Socket)?;

    connection_status_from_store(state, store)
        .await
        .map_err(CloudConnectionStartError::Status)
}

pub async fn prepare_connection_from_store(
    state: &AppState,
    client: &CloudClient,
    store: &impl SecretStore,
    input: CloudConnectionPrepareInput<'_>,
) -> Result<CloudConnectionStatus, CloudConnectionPrepareError> {
    let mut cloud_socket = state.cloud_socket.lock().await;
    if cloud_socket.is_running() {
        return Err(CloudConnectionPrepareError::AlreadyRunning);
    }
    drop(cloud_socket);

    let _prepare_guard = state.cloud_connection_prepare_lock.lock().await;
    let mut cloud_socket = state.cloud_socket.lock().await;
    if cloud_socket.is_running() {
        return Err(CloudConnectionPrepareError::AlreadyRunning);
    }
    drop(cloud_socket);

    prepare_connection_from_store_locked(state, client, store, input).await
}

async fn reject_running_cloud_socket(state: &AppState) -> Result<(), CloudConnectionStartError> {
    let mut cloud_socket = state.cloud_socket.lock().await;
    if cloud_socket.is_running() {
        return Err(CloudConnectionStartError::Socket(
            CloudSocketStartError::AlreadyRunning,
        ));
    }
    Ok(())
}

async fn prepare_connection_from_store_locked(
    state: &AppState,
    client: &CloudClient,
    store: &impl SecretStore,
    input: CloudConnectionPrepareInput<'_>,
) -> Result<CloudConnectionStatus, CloudConnectionPrepareError> {
    let mut inflight =
        CloudConnectionPrepareInflight::mark_connecting(Arc::clone(&state.cloud_connection)).await;
    let result = client
        .prepare_stored_daemon_connect(store, input.into_stored_daemon_connect_input())
        .await;
    let prepare = state
        .cloud_connection
        .write()
        .await
        .record_prepare_result(result);
    inflight.disarm();
    let prepare = prepare.map_err(CloudConnectionPrepareError::Prepare)?;

    match prepare {
        CloudConnectionPrepareResult::Prepared(_) => connection_status_from_store(state, store)
            .await
            .map_err(CloudConnectionPrepareError::Status),
        CloudConnectionPrepareResult::MissingIdentity => {
            Err(CloudConnectionPrepareError::MissingIdentity)
        }
        CloudConnectionPrepareResult::MissingRefreshToken => {
            Err(CloudConnectionPrepareError::MissingRefreshToken)
        }
    }
}

struct CloudConnectionPrepareInflight {
    runtime: Arc<tokio::sync::RwLock<CloudConnectionRuntime>>,
    armed: bool,
}

impl CloudConnectionPrepareInflight {
    async fn mark_connecting(runtime: Arc<tokio::sync::RwLock<CloudConnectionRuntime>>) -> Self {
        runtime.write().await.mark_connecting();
        Self {
            runtime,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CloudConnectionPrepareInflight {
    fn drop(&mut self) {
        if self.armed
            && let Ok(mut runtime) = self.runtime.try_write()
        {
            runtime.mark_backoff("cloud connection prepare cancelled");
        }
    }
}

async fn connection_status_from_store(
    state: &AppState,
    store: &impl SecretStore,
) -> Result<CloudConnectionStatus, String> {
    let cloud_config = cloud_config(state);
    let session = match session_status_from_store(store) {
        Ok(status) => status,
        Err(error) => {
            return Err(format!("failed to read cloud session: {error}"));
        }
    };
    let entitlement = match load_cached_entitlement(entitlement_cache_path()).await {
        Ok(entitlement) => entitlement,
        Err(error) => {
            return Err(format!("failed to read cloud entitlement cache: {error}"));
        }
    };
    let runtime = state.cloud_connection.read().await.snapshot();

    Ok(connection_status_from_parts(
        &cloud_config,
        &session,
        entitlement.as_ref(),
        &runtime,
    ))
}

pub async fn logout(State(state): State<Arc<AppState>>) -> Response {
    let store = match KeyringSecretStore::new_native() {
        Ok(store) => store,
        Err(error) => return ApiError::internal(format!("failed to open cloud keyring: {error}")),
    };

    let cleared_sessions = {
        let mut sessions = state.cloud_login_sessions.lock().await;
        let count = sessions.len();
        sessions.clear();
        count
    };

    match logout_from_store(&store, cleared_sessions) {
        Ok(mut status) => match delete_cached_entitlement(entitlement_cache_path()).await {
            Ok(deleted) => {
                status.entitlement_cache_deleted = deleted;
                ApiResponse::ok(status)
            }
            Err(error) => {
                ApiError::internal(format!("failed to clear cloud entitlement cache: {error}"))
            }
        },
        Err(error) => ApiError::internal(format!("failed to clear cloud session: {error}")),
    }
}

pub async fn start_login(State(state): State<Arc<AppState>>) -> Response {
    prune_expired_login_sessions(&state).await;

    let client = match cloud_client(&state) {
        Ok(client) => client,
        Err(error) => return ApiError::internal(format!("invalid cloud configuration: {error}")),
    };
    let session = match client.begin_device_authorization().await {
        Ok(session) => session,
        Err(error) => return ApiError::internal(format!("failed to start cloud login: {error}")),
    };
    let login_id = Uuid::new_v4();
    let response = CloudLoginStart {
        login_id: login_id.hyphenated().to_string(),
        user_code: session.user_code().to_owned(),
        verification_uri: session.verification_uri().to_owned(),
        verification_uri_complete: session.verification_uri_complete().map(str::to_owned),
        expires_in: session.response().expires_in,
        interval: session.poll_interval().as_secs(),
        retry_after_ms: duration_millis(session.poll_interval()),
    };

    if session.is_expired() {
        return ApiError::conflict("cloud login authorization expired immediately");
    }

    state
        .cloud_login_sessions
        .lock()
        .await
        .insert(login_id, session);

    ApiResponse::created(response)
}

pub async fn prune_expired_login_sessions(state: &AppState) -> usize {
    let mut sessions = state.cloud_login_sessions.lock().await;
    let before = sessions.len();
    sessions.retain(|_, session| !session.is_expired());
    before - sessions.len()
}

pub async fn poll_login(
    State(state): State<Arc<AppState>>,
    Path(login_id): Path<Uuid>,
) -> Response {
    let client = match cloud_client(&state) {
        Ok(client) => client,
        Err(error) => return ApiError::internal(format!("invalid cloud configuration: {error}")),
    };

    let Some(mut session) = state.cloud_login_sessions.lock().await.remove(&login_id) else {
        return ApiError::not_found(format!("cloud login session {login_id} was not found"));
    };

    let status = match client.poll_device_authorization(&mut session).await {
        Ok(status) => status,
        Err(error) => {
            state
                .cloud_login_sessions
                .lock()
                .await
                .insert(login_id, session);
            return ApiError::internal(format!("failed to poll cloud login: {error}"));
        }
    };

    match status {
        DeviceAuthorizationStatus::Pending { error, retry_after } => {
            state
                .cloud_login_sessions
                .lock()
                .await
                .insert(login_id, session);
            ApiResponse::ok(CloudLoginPoll::pending(login_id, error, retry_after))
        }
        DeviceAuthorizationStatus::Authorized(token) => {
            match complete_authorized_login(&state, &client, login_id, &token).await {
                Ok(response) => ApiResponse::ok(response),
                Err(error) => {
                    ApiError::internal(format!("failed to complete cloud login: {error}"))
                }
            }
        }
        DeviceAuthorizationStatus::Expired(error) => ApiResponse::ok(
            CloudLoginPoll::terminal_error(login_id, CloudLoginState::Expired, error),
        ),
        DeviceAuthorizationStatus::Rejected(error) => ApiResponse::ok(
            CloudLoginPoll::terminal_error(login_id, CloudLoginState::Rejected, error),
        ),
    }
}

pub fn session_status_from_store(
    store: &impl SecretStore,
) -> Result<CloudSessionStatus, hypercolor_cloud_client::CloudClientError> {
    let refresh_token_present = load_refresh_token(store, RefreshTokenOwner::Daemon)?.is_some();
    let identity = load_identity(store)?;
    let identity_present = identity.is_some();
    let (daemon_id, identity_pubkey) = identity.map_or((None, None), |identity| {
        (
            Some(identity.daemon_id().hyphenated().to_string()),
            Some(identity.keypair().public_key().as_str().to_owned()),
        )
    });

    Ok(CloudSessionStatus {
        authenticated: refresh_token_present && identity_present,
        refresh_token_present,
        identity_present,
        daemon_id,
        identity_pubkey,
        credential_storage: "os_keyring".to_owned(),
    })
}

pub fn logout_from_store(
    store: &impl SecretStore,
    pending_login_sessions_cleared: usize,
) -> Result<CloudLogoutStatus, hypercolor_cloud_client::CloudClientError> {
    let refresh_token_deleted = load_refresh_token(store, RefreshTokenOwner::Daemon)?.is_some();
    delete_refresh_token(store, RefreshTokenOwner::Daemon)?;

    let identity = load_identity(store)?;
    let identity_preserved = identity.is_some();
    let daemon_id = identity.map(|identity| identity.daemon_id().hyphenated().to_string());

    Ok(CloudLogoutStatus {
        authenticated: false,
        refresh_token_deleted,
        entitlement_cache_deleted: false,
        identity_preserved,
        daemon_id,
        pending_login_sessions_cleared,
        credential_storage: "os_keyring".to_owned(),
    })
}

pub fn connection_status_from_parts(
    config: &CloudConfig,
    session: &CloudSessionStatus,
    entitlement: Option<&CachedCloudEntitlement>,
    runtime: &CloudConnectionSnapshot,
) -> CloudConnectionStatus {
    let (connect_url, last_error) = CloudClientConfig::try_from(config)
        .and_then(|config| config.daemon_connect_url())
        .map_or_else(
            |error| (None, Some(error.to_string())),
            |url| (Some(url.to_string()), None),
        );
    let state = if !config.enabled {
        CloudConnectionState::Disabled
    } else if !session.refresh_token_present {
        CloudConnectionState::SignedOut
    } else if !session.identity_present {
        CloudConnectionState::MissingIdentity
    } else {
        CloudConnectionState::Ready
    };
    let can_connect = matches!(state, CloudConnectionState::Ready);
    let connected = can_connect && runtime.connected;
    let last_error = last_error.or_else(|| runtime.last_error.clone());

    CloudConnectionStatus {
        state,
        runtime_state: runtime.runtime_state,
        connected,
        can_connect,
        connect_on_start: config.connect_on_start,
        connect_url,
        authenticated: session.authenticated,
        identity_present: session.identity_present,
        entitlement_cached: entitlement.is_some(),
        entitlement_stale: entitlement.map(|entitlement| {
            entitlement.is_stale_at_unix(crate::cloud_entitlements::unix_now_seconds())
        }),
        session_id: runtime.session_id.clone(),
        available_channels: runtime.available_channels.clone(),
        denied_channels: runtime.denied_channels.clone(),
        last_error,
    }
}

impl CloudLoginPoll {
    fn pending(login_id: Uuid, error: cloud_api::DeviceTokenError, retry_after: Duration) -> Self {
        Self {
            login_id: login_id.hyphenated().to_string(),
            status: CloudLoginState::Pending,
            retry_after_ms: Some(duration_millis(retry_after)),
            refresh_token_stored: false,
            daemon_id: None,
            identity_pubkey: None,
            device_install_id: None,
            device_registered: false,
            registration_token_issued: false,
            entitlement_cached: false,
            entitlement_error: None,
            error: Some(CloudLoginError::from_token_error(error)),
        }
    }

    fn terminal_error(
        login_id: Uuid,
        status: CloudLoginState,
        error: cloud_api::DeviceTokenError,
    ) -> Self {
        Self {
            login_id: login_id.hyphenated().to_string(),
            status,
            retry_after_ms: None,
            refresh_token_stored: false,
            daemon_id: None,
            identity_pubkey: None,
            device_install_id: None,
            device_registered: false,
            registration_token_issued: false,
            entitlement_cached: false,
            entitlement_error: None,
            error: Some(CloudLoginError::from_token_error(error)),
        }
    }
}

impl CloudEntitlementStatus {
    fn empty() -> Self {
        Self {
            cached: false,
            jwt_present: false,
            stale: false,
            cached_at: None,
            expires_at: None,
            update_until: None,
            tier: None,
            device_install_id: None,
            features: Vec::new(),
            channels: Vec::new(),
        }
    }

    fn from_cached(entitlement: &CachedCloudEntitlement) -> Self {
        Self {
            cached: true,
            jwt_present: !entitlement.jwt.is_empty(),
            stale: entitlement.is_stale_at_unix(unix_now_seconds()),
            cached_at: Some(entitlement.cached_at.clone()),
            expires_at: Some(entitlement.expires_at.clone()),
            update_until: Some(iso8601_from_unix_seconds(entitlement.claims.update_until)),
            tier: Some(entitlement.claims.tier.clone()),
            device_install_id: Some(
                entitlement
                    .claims
                    .device_install_id
                    .hyphenated()
                    .to_string(),
            ),
            features: entitlement
                .claims
                .features
                .iter()
                .map(|feature| feature.as_str().to_owned())
                .collect(),
            channels: entitlement
                .claims
                .channels
                .iter()
                .map(|channel| channel.as_str().to_owned())
                .collect(),
        }
    }
}

impl CloudLoginError {
    fn from_token_error(error: cloud_api::DeviceTokenError) -> Self {
        Self {
            code: token_error_code(error.error).to_owned(),
            description: error.error_description,
        }
    }
}

async fn complete_authorized_login(
    state: &AppState,
    client: &CloudClient,
    login_id: Uuid,
    token: &cloud_api::DeviceTokenResponse,
) -> Result<CloudLoginPoll, String> {
    let store = KeyringSecretStore::new_native().map_err(|error| error.to_string())?;
    let refresh_token_stored = persist_device_token(&store, RefreshTokenOwner::Daemon, token)
        .map_err(|error| error.to_string())?;
    if !refresh_token_stored {
        return Err("device authorization response did not include a refresh token".to_owned());
    }

    let identity = load_or_create_identity(&store).map_err(|error| error.to_string())?;
    let request = signed_device_registration(
        DeviceRegistrationInput {
            daemon_id: identity.daemon_id(),
            install_name: state.server_identity.instance_name.clone(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            daemon_version: state.server_identity.version.clone(),
            nonce: IdentityNonce::generate(),
        },
        identity.keypair(),
    );
    let registration = client
        .register_device(&token.access_token, &request)
        .await
        .map_err(|error| error.to_string())?;
    let (entitlement_cached, entitlement_error) =
        cache_entitlement_from_access_token(client, &token.access_token).await;

    Ok(CloudLoginPoll {
        login_id: login_id.hyphenated().to_string(),
        status: CloudLoginState::Authorized,
        retry_after_ms: None,
        refresh_token_stored,
        daemon_id: Some(identity.daemon_id().hyphenated().to_string()),
        identity_pubkey: Some(identity.keypair().public_key().as_str().to_owned()),
        device_install_id: Some(registration.device.id.hyphenated().to_string()),
        device_registered: true,
        registration_token_issued: !registration.registration_token.is_empty(),
        entitlement_cached,
        entitlement_error,
        error: None,
    })
}

async fn cache_entitlement_from_access_token(
    client: &CloudClient,
    access_token: &str,
) -> (bool, Option<String>) {
    match client.fetch_entitlement_token(access_token).await {
        Ok(entitlement) => {
            match store_entitlement_response(entitlement_cache_path(), &entitlement).await {
                Ok(_) => (true, None),
                Err(error) => (false, Some(error.to_string())),
            }
        }
        Err(error) => (false, Some(error.to_string())),
    }
}

fn cloud_client(
    state: &AppState,
) -> Result<CloudClient, hypercolor_cloud_client::CloudClientError> {
    CloudClientConfig::try_from(&cloud_config(state)).map(CloudClient::new)
}

fn cloud_config(state: &AppState) -> CloudConfig {
    state
        .config_manager
        .as_ref()
        .map(|manager| manager.get().cloud.clone())
        .unwrap_or_default()
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

const fn token_error_code(error: cloud_api::DeviceTokenErrorCode) -> &'static str {
    match error {
        cloud_api::DeviceTokenErrorCode::AuthorizationPending => "authorization_pending",
        cloud_api::DeviceTokenErrorCode::SlowDown => "slow_down",
        cloud_api::DeviceTokenErrorCode::ExpiredToken => "expired_token",
        cloud_api::DeviceTokenErrorCode::AccessDenied => "access_denied",
        cloud_api::DeviceTokenErrorCode::InvalidGrant => "invalid_grant",
        cloud_api::DeviceTokenErrorCode::Unknown => "unknown",
    }
}
