use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://developer-api.govee.com/v1/";
const API_KEY_HEADER: &str = "Govee-API-Key";

#[derive(Debug, Clone)]
pub struct CloudClient {
    http: reqwest::Client,
    api_key: String,
    base_url: Url,
}

impl CloudClient {
    /// Create a client for Govee's public Developer API v1.
    ///
    /// # Errors
    ///
    /// Returns an error if the built-in API base URL cannot be parsed.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    /// Create a client with a custom base URL, used by tests and local shims.
    ///
    /// # Errors
    ///
    /// Returns an error if `base_url` is not a valid URL.
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl AsRef<str>) -> Result<Self> {
        let base_url = normalize_base_url(base_url.as_ref())?;

        Ok(Self {
            http: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url,
        })
    }

    /// List lights, plugs, and switches exposed through Govee Developer API v1.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the key is rejected, or Govee
    /// returns a non-success API code.
    pub async fn list_v1_devices(&self) -> Result<Vec<V1Device>> {
        let url = self
            .base_url
            .join("devices")
            .context("failed to build Govee device-list URL")?;
        let response = self
            .http
            .get(url)
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await
            .context("failed to call Govee device-list API")?;

        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            bail!("Govee rejected the API key");
        }

        let envelope: V1Envelope<V1DevicesData> = response
            .error_for_status()
            .context("Govee device-list API returned an HTTP error")?
            .json()
            .await
            .context("failed to parse Govee device-list response")?;

        if envelope.code != 200 {
            let message = envelope
                .message
                .filter(|message| !message.trim().is_empty())
                .unwrap_or_else(|| "unknown error".to_owned());
            bail!("Govee API returned code {}: {message}", envelope.code);
        }

        Ok(envelope
            .data
            .map_or_else(Vec::new, |data| data.devices))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct V1Device {
    pub device: String,
    pub model: String,
    #[serde(rename = "deviceName")]
    pub device_name: String,
    pub controllable: bool,
    pub retrievable: bool,
    #[serde(default, rename = "supportCmds")]
    pub support_cmds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct V1Envelope<T> {
    code: i64,
    message: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct V1DevicesData {
    #[serde(default)]
    devices: Vec<V1Device>,
}

fn normalize_base_url(base_url: &str) -> Result<Url> {
    let normalized = if base_url.ends_with('/') {
        base_url.to_owned()
    } else {
        format!("{base_url}/")
    };

    Url::parse(&normalized).context("invalid Govee API base URL")
}
