//! Realtime-mode lifecycle and HTTP health probes for WLED backends.
//!
//! WLED's realtime receiver (UDP DDP/E1.31) will only accept frames while
//! the device is in "realtime mode". This module covers the entry/exit
//! HTTP dance on `/json/state`, the three-frame priming flush, the clear
//! frame sent at disconnect, and the `/json/info` reachability probe used
//! for cheap health checks.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{debug, warn};

use super::protocol::{
    WledColorFormat, WledDevice, WledProtocol, parse_wled_live_receiver_config,
    wled_receiver_config_mismatches,
};

pub(super) const REALTIME_HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const REALTIME_PRIME_FRAMES: usize = 3;
const REALTIME_PRIME_DELAY: Duration = Duration::from_millis(50);

/// Check if a WLED device responds to `/json/info`.
///
/// # Errors
///
/// Returns an error if the HTTP client cannot be built.
pub(super) async fn probe_device_reachable(ip: IpAddr) -> Result<bool> {
    let url = format!("http://{ip}/json/info");
    let client = reqwest::Client::builder()
        .timeout(REALTIME_HTTP_TIMEOUT)
        .build()
        .context("Failed to build WLED HTTP client")?;

    Ok(client.get(url).send().await.is_ok())
}

async fn post_realtime_state(ip: IpAddr, body: serde_json::Value) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(REALTIME_HTTP_TIMEOUT)
        .build()
        .context("Failed to build WLED HTTP client")?;

    client
        .post(format!("http://{ip}/json/state"))
        .json(&body)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Failed to update WLED realtime state for {ip}"))?;

    Ok(())
}

pub(super) async fn enter_realtime_mode(ip: IpAddr) -> Result<()> {
    post_realtime_state(
        ip,
        serde_json::json!({
            // Let the incoming realtime protocol own pixel data instead of
            // forcing WLED's local override state.
            "lor": 0,
            "live": true,
            "transition": 0,
        }),
    )
    .await
}

pub(super) async fn exit_realtime_mode(ip: IpAddr) -> Result<()> {
    post_realtime_state(
        ip,
        serde_json::json!({
            "live": false,
            "lor": 0,
            "transition": 7,
        }),
    )
    .await
}

async fn fetch_wled_live_receiver_config(
    ip: IpAddr,
) -> Result<Option<super::protocol::WledLiveReceiverConfig>> {
    let client = reqwest::Client::builder()
        .timeout(REALTIME_HTTP_TIMEOUT)
        .build()
        .context("Failed to build WLED HTTP client")?;

    let json: serde_json::Value = client
        .get(format!("http://{ip}/json/cfg"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Failed to fetch WLED config for {ip}"))?
        .json()
        .await
        .with_context(|| format!("Failed to parse WLED config JSON for {ip}"))?;

    parse_wled_live_receiver_config(&json)
}

pub(super) async fn validate_wled_receiver_config(
    ip: IpAddr,
    protocol: WledProtocol,
    color_format: WledColorFormat,
    e131_start_universe: u16,
) {
    let config = match fetch_wled_live_receiver_config(ip).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            debug!(ip = %ip, "WLED /json/cfg did not expose realtime receiver settings");
            return;
        }
        Err(error) => {
            debug!(
                ip = %ip,
                error = %error,
                "Failed to validate WLED realtime receiver config"
            );
            return;
        }
    };

    let mismatches =
        wled_receiver_config_mismatches(&config, protocol, color_format, e131_start_universe);

    if mismatches.is_empty() {
        debug!(
            ip = %ip,
            protocol = ?protocol,
            port = config.port,
            dmx_address = config.dmx_address,
            dmx_universe = config.dmx_universe,
            dmx_mode = config.dmx_mode,
            "WLED realtime receiver config matches Hypercolor streaming configuration"
        );
    } else {
        match protocol {
            WledProtocol::Ddp => {
                warn!(
                    ip = %ip,
                    protocol = ?protocol,
                    port = config.port,
                    dmx_address = config.dmx_address,
                    dmx_universe = config.dmx_universe,
                    dmx_mode = config.dmx_mode,
                    mismatches = %mismatches.join("; "),
                    "WLED realtime receiver config does not match Hypercolor output"
                );
            }
            WledProtocol::E131 => {
                warn!(
                    ip = %ip,
                    protocol = ?protocol,
                    port = config.port,
                    dmx_address = config.dmx_address,
                    dmx_universe = config.dmx_universe,
                    dmx_mode = config.dmx_mode,
                    expected_universe = e131_start_universe,
                    expected_mode = super::protocol::expected_wled_e131_mode(color_format),
                    expected_mode_name = super::protocol::wled_e131_mode_name(
                        super::protocol::expected_wled_e131_mode(color_format),
                    ),
                    mismatches = %mismatches.join("; "),
                    "WLED realtime receiver config does not match Hypercolor output"
                );
            }
        }
    }
}

/// Prime a device with black frames to flush WLED's internal state.
///
// Bypasses dedup intentionally — all three frames are identical black
// but must actually be sent to ensure WLED fully transitions to realtime.
pub(super) async fn prime_device(device: &mut WledDevice) -> Result<()> {
    let black_frame = device.black_frame();

    for _ in 0..REALTIME_PRIME_FRAMES {
        device.send_frame_forced(&black_frame).await?;
        tokio::time::sleep(REALTIME_PRIME_DELAY).await;
    }

    // Seed dedup cache with the black frame so the first real render
    // frame won't be suppressed if it happens to also be black.
    device.last_sent_pixels = Some(black_frame);
    device.last_frame_at = Some(Instant::now());

    Ok(())
}

pub(super) async fn clear_device(device: &mut WledDevice) -> Result<()> {
    let black_frame = device.black_frame();
    device.send_frame_forced(&black_frame).await
}
