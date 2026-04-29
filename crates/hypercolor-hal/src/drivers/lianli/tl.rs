//! Lian Li TL Fan Hub protocol encoder.
//!
//! Covers only the TL Fan hub (`0x7372`), which speaks a fundamentally
//! different framed HID protocol from the ENE 6K77 UNI Hub family:
//! 64-byte packets with an incrementing 16-bit counter, per-fan `SetLight`
//! commands carrying averaged colors, and handshake/product-info init
//! responses that populate per-port fan counts and firmware strings.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

use super::common::{LianLiHubVariant, TL_REPORT_ID};

const TL_PACKET_LEN: usize = 64;
const TL_PAYLOAD_LEN: usize = 58;
const TL_SET_LIGHT_LEN: usize = 20;
const TL_LEDS_PER_FAN: usize = 26;
pub(super) const LEDS_PER_FAN_U8: u8 = 26;
const TL_LEDS_PER_FAN_U32: u32 = 26;
const TL_MAX_PORTS: usize = 4;
const TL_MAX_FANS_PER_PORT: usize = 10;
const TL_MAX_TOTAL_FANS: usize = 16;
const TL_RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);
const TL_FRAME_INTERVAL: Duration = Duration::from_millis(100);
const TL_EFFECT_STATIC: u8 = 0x01;
const TL_BRIGHTNESS_FULL: u8 = 0x04;
const TL_SPEED_MEDIUM: u8 = 0x02;
const TL_DIRECTION_CLOCKWISE: u8 = 0x00;

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct TlPacket {
    report_id: u8,
    command: u8,
    reserved: u8,
    packet_hi: u8,
    packet_lo: u8,
    data_len: u8,
    payload: [u8; TL_PAYLOAD_LEN],
}

const _: () = assert!(
    std::mem::size_of::<TlPacket>() == TL_PACKET_LEN,
    "TlPacket must match the 64-byte TL HID packet size"
);

#[derive(Debug, Clone, Default)]
struct TlFanState {
    port_fan_counts: [u8; TL_MAX_PORTS],
    firmware: Option<String>,
}

/// Framed HID protocol for Lian Li TL fan hubs.
pub struct TlFanProtocol {
    packet_counter: AtomicU16,
    state: RwLock<TlFanState>,
}

impl TlFanProtocol {
    /// Create a new TL Fan protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            packet_counter: AtomicU16::new(0),
            state: RwLock::new(TlFanState::default()),
        }
    }

    /// Override the discovered per-port fan counts, primarily for tests.
    #[must_use]
    pub fn with_port_fan_counts(self, port_fan_counts: [u8; TL_MAX_PORTS]) -> Self {
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = TlFanState {
            port_fan_counts,
            firmware: None,
        };
        self
    }

    /// Latest parsed TL firmware string.
    #[must_use]
    pub fn firmware(&self) -> Option<String> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .firmware
            .clone()
    }

    /// Current discovered per-port fan counts.
    #[must_use]
    pub fn port_fan_counts(&self) -> [u8; TL_MAX_PORTS] {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .port_fan_counts
    }

    fn next_packet_number(&self) -> u16 {
        self.packet_counter
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn command(&self, command: u8, payload: &[u8], expects_response: bool) -> ProtocolCommand {
        let mut packet = TlPacket::new_zeroed();
        let packet_number = self.next_packet_number();
        let [packet_hi, packet_lo] = packet_number.to_be_bytes();
        packet.report_id = TL_REPORT_ID;
        packet.command = command;
        packet.packet_hi = packet_hi;
        packet.packet_lo = packet_lo;
        packet.data_len = u8::try_from(payload.len()).expect("TL payload length should fit in u8");
        packet.payload[..payload.len()].copy_from_slice(payload);

        ProtocolCommand {
            data: packet.as_bytes().to_vec(),
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn handshake_command(&self) -> ProtocolCommand {
        self.command(0xA1, &[], true)
    }

    fn product_info_command(&self) -> ProtocolCommand {
        self.command(0xA6, &[], true)
    }

    /// Encode one TL per-fan PWM duty write.
    #[must_use]
    pub fn encode_fan_speed(&self, port: u8, fan_index: u8, duty: u8) -> Option<ProtocolCommand> {
        if usize::from(port) >= TL_MAX_PORTS || usize::from(fan_index) >= TL_MAX_FANS_PER_PORT {
            return None;
        }

        Some(self.command(0xAA, &[(port << 4) | (fan_index & 0x0F), duty], true))
    }

    fn effective_port_fan_counts(&self, color_len: usize) -> [u8; TL_MAX_PORTS] {
        let discovered = self.port_fan_counts();
        if discovered.iter().any(|count| *count > 0) {
            return discovered;
        }

        infer_tl_fan_counts(color_len)
    }
}

impl Default for TlFanProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for TlFanProtocol {
    fn name(&self) -> &'static str {
        LianLiHubVariant::TlFan.name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![self.handshake_command(), self.product_info_command()]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let counts = self.effective_port_fan_counts(colors.len());
        let mut color_offset = 0_usize;
        let mut encoder = CommandBuffer::new(commands);

        for (port, fan_count) in counts.into_iter().enumerate() {
            for fan_index in 0..usize::from(fan_count) {
                if color_offset >= colors.len() {
                    break;
                }

                let end = colors.len().min(color_offset + TL_LEDS_PER_FAN);
                let fan_colors = &colors[color_offset..end];
                color_offset = end;

                let average = average_rgb(fan_colors);
                let mut payload = [0_u8; TL_SET_LIGHT_LEN];
                payload[0] = u8::try_from(port).expect("port index should fit in u8") << 4;
                payload[1] = (u8::try_from(port).expect("port index should fit in u8") << 4)
                    | u8::try_from(fan_index).expect("fan index should fit in u8");
                payload[2] = TL_EFFECT_STATIC;
                payload[3] = TL_BRIGHTNESS_FULL;
                payload[4] = TL_SPEED_MEDIUM;
                payload[5] = average[0];
                payload[6] = average[1];
                payload[7] = average[2];
                payload[17] = TL_DIRECTION_CLOCKWISE;
                payload[18] = 0x00;
                payload[19] = 0x01;

                let mut packet = TlPacket::new_zeroed();
                let packet_number = self.next_packet_number();
                let [packet_hi, packet_lo] = packet_number.to_be_bytes();
                packet.report_id = TL_REPORT_ID;
                packet.command = 0xA3;
                packet.packet_hi = packet_hi;
                packet.packet_lo = packet_lo;
                packet.data_len =
                    u8::try_from(TL_SET_LIGHT_LEN).expect("TL light payload length should fit");
                packet.payload[..TL_SET_LIGHT_LEN].copy_from_slice(&payload);

                encoder.push_struct(
                    &packet,
                    true,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
            }
        }

        encoder.finish();
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let (command_index, data_len_index, payload_index) =
            if data.first().copied() == Some(TL_REPORT_ID) {
                (1_usize, 5_usize, 6_usize)
            } else {
                (0_usize, 4_usize, 5_usize)
            };

        if data.len() <= data_len_index {
            return Err(ProtocolError::MalformedResponse {
                detail: format!("TL response too short: {}", data.len()),
            });
        }

        let data_len = usize::from(data[data_len_index]);
        if data.len() < payload_index + data_len {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "TL response declared {data_len} payload bytes but only {} were present",
                    data.len().saturating_sub(payload_index)
                ),
            });
        }

        let command = data[command_index];
        let payload = &data[payload_index..payload_index + data_len];

        match command {
            0xA1 => {
                let mut counts = [0_u8; TL_MAX_PORTS];
                for chunk in payload.chunks_exact(3) {
                    let descriptor = chunk[0];
                    if descriptor & 0x80 == 0 {
                        continue;
                    }

                    let port = usize::from((descriptor >> 4) & 0x03);
                    let fan_index = descriptor & 0x0F;
                    counts[port] = counts[port].max(fan_index.saturating_add(1));
                }

                self.state
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .port_fan_counts = counts;
            }
            0xA6 => {
                let firmware = payload
                    .iter()
                    .take_while(|byte| **byte != 0x00)
                    .copied()
                    .collect::<Vec<_>>();
                let firmware = String::from_utf8_lossy(&firmware).trim().to_owned();
                if !firmware.is_empty() {
                    self.state
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .firmware = Some(firmware);
                }
            }
            _ => {}
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: payload.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        TL_RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let counts = self.port_fan_counts();
        let total_fans: usize = counts.iter().map(|count| usize::from(*count)).sum();
        let mut zones = Vec::with_capacity(total_fans);
        let led_count = TL_LEDS_PER_FAN_U32;

        for (port, fan_count) in counts.into_iter().enumerate() {
            for fan_index in 0..fan_count {
                zones.push(ProtocolZone {
                    name: format!("Port {} Fan {}", port + 1, fan_index + 1),
                    led_count,
                    topology: DeviceTopologyHint::Ring { count: led_count },
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                });
            }
        }

        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            max_fps: 10,
            ..DeviceCapabilities::default()
        }
    }

    fn total_leds(&self) -> u32 {
        self.zones().iter().map(|zone| zone.led_count).sum()
    }

    fn frame_interval(&self) -> Duration {
        TL_FRAME_INTERVAL
    }
}

fn infer_tl_fan_counts(color_len: usize) -> [u8; TL_MAX_PORTS] {
    let needed_fans = color_len.div_ceil(TL_LEDS_PER_FAN).min(TL_MAX_TOTAL_FANS);
    let mut counts = [0_u8; TL_MAX_PORTS];
    let mut remaining = needed_fans;

    for count in &mut counts {
        if remaining == 0 {
            break;
        }

        let assigned = remaining.min(TL_MAX_FANS_PER_PORT);
        *count = u8::try_from(assigned).expect("assigned TL fan count should fit in u8");
        remaining -= assigned;
    }

    counts
}

fn average_rgb(colors: &[[u8; 3]]) -> [u8; 3] {
    if colors.is_empty() {
        return [0, 0, 0];
    }

    let (r, g, b) = colors.iter().fold((0_u32, 0_u32, 0_u32), |acc, color| {
        (
            acc.0 + u32::from(color[0]),
            acc.1 + u32::from(color[1]),
            acc.2 + u32::from(color[2]),
        )
    });
    let len = u32::try_from(colors.len()).expect("color length should fit in u32");

    [
        u8::try_from(r / len).expect("averaged red should fit in u8"),
        u8::try_from(g / len).expect("averaged green should fit in u8"),
        u8::try_from(b / len).expect("averaged blue should fit in u8"),
    ]
}
