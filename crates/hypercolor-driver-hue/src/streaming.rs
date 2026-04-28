//! `HueStream` packet encoding utilities.

use anyhow::{Result, bail};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use webrtc_dtls::cipher_suite::CipherSuiteId;
use webrtc_dtls::config::Config as DtlsConfig;
use webrtc_dtls::conn::DTLSConn;
use webrtc_util::Conn;

use super::color::CieXyb;
use super::types::HueChannel;

const HUESTREAM_HEADER_SIZE: usize = 52;
const CHANNEL_BYTES: usize = 7;
const PROTOCOL_NAME: &[u8; 9] = b"HueStream";
const HUE_STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HUE_STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const HUE_STREAM_PORT: u16 = 2_100;

/// Active DTLS streaming session to a Hue bridge.
pub struct HueStreamSession {
    conn: DTLSConn,
    config_id: String,
    channels: Vec<HueChannel>,
    sequence: u8,
    packet_buf: Vec<u8>,
}

impl HueStreamSession {
    /// Establish a DTLS connection to a Hue bridge entertainment endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the client key is not valid hex, the UDP socket
    /// cannot connect, or the DTLS handshake fails.
    pub async fn connect(
        bridge_ip: IpAddr,
        api_key: &str,
        client_key_hex: &str,
        config_id: &str,
        channels: Vec<HueChannel>,
    ) -> Result<Self> {
        install_rustls_provider();

        let client_key = decode_hex(client_key_hex)?;
        if client_key.is_empty() {
            bail!(
                "Hue client key is empty — refusing to open DTLS session without PSK authentication"
            );
        }
        if api_key.is_empty() {
            bail!(
                "Hue api key (PSK identity hint) is empty — refusing to open DTLS session without PSK authentication"
            );
        }
        let bind_addr = SocketAddr::new(
            match bridge_ip {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            },
            0,
        );
        let socket = UdpSocket::bind(bind_addr).await?;
        socket
            .connect(SocketAddr::new(bridge_ip, HUE_STREAM_PORT))
            .await?;

        // SAFETY: `insecure_skip_verify` disables X.509 chain verification, NOT the DTLS
        // handshake itself. Hue bridges ship self-signed certificates tied to the bridge
        // serial, and the Entertainment API mandates a pure PSK handshake
        // (TLS_PSK_WITH_AES_128_GCM_SHA256) — there is no public CA to validate against,
        // so chain verification would always fail. Authentication is enforced by the
        // pre-shared key negotiated via the `/api` user/clientkey pairing flow; the
        // non-empty checks above guarantee we never reach this point with PSK auth
        // disabled. Do NOT remove `insecure_skip_verify` without replacing it with a
        // bridge-specific cert pin.
        let psk = Arc::new(client_key);
        let config = DtlsConfig {
            psk: Some(Arc::new(move |_| Ok(psk.as_ref().clone()))),
            psk_identity_hint: Some(api_key.as_bytes().to_vec()),
            cipher_suites: vec![CipherSuiteId::Tls_Psk_With_Aes_128_Gcm_Sha256],
            insecure_skip_verify: true,
            flight_interval: HUE_STREAM_CONNECT_TIMEOUT,
            ..Default::default()
        };

        let conn: Arc<dyn Conn + Send + Sync> = Arc::new(socket);
        let conn = DTLSConn::new(conn, config, true, None).await?;
        let packet_buf = Vec::with_capacity(HUESTREAM_HEADER_SIZE + channels.len() * CHANNEL_BYTES);

        Ok(Self {
            conn,
            config_id: config_id.to_owned(),
            channels,
            sequence: 0,
            packet_buf,
        })
    }

    /// Send one pre-converted `HueStream` frame.
    ///
    /// # Errors
    ///
    /// Returns an error when packet encoding fails or the DTLS write times out.
    pub async fn send_frame(&mut self, colors: &[CieXyb]) -> Result<()> {
        encode_packet_into(
            &mut self.packet_buf,
            &self.config_id,
            self.sequence,
            self.channels.as_slice(),
            colors,
        )?;
        self.conn
            .write(self.packet_buf.as_slice(), Some(HUE_STREAM_WRITE_TIMEOUT))
            .await?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }

    /// Close the DTLS session.
    ///
    /// # Errors
    ///
    /// Returns an error if the DTLS close-notify cannot be sent.
    pub async fn close(&self) -> Result<()> {
        self.conn.close().await?;
        Ok(())
    }
}

/// Encode one `HueStream` v2 packet into a reusable buffer.
///
/// # Errors
///
/// Returns an error when the entertainment config ID is not a 36-byte ASCII
/// UUID string or the packet would exceed the Hue channel limit.
pub fn encode_packet_into(
    packet_buf: &mut Vec<u8>,
    config_id: &str,
    sequence: u8,
    channels: &[HueChannel],
    colors: &[CieXyb],
) -> Result<()> {
    if !config_id.is_ascii() || config_id.len() != 36 {
        bail!("Hue entertainment config ID must be a 36-byte ASCII UUID");
    }
    if channels.len() > 20 {
        bail!("Hue entertainment streaming supports at most 20 channels");
    }

    let required_len = HUESTREAM_HEADER_SIZE + channels.len() * CHANNEL_BYTES;
    packet_buf.resize(required_len, 0);

    packet_buf[..9].copy_from_slice(PROTOCOL_NAME);
    packet_buf[9] = 0x02;
    packet_buf[10] = 0x00;
    packet_buf[11] = sequence;
    packet_buf[12] = 0x00;
    packet_buf[13] = 0x00;
    packet_buf[14] = 0x01;
    packet_buf[15] = 0x00;
    packet_buf[16..52].copy_from_slice(config_id.as_bytes());

    for (index, channel) in channels.iter().enumerate() {
        let color = colors.get(index).copied().unwrap_or(CieXyb {
            x: 0.0,
            y: 0.0,
            brightness: 0.0,
        });
        let offset = HUESTREAM_HEADER_SIZE + index * CHANNEL_BYTES;

        packet_buf[offset] = channel.id;
        packet_buf[offset + 1..offset + 3].copy_from_slice(&encode_unit_u16(color.x).to_be_bytes());
        packet_buf[offset + 3..offset + 5].copy_from_slice(&encode_unit_u16(color.y).to_be_bytes());
        packet_buf[offset + 5..offset + 7]
            .copy_from_slice(&encode_unit_u16(color.brightness).to_be_bytes());
    }

    Ok(())
}

fn install_rustls_provider() {
    let _install_result = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "HueStream wire format requires unit floats to be quantized into u16 values"
)]
fn encode_unit_u16(value: f64) -> u16 {
    let scaled = (value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round();
    scaled as u16
}

fn decode_hex(raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim();
    if !trimmed.len().is_multiple_of(2) {
        bail!("Hue client key must contain an even number of hex digits");
    }

    let mut bytes = Vec::with_capacity(trimmed.len() / 2);
    for pair in trimmed.as_bytes().chunks_exact(2) {
        let pair = std::str::from_utf8(pair)?;
        bytes.push(u8::from_str_radix(pair, 16)?);
    }

    Ok(bytes)
}
