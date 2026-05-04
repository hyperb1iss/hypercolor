use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::protocol::{
    GEN2_COLOR_REPORT_SIZE, GEN2_PHYSICAL_CHANNELS, GEN2_SETTINGS_REPORT_SIZE, GpuCableType,
    LEDS_ATX_STRIMER, LEDS_GEN2_CHANNEL, Nollie32Config, NollieModel, NollieProtocol,
    ProtocolVersion, command_from_packet, encode_color,
};

const GROUP_LED_CAP: u16 = 340;
const FLAG1_CHANNEL: u8 = 15;
const FLAG2_CHANNEL: u8 = 31;
const SETTINGS_DELAY: Duration = Duration::from_millis(50);
const FLAG_BOUNDARY_DELAY: Duration = Duration::from_millis(8);

pub const NOLLIE16V3_CHANNEL_REMAP: [u8; 16] = [
    19, 18, 17, 16, 24, 25, 26, 27, 20, 21, 22, 23, 31, 30, 29, 28,
];

pub const NOLLIE32_MAIN_CHANNEL_REMAP: [u8; 20] = [
    5, 4, 3, 2, 1, 0, 15, 14, 26, 27, 28, 29, 30, 31, 8, 9, 13, 12, 11, 10,
];

pub const NOLLIE32_ATX_CABLE_REMAP: [u8; 6] = [19, 18, 17, 16, 7, 6];
pub const NOLLIE32_GPU_CABLE_REMAP: [u8; 6] = [25, 24, 23, 22, 21, 20];

#[derive(Debug, Clone, Copy)]
struct ChannelEntry {
    physical: u8,
    led_count: u16,
    color_start: usize,
}

#[derive(Debug, Clone, Copy)]
struct Group {
    start_index: usize,
    end_index: usize,
    marker: u8,
}

const EMPTY_CHANNEL_ENTRY: ChannelEntry = ChannelEntry {
    physical: 0,
    led_count: 0,
    color_start: 0,
};

const EMPTY_GROUP: Group = Group {
    start_index: 0,
    end_index: 0,
    marker: 0,
};

pub(super) fn init_sequence(model: NollieModel, config: Nollie32Config) -> Vec<ProtocolCommand> {
    let counts = default_counts(model, config);
    let mut commands = Vec::new();
    push_count_config(counts, &mut commands);
    commands.push(settings_command(config, [0, 0, 0]));
    commands
}

pub(super) fn shutdown_sequence(config: Nollie32Config) -> Vec<ProtocolCommand> {
    vec![
        settings_command(config, [0, 0, 0]),
        shutdown_latch_command(),
    ]
}

pub(super) fn encode_frame_into(
    protocol: &NollieProtocol,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let normalized = protocol.normalize_colors(colors);
    let (entries, entry_count) = build_entries(protocol.model(), protocol.nollie32_config());
    let entries = &entries[..entry_count];
    let counts = counts_for_entries(entries);
    let mut command_buffer = CommandBuffer::new(commands);
    if protocol.gen2_counts_changed(counts) {
        push_count_config_into(counts, &mut command_buffer);
    }

    match protocol.model() {
        NollieModel::Nollie32 {
            protocol_version: ProtocolVersion::V1,
        } => push_v1_entries(entries, normalized.as_ref(), &mut command_buffer),
        NollieModel::Nollie16v3 | NollieModel::Nollie32 { .. } => {
            push_v2_entries(
                protocol.model(),
                entries,
                normalized.as_ref(),
                &mut command_buffer,
            );
        }
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
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
        | NollieModel::NollieL2V12
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => {}
    }

    command_buffer.finish();
}

pub(super) fn push_count_config(
    counts: [u16; GEN2_PHYSICAL_CHANNELS],
    commands: &mut Vec<ProtocolCommand>,
) {
    let mut packet = vec![0_u8; GEN2_COLOR_REPORT_SIZE];
    packet[1] = 0x88;
    for (index, count) in counts.iter().copied().enumerate() {
        let offset = 2 + index * 2;
        packet[offset] = u8::try_from(count >> 8).unwrap_or(u8::MAX);
        packet[offset + 1] = u8::try_from(count & 0x00FF).unwrap_or(u8::MAX);
    }
    commands.push(command_from_packet(
        packet,
        false,
        Duration::ZERO,
        Duration::ZERO,
    ));
}

fn push_count_config_into(
    counts: [u16; GEN2_PHYSICAL_CHANNELS],
    command_buffer: &mut CommandBuffer<'_>,
) {
    command_buffer.push_fill(
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Primary,
        |buffer| {
            buffer.resize(GEN2_COLOR_REPORT_SIZE, 0);
            fill_count_config_packet(buffer, counts);
        },
    );
}

fn fill_count_config_packet(buffer: &mut [u8], counts: [u16; GEN2_PHYSICAL_CHANNELS]) {
    buffer[1] = 0x88;
    for (index, count) in counts.iter().copied().enumerate() {
        let offset = 2 + index * 2;
        buffer[offset] = u8::try_from(count >> 8).unwrap_or(u8::MAX);
        buffer[offset + 1] = u8::try_from(count & 0x00FF).unwrap_or(u8::MAX);
    }
}

fn build_entries(
    model: NollieModel,
    config: Nollie32Config,
) -> ([ChannelEntry; GEN2_PHYSICAL_CHANNELS], usize) {
    let mut by_physical = [EMPTY_CHANNEL_ENTRY; GEN2_PHYSICAL_CHANNELS];
    let mut cursor = 0;

    match model {
        NollieModel::Nollie16v3 => {
            for physical in NOLLIE16V3_CHANNEL_REMAP {
                push_entry(&mut by_physical, physical, cursor, LEDS_GEN2_CHANNEL);
                cursor += LEDS_GEN2_CHANNEL;
            }
        }
        NollieModel::Nollie32 { .. } => {
            for physical in NOLLIE32_MAIN_CHANNEL_REMAP {
                push_entry(&mut by_physical, physical, cursor, LEDS_GEN2_CHANNEL);
                cursor += LEDS_GEN2_CHANNEL;
            }
            if config.atx_cable_present {
                for (row, physical) in NOLLIE32_ATX_CABLE_REMAP.iter().copied().enumerate() {
                    let row_start = cursor + row * 20;
                    push_entry(&mut by_physical, physical, row_start, 20);
                }
                cursor += LEDS_ATX_STRIMER;
            }
            if config.gpu_cable_type != GpuCableType::None {
                for (row, physical) in NOLLIE32_GPU_CABLE_REMAP
                    .iter()
                    .copied()
                    .take(config.gpu_cable_type.rows())
                    .enumerate()
                {
                    let row_start = cursor + row * 27;
                    push_entry(&mut by_physical, physical, row_start, 27);
                }
            }
        }
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
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
        | NollieModel::NollieL2V12
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => {}
    }

    compact_entries(&by_physical)
}

fn push_entry(
    entries: &mut [ChannelEntry; GEN2_PHYSICAL_CHANNELS],
    physical: u8,
    color_start: usize,
    led_count: usize,
) {
    entries[usize::from(physical)] = ChannelEntry {
        physical,
        led_count: u16::try_from(led_count).unwrap_or(u16::MAX),
        color_start,
    };
}

fn compact_entries(
    by_physical: &[ChannelEntry; GEN2_PHYSICAL_CHANNELS],
) -> ([ChannelEntry; GEN2_PHYSICAL_CHANNELS], usize) {
    let mut entries = [EMPTY_CHANNEL_ENTRY; GEN2_PHYSICAL_CHANNELS];
    let mut len = 0;
    for entry in by_physical.iter().copied() {
        if entry.led_count != 0 {
            entries[len] = entry;
            len += 1;
        }
    }
    (entries, len)
}

fn push_v2_entries(
    model: NollieModel,
    entries: &[ChannelEntry],
    colors: &[[u8; 3]],
    command_buffer: &mut CommandBuffer<'_>,
) {
    let (groups, group_count) = groups_for_entries(model, entries);
    for group in &groups[..group_count] {
        command_buffer.push_fill(
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
            |buffer| {
                buffer.resize(GEN2_COLOR_REPORT_SIZE, 0);
                buffer[1] = 0x40;
                buffer[2] = entries[group.start_index].physical;
                buffer[3] = entries[group.end_index].physical;
                buffer[4] = group.marker;

                let mut cursor = 5;
                for entry in &entries[group.start_index..=group.end_index] {
                    cursor = encode_entry_colors(buffer, cursor, entry, colors);
                }
            },
        );
    }
}

fn push_v1_entries(
    entries: &[ChannelEntry],
    colors: &[[u8; 3]],
    command_buffer: &mut CommandBuffer<'_>,
) {
    for (index, entry) in entries.iter().enumerate() {
        let post_delay = if entry.physical < 16
            && entries
                .get(index + 1)
                .is_some_and(|next| next.physical >= 16)
        {
            FLAG_BOUNDARY_DELAY
        } else {
            Duration::ZERO
        };

        command_buffer.push_fill(
            false,
            Duration::ZERO,
            post_delay,
            TransferType::Primary,
            |buffer| {
                buffer.resize(GEN2_COLOR_REPORT_SIZE, 0);
                buffer[1] = entry.physical;
                buffer[2] = marker_for_v1(entry.physical);
                buffer[3] = u8::try_from(entry.led_count >> 8).unwrap_or(u8::MAX);
                buffer[4] = u8::try_from(entry.led_count & 0x00FF).unwrap_or(u8::MAX);
                let _ = encode_entry_colors(buffer, 5, entry, colors);
            },
        );
    }
}

fn encode_entry_colors(
    buffer: &mut [u8],
    mut cursor: usize,
    entry: &ChannelEntry,
    colors: &[[u8; 3]],
) -> usize {
    let start = entry.color_start;
    let end = start + usize::from(entry.led_count);
    for color in &colors[start..end] {
        let encoded = encode_color(
            *color,
            1.0,
            hypercolor_types::device::DeviceColorFormat::Grb,
        );
        buffer[cursor..cursor + 3].copy_from_slice(&encoded);
        cursor += 3;
    }
    cursor
}

fn groups_for_entries(
    model: NollieModel,
    entries: &[ChannelEntry],
) -> ([Group; GEN2_PHYSICAL_CHANNELS], usize) {
    let mut groups = [EMPTY_GROUP; GEN2_PHYSICAL_CHANNELS];
    let mut group_count = 0;
    if matches!(model, NollieModel::Nollie32 { .. }) {
        push_groups_for_range(entries, 0..16, &mut groups, &mut group_count);
        let lower_group_count = group_count;
        push_groups_for_range(entries, 16..32, &mut groups, &mut group_count);
        assign_markers(&mut groups[..group_count], lower_group_count);
    } else {
        push_groups_for_range(entries, 16..32, &mut groups, &mut group_count);
        assign_markers(&mut groups[..group_count], 0);
    }
    (groups, group_count)
}

fn push_groups_for_range(
    entries: &[ChannelEntry],
    physical_range: std::ops::Range<u8>,
    groups: &mut [Group; GEN2_PHYSICAL_CHANNELS],
    group_count: &mut usize,
) {
    let mut index = 0;
    while index < entries.len() {
        if !physical_range.contains(&entries[index].physical) || entries[index].led_count == 0 {
            index += 1;
            continue;
        }

        let start_index = index;
        let mut end_index = index;
        let mut group_leds = entries[index].led_count;
        index += 1;

        while index < entries.len()
            && physical_range.contains(&entries[index].physical)
            && entries[index].led_count != 0
            && group_leds.saturating_add(entries[index].led_count) <= GROUP_LED_CAP
        {
            group_leds = group_leds.saturating_add(entries[index].led_count);
            end_index = index;
            index += 1;
        }

        groups[*group_count] = Group {
            start_index,
            end_index,
            marker: 0,
        };
        *group_count += 1;
    }
}

fn assign_markers(groups: &mut [Group], lower_group_count: usize) {
    if groups.is_empty() {
        return;
    }

    if lower_group_count > 0 && lower_group_count < groups.len() {
        groups[lower_group_count - 1].marker = 1;
    }

    if let Some(last) = groups.last_mut() {
        last.marker = 2;
    }
}

fn counts_for_entries(entries: &[ChannelEntry]) -> [u16; GEN2_PHYSICAL_CHANNELS] {
    let mut counts = [0_u16; GEN2_PHYSICAL_CHANNELS];
    for entry in entries {
        counts[usize::from(entry.physical)] = entry.led_count;
    }
    counts
}

fn default_counts(model: NollieModel, config: Nollie32Config) -> [u16; GEN2_PHYSICAL_CHANNELS] {
    let mut counts = [0_u16; GEN2_PHYSICAL_CHANNELS];
    match model {
        NollieModel::Nollie16v3 => {
            for physical in NOLLIE16V3_CHANNEL_REMAP {
                counts[usize::from(physical)] = LEDS_GEN2_CHANNEL as u16;
            }
        }
        NollieModel::Nollie32 { .. } => {
            for physical in NOLLIE32_MAIN_CHANNEL_REMAP {
                counts[usize::from(physical)] = LEDS_GEN2_CHANNEL as u16;
            }
            if config.atx_cable_present {
                for physical in NOLLIE32_ATX_CABLE_REMAP {
                    counts[usize::from(physical)] = 20;
                }
            }
            for physical in NOLLIE32_GPU_CABLE_REMAP
                .iter()
                .copied()
                .take(config.gpu_cable_type.rows())
            {
                counts[usize::from(physical)] = 27;
            }
        }
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
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
        | NollieModel::NollieL2V12
        | NollieModel::Nollie4
        | NollieModel::Nollie8Youth => {}
    }
    counts
}

fn settings_command(config: Nollie32Config, idle_color: [u8; 3]) -> ProtocolCommand {
    let mut packet = vec![0_u8; GEN2_SETTINGS_REPORT_SIZE];
    packet[1] = 0x80;
    packet[2] = config.gpu_cable_type.mos_byte();
    packet[3] = 0x03;
    packet[4] = idle_color[0];
    packet[5] = idle_color[1];
    packet[6] = idle_color[2];
    command_from_packet(packet, false, Duration::ZERO, SETTINGS_DELAY)
}

fn shutdown_latch_command() -> ProtocolCommand {
    let mut packet = vec![0_u8; GEN2_SETTINGS_REPORT_SIZE];
    packet[1] = 0xFF;
    command_from_packet(packet, false, Duration::ZERO, SETTINGS_DELAY)
}

const fn marker_for_v1(physical: u8) -> u8 {
    match physical {
        FLAG1_CHANNEL => 1,
        FLAG2_CHANNEL => 2,
        _ => 0,
    }
}
