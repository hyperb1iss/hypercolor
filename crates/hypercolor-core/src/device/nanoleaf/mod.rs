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
