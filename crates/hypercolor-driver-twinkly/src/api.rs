//! Local REST API client: gestalt, firmware version, the login and verify
//! handshake, LED mode, and LED layout (Spec 92 §4 and §5).

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::{HTTP_PORT, RT_PORT, TOKEN_LEN};

/// Application code every successful reply carries.
pub const CODE_OK: i64 = 1000;
/// Header that carries the authentication token.
pub const AUTH_HEADER: &str = "X-Auth-Token";
/// Bytes of random challenge sent with a login.
pub const CHALLENGE_LEN: usize = 32;
/// Default timeout for one REST request.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Where one device's REST API and real-time listener live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TwinklyEndpoint {
    pub ip: IpAddr,
    pub http_port: u16,
    pub rt_port: u16,
}

impl TwinklyEndpoint {
    /// Endpoint on the standard ports.
    #[must_use]
    pub const fn new(ip: IpAddr) -> Self {
        Self {
            ip,
            http_port: HTTP_PORT,
            rt_port: RT_PORT,
        }
    }

    /// REST API socket address.
    #[must_use]
    pub const fn http_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.http_port)
    }

    /// Real-time UDP socket address.
    #[must_use]
    pub const fn rt_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.rt_port)
    }

    /// Base URL for `/xled/v1/` endpoints.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}/xled/v1/", self.http_addr())
    }
}

impl fmt::Display for TwinklyEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.http_addr())
    }
}

/// Errors from the REST API.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Twinkly request to {endpoint} failed: {source}")]
    Transport {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("Twinkly device rejected the authentication token on {endpoint}")]
    Unauthorized { endpoint: String },
    #[error("Twinkly device answered {endpoint} with HTTP {status}")]
    Status { endpoint: String, status: u16 },
    #[error("Twinkly device answered {endpoint} with application code {code}")]
    Code { endpoint: String, code: i64 },
    #[error("Twinkly reply from {endpoint} is malformed: {message}")]
    Malformed { endpoint: String, message: String },
}

impl ApiError {
    /// Whether a fresh login could clear this error.
    #[must_use]
    pub const fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized { .. })
    }
}

/// Device facts from `GET /xled/v1/gestalt` (no authentication).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gestalt {
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub hardware_version: Option<String>,
    #[serde(default)]
    pub bytes_per_led: Option<u32>,
    #[serde(default)]
    pub led_type: Option<u32>,
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub fw_family: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    pub mac: String,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub max_supported_led: Option<u32>,
    pub number_of_led: u32,
    #[serde(default)]
    pub led_profile: Option<String>,
    #[serde(default)]
    pub frame_rate: Option<f64>,
    #[serde(default)]
    pub movie_capacity: Option<u32>,
    #[serde(default)]
    pub code: Option<i64>,
}

/// Reply to `GET /xled/v1/fw/version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareVersionReply {
    pub version: String,
    #[serde(default)]
    pub code: Option<i64>,
}

/// Reply to `POST /xled/v1/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginReply {
    pub authentication_token: String,
    #[serde(default)]
    pub authentication_token_expires_in: Option<u64>,
    #[serde(rename = "challenge-response")]
    pub challenge_response: String,
    #[serde(default)]
    pub code: Option<i64>,
}

/// Reply to `GET /xled/v1/led/mode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeReply {
    pub mode: String,
    #[serde(default)]
    pub code: Option<i64>,
}

/// One LED position from the layout.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutCoordinate {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub z: f64,
}

/// Reply to `GET /xled/v1/led/layout/full`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutReply {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub synthesized: Option<bool>,
    #[serde(default)]
    pub coordinates: Vec<LayoutCoordinate>,
    #[serde(default)]
    pub code: Option<i64>,
}

/// A verified authentication token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthToken {
    /// Base64 text sent in the `X-Auth-Token` header.
    pub text: String,
    /// Decoded bytes placed in every real-time datagram.
    pub bytes: [u8; TOKEN_LEN],
    /// Lifetime the device granted, when it said.
    pub expires_in: Option<Duration>,
}

impl AuthToken {
    /// Decode a token returned by login.
    ///
    /// # Errors
    ///
    /// Returns a description when the text is not base64 of eight bytes.
    pub fn decode(text: &str, expires_in: Option<Duration>) -> Result<Self, String> {
        let decoded = STANDARD
            .decode(text.trim())
            .map_err(|error| format!("token is not base64: {error}"))?;
        let bytes: [u8; TOKEN_LEN] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| format!("token decodes to {} bytes, not 8", decoded.len()))?;
        Ok(Self {
            text: text.trim().to_owned(),
            bytes,
            expires_in,
        })
    }
}

/// JSON body of a login request.
#[must_use]
pub fn login_request_body(challenge: &[u8; CHALLENGE_LEN]) -> serde_json::Value {
    serde_json::json!({ "challenge": STANDARD.encode(challenge) })
}

/// JSON body of a verify request.
#[must_use]
pub fn verify_request_body(challenge_response: &str) -> serde_json::Value {
    serde_json::json!({ "challenge-response": challenge_response })
}

/// JSON body of a mode change.
#[must_use]
pub fn mode_request_body(mode: &str) -> serde_json::Value {
    serde_json::json!({ "mode": mode })
}

/// REST client for one device.
#[derive(Debug, Clone)]
pub struct TwinklyClient {
    http: reqwest::Client,
    endpoint: TwinklyEndpoint,
    base_url: String,
}

impl TwinklyClient {
    /// Client for one endpoint with a per-request timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(endpoint: TwinklyEndpoint, timeout: Duration) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .no_proxy()
            .build()
            .map_err(|source| ApiError::Transport {
                endpoint: endpoint.base_url(),
                source,
            })?;
        Ok(Self {
            http,
            base_url: endpoint.base_url(),
            endpoint,
        })
    }

    #[must_use]
    pub const fn endpoint(&self) -> TwinklyEndpoint {
        self.endpoint
    }

    /// Read the gestalt.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the reply is malformed.
    pub async fn gestalt(&self) -> Result<Gestalt, ApiError> {
        self.get("gestalt", None).await
    }

    /// Read the firmware version string.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the reply is malformed.
    pub async fn firmware_version(&self) -> Result<FirmwareVersionReply, ApiError> {
        self.get("fw/version", None).await
    }

    /// Log in and verify, returning a token ready for use.
    ///
    /// A successful login invalidates whatever token the device issued
    /// before, including one held by the Twinkly app.
    ///
    /// # Errors
    ///
    /// Returns an error when either step fails or the token is malformed.
    pub async fn login(&self) -> Result<AuthToken, ApiError> {
        let challenge: [u8; CHALLENGE_LEN] = rand::random();
        let reply: LoginReply = self
            .post("login", None, &login_request_body(&challenge))
            .await?;
        let expires_in = reply
            .authentication_token_expires_in
            .map(Duration::from_secs);
        let token =
            AuthToken::decode(&reply.authentication_token, expires_in).map_err(|message| {
                ApiError::Malformed {
                    endpoint: self.url("login"),
                    message,
                }
            })?;
        let _: serde_json::Value = self
            .post(
                "verify",
                Some(&token.text),
                &verify_request_body(&reply.challenge_response),
            )
            .await?;
        Ok(token)
    }

    /// Read the current LED mode.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Unauthorized`] when the token is no longer valid.
    pub async fn mode(&self, token: &AuthToken) -> Result<ModeReply, ApiError> {
        self.get("led/mode", Some(&token.text)).await
    }

    /// Change the LED mode.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Unauthorized`] when the token is no longer valid.
    pub async fn set_mode(&self, token: &AuthToken, mode: &str) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .post("led/mode", Some(&token.text), &mode_request_body(mode))
            .await?;
        Ok(())
    }

    /// Read the LED layout.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Unauthorized`] when the token is no longer valid.
    pub async fn layout(&self, token: &AuthToken) -> Result<LayoutReply, ApiError> {
        self.get("led/layout/full", Some(&token.text)).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        token: Option<&str>,
    ) -> Result<T, ApiError> {
        let url = self.url(path);
        let mut request = self.http.get(&url);
        if let Some(token) = token {
            request = request.header(AUTH_HEADER, token);
        }
        let response = request.send().await.map_err(|source| ApiError::Transport {
            endpoint: url.clone(),
            source,
        })?;
        Self::decode(url, response).await
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        token: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<T, ApiError> {
        let url = self.url(path);
        let mut request = self.http.post(&url).json(body);
        if let Some(token) = token {
            request = request.header(AUTH_HEADER, token);
        }
        let response = request.send().await.map_err(|source| ApiError::Transport {
            endpoint: url.clone(),
            source,
        })?;
        Self::decode(url, response).await
    }

    async fn decode<T: DeserializeOwned>(
        url: String,
        response: reqwest::Response,
    ) -> Result<T, ApiError> {
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized { endpoint: url });
        }
        if !status.is_success() {
            return Err(ApiError::Status {
                endpoint: url,
                status: status.as_u16(),
            });
        }
        let body = response
            .bytes()
            .await
            .map_err(|source| ApiError::Transport {
                endpoint: url.clone(),
                source,
            })?;
        decode_reply(&url, &body)
    }
}

/// Decode a JSON reply and check its application code.
///
/// # Errors
///
/// Returns [`ApiError::Malformed`] for bad JSON and [`ApiError::Code`] when
/// the reply carries a code other than 1000.
pub fn decode_reply<T: DeserializeOwned>(endpoint: &str, body: &[u8]) -> Result<T, ApiError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|error| ApiError::Malformed {
            endpoint: endpoint.to_owned(),
            message: error.to_string(),
        })?;
    if let Some(code) = value.get("code").and_then(serde_json::Value::as_i64)
        && code != CODE_OK
    {
        return Err(ApiError::Code {
            endpoint: endpoint.to_owned(),
            code,
        });
    }
    serde_json::from_value(value).map_err(|error| ApiError::Malformed {
        endpoint: endpoint.to_owned(),
        message: error.to_string(),
    })
}
