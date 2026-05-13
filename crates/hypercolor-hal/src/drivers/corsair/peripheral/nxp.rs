//! Corsair NXP/CUE RGB packet encoder.

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceFeatures};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
    TransferType,
};

use super::types::{
    NXP_PACKET_SIZE, NXP_RESPONSE_TIMEOUT, NxpColorSelector, NxpCommand, NxpDeviceConfig, NxpField,
    NxpLightingKind, NxpLightingMode,
};

const FULL_RANGE_CHUNKS: [(u8, usize); 3] = [(0x01, 60), (0x02, 60), (0x03, 48)];
const PACKED_512_CHUNKS: [(u8, usize); 4] = [(0x01, 60), (0x02, 60), (0x03, 60), (0x04, 36)];

#[derive(Debug, Clone)]
pub struct CorsairNxpProtocol {
    config: NxpDeviceConfig,
}

impl CorsairNxpProtocol {
    #[must_use]
    pub const fn new(config: NxpDeviceConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub const fn config(&self) -> NxpDeviceConfig {
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
            "corsair nxp frame length mismatch; applying truncate/pad"
        );

        Cow::Owned(normalized)
    }

    fn command(packet: Vec<u8>, expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: packet,
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn fixed_packet() -> Vec<u8> {
        vec![0_u8; NXP_PACKET_SIZE]
    }

    fn get_field(field: NxpField) -> ProtocolCommand {
        let mut packet = Self::fixed_packet();
        packet[0] = NxpCommand::Get as u8;
        packet[1] = field as u8;
        Self::command(packet, true)
    }

    fn set_lighting_mode(mode: NxpLightingMode) -> ProtocolCommand {
        let mut packet = Self::fixed_packet();
        packet[0] = NxpCommand::Set as u8;
        packet[1] = NxpField::Lighting as u8;
        packet[2] = mode as u8;
        packet[3] = 0x00;
        Self::command(packet, true)
    }

    fn append_full_range_keyboard(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        for (component, selector) in [
            (0_usize, NxpColorSelector::Red),
            (1, NxpColorSelector::Green),
            (2, NxpColorSelector::Blue),
        ] {
            let plane = colors
                .iter()
                .map(|color| color[component])
                .collect::<Vec<_>>();
            append_declared_bulk_chunks(&plane, &FULL_RANGE_CHUNKS, commands);
            commands.push(component_commit(selector));
        }
    }

    fn append_monochrome_keyboard(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let plane = colors
            .iter()
            .map(|color| color[0].max(color[1]).max(color[2]))
            .collect::<Vec<_>>();
        append_declared_bulk_chunks(&plane, &FULL_RANGE_CHUNKS, commands);
        commands.push(component_commit(NxpColorSelector::Red));
    }

    fn append_packed_keyboard(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let payload = packed_512_payload(colors);
        append_declared_bulk_chunks(&payload, &PACKED_512_CHUNKS, commands);

        let mut packet = Self::fixed_packet();
        packet[0] = NxpCommand::Set as u8;
        packet[1] = NxpField::KeyboardPackedColor as u8;
        packet[4] = 0xD8;
        commands.push(Self::command(packet, true));
    }

    fn append_zoned_keyboard(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let mut packet = Self::fixed_packet();
        packet[0] = NxpCommand::Set as u8;
        packet[1] = NxpField::KeyboardZoneColor as u8;
        packet[2] = 0x00;
        packet[3] = 0x00;

        for (index, color) in colors.iter().take(3).enumerate() {
            let offset = 4 + index * 3;
            packet[offset..offset + 3].copy_from_slice(color);
        }

        commands.push(Self::command(packet, true));
    }

    fn append_mouse(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let zone_count = colors.len().min(15);
        let mut packet = Self::fixed_packet();
        packet[0] = NxpCommand::Set as u8;
        packet[1] = NxpField::MouseColor as u8;
        packet[2] = u8::try_from(zone_count).unwrap_or(u8::MAX);
        packet[3] = 0x01;

        for (index, color) in colors.iter().take(zone_count).enumerate() {
            let offset = 4 + index * 4;
            packet[offset] = u8::try_from(index + 1).unwrap_or(u8::MAX);
            packet[offset + 1..offset + 4].copy_from_slice(color);
        }

        commands.push(Self::command(packet, true));
    }

    fn append_mousepad(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let zone_count = colors.len().min(20);
        let mut packet = Self::fixed_packet();
        packet[0] = NxpCommand::Set as u8;
        packet[1] = NxpField::MouseColor as u8;
        packet[2] = u8::try_from(zone_count).unwrap_or(u8::MAX);
        packet[3] = 0x00;

        for (index, color) in colors.iter().take(zone_count).enumerate() {
            let offset = 4 + index * 3;
            packet[offset..offset + 3].copy_from_slice(color);
        }

        commands.push(Self::command(packet, true));
    }
}

impl Protocol for CorsairNxpProtocol {
    fn name(&self) -> &'static str {
        self.config.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        if !self.config.supports_direct() {
            return vec![Self::get_field(NxpField::Ident)];
        }

        vec![
            Self::get_field(NxpField::Ident),
            Self::set_lighting_mode(NxpLightingMode::Software),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        if self.config.requires_unclean_exit || !self.config.supports_direct() {
            return Vec::new();
        }

        vec![Self::set_lighting_mode(NxpLightingMode::Hardware)]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        commands.clear();
        if !self.config.supports_direct() {
            return;
        }

        let normalized = self.normalize_colors(colors);
        match self.config.lighting_kind {
            NxpLightingKind::FullRangeKeyboard => {
                self.append_full_range_keyboard(normalized.as_ref(), commands);
            }
            NxpLightingKind::Packed512Keyboard => {
                self.append_packed_keyboard(normalized.as_ref(), commands);
            }
            NxpLightingKind::ZonedKeyboard => {
                self.append_zoned_keyboard(normalized.as_ref(), commands);
            }
            NxpLightingKind::MonochromeKeyboard => {
                self.append_monochrome_keyboard(normalized.as_ref(), commands);
            }
            NxpLightingKind::Mouse => {
                self.append_mouse(normalized.as_ref(), commands);
            }
            NxpLightingKind::Mousepad => {
                self.append_mousepad(normalized.as_ref(), commands);
            }
            NxpLightingKind::NoLights => {}
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.is_empty() {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty Corsair NXP response".to_owned(),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        NXP_RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        if !self.config.supports_direct() {
            return Vec::new();
        }

        vec![ProtocolZone {
            name: self.config.class.zone_name().to_owned(),
            led_count: u32::try_from(self.config.led_count).unwrap_or(u32::MAX),
            topology: self.config.topology.hint(),
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: self.config.supports_direct(),
            supports_brightness: false,
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
        self.config.frame_interval()
    }
}

fn append_declared_bulk_chunks(
    payload: &[u8],
    chunks: &[(u8, usize)],
    commands: &mut Vec<ProtocolCommand>,
) {
    let mut cursor = 0_usize;
    for (index, declared_len) in chunks {
        let mut packet = CorsairNxpProtocol::fixed_packet();
        packet[0] = NxpCommand::WriteBulk as u8;
        packet[1] = *index;
        packet[2] = u8::try_from(*declared_len).unwrap_or(u8::MAX);
        packet[3] = 0x00;

        let copy_len = payload
            .len()
            .saturating_sub(cursor)
            .min(*declared_len)
            .min(NXP_PACKET_SIZE - 4);
        packet[4..4 + copy_len].copy_from_slice(&payload[cursor..cursor + copy_len]);
        commands.push(CorsairNxpProtocol::command(packet, true));
        cursor = cursor.saturating_add(*declared_len);
    }
}

fn component_commit(selector: NxpColorSelector) -> ProtocolCommand {
    let mut packet = CorsairNxpProtocol::fixed_packet();
    packet[0] = NxpCommand::Set as u8;
    packet[1] = NxpField::KeyboardColor as u8;
    packet[2] = selector as u8;
    packet[3] = 0x03;
    packet[4] = if selector == NxpColorSelector::Blue {
        0x02
    } else {
        0x01
    };
    CorsairNxpProtocol::command(packet, true)
}

fn packed_512_payload(colors: &[[u8; 3]]) -> Vec<u8> {
    let packed_plane = |component: usize| {
        colors
            .chunks(2)
            .map(|pair| {
                let first = pair[0][component] >> 5;
                let second = pair.get(1).map_or(0, |color| color[component] >> 5);
                ((7 - second) << 4) | (7 - first)
            })
            .collect::<Vec<_>>()
    };

    let mut payload = Vec::with_capacity(colors.len().div_ceil(2).saturating_mul(3));
    payload.extend(packed_plane(0));
    payload.extend(packed_plane(1));
    payload.extend(packed_plane(2));
    payload
}
