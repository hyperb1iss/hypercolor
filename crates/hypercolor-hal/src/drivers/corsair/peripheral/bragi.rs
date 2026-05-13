//! Corsair Bragi HID RGB protocol.

use std::borrow::Cow;
use std::cmp::min;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceFeatures};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

use super::topology::zones_for_bragi;
use super::types::{
    BRAGI_KEEPALIVE_INTERVAL, BRAGI_MAGIC, BRAGI_RESPONSE_TIMEOUT, BragiCommand, BragiDeviceConfig,
    BragiHandle, BragiLightingFormat, BragiLightingMode, BragiProperty,
};

#[derive(Debug)]
pub struct CorsairBragiProtocol {
    config: BragiDeviceConfig,
    last_frame_nonzero: AtomicBool,
}

impl CorsairBragiProtocol {
    #[must_use]
    pub const fn new(config: BragiDeviceConfig) -> Self {
        Self {
            config,
            last_frame_nonzero: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub const fn config(&self) -> BragiDeviceConfig {
        self.config
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.config.led_count;
        if expected == 0 {
            return Cow::Borrowed(&[]);
        }

        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = min(colors.len(), expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        warn!(
            expected,
            actual = colors.len(),
            device = self.config.name,
            "corsair bragi frame length mismatch; applying truncate/pad"
        );

        Cow::Owned(normalized)
    }

    fn command(data: Vec<u8>, expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data,
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn fixed_packet(&self) -> Vec<u8> {
        let mut packet = vec![0_u8; self.config.packet_size];
        packet[0] = BRAGI_MAGIC;
        packet
    }

    fn property_set(&self, property: BragiProperty, value: u16) -> ProtocolCommand {
        let mut packet = self.fixed_packet();
        packet[1] = BragiCommand::Set as u8;
        packet[2] = property as u8;
        packet[3] = 0x00;
        packet[4..6].copy_from_slice(&value.to_le_bytes());
        Self::command(packet, true)
    }

    fn open_handle(&self) -> ProtocolCommand {
        let mut packet = self.fixed_packet();
        packet[1] = BragiCommand::OpenHandle as u8;
        packet[2] = BragiHandle::Lighting as u8;
        packet[3..5].copy_from_slice(&(self.config.resource() as u16).to_le_bytes());
        packet[5] = 0x00;
        Self::command(packet, true)
    }

    fn close_handle(&self) -> ProtocolCommand {
        let mut packet = self.fixed_packet();
        packet[1] = BragiCommand::CloseHandle as u8;
        packet[2] = 0x01;
        packet[3] = BragiHandle::Lighting as u8;
        packet[4] = 0x00;
        Self::command(packet, true)
    }

    fn poll(&self) -> ProtocolCommand {
        let mut packet = self.fixed_packet();
        packet[1] = BragiCommand::Poll as u8;
        Self::command(packet, true)
    }

    fn frame_payload(&self, colors: &[[u8; 3]]) -> Vec<u8> {
        match self.config.lighting_format {
            BragiLightingFormat::RgbPlanar => planar_rgb_payload(colors),
            BragiLightingFormat::Monochrome => monochrome_payload(colors),
            BragiLightingFormat::AlternateRgb => alternate_rgb_payload(colors),
        }
    }

    fn append_write_commands(&self, payload: &[u8], commands: &mut Vec<ProtocolCommand>) {
        let first_capacity = self.config.packet_size.saturating_sub(7);
        let continue_capacity = self.config.packet_size.saturating_sub(3);
        let declared_len = u32::try_from(payload.len()).unwrap_or(u32::MAX);

        let mut first = self.fixed_packet();
        first[1] = BragiCommand::WriteData as u8;
        first[2] = BragiHandle::Lighting as u8;
        first[3..7].copy_from_slice(&declared_len.to_le_bytes());

        let first_len = payload.len().min(first_capacity);
        first[7..7 + first_len].copy_from_slice(&payload[..first_len]);
        commands.push(Self::command(first, true));

        for chunk in payload[first_len..].chunks(continue_capacity) {
            let mut packet = self.fixed_packet();
            packet[1] = BragiCommand::ContinueWrite as u8;
            packet[2] = BragiHandle::Lighting as u8;
            packet[3..3 + chunk.len()].copy_from_slice(chunk);
            commands.push(Self::command(packet, true));
        }
    }
}

impl Protocol for CorsairBragiProtocol {
    fn name(&self) -> &'static str {
        self.config.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            self.property_set(BragiProperty::Mode, BragiLightingMode::Software as u16),
            self.open_handle(),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            self.close_handle(),
            self.property_set(BragiProperty::Mode, BragiLightingMode::Hardware as u16),
        ]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        commands.truncate(0);
        if self.config.led_count == 0 {
            return;
        }

        let normalized = self.normalize_colors(colors);
        let payload = self.frame_payload(normalized.as_ref());
        self.append_write_commands(&payload, commands);

        let any_nonzero = normalized
            .iter()
            .any(|color| color.iter().any(|channel| *channel != 0));
        let previous_nonzero = self.last_frame_nonzero.swap(any_nonzero, Ordering::AcqRel);

        if previous_nonzero && !any_nonzero {
            commands.push(self.property_set(BragiProperty::Brightness, 0));
        } else if !previous_nonzero && any_nonzero {
            commands.push(self.property_set(BragiProperty::Brightness, 1_000));
        }
    }

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        let scaled = u16::try_from((u32::from(brightness) * 1_000) / 255).unwrap_or(1_000);
        Some(vec![self.property_set(BragiProperty::Brightness, scaled)])
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        Some(ProtocolKeepalive {
            interval: BRAGI_KEEPALIVE_INTERVAL,
            commands: vec![self.poll()],
        })
    }

    fn keepalive_commands(&self) -> Vec<ProtocolCommand> {
        vec![self.poll()]
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let Some((&magic, _rest)) = data.split_first() else {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty Corsair Bragi response".to_owned(),
            });
        };

        if magic != BRAGI_MAGIC {
            return Err(ProtocolError::MalformedResponse {
                detail: format!("unexpected Corsair Bragi response prefix 0x{magic:02X}"),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        BRAGI_RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        zones_for_bragi(&self.config)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: self.config.led_count > 0,
            supports_brightness: self.config.led_count > 0,
            has_display: false,
            display_resolution: None,
            max_fps: self.config.max_fps,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(self.config.led_count).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        self.config.class.default_frame_interval()
    }
}

fn planar_rgb_payload(colors: &[[u8; 3]]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(colors.len().saturating_mul(3));
    payload.extend(colors.iter().map(|color| color[0]));
    payload.extend(colors.iter().map(|color| color[1]));
    payload.extend(colors.iter().map(|color| color[2]));
    payload
}

fn monochrome_payload(colors: &[[u8; 3]]) -> Vec<u8> {
    colors
        .iter()
        .map(|color| color[0].max(color[1]).max(color[2]))
        .collect()
}

fn alternate_rgb_payload(colors: &[[u8; 3]]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(2 + colors.len().saturating_mul(3));
    payload.extend_from_slice(&[0x12, 0x00]);
    for color in colors {
        payload.extend_from_slice(color);
    }
    payload
}

#[must_use]
pub fn build_property_get_packet_for_testing(
    packet_size: usize,
    property: BragiProperty,
) -> Vec<u8> {
    let mut packet = vec![0_u8; packet_size];
    packet[0] = BRAGI_MAGIC;
    packet[1] = BragiCommand::Get as u8;
    packet[2] = property as u8;
    packet[3] = 0x00;
    packet
}
