//! Hue bridge discovery and CLIP API client.

use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use super::color::ColorGamut;
use super::types::{
    HueBridgeIdentity, HueChannel, HueChannelMember, HueEntertainmentConfig, HueEntertainmentType,
    HueLight, HuePairResult, HuePosition,
};

/// Default HTTPS API port exposed by Hue bridges.
pub const DEFAULT_HUE_API_PORT: u16 = 443;

/// Default Hue entertainment DTLS port.
pub const DEFAULT_HUE_STREAM_PORT: u16 = 2_100;

const HUE_DISCOVERY_URL: &str = "https://discovery.meethue.com";
const HUE_APPLICATION_KEY_HEADER: &str = "hue-application-key";

static HUE_HTTPS_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|error| error.to_string())
});

static HUE_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
});

/// Public N-UPnP bridge entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HueNupnpBridge {
    pub bridge_id: String,
    pub ip: IpAddr,
}

/// CLIP bridge client with automatic HTTPS → HTTP fallback.
pub struct HueBridgeClient {
    ip: IpAddr,
    api_port: u16,
    api_key: Option<String>,
}

impl HueBridgeClient {
    /// Create an unauthenticated client for bridge discovery and pairing.
    #[must_use]
    pub fn new(ip: IpAddr) -> Self {
        Self::with_port(ip, DEFAULT_HUE_API_PORT)
    }

    /// Create an authenticated client with a stored application key.
    #[must_use]
    pub fn authenticated(ip: IpAddr, api_key: String) -> Self {
        Self::authenticated_with_port(ip, DEFAULT_HUE_API_PORT, api_key)
    }

    /// Create an unauthenticated client with an explicit API port.
    #[must_use]
    pub fn with_port(ip: IpAddr, api_port: u16) -> Self {
        Self {
            ip,
            api_port,
            api_key: None,
        }
    }

    /// Create an authenticated client with an explicit API port.
    #[must_use]
    pub fn authenticated_with_port(ip: IpAddr, api_port: u16, api_key: String) -> Self {
        Self {
            ip,
            api_port,
            api_key: Some(api_key),
        }
    }

    /// Fetch the public bridge identity from `/api/config`.
    ///
    /// # Errors
    ///
    /// Returns an error if the bridge is unreachable or the response is
    /// missing the required identity fields.
    pub async fn bridge_identity(&self) -> Result<HueBridgeIdentity> {
        let json = self
            .request_value(Method::GET, "/api/config", false, None)
            .await?;

        Ok(HueBridgeIdentity {
            bridge_id: required_str(&json, "bridgeid")?.to_ascii_lowercase(),
            name: optional_str(&json, "name").unwrap_or_default(),
            model_id: optional_str(&json, "modelid").unwrap_or_default(),
            sw_version: optional_str(&json, "swversion").unwrap_or_default(),
        })
    }

    /// Attempt to pair with the bridge.
    ///
    /// Returns `Ok(None)` when the physical link button has not been pressed yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the bridge rejects the request for any reason other
    /// than waiting for the link button, or if the success payload is malformed.
    pub async fn pair_with_status(&self, app_name: &str) -> Result<Option<HuePairResult>> {
        let request_body = json!({
            "devicetype": app_name,
            "generateclientkey": true,
        });
        let json = self
            .request_value(Method::POST, "/api", false, Some(request_body))
            .await?;

        let Some(entries) = json.as_array() else {
            bail!("Hue bridge pairing response must be an array");
        };
        let Some(entry) = entries.first() else {
            bail!("Hue bridge pairing response was empty");
        };

        if let Some(error) = entry.get("error") {
            let error_type = error
                .get("type")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            if error_type == 101 {
                return Ok(None);
            }
            let description = error
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("unknown Hue pairing error");
            bail!("Hue bridge pairing failed: {description}");
        }

        let Some(success) = entry.get("success") else {
            bail!("Hue bridge pairing response did not include success payload");
        };

        Ok(Some(HuePairResult {
            api_key: required_str(success, "username")?.to_owned(),
            client_key: required_str(success, "clientkey")?.to_owned(),
        }))
    }

    /// Pair with the bridge and require immediate success.
    ///
    /// # Errors
    ///
    /// Returns an error if the bridge is still waiting for the user to press
    /// the link button or if the pairing flow fails.
    pub async fn pair(&self, app_name: &str) -> Result<HuePairResult> {
        self.pair_with_status(app_name)
            .await?
            .ok_or_else(|| anyhow!("Hue bridge is waiting for the link button to be pressed"))
    }

    /// Fetch all light resources visible to the application key.
    ///
    /// # Errors
    ///
    /// Returns an error if authentication fails or the CLIP v2 response cannot
    /// be parsed into light metadata.
    pub async fn lights(&self) -> Result<Vec<HueLight>> {
        let json = self
            .request_value(Method::GET, "/clip/v2/resource/light", true, None)
            .await?;
        let Some(items) = json.get("data").and_then(Value::as_array) else {
            bail!("Hue lights response did not include a data array");
        };

        items.iter().map(parse_light).collect()
    }

    /// Fetch the entertainment configurations available on the bridge.
    ///
    /// # Errors
    ///
    /// Returns an error if authentication fails or the CLIP v2 response is malformed.
    pub async fn entertainment_configs(&self) -> Result<Vec<HueEntertainmentConfig>> {
        let json = self
            .request_value(
                Method::GET,
                "/clip/v2/resource/entertainment_configuration",
                true,
                None,
            )
            .await?;
        let Some(items) = json.get("data").and_then(Value::as_array) else {
            bail!("Hue entertainment response did not include a data array");
        };

        items.iter().map(parse_entertainment_config).collect()
    }

    /// Activate entertainment streaming for one configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the bridge rejects the action.
    pub async fn start_streaming(&self, config_id: &str) -> Result<()> {
        self.request_empty(
            Method::PUT,
            &format!("/clip/v2/resource/entertainment_configuration/{config_id}"),
            true,
            Some(json!({ "action": "start" })),
        )
        .await
    }

    /// Deactivate entertainment streaming for one configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the bridge rejects the action.
    pub async fn stop_streaming(&self, config_id: &str) -> Result<()> {
        self.request_empty(
            Method::PUT,
            &format!("/clip/v2/resource/entertainment_configuration/{config_id}"),
            true,
            Some(json!({ "action": "stop" })),
        )
        .await
    }

    /// Fetch public N-UPnP bridge listings.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery service cannot be reached or emits an
    /// invalid payload.
    pub async fn discover_bridges() -> Result<Vec<HueNupnpBridge>> {
        Self::discover_bridges_with_url(HUE_DISCOVERY_URL).await
    }

    /// Fetch N-UPnP bridge listings from a custom endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the discovery service cannot be reached or emits an
    /// invalid payload.
    pub async fn discover_bridges_with_url(url: &str) -> Result<Vec<HueNupnpBridge>> {
        let client = hue_http_client()?;
        let entries: Vec<HueNupnpEntry> = client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("Hue N-UPnP request to {url} failed"))?
            .json()
            .await
            .with_context(|| format!("failed to parse Hue N-UPnP response from {url}"))?;

        entries
            .into_iter()
            .map(|entry| {
                Ok(HueNupnpBridge {
                    bridge_id: entry.id.to_ascii_lowercase(),
                    ip: entry.internalipaddress.parse().with_context(|| {
                        format!("invalid Hue bridge IP {}", entry.internalipaddress)
                    })?,
                })
            })
            .collect()
    }

    async fn request_empty(
        &self,
        method: Method,
        path: &str,
        authenticated: bool,
        body: Option<Value>,
    ) -> Result<()> {
        let _ = self
            .request_response(method, path, authenticated, body)
            .await?
            .bytes()
            .await
            .with_context(|| format!("failed to consume Hue response body for {path}"))?;
        Ok(())
    }

    async fn request_value(
        &self,
        method: Method,
        path: &str,
        authenticated: bool,
        body: Option<Value>,
    ) -> Result<Value> {
        self.request_response(method, path, authenticated, body)
            .await?
            .json()
            .await
            .with_context(|| format!("failed to parse Hue JSON response for {path}"))
    }

    async fn request_response(
        &self,
        method: Method,
        path: &str,
        authenticated: bool,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        let secure_client = hue_https_client()?;
        let fallback_client = hue_http_client()?;
        let schemes: [(&str, &reqwest::Client); 2] =
            if self.ip.is_loopback() || self.api_port != DEFAULT_HUE_API_PORT {
                [("http", fallback_client), ("https", secure_client)]
            } else {
                [("https", secure_client), ("http", fallback_client)]
            };

        let mut last_error = None;
        for (scheme, client) in schemes {
            let url = format!("{scheme}://{}:{}{path}", self.ip, self.api_port);
            let mut request = client.request(method.clone(), &url);
            if authenticated {
                let api_key = self
                    .api_key
                    .as_deref()
                    .ok_or_else(|| anyhow!("Hue request to {path} requires an application key"))?;
                request = request.header(HUE_APPLICATION_KEY_HEADER, api_key);
            }
            if let Some(body) = body.as_ref() {
                request = request.json(body);
            }

            match request.send().await {
                Ok(response) => {
                    return response
                        .error_for_status()
                        .with_context(|| format!("Hue request to {url} failed"));
                }
                Err(error) => last_error = Some(anyhow!("Hue request to {url} failed: {error}")),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Hue request {path} failed")))
    }
}

fn hue_https_client() -> Result<&'static reqwest::Client> {
    HUE_HTTPS_CLIENT
        .as_ref()
        .map_err(|error| anyhow!("failed to build Hue HTTPS client: {error}"))
}

fn hue_http_client() -> Result<&'static reqwest::Client> {
    HUE_HTTP_CLIENT
        .as_ref()
        .map_err(|error| anyhow!("failed to build Hue HTTP client: {error}"))
}

fn parse_light(raw: &Value) -> Result<HueLight> {
    Ok(HueLight {
        id: required_str(raw, "id")?.to_owned(),
        name: raw
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or("Hue Light")
            .to_owned(),
        model_id: raw
            .pointer("/product_data/model_id")
            .or_else(|| raw.pointer("/product_data/product_name"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        gamut_type: raw
            .pointer("/color/gamut_type")
            .or_else(|| raw.pointer("/color/gamut/gamut_type"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        gamut: parse_gamut(raw.pointer("/color/gamut")),
    })
}

fn parse_entertainment_config(raw: &Value) -> Result<HueEntertainmentConfig> {
    let Some(channels) = raw.get("channels").and_then(Value::as_array) else {
        bail!("Hue entertainment configuration is missing channels");
    };

    Ok(HueEntertainmentConfig {
        id: required_str(raw, "id")?.to_owned(),
        name: raw
            .pointer("/metadata/name")
            .or_else(|| raw.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Hue Entertainment")
            .to_owned(),
        config_type: raw
            .get("configuration_type")
            .and_then(Value::as_str)
            .map_or(HueEntertainmentType::Other, HueEntertainmentType::from),
        channels: channels
            .iter()
            .map(parse_channel)
            .collect::<Result<Vec<_>>>()?,
    })
}

fn parse_channel(raw: &Value) -> Result<HueChannel> {
    let id = raw
        .get("channel_id")
        .or_else(|| raw.get("id"))
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("Hue entertainment channel is missing a channel_id"))?;
    let members = raw
        .get("members")
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .map(parse_channel_member)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(HueChannel {
        id: u8::try_from(id).context("Hue channel ID exceeded u8 range")?,
        name: raw
            .pointer("/metadata/name")
            .or_else(|| raw.get("name"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("Channel {id}"), ToOwned::to_owned),
        position: HuePosition {
            x: raw
                .pointer("/position/x")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            y: raw
                .pointer("/position/y")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            z: raw
                .pointer("/position/z")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
        },
        segment_count: raw
            .get("segments")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| u32::try_from(members.len()).ok())
            .unwrap_or(1)
            .max(1),
        members,
    })
}

fn parse_channel_member(raw: &Value) -> Result<HueChannelMember> {
    let light_id = raw
        .pointer("/service/rid")
        .or_else(|| raw.get("rid"))
        .or_else(|| raw.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| light_id.clone())
        .ok_or_else(|| anyhow!("Hue entertainment member is missing an identifier"))?;

    Ok(HueChannelMember { id, light_id })
}

fn parse_gamut(raw: Option<&Value>) -> Option<ColorGamut> {
    let raw = raw?;
    Some(ColorGamut {
        red: (
            raw.pointer("/red/x")?.as_f64()?,
            raw.pointer("/red/y")?.as_f64()?,
        ),
        green: (
            raw.pointer("/green/x")?.as_f64()?,
            raw.pointer("/green/y")?.as_f64()?,
        ),
        blue: (
            raw.pointer("/blue/x")?.as_f64()?,
            raw.pointer("/blue/y")?.as_f64()?,
        ),
    })
}

fn required_str<'a>(json: &'a Value, key: &str) -> Result<&'a str> {
    json.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Hue response is missing required string field '{key}'"))
}

fn optional_str(json: &Value, key: &str) -> Option<String> {
    json.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Deserialize)]
struct HueNupnpEntry {
    id: String,
    internalipaddress: String,
}

impl From<&str> for HueEntertainmentType {
    fn from(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "screen" => Self::Screen,
            "monitor" => Self::Monitor,
            "music" => Self::Music,
            "3dspace" => Self::ThreeDSpace,
            _ => Self::Other,
        }
    }
}
