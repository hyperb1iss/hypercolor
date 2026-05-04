use reqwest::Url;

use crate::CloudClientError;

pub const DEFAULT_AUTH_BASE_URL: &str = "https://hypercolor.lighting";
pub const DEFAULT_DEVICE_CLIENT_ID: &str = "hypercolor-daemon";
pub const DEFAULT_DEVICE_SCOPE: &str = "openid profile email";
pub const DEVICE_CODE_PATH: &str = "/api/auth/device/code";
pub const DEVICE_TOKEN_PATH: &str = "/api/auth/device/token";

#[derive(Debug, Clone)]
pub struct CloudClientConfig {
    base_url: Url,
    auth_base_url: Url,
    device_client_id: String,
    device_scope: String,
}

impl CloudClientConfig {
    pub fn new(base_url: impl AsRef<str>) -> Result<Self, CloudClientError> {
        Self::with_auth_base_url(base_url, DEFAULT_AUTH_BASE_URL)
    }

    pub fn with_auth_base_url(
        base_url: impl AsRef<str>,
        auth_base_url: impl AsRef<str>,
    ) -> Result<Self, CloudClientError> {
        Ok(Self {
            base_url: Url::parse(base_url.as_ref())
                .map_err(|error| CloudClientError::InvalidBaseUrl(error.to_string()))?,
            auth_base_url: Url::parse(auth_base_url.as_ref())
                .map_err(|error| CloudClientError::InvalidBaseUrl(error.to_string()))?,
            device_client_id: DEFAULT_DEVICE_CLIENT_ID.to_owned(),
            device_scope: DEFAULT_DEVICE_SCOPE.to_owned(),
        })
    }

    #[must_use]
    pub fn with_device_client(
        mut self,
        client_id: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        self.device_client_id = client_id.into();
        self.device_scope = scope.into();
        self
    }

    #[must_use]
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    #[must_use]
    pub fn auth_base_url(&self) -> &Url {
        &self.auth_base_url
    }

    #[must_use]
    pub fn device_client_id(&self) -> &str {
        &self.device_client_id
    }

    #[must_use]
    pub fn device_scope(&self) -> &str {
        &self.device_scope
    }

    pub fn api_url(&self, path: &str) -> Result<Url, CloudClientError> {
        self.base_url
            .join(path)
            .map_err(|error| CloudClientError::InvalidBaseUrl(error.to_string()))
    }

    pub fn auth_url(&self, path: &str) -> Result<Url, CloudClientError> {
        self.auth_base_url
            .join(path)
            .map_err(|error| CloudClientError::InvalidBaseUrl(error.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct CloudClient {
    config: CloudClientConfig,
    http_client: reqwest::Client,
}

impl CloudClient {
    #[must_use]
    pub fn new(config: CloudClientConfig) -> Self {
        Self::with_http_client(config, reqwest::Client::new())
    }

    #[must_use]
    pub const fn with_http_client(config: CloudClientConfig, http_client: reqwest::Client) -> Self {
        Self {
            config,
            http_client,
        }
    }

    #[must_use]
    pub const fn config(&self) -> &CloudClientConfig {
        &self.config
    }

    #[must_use]
    pub const fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
}
