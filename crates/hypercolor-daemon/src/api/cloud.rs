//! Hypercolor Cloud endpoints.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::Response;
use hypercolor_cloud_client::api as cloud_api;
use hypercolor_cloud_client::daemon_link::IdentityNonce;
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
use crate::api::envelope::{ApiError, ApiResponse};
use crate::cloud_entitlements::{
    CachedCloudEntitlement, delete_cached_entitlement, entitlement_cache_path,
    iso8601_from_unix_seconds, load_cached_entitlement, store_entitlement_response,
    unix_now_seconds,
};

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
    let cloud_config = state
        .config_manager
        .as_ref()
        .map(|manager| manager.get().cloud.clone())
        .unwrap_or_default();

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
    let cloud_config = state
        .config_manager
        .as_ref()
        .map(|manager| manager.get().cloud.clone())
        .unwrap_or_default();
    CloudClientConfig::try_from(&cloud_config).map(CloudClient::new)
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
