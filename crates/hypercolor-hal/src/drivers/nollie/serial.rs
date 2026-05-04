use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::protocol::{
    CDC_SERIAL_REPORT_SIZE, GEN1_LEDS_PER_PACKET, LEDS_NOLLIE_1, LEDS_NOLLIE_8, NollieModel,
    NollieProtocol, encode_color,
};

pub(super) fn shutdown_sequence(model: NollieModel) -> Vec<ProtocolCommand> {
    let colors = vec![[0_u8; 3]; expected_leds(model)];
    let mut commands = Vec::new();
    encode_frame_for_model(model, &colors, &mut commands);
    commands
}

pub(super) fn encode_frame_into(
    protocol: &NollieProtocol,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let normalized = protocol.normalize_colors(colors);
    encode_frame_for_model(protocol.model(), normalized.as_ref(), commands);
}

fn encode_frame_for_model(
    model: NollieModel,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let channels = channel_count(model);
    let leds_per_channel = leds_per_channel(model);
    let mut command_buffer = CommandBuffer::new(commands);

    for channel in 0..channels {
        let start = channel * leds_per_channel;
        let end = start + leds_per_channel;
        for (packet_index, chunk) in colors[start..end].chunks(GEN1_LEDS_PER_PACKET).enumerate() {
            let packet_id = u8::try_from(packet_index + channel * 6).unwrap_or(u8::MAX);
            command_buffer.push_fill(
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
                |buffer| {
                    buffer.resize(CDC_SERIAL_REPORT_SIZE, 0);
                    buffer[0] = packet_id;
                    for (index, color) in chunk.iter().enumerate() {
                        let encoded =
                            encode_color(*color, model.brightness_scale(), model.color_format());
                        let offset = 1 + index * 3;
                        buffer[offset..offset + 3].copy_from_slice(&encoded);
                    }
                },
            );
        }
    }

    command_buffer.push_fill(
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Primary,
        fill_show_packet,
    );
    command_buffer.finish();
}

fn fill_show_packet(buffer: &mut Vec<u8>) {
    buffer.resize(CDC_SERIAL_REPORT_SIZE, 0);
    buffer[0] = 0xFF;
}

const fn channel_count(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie1Cdc => 1,
        NollieModel::Nollie8Cdc => 8,
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
        | NollieModel::Nollie16v3
        | NollieModel::Nollie32 { .. }
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
        | NollieModel::NollieL2V12
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => 0,
    }
}

const fn leds_per_channel(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie1Cdc => LEDS_NOLLIE_1,
        NollieModel::Nollie8Cdc => LEDS_NOLLIE_8,
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
        | NollieModel::Nollie16v3
        | NollieModel::Nollie32 { .. }
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
        | NollieModel::NollieL2V12
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => 0,
    }
}

const fn expected_leds(model: NollieModel) -> usize {
    channel_count(model) * leds_per_channel(model)
}
