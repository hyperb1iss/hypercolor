//! WLED device backend — DDP and E1.31 protocol support for ESP32/ESP8266 controllers.
//!
//! This module provides everything needed to discover, connect to, and stream
//! pixel data to [WLED](https://kno.wled.ge/) devices over the network.
//!
//! Two streaming protocols are supported:
//!
//! - **DDP** (Distributed Display Protocol) — preferred, smaller header, no universe limits
//! - **E1.31/sACN** (Streaming ACN) — fallback for older firmware or DMX interop

pub mod backend;
mod ddp;
mod e131;
mod scanner;

pub use backend::{
    WledBackend, WledColorFormat, WledDevice, WledDeviceInfo, WledProtocol, WledSegmentInfo,
};
pub use ddp::{DdpPacket, DdpSequence, build_ddp_frame};
pub use e131::{
    E131_PIXELS_PER_UNIVERSE_RGB, E131_PIXELS_PER_UNIVERSE_RGBW, E131Packet, E131SequenceTracker,
    universes_needed,
};
pub use scanner::WledScanner;

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};

/// Fetch and parse `/json/info` from a WLED device over HTTP.
///
/// Shared between the backend's `probe_ip` and the scanner's `enrich_via_http`
/// to avoid duplicating HTTP client construction and JSON parsing logic.
async fn fetch_wled_info(ip: IpAddr) -> Result<backend::WledDeviceInfo> {
    let url = format!("http://{ip}/json/info");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("Failed to build HTTP client")?;

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("HTTP request to {url} failed"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .with_context(|| format!("Failed to parse JSON from {url}"))?;

    backend::parse_wled_info(&json)
}
