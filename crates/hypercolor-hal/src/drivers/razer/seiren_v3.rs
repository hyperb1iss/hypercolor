//! Razer Seiren V3 Chroma protocol.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use zerocopy::{FromZeros, IntoBytes, KnownLayout, Immutable};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse,
    ProtocolZone, ResponseStatus, TransferType,
};

const SEIREN_V3_PAYLOAD_LEN: usize = 63;
const SEIREN_V3_TRANSACTION_ID: u8 = 0x1F;
const SEIREN_V3_LED_ORDER: [usize; 10] = [7, 6, 8, 9, 2, 1, 3, 0, 4, 5];
const SOFTWARE_MODE_DATA_SIZE: u8 = 0x02;
const MODERN_EFFECT_DATA_SIZE: u8 = 0x06;
const FRAME_DATA_SIZE: u8 = 0x23;

const _: () = assert!(
    std::mem::size_of::<SeirenV3Report>() == SEIREN_V3_PAYLOAD_LEN,
    "SeirenV3Report must match SEIREN_V3_PAYLOAD_LEN (63 bytes)"
);

/// Wire-format Razer Seiren V3 HID report (63 bytes).
///
/// Compact Razer-family report used by the Seiren V3 Chroma microphone.
/// Same field layout as the standard 90-byte [`RazerReport`] but with a
/// smaller args region (53 bytes).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct SeirenV3Report {
    /// Response status (always `0x00` for outgoing requests).
    status: u8,
    /// Transaction ID (always `0x1F`).
    transaction_id: u8,
    /// Remaining packets (always `0x0000`).
    remaining_packets: [u8; 2],
    /// Protocol type marker (always `0x00`).
    protocol_type: u8,
    /// Declared argument payload size.
    data_size: u8,
    /// Command class.
    command_class: u8,
    /// Command ID.
    command_id: u8,
    /// Variable-length argument field (up to 53 bytes).
    args: [u8; 53],
    /// XOR checksum of bytes `[1..61]`.
    crc: u8,
    /// Reserved trailing byte (always `0x00`).
    reserved: u8,
}

/// Write-only protocol for the Seiren V3 Chroma microphone ring.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeirenV3Protocol;

impl SeirenV3Protocol {
    fn encode_color(color: [u8; 3]) -> [u8; 3] {
        // The Seiren V3 ring expects RBG channel order on the wire.
        [color[0], color[2], color[1]]
    }

    fn seiren_crc(report: &SeirenV3Report) -> u8 {
        let bytes = report.as_bytes();
        bytes[1..61].iter().fold(0_u8, |acc, byte| acc ^ byte)
    }

    fn write_packet(
        buffer: &mut Vec<u8>,
        data_size: u8,
        command_class: u8,
        command_id: u8,
        args: &[u8],
    ) {
        let mut report = SeirenV3Report::new_zeroed();
        report.transaction_id = SEIREN_V3_TRANSACTION_ID;
        report.data_size = data_size;
        report.command_class = command_class;
        report.command_id = command_id;
        report.args[..args.len()].copy_from_slice(args);
        report.crc = Self::seiren_crc(&report);

        buffer.extend_from_slice(report.as_bytes());
    }

    fn build_packet(
        data_size: u8,
        command_class: u8,
        command_id: u8,
        args: &[u8],
        post_delay: Duration,
    ) -> ProtocolCommand {
        let mut data = Vec::with_capacity(SEIREN_V3_PAYLOAD_LEN);
        Self::write_packet(&mut data, data_size, command_class, command_id, args);
        ProtocolCommand {
            data,
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay,
            transfer_type: TransferType::Primary,
        }
    }
}

impl Protocol for SeirenV3Protocol {
    fn name(&self) -> &'static str {
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
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
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

        let mut encoder = CommandBuffer::new(commands);
        encoder.push_fill(
            false,
            Duration::ZERO,
            Duration::from_millis(1),
            TransferType::Primary,
            |buffer| Self::write_packet(buffer, FRAME_DATA_SIZE, 0x0F, 0x03, &args),
        );
        encoder.finish();
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
