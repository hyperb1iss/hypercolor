//! `HueStream` packet encoding utilities.

use anyhow::{Result, bail};

use super::color::CieXyb;
use super::types::HueChannel;

const HUESTREAM_HEADER_SIZE: usize = 52;
const CHANNEL_BYTES: usize = 7;
const PROTOCOL_NAME: &[u8; 9] = b"HueStream";

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
