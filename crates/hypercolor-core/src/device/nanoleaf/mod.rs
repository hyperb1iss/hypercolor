//! Nanoleaf network backend.

pub mod backend;
mod scanner;
mod streaming;
mod topology;
mod types;

pub use backend::NanoleafBackend;
pub use scanner::{NanoleafKnownDevice, NanoleafScanner};
pub use streaming::{
    DEFAULT_NANOLEAF_API_PORT, DEFAULT_NANOLEAF_STREAM_PORT, NanoleafStreamSession,
    encode_frame_into,
};
pub use topology::NanoleafShapeType;
pub use types::{NanoleafDeviceInfo, NanoleafDiscoveredDevice, NanoleafPanelLayout};
#[doc(hidden)]
pub use types::{build_device_info, panel_ids_from_layout};

use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::Deserialize;

use self::types::NanoleafPanelLayoutResponse;

static NANOLEAF_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
});

fn nanoleaf_http_client() -> Result<&'static reqwest::Client> {
    NANOLEAF_HTTP_CLIENT
        .as_ref()
        .map_err(|error| anyhow::anyhow!("failed to build shared Nanoleaf HTTP client: {error}"))
}

/// Result of a successful Nanoleaf pairing attempt.
#[derive(Debug, Clone)]
pub struct NanoleafPairResult {
    pub auth_token: String,
    pub device_key: String,
    pub name: String,
    pub model: String,
    pub firmware_version: String,
    pub serial_no: String,
}

/// Attempt to pair with a Nanoleaf device.
///
/// Returns `Ok(None)` when the device is not in pairing mode.
///
/// # Errors
///
/// Returns an error if the pairing request fails or the device-info fetch after
/// pairing is malformed.
pub async fn pair_device_with_status(
    ip: IpAddr,
    api_port: u16,
) -> Result<Option<NanoleafPairResult>> {
    let url = format!("http://{ip}:{api_port}/api/v1/new");
    let client = nanoleaf_http_client()?;
    let response = client
        .post(&url)
        .send()
        .await
        .with_context(|| format!("Nanoleaf pairing request to {url} failed"))?;
    if matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
    ) {
        return Ok(None);
    }

    let response = response
        .error_for_status()
        .with_context(|| format!("Nanoleaf pairing request to {url} failed"))?;
    let payload: NanoleafPairResponse = response
        .json()
        .await
        .with_context(|| format!("failed to parse Nanoleaf pairing response from {url}"))?;
    let device_info = fetch_device_info(ip, api_port, &payload.auth_token).await?;
    let device_key = normalized_device_key(&device_info, ip);

    Ok(Some(NanoleafPairResult {
        auth_token: payload.auth_token,
        device_key,
        name: device_info.name,
        model: device_info.model,
        firmware_version: device_info.firmware_version,
        serial_no: device_info.serial_no,
    }))
}

/// Pair with a Nanoleaf device and require immediate success.
///
/// # Errors
///
/// Returns an error if the device is not in pairing mode or if the pairing
/// flow fails.
pub async fn pair_device(ip: IpAddr, api_port: u16) -> Result<NanoleafPairResult> {
    pair_device_with_status(ip, api_port)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Nanoleaf device is waiting for the pairing button hold"))
}

async fn fetch_device_info(
    ip: IpAddr,
    api_port: u16,
    auth_token: &str,
) -> Result<NanoleafDeviceInfo> {
    let url = format!("http://{ip}:{api_port}/api/v1/{auth_token}");
    let client = nanoleaf_http_client()?;

    client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Nanoleaf device-info request to {url} failed"))?
        .json()
        .await
        .with_context(|| format!("failed to parse Nanoleaf device-info response from {url}"))
}

async fn fetch_panel_layout(
    ip: IpAddr,
    api_port: u16,
    auth_token: &str,
) -> Result<NanoleafPanelLayoutResponse> {
    let url = format!("http://{ip}:{api_port}/api/v1/{auth_token}/panelLayout/layout");
    let client = nanoleaf_http_client()?;

    client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Nanoleaf panel-layout request to {url} failed"))?
        .json()
        .await
        .with_context(|| format!("failed to parse Nanoleaf panel-layout response from {url}"))
}

#[derive(Debug, Deserialize)]
struct NanoleafPairResponse {
    auth_token: String,
}

fn normalized_device_key(device_info: &NanoleafDeviceInfo, ip: IpAddr) -> String {
    if !device_info.serial_no.trim().is_empty() {
        return device_info.serial_no.trim().to_ascii_lowercase();
    }
    if !device_info.name.trim().is_empty() {
        return device_info
            .name
            .trim()
            .to_ascii_lowercase()
            .replace(' ', "-");
    }
    format!("ip:{ip}")
}
