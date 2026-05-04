use serde::{Deserialize, Serialize};

pub const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCodeRequest {
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl DeviceCodeRequest {
    #[must_use]
    pub fn new(client_id: impl Into<String>, scope: impl Into<String>) -> Self {
        let scope = scope.into();
        Self {
            client_id: client_id.into(),
            scope: (!scope.is_empty()).then_some(scope),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default)]
    pub interval: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTokenRequest {
    pub grant_type: String,
    pub device_code: String,
    pub client_id: String,
}

impl DeviceTokenRequest {
    #[must_use]
    pub fn new(device_code: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            grant_type: DEVICE_CODE_GRANT_TYPE.to_owned(),
            device_code: device_code.into(),
            client_id: client_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTokenResponse {
    pub access_token: String,
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceTokenError {
    pub error: DeviceTokenErrorCode,
    #[serde(default)]
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTokenErrorCode {
    AuthorizationPending,
    SlowDown,
    ExpiredToken,
    AccessDenied,
    InvalidGrant,
    #[serde(other)]
    Unknown,
}

impl DeviceTokenErrorCode {
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::AuthorizationPending | Self::SlowDown)
    }
}
