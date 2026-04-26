use std::time::Duration;

use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::protocol::{
    CHANNELS_NOLLIE_1, CHANNELS_NOLLIE_8, CHANNELS_NOLLIE_28_12, GEN1_HID_REPORT_SIZE,
    GEN1_LEDS_PER_PACKET, LEDS_NOLLIE_1, LEDS_NOLLIE_8, LEDS_NOLLIE_28_12, NollieModel,
    NollieProtocol, command_from_packet, encode_color,
};

const _: () = assert!(
    std::mem::size_of::<Gen1DataPacket>() == GEN1_HID_REPORT_SIZE,
    "Gen1DataPacket must match GEN1_HID_REPORT_SIZE (65 bytes)"
);

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Gen1DataPacket {
    report_id: u8,
    packet_id: u8,
    colors: [u8; 63],
}

pub(super) fn init_sequence() -> Vec<ProtocolCommand> {
    let mut firmware = vec![0_u8; GEN1_HID_REPORT_SIZE];
    firmware[1] = 0xFC;
    firmware[2] = 0x01;

    let mut channels = vec![0_u8; GEN1_HID_REPORT_SIZE];
    channels[1] = 0xFC;
    channels[2] = 0x03;

    let hardware_effect = hardware_effect_packet();

    vec![
        command_from_packet(firmware, true, Duration::ZERO, Duration::ZERO),
        command_from_packet(channels, true, Duration::ZERO, Duration::ZERO),
        command_from_packet(hardware_effect, false, Duration::ZERO, Duration::ZERO),
    ]
}

pub(super) fn shutdown_sequence(model: NollieModel) -> Vec<ProtocolCommand> {
    let mut commands = Vec::new();
    if matches!(model, NollieModel::Nollie1 | NollieModel::Nollie28_12) {
        let mut commit = vec![0_u8; GEN1_HID_REPORT_SIZE];
        commit[1] = 0xFF;
        commands.push(command_from_packet(
            commit,
            false,
            Duration::ZERO,
            Duration::ZERO,
        ));
    }

    commands.push(command_from_packet(
        hardware_effect_packet(),
        false,
        Duration::ZERO,
        Duration::ZERO,
    ));

    let mut hardware_mode = vec![0_u8; GEN1_HID_REPORT_SIZE];
    hardware_mode[1] = 0xFE;
    hardware_mode[2] = 0x01;
    commands.push(command_from_packet(
        hardware_mode,
        false,
        Duration::ZERO,
        Duration::ZERO,
    ));

    commands
}

pub(super) fn encode_frame_into(
    protocol: &NollieProtocol,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let normalized = protocol.normalize_colors(colors);
    let normalized = normalized.as_ref();
    let model = protocol.model();
    let channel_count = channel_count(model);
    let leds_per_channel = leds_per_channel(model);
    let packet_interval = packet_interval(model);

    let mut command_buffer = CommandBuffer::new(commands);

    for channel in 0..channel_count {
        let start = channel * leds_per_channel;
        let end = start + leds_per_channel;
        for (packet_index, chunk) in normalized[start..end]
            .chunks(GEN1_LEDS_PER_PACKET)
            .enumerate()
        {
            let mut packet = Gen1DataPacket::new_zeroed();
            packet.packet_id =
                u8::try_from(packet_index + channel * packet_interval).unwrap_or(u8::MAX);

            for (index, color) in chunk.iter().enumerate() {
                let encoded = encode_color(*color, model.brightness_scale(), model.color_format());
                let offset = index * 3;
                packet.colors[offset..offset + 3].copy_from_slice(&encoded);
            }

            command_buffer.push_struct(
                &packet,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
        }
    }

    if emits_render_commit(model) {
        let mut commit = Gen1DataPacket::new_zeroed();
        commit.packet_id = 0xFF;
        command_buffer.push_struct(
            &commit,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
    }

    command_buffer.finish();
}

fn hardware_effect_packet() -> Vec<u8> {
    let mut hardware_effect = vec![0_u8; GEN1_HID_REPORT_SIZE];
    hardware_effect[1] = 0xFE;
    hardware_effect[2] = 0x02;
    hardware_effect[7] = 0x64;
    hardware_effect[8] = 0x0A;
    hardware_effect[10] = 0x01;
    hardware_effect
}

const fn channel_count(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie1 => CHANNELS_NOLLIE_1,
        NollieModel::Nollie8 | NollieModel::Prism8 => CHANNELS_NOLLIE_8,
        NollieModel::Nollie28_12 => CHANNELS_NOLLIE_28_12,
        NollieModel::Nollie16v3 | NollieModel::Nollie32 { .. } => 0,
    }
}

const fn leds_per_channel(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie1 => LEDS_NOLLIE_1,
        NollieModel::Nollie8 | NollieModel::Prism8 => LEDS_NOLLIE_8,
        NollieModel::Nollie28_12 => LEDS_NOLLIE_28_12,
        NollieModel::Nollie16v3 | NollieModel::Nollie32 { .. } => 0,
    }
}

const fn packet_interval(model: NollieModel) -> usize {
    match model {
        NollieModel::Nollie1 => 30,
        NollieModel::Nollie8 | NollieModel::Prism8 => 6,
        NollieModel::Nollie28_12 => 2,
        NollieModel::Nollie16v3 | NollieModel::Nollie32 { .. } => 0,
    }
}

const fn emits_render_commit(model: NollieModel) -> bool {
    matches!(model, NollieModel::Nollie8 | NollieModel::Prism8)
}
