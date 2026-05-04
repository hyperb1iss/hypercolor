use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::protocol::{
    CHANNELS_NOLLIE_4, CHANNELS_NOLLIE_8, GEN1_HID_REPORT_SIZE, LEDS_NOLLIE_4, LEDS_NOLLIE_8_YOUTH,
    NollieModel, NollieProtocol, STREAM65_LEDS_PER_PACKET, encode_color,
};

pub(super) fn encode_frame_into(
    protocol: &NollieProtocol,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let normalized = protocol.normalize_colors(colors);
    let channels = channel_count(protocol.model());
    let leds_per_channel = leds_per_channel(protocol.model());
    let counts = [u16::try_from(leds_per_channel).unwrap_or(u16::MAX); CHANNELS_NOLLIE_8];

    let mut command_buffer = CommandBuffer::new(commands);
    command_buffer.push_fill(
        false,
        Duration::ZERO,
        Duration::from_millis(200),
        TransferType::Primary,
        |buffer| {
            buffer.resize(GEN1_HID_REPORT_SIZE, 0);
            fill_count_config_packet(buffer, channels, counts);
        },
    );

    let mut packet_index = 0_u8;
    for channel in 0..channels {
        let start = channel * leds_per_channel;
        let end = start + leds_per_channel;
        for chunk in normalized.as_ref()[start..end].chunks(STREAM65_LEDS_PER_PACKET) {
            let packet_id = packet_index;
            packet_index = packet_index.saturating_add(1);
            command_buffer.push_fill(
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
                |buffer| {
                    buffer.resize(GEN1_HID_REPORT_SIZE, 0);
                    buffer[1] = packet_id;
                    for (index, color) in chunk.iter().enumerate() {
                        let encoded = encode_color(
                            *color,
                            protocol.model().brightness_scale(),
                            protocol.model().color_format(),
                        );
                        let offset = 2 + index * 3;
                        buffer[offset..offset + 3].copy_from_slice(&encoded);
                    }
                },
            );
        }
    }

    command_buffer.finish();
}

fn fill_count_config_packet(packet: &mut [u8], channels: usize, counts: [u16; CHANNELS_NOLLIE_8]) {
    packet[1] = 0x86;
    for (index, count) in counts.iter().copied().take(channels).enumerate() {
        let offset = 3 + index * 2;
        packet[offset] = u8::try_from(count >> 8).unwrap_or(u8::MAX);
        packet[offset + 1] = u8::try_from(count & 0x00FF).unwrap_or(u8::MAX);
    }
}

const fn channel_count(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie4 => CHANNELS_NOLLIE_4,
        NollieModel::Nollie8Youth => CHANNELS_NOLLIE_8,
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
        | NollieModel::Nollie16v3
        | NollieModel::Nollie32 { .. }
        | NollieModel::Nollie1Cdc
        | NollieModel::Nollie8Cdc
        | NollieModel::Nollie16v3Nos2
        | NollieModel::Nollie32Nos2
        | NollieModel::NollieMatrix
        | NollieModel::NollieLegacy8
        | NollieModel::NollieLegacy2
        | NollieModel::NollieLegacyTt
        | NollieModel::NollieLegacy16_1
        | NollieModel::NollieLegacy16_2
        | NollieModel::NollieLegacy28_12
        | NollieModel::NollieLegacy28L1
        | NollieModel::NollieLegacy28L2
        | NollieModel::Nollie8V12
        | NollieModel::Nollie16_1V12
        | NollieModel::Nollie16_2V12
        | NollieModel::NollieL1V12
        | NollieModel::NollieL2V12 => 0,
    }
}

const fn leds_per_channel(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie4 => LEDS_NOLLIE_4,
        NollieModel::Nollie8Youth => LEDS_NOLLIE_8_YOUTH,
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
        | NollieModel::Nollie16v3
        | NollieModel::Nollie32 { .. }
        | NollieModel::Nollie1Cdc
        | NollieModel::Nollie8Cdc
        | NollieModel::Nollie16v3Nos2
        | NollieModel::Nollie32Nos2
        | NollieModel::NollieMatrix
        | NollieModel::NollieLegacy8
        | NollieModel::NollieLegacy2
        | NollieModel::NollieLegacyTt
        | NollieModel::NollieLegacy16_1
        | NollieModel::NollieLegacy16_2
        | NollieModel::NollieLegacy28_12
        | NollieModel::NollieLegacy28L1
        | NollieModel::NollieLegacy28L2
        | NollieModel::Nollie8V12
        | NollieModel::Nollie16_1V12
        | NollieModel::Nollie16_2V12
        | NollieModel::NollieL1V12
        | NollieModel::NollieL2V12 => 0,
    }
}
