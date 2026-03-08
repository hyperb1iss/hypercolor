//! Razer Seiren V3 Chroma protocol.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

const SEIREN_V3_PAYLOAD_LEN: usize = 63;
const SEIREN_V3_TRANSACTION_ID: u8 = 0x1F;
const SEIREN_V3_LED_ORDER: [usize; 10] = [7, 6, 8, 9, 2, 1, 3, 0, 4, 5];
const SOFTWARE_MODE_DATA_SIZE: u8 = 0x02;
const MODERN_EFFECT_DATA_SIZE: u8 = 0x06;
const FRAME_DATA_SIZE: u8 = 0x23;
const CRC_OFFSET: usize = 61;

/// Write-only protocol for the Seiren V3 Chroma microphone ring.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeirenV3Protocol;

impl SeirenV3Protocol {
    fn encode_color(color: [u8; 3]) -> [u8; 3] {
        // The Seiren V3 ring expects RBG channel order on the wire.
        [color[0], color[2], color[1]]
    }

    fn crc(payload: &[u8; SEIREN_V3_PAYLOAD_LEN]) -> u8 {
        payload[1..].iter().fold(0_u8, |acc, byte| acc ^ byte)
    }

    fn build_packet(
        data_size: u8,
        command_class: u8,
        command_id: u8,
        args: &[u8],
        post_delay: Duration,
    ) -> ProtocolCommand {
        let mut payload = [0_u8; SEIREN_V3_PAYLOAD_LEN];
        payload[1] = SEIREN_V3_TRANSACTION_ID;
        payload[5] = data_size;
        payload[6] = command_class;
        payload[7] = command_id;
        payload[8..8 + args.len()].copy_from_slice(args);
        payload[CRC_OFFSET] = Self::crc(&payload);

        ProtocolCommand {
            data: payload.to_vec(),
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay,
            transfer_type: TransferType::Primary,
        }
    }
}

impl Protocol for SeirenV3Protocol {
    fn name(&self) -> &str {
        "Razer Seiren V3"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            Self::build_packet(
                SOFTWARE_MODE_DATA_SIZE,
                0x00,
                0x04,
                &[0x03],
                Duration::from_millis(10),
            ),
            Self::build_packet(
                MODERN_EFFECT_DATA_SIZE,
                0x0F,
                0x02,
                &[0x00, 0x00, 0x08, 0x00, 0x01],
                Duration::from_millis(10),
            ),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut ordered = [[0_u8; 3]; SEIREN_V3_LED_ORDER.len()];

        for (index, slot) in SEIREN_V3_LED_ORDER.iter().copied().enumerate() {
            if let Some(color) = colors.get(index).copied() {
                ordered[slot] = color;
            }
        }

        let mut args = Vec::with_capacity(34);
        args.extend_from_slice(&[0x00, 0x00, 0x00, 0x09]);
        for color in ordered {
            args.extend_from_slice(&Self::encode_color(color));
        }

        vec![Self::build_packet(
            FRAME_DATA_SIZE,
            0x0F,
            0x03,
            &args,
            Duration::from_millis(1),
        )]
    }

    fn encode_brightness(&self, _brightness: u8) -> Option<Vec<ProtocolCommand>> {
        None
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        None
    }

    fn parse_response(&self, _data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        vec![ProtocolZone {
            name: "Main".to_owned(),
            led_count: 10,
            topology: DeviceTopologyHint::Custom,
            color_format: DeviceColorFormat::Rgb,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 500,
        }
    }

    fn total_leds(&self) -> u32 {
        10
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(2)
    }
}
