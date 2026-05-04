use std::time::{Duration, Instant};

use hypercolor_cloud_api::{
    DeviceCodeRequest, DeviceCodeResponse, DeviceTokenError, DeviceTokenErrorCode,
    DeviceTokenRequest, DeviceTokenResponse,
};

use crate::config::{DEVICE_CODE_PATH, DEVICE_TOKEN_PATH};
use crate::{CloudClient, CloudClientError, RefreshTokenOwner, SecretStore, store_refresh_token};

pub const DEFAULT_DEVICE_AUTHORIZATION_POLL_INTERVAL: Duration = Duration::from_secs(5);
pub const SLOW_DOWN_POLL_INTERVAL_STEP: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    Authorized(DeviceTokenResponse),
    Waiting(DeviceTokenError),
}

impl DeviceTokenPoll {
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::Authorized(_) => false,
            Self::Waiting(error) => error.error.is_retryable(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceAuthorizationSession {
    response: DeviceCodeResponse,
    started_at: Instant,
    poll_interval: Duration,
}

impl DeviceAuthorizationSession {
    #[must_use]
    pub fn new(response: DeviceCodeResponse) -> Self {
        let poll_interval = response
            .interval
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_DEVICE_AUTHORIZATION_POLL_INTERVAL)
            .max(Duration::from_secs(1));

        Self {
            response,
            started_at: Instant::now(),
            poll_interval,
        }
    }

    #[must_use]
    pub const fn response(&self) -> &DeviceCodeResponse {
        &self.response
    }

    #[must_use]
    pub fn device_code(&self) -> &str {
        &self.response.device_code
    }

    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.response.user_code
    }

    #[must_use]
    pub fn verification_uri(&self) -> &str {
        &self.response.verification_uri
    }

    #[must_use]
    pub fn verification_uri_complete(&self) -> Option<&str> {
        self.response.verification_uri_complete.as_deref()
    }

    #[must_use]
    pub const fn started_at(&self) -> Instant {
        self.started_at
    }

    #[must_use]
    pub const fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    #[must_use]
    pub fn expires_at(&self) -> Instant {
        self.started_at + Duration::from_secs(self.response.expires_in)
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.is_expired_at(Instant::now())
    }

    #[must_use]
    pub fn is_expired_at(&self, now: Instant) -> bool {
        now >= self.expires_at()
    }

    #[must_use]
    pub fn record_poll_result(&mut self, poll: DeviceTokenPoll) -> DeviceAuthorizationStatus {
        match poll {
            DeviceTokenPoll::Authorized(response) => {
                DeviceAuthorizationStatus::Authorized(response)
            }
            DeviceTokenPoll::Waiting(error) => self.record_waiting_error(error),
        }
    }

    fn record_waiting_error(&mut self, error: DeviceTokenError) -> DeviceAuthorizationStatus {
        match error.error {
            DeviceTokenErrorCode::AuthorizationPending => DeviceAuthorizationStatus::Pending {
                error,
                retry_after: self.poll_interval,
            },
            DeviceTokenErrorCode::SlowDown => {
                self.poll_interval += SLOW_DOWN_POLL_INTERVAL_STEP;
                DeviceAuthorizationStatus::Pending {
                    error,
                    retry_after: self.poll_interval,
                }
            }
            DeviceTokenErrorCode::ExpiredToken => DeviceAuthorizationStatus::Expired(error),
            DeviceTokenErrorCode::AccessDenied
            | DeviceTokenErrorCode::InvalidGrant
            | DeviceTokenErrorCode::Unknown => DeviceAuthorizationStatus::Rejected(error),
        }
    }

    fn local_expired_status() -> DeviceAuthorizationStatus {
        DeviceAuthorizationStatus::Expired(DeviceTokenError {
            error: DeviceTokenErrorCode::ExpiredToken,
            error_description: Some("device authorization expired before approval".to_owned()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceAuthorizationStatus {
    Authorized(DeviceTokenResponse),
    Pending {
        error: DeviceTokenError,
        retry_after: Duration,
    },
    Expired(DeviceTokenError),
    Rejected(DeviceTokenError),
}

impl DeviceAuthorizationStatus {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending { .. })
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Pending { retry_after, .. } => Some(*retry_after),
            Self::Authorized(_) | Self::Expired(_) | Self::Rejected(_) => None,
        }
    }
}

pub fn persist_device_token(
    store: &impl SecretStore,
    owner: RefreshTokenOwner,
    response: &DeviceTokenResponse,
) -> Result<bool, CloudClientError> {
    let Some(refresh_token) = response.refresh_token.as_deref() else {
        return Ok(false);
    };

    store_refresh_token(store, owner, refresh_token)?;
    Ok(true)
}

impl CloudClient {
    pub async fn start_device_authorization(&self) -> Result<DeviceCodeResponse, CloudClientError> {
        let request = DeviceCodeRequest::new(
            self.config().device_client_id(),
            self.config().device_scope(),
        );
        let response = self
            .http_client()
            .post(self.config().auth_url(DEVICE_CODE_PATH)?)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json::<DeviceCodeResponse>()
            .await?;

        Ok(response)
    }

    pub async fn begin_device_authorization(
        &self,
    ) -> Result<DeviceAuthorizationSession, CloudClientError> {
        self.start_device_authorization()
            .await
            .map(DeviceAuthorizationSession::new)
    }

    pub async fn poll_device_token(
        &self,
        device_code: impl Into<String>,
    ) -> Result<DeviceTokenPoll, CloudClientError> {
        let request = DeviceTokenRequest::new(device_code, self.config().device_client_id());
        let response = self
            .http_client()
            .post(self.config().auth_url(DEVICE_TOKEN_PATH)?)
            .json(&request)
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(DeviceTokenPoll::Authorized(
                response.json::<DeviceTokenResponse>().await?,
            ));
        }

        Ok(DeviceTokenPoll::Waiting(
            response.json::<DeviceTokenError>().await?,
        ))
    }

    pub async fn poll_device_authorization(
        &self,
        session: &mut DeviceAuthorizationSession,
    ) -> Result<DeviceAuthorizationStatus, CloudClientError> {
        if session.is_expired() {
            return Ok(DeviceAuthorizationSession::local_expired_status());
        }

        let poll = self.poll_device_token(session.device_code()).await?;
        Ok(session.record_poll_result(poll))
    }
}
