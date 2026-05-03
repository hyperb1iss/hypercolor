use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::gen2::{
    NOLLIE32_ATX_CABLE_REMAP, NOLLIE32_GPU_CABLE_REMAP, NOLLIE32_MAIN_CHANNEL_REMAP,
};
use super::protocol::{
    GEN2_COLOR_REPORT_SIZE, GpuCableType, LEDS_ATX_STRIMER, LEDS_GEN2_CHANNEL, Nollie32Config,
    NollieModel, NollieProtocol, encode_color,
};

const NOLLIE16V3_NOS2_CHANNEL_REMAP: [u8; 16] =
    [3, 2, 1, 0, 8, 9, 10, 11, 4, 5, 6, 7, 15, 14, 13, 12];

#[derive(Debug, Clone)]
struct DirectEntry {
    physical: u8,
    led_count: u16,
    color_start: usize,
}

pub(super) fn encode_frame_into(
    protocol: &NollieProtocol,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let normalized = protocol.normalize_colors(colors);
    let mut entries = build_entries(protocol.model(), protocol.nollie32_config());
    entries.sort_by_key(|entry| entry.physical);
    let (max_low, max_high) = marker_channels(&entries);

    let mut command_buffer = CommandBuffer::new(commands);
    for entry in entries {
        command_buffer.push_fill(
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
            |buffer| {
                buffer.resize(GEN2_COLOR_REPORT_SIZE, 0);
                buffer[1] = entry.physical;
                buffer[2] = marker_for_entry(entry.physical, max_low, max_high);
                encode_entry_colors(buffer, &entry, normalized.as_ref());
            },
        );
    }
    command_buffer.finish();
}

fn build_entries(model: NollieModel, config: Nollie32Config) -> Vec<DirectEntry> {
    let mut entries = Vec::new();
    let mut cursor = 0;

    match model {
        NollieModel::Nollie16v3Nos2 => {
            for physical in NOLLIE16V3_NOS2_CHANNEL_REMAP {
                push_entry(&mut entries, physical, cursor, LEDS_GEN2_CHANNEL);
                cursor += LEDS_GEN2_CHANNEL;
            }
        }
        NollieModel::Nollie32Nos2 => {
            for physical in NOLLIE32_MAIN_CHANNEL_REMAP {
                push_entry(&mut entries, physical, cursor, LEDS_GEN2_CHANNEL);
                cursor += LEDS_GEN2_CHANNEL;
            }
            if config.atx_cable_present {
                for (row, physical) in NOLLIE32_ATX_CABLE_REMAP.iter().copied().enumerate() {
                    push_entry(&mut entries, physical, cursor + row * 20, 20);
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
                    push_entry(&mut entries, physical, cursor + row * 27, 27);
                }
            }
        }
        NollieModel::Nollie1
        | NollieModel::Nollie8
        | NollieModel::Nollie28_12
        | NollieModel::Prism8
        | NollieModel::Nollie16v3
        | NollieModel::Nollie32 { .. }
        | NollieModel::Nollie1Cdc
        | NollieModel::Nollie8Cdc
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

    entries
}

fn push_entry(entries: &mut Vec<DirectEntry>, physical: u8, color_start: usize, led_count: usize) {
    entries.push(DirectEntry {
        physical,
        led_count: u16::try_from(led_count).unwrap_or(u16::MAX),
        color_start,
    });
}

fn marker_channels(entries: &[DirectEntry]) -> (Option<u8>, Option<u8>) {
    let mut max_low: Option<u8> = None;
    let mut max_high: Option<u8> = None;

    for entry in entries {
        if entry.physical <= 15 {
            max_low = Some(max_low.map_or(entry.physical, |value| value.max(entry.physical)));
        } else {
            max_high = Some(max_high.map_or(entry.physical, |value| value.max(entry.physical)));
        }
    }

    (max_low, max_high)
}

fn marker_for_entry(physical: u8, max_low: Option<u8>, max_high: Option<u8>) -> u8 {
    if max_low == Some(physical) {
        return 1;
    }
    if max_high == Some(physical) {
        return 2;
    }
    0
}

fn encode_entry_colors(buffer: &mut [u8], entry: &DirectEntry, colors: &[[u8; 3]]) {
    let start = entry.color_start;
    let end = start + usize::from(entry.led_count);
    let mut cursor = 5;

    for color in &colors[start..end] {
        let encoded = encode_color(
            *color,
            1.0,
            hypercolor_types::device::DeviceColorFormat::Grb,
        );
        buffer[cursor..cursor + 3].copy_from_slice(&encoded);
        cursor += 3;
    }
}
