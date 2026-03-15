//! Nanoleaf `UDP` external-control streaming.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::net::UdpSocket;

use super::nanoleaf_http_client;

pub const DEFAULT_NANOLEAF_API_PORT: u16 = 16_021;
pub const DEFAULT_NANOLEAF_STREAM_PORT: u16 = 60_222;

/// Active `UDP` streaming session to one Nanoleaf device.
pub struct NanoleafStreamSession {
    socket: UdpSocket,
    panel_ids: Vec<u16>,
    packet_buf: Vec<u8>,
}

impl NanoleafStreamSession {
    /// Enable external control and connect a `UDP` socket to the device.
    ///
    /// # Errors
    ///
    /// Returns an error if the REST activation or `UDP` socket setup fails.
    pub async fn connect(
        device_ip: IpAddr,
        api_port: u16,
        auth_token: &str,
        panel_ids: Vec<u16>,
    ) -> Result<Self> {
        Self::connect_with_udp_port(
            device_ip,
            api_port,
            DEFAULT_NANOLEAF_STREAM_PORT,
            auth_token,
            panel_ids,
        )
        .await
    }

    /// Test-friendly variant that overrides the `UDP` destination port.
    ///
    /// # Errors
    ///
    /// Returns an error if the REST activation or `UDP` socket setup fails.
    pub async fn connect_with_udp_port(
        device_ip: IpAddr,
        api_port: u16,
        udp_port: u16,
        auth_token: &str,
        panel_ids: Vec<u16>,
    ) -> Result<Self> {
        enable_external_control(device_ip, api_port, auth_token).await?;

        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("failed to bind Nanoleaf UDP socket")?;
        socket
            .connect(SocketAddr::new(device_ip, udp_port))
            .await
            .with_context(|| {
                format!("failed to connect Nanoleaf UDP socket to {device_ip}:{udp_port}")
            })?;

        let mut packet_buf = Vec::new();
        encode_frame_into(&mut packet_buf, &panel_ids, &[], 0)?;

        Ok(Self {
            socket,
            panel_ids,
            packet_buf,
        })
    }

    /// Send one color frame.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be encoded or the `UDP` write
    /// fails.
    pub async fn send_frame(&mut self, colors: &[[u8; 3]], transition_time: u16) -> Result<()> {
        encode_frame_into(
            &mut self.packet_buf,
            self.panel_ids.as_slice(),
            colors,
            transition_time,
        )?;
        self.socket
            .send(self.packet_buf.as_slice())
            .await
            .context("failed to send Nanoleaf UDP frame")?;
        Ok(())
    }
}

/// Encode one Nanoleaf `UDP` frame into a reusable buffer.
///
/// # Errors
///
/// Returns an error when the panel count exceeds the protocol limit.
pub fn encode_frame_into(
    packet_buf: &mut Vec<u8>,
    panel_ids: &[u16],
    colors: &[[u8; 3]],
    transition_time: u16,
) -> Result<()> {
    let panel_count = u16::try_from(panel_ids.len())
        .context("Nanoleaf frame cannot encode more than 65535 panels")?;
    let required_len = 2 + panel_ids.len() * 8;
    packet_buf.resize(required_len, 0);

    packet_buf[..2].copy_from_slice(&panel_count.to_be_bytes());

    for (index, panel_id) in panel_ids.iter().copied().enumerate() {
        let color = colors.get(index).copied().unwrap_or([0, 0, 0]);
        let offset = 2 + index * 8;

        packet_buf[offset..offset + 2].copy_from_slice(&panel_id.to_be_bytes());
        packet_buf[offset + 2] = color[0];
        packet_buf[offset + 3] = color[1];
        packet_buf[offset + 4] = color[2];
        packet_buf[offset + 5] = 0;
        packet_buf[offset + 6..offset + 8].copy_from_slice(&transition_time.to_be_bytes());
    }

    Ok(())
}

async fn enable_external_control(device_ip: IpAddr, api_port: u16, auth_token: &str) -> Result<()> {
    if auth_token.is_empty() {
        bail!("Nanoleaf external control requires a non-empty auth token");
    }

    let client = nanoleaf_http_client()?;
    let url = format!("http://{device_ip}:{api_port}/api/v1/{auth_token}/effects");
    let body = ExternalControlRequest {
        write: ExternalControlWrite {
            command: "display",
            anim_type: "extControl",
            ext_control_version: "v2",
        },
    };

    client
        .put(&url)
        .json(&body)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("failed to enable Nanoleaf external control via {url}"))?;

    Ok(())
}

#[derive(Debug, Serialize)]
struct ExternalControlRequest<'a> {
    write: ExternalControlWrite<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalControlWrite<'a> {
    command: &'a str,
    anim_type: &'a str,
    ext_control_version: &'a str,
}
