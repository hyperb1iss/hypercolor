//! Hypercolor Cloud endpoints.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::Response;
use hypercolor_cloud_client::api as cloud_api;
use hypercolor_cloud_client::daemon_link::IdentityNonce;
use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, DeviceAuthorizationStatus, DeviceRegistrationInput,
    KeyringSecretStore, RefreshTokenOwner, load_or_create_identity, persist_device_token,
    signed_device_registration,
};
use hypercolor_types::config::CloudConfig;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

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
    pub error: Option<CloudLoginError>,
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

pub async fn start_login(State(state): State<Arc<AppState>>) -> Response {
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
            error: Some(CloudLoginError::from_token_error(error)),
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
        error: None,
    })
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
