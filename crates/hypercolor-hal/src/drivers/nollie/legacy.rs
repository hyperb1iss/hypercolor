use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::protocol::{
    GEN1_HID_REPORT_SIZE, GEN1_LEDS_PER_PACKET, LEDS_NOLLIE_28_12, LEDS_NOLLIE_LEGACY_2,
    LEDS_NOLLIE_LEGACY_8, LEDS_NOLLIE_LEGACY_28_12, LEDS_NOLLIE_MATRIX, LEDS_NOLLIE_V12_HIGH,
    LEGACY_LEDS_PER_PACKET, NollieModel, NollieProtocol, NollieProtocolKind, command_from_packet,
    encode_color,
};

#[derive(Debug, Clone, Copy)]
struct DenseSpec {
    channels: usize,
    leds_per_channel: usize,
    packet_interval: usize,
    emits_commit: bool,
    init_hardware_mode: bool,
}

#[derive(Debug, Clone, Copy)]
struct HeaderSpec {
    channels: usize,
    leds_per_channel: usize,
    channel_count_marker: u8,
    emits_shutdown_commit: bool,
}

pub(super) fn init_sequence(model: NollieModel) -> Vec<ProtocolCommand> {
    match model.protocol_kind() {
        NollieProtocolKind::LegacyHeader => {
            let mut packet = vec![0_u8; GEN1_HID_REPORT_SIZE];
            packet[2] = 0x01;
            vec![command_from_packet(
                packet,
                false,
                Duration::ZERO,
                Duration::ZERO,
            )]
        }
        NollieProtocolKind::DenseGen1 if dense_spec(model).init_hardware_mode => {
            vec![command_from_packet(
                hardware_mode_packet(),
                false,
                Duration::ZERO,
                Duration::ZERO,
            )]
        }
        NollieProtocolKind::DenseGen1 => Vec::new(),
        NollieProtocolKind::ModernGen1
        | NollieProtocolKind::SerialCdc
        | NollieProtocolKind::Gen2Grouped
        | NollieProtocolKind::Nos2Hid
        | NollieProtocolKind::Stream65 => Vec::new(),
    }
}

pub(super) fn shutdown_sequence(model: NollieModel) -> Vec<ProtocolCommand> {
    let colors = vec![[0_u8; 3]; model.expected_leds(super::protocol::Nollie32Config::default())];
    let mut commands = Vec::new();
    encode_frame_into_model(model, &colors, &mut commands);

    match model.protocol_kind() {
        NollieProtocolKind::DenseGen1 if dense_spec(model).init_hardware_mode => {
            commands.push(command_from_packet(
                hardware_mode_packet(),
                false,
                Duration::ZERO,
                Duration::ZERO,
            ));
        }
        NollieProtocolKind::LegacyHeader if header_spec(model).emits_shutdown_commit => {
            commands.push(command_from_packet(
                legacy_shutdown_commit(),
                false,
                Duration::ZERO,
                Duration::ZERO,
            ));
        }
        NollieProtocolKind::ModernGen1
        | NollieProtocolKind::DenseGen1
        | NollieProtocolKind::LegacyHeader
        | NollieProtocolKind::SerialCdc
        | NollieProtocolKind::Gen2Grouped
        | NollieProtocolKind::Nos2Hid
        | NollieProtocolKind::Stream65 => {}
    }

    commands
}

pub(super) fn encode_frame_into(
    protocol: &NollieProtocol,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let normalized = protocol.normalize_colors(colors);
    encode_frame_into_model(protocol.model(), normalized.as_ref(), commands);
}

fn encode_frame_into_model(
    model: NollieModel,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    match model.protocol_kind() {
        NollieProtocolKind::DenseGen1 => encode_dense_frame_into(model, colors, commands),
        NollieProtocolKind::LegacyHeader => encode_header_frame_into(model, colors, commands),
        NollieProtocolKind::ModernGen1
        | NollieProtocolKind::SerialCdc
        | NollieProtocolKind::Gen2Grouped
        | NollieProtocolKind::Nos2Hid
        | NollieProtocolKind::Stream65 => commands.clear(),
    }
}

fn encode_dense_frame_into(
    model: NollieModel,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let spec = dense_spec(model);
    let mut command_buffer = CommandBuffer::new(commands);

    for channel in 0..spec.channels {
        let start = channel * spec.leds_per_channel;
        let end = start + spec.leds_per_channel;
        for (packet_index, chunk) in colors[start..end].chunks(GEN1_LEDS_PER_PACKET).enumerate() {
            let packet_id =
                u8::try_from(packet_index + channel * spec.packet_interval).unwrap_or(u8::MAX);
            command_buffer.push_fill(
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
                |buffer| {
                    buffer.resize(GEN1_HID_REPORT_SIZE, 0);
                    buffer[1] = packet_id;
                    copy_encoded_colors(&mut buffer[2..], chunk, model);
                },
            );
        }
    }

    if spec.emits_commit {
        command_buffer.push_fill(
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
            fill_frame_commit_packet,
        );
    }

    command_buffer.finish();
}

fn encode_header_frame_into(
    model: NollieModel,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let spec = header_spec(model);
    let mut command_buffer = CommandBuffer::new(commands);

    for channel in 0..spec.channels {
        let start = channel * spec.leds_per_channel;
        let end = start + spec.leds_per_channel;
        let num_packets = spec.leds_per_channel.div_ceil(LEGACY_LEDS_PER_PACKET);
        for (packet_index, chunk) in colors[start..end]
            .chunks(LEGACY_LEDS_PER_PACKET)
            .enumerate()
        {
            let packet_id = u8::try_from(packet_index + 1).unwrap_or(u8::MAX);
            let packet_count = u8::try_from(num_packets).unwrap_or(u8::MAX);
            let channel_id = u8::try_from(channel + 1).unwrap_or(u8::MAX);
            command_buffer.push_fill(
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
                |buffer| {
                    buffer.resize(GEN1_HID_REPORT_SIZE, 0);
                    buffer[1] = packet_id;
                    buffer[2] = spec.channel_count_marker;
                    buffer[3] = packet_count;
                    buffer[4] = channel_id;
                    copy_encoded_colors(&mut buffer[5..], chunk, model);
                },
            );
        }
    }

    command_buffer.finish();
}

fn copy_encoded_colors(target: &mut [u8], colors: &[[u8; 3]], model: NollieModel) {
    for (index, color) in colors.iter().enumerate() {
        let encoded = encode_color(*color, model.brightness_scale(), model.color_format());
        let offset = index * 3;
        target[offset..offset + 3].copy_from_slice(&encoded);
    }
}

fn dense_spec(model: NollieModel) -> DenseSpec {
    match model {
        NollieModel::Nollie28_12 => DenseSpec {
            channels: 12,
            leds_per_channel: LEDS_NOLLIE_28_12,
            packet_interval: 2,
            emits_commit: true,
            init_hardware_mode: true,
        },
        NollieModel::NollieMatrix => DenseSpec {
            channels: 1,
            leds_per_channel: LEDS_NOLLIE_MATRIX,
            packet_interval: 2,
            emits_commit: false,
            init_hardware_mode: false,
        },
        NollieModel::Nollie8V12
        | NollieModel::Nollie16_1V12
        | NollieModel::Nollie16_2V12
        | NollieModel::NollieL1V12
        | NollieModel::NollieL2V12 => DenseSpec {
            channels: 8,
            leds_per_channel: LEDS_NOLLIE_V12_HIGH,
            packet_interval: 25,
            emits_commit: true,
            init_hardware_mode: true,
        },
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Prism8
        | NollieModel::Nollie16v3
        | NollieModel::Nollie32 { .. }
        | NollieModel::Nollie1Cdc
        | NollieModel::Nollie8Cdc
        | NollieModel::Nollie16v3Nos2
        | NollieModel::Nollie32Nos2
        | NollieModel::NollieLegacy8
        | NollieModel::NollieLegacy2
        | NollieModel::NollieLegacyTt
        | NollieModel::NollieLegacy16_1
        | NollieModel::NollieLegacy16_2
        | NollieModel::NollieLegacy28_12
        | NollieModel::NollieLegacy28L1
        | NollieModel::NollieLegacy28L2
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => DenseSpec {
            channels: 0,
            leds_per_channel: 0,
            packet_interval: 0,
            emits_commit: false,
            init_hardware_mode: false,
        },
    }
}

fn header_spec(model: NollieModel) -> HeaderSpec {
    match model {
        NollieModel::NollieLegacy8
        | NollieModel::NollieLegacy16_1
        | NollieModel::NollieLegacy16_2
        | NollieModel::NollieLegacy28L1
        | NollieModel::NollieLegacy28L2 => HeaderSpec {
            channels: 8,
            leds_per_channel: LEDS_NOLLIE_LEGACY_8,
            channel_count_marker: 8,
            emits_shutdown_commit: true,
        },
        NollieModel::NollieLegacy2 | NollieModel::NollieLegacyTt => HeaderSpec {
            channels: 2,
            leds_per_channel: LEDS_NOLLIE_LEGACY_2,
            channel_count_marker: 0,
            emits_shutdown_commit: false,
        },
        NollieModel::NollieLegacy28_12 => HeaderSpec {
            channels: 12,
            leds_per_channel: LEDS_NOLLIE_LEGACY_28_12,
            channel_count_marker: 0,
            emits_shutdown_commit: false,
        },
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
        | NollieModel::Nollie8V12
        | NollieModel::Nollie16_1V12
        | NollieModel::Nollie16_2V12
        | NollieModel::NollieL1V12
        | NollieModel::NollieL2V12
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => HeaderSpec {
            channels: 0,
            leds_per_channel: 0,
            channel_count_marker: 0,
            emits_shutdown_commit: false,
        },
    }
}

fn hardware_mode_packet() -> Vec<u8> {
    let mut packet = vec![0_u8; GEN1_HID_REPORT_SIZE];
    packet[1] = 0xFE;
    packet[2] = 0x01;
    packet
}

fn frame_commit_packet() -> Vec<u8> {
    let mut packet = vec![0_u8; GEN1_HID_REPORT_SIZE];
    packet[1] = 0xFF;
    packet
}

fn fill_frame_commit_packet(buffer: &mut Vec<u8>) {
    buffer.resize(GEN1_HID_REPORT_SIZE, 0);
    buffer[1] = 0xFF;
}

fn legacy_shutdown_commit() -> Vec<u8> {
    frame_commit_packet()
}
