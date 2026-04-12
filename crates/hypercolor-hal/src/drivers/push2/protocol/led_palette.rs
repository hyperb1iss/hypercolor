//! Push 2 LED palette encoding and caching.
//!
//! The Push 2 uses a 128-entry RGBW palette stored on-device. Each LED is
//! addressed by palette index rather than raw color, so the host must manage
//! slot assignment, white-button quantization, and factory palette restoration.

use std::collections::HashMap;
use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, ProtocolError, TransferType};

use super::{
    PAD_NOTE_MAP, PUSH2_CMD_SET_TOUCH_STRIP_LEDS, PUSH2_MIDI_LED_COUNT, PUSH2_PAD_COUNT,
    PUSH2_PALETTE_SIZE, PUSH2_REAPPLY_PALETTE_MESSAGE, PUSH2_RGB_BUTTON_COUNT, PUSH2_RGB_LED_COUNT,
    PUSH2_RGB_SLOT_LIMIT, PUSH2_TOUCH_STRIP_LED_COUNT, PUSH2_WHITE_BUTTON_COUNT,
    PUSH2_WHITE_SLOT_COUNT, PUSH2_WHITE_SLOT_START, Push2State, RGB_BUTTON_CC_MAP,
    WHITE_BUTTON_CC_MAP, decode_sysex_byte, primary_command, primary_command_slice,
    set_palette_entry_message,
};

pub(super) fn restore_factory_palette_commands(state: &mut Push2State) -> Vec<ProtocolCommand> {
    let mut commands = Vec::new();
    let mut restored_any = false;

    for (index, is_valid) in state.factory_palette_valid.iter().copied().enumerate() {
        if !is_valid {
            continue;
        }

        let factory = state.factory_palette[index];
        if state.palette[index] == factory {
            continue;
        }

        let message = set_palette_entry_message(u8::try_from(index).unwrap_or(u8::MAX), factory);
        commands.push(primary_command_slice(&message, false));
        state.palette[index] = factory;
        restored_any = true;
    }

    if restored_any {
        commands.push(primary_command_slice(&PUSH2_REAPPLY_PALETTE_MESSAGE, false));
    }

    commands
}

#[expect(
    clippy::too_many_lines,
    reason = "push2 frame encoding has inherent per-zone complexity"
)]
pub(super) fn encode_led_frame(
    state: &mut Push2State,
    normalized: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
) {
    let rgb_colors = &normalized[..PUSH2_RGB_LED_COUNT];
    let white_button_colors = &normalized[PUSH2_RGB_LED_COUNT..PUSH2_MIDI_LED_COUNT];
    let touch_strip_colors = &normalized[PUSH2_MIDI_LED_COUNT..];
    let mut color_slots = HashMap::with_capacity(PUSH2_RGB_LED_COUNT);
    let mut assigned_slots = [false; PUSH2_PALETTE_SIZE];
    let live_rgb_slots = collect_live_rgb_slots(&state.prev_led_indices[..PUSH2_RGB_LED_COUNT]);
    let mut white_button_slots = [0_u8; PUSH2_WHITE_BUTTON_COUNT];
    assigned_slots[0] = true;
    assigned_slots[PUSH2_RGB_SLOT_LIMIT..].fill(true);
    color_slots.insert([0, 0, 0], 0_u8);

    let mut command_buffer = CommandBuffer::new(commands);
    let mut palette_dirty = false;

    for (index, color) in rgb_colors.iter().enumerate() {
        if color_slots.contains_key(color) {
            continue;
        }

        let entry = palette_entry(*color);
        let slot = if let Some(preferred) =
            preferred_rgb_slot(state, rgb_colors, index, entry, &assigned_slots)
        {
            if state.palette[usize::from(preferred)] != entry {
                let message = set_palette_entry_message(preferred, entry);
                command_buffer.push_slice(
                    &message,
                    false,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
                state.palette[usize::from(preferred)] = entry;
                palette_dirty = true;
            }
            preferred
        } else if let Some(existing) = find_existing_slot(state, entry, &assigned_slots) {
            existing
        } else if let Some(free_slot) = next_free_inactive_slot(&assigned_slots, &live_rgb_slots) {
            if state.palette[usize::from(free_slot)] != entry {
                let message = set_palette_entry_message(free_slot, entry);
                command_buffer.push_slice(
                    &message,
                    false,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
                state.palette[usize::from(free_slot)] = entry;
                palette_dirty = true;
            }
            free_slot
        } else {
            let free_slot = next_free_slot(&assigned_slots)
                .expect("Push 2 RGB zones use at most 92 unique colors");
            if state.palette[usize::from(free_slot)] != entry {
                let message = set_palette_entry_message(free_slot, entry);
                command_buffer.push_slice(
                    &message,
                    false,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
                state.palette[usize::from(free_slot)] = entry;
                palette_dirty = true;
            }
            free_slot
        };
        assigned_slots[usize::from(slot)] = true;
        color_slots.insert(*color, slot);
    }

    for (index, color) in white_button_colors.iter().enumerate() {
        let (slot, entry) = white_button_palette_slot(*color);
        white_button_slots[index] = slot;
        if slot != 0 && state.palette[usize::from(slot)] != entry {
            let message = set_palette_entry_message(slot, entry);
            command_buffer.push_slice(
                &message,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            state.palette[usize::from(slot)] = entry;
            palette_dirty = true;
        }
    }

    if palette_dirty {
        command_buffer.push_slice(
            &PUSH2_REAPPLY_PALETTE_MESSAGE,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
    }

    for (index, color) in rgb_colors.iter().take(PUSH2_PAD_COUNT).enumerate() {
        let slot = *color_slots
            .get(color)
            .expect("pad colors should always resolve to a palette slot");
        if state.prev_led_indices[index] == slot {
            continue;
        }

        command_buffer.push_slice(
            &[0x90, PAD_NOTE_MAP[index], slot],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        state.prev_led_indices[index] = slot;
    }

    for (index, color) in rgb_colors[PUSH2_PAD_COUNT..].iter().enumerate() {
        let slot = *color_slots
            .get(color)
            .expect("button colors should always resolve to a palette slot");
        let led_index = PUSH2_PAD_COUNT + index;
        if state.prev_led_indices[led_index] == slot {
            continue;
        }

        command_buffer.push_slice(
            &[0xB0, RGB_BUTTON_CC_MAP[index], slot],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        state.prev_led_indices[led_index] = slot;
    }

    for (index, slot) in white_button_slots.iter().copied().enumerate() {
        let led_index = PUSH2_RGB_LED_COUNT + index;
        if state.prev_led_indices[led_index] == slot {
            continue;
        }

        command_buffer.push_slice(
            &[0xB0, WHITE_BUTTON_CC_MAP[index], slot],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        state.prev_led_indices[led_index] = slot;
    }

    let strip_levels = quantize_touch_strip(touch_strip_colors);
    if strip_levels != state.prev_touch_strip {
        let packed = encode_touch_strip(&strip_levels);
        let message = touch_strip_message(&packed);
        command_buffer.push_slice(
            &message,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        state.prev_touch_strip = strip_levels;
    }

    command_buffer.finish();
}

pub(super) fn parse_palette_entry_response(
    args: &[u8],
    state: &mut Push2State,
) -> Result<(), ProtocolError> {
    if args.len() != 9 {
        return Err(ProtocolError::MalformedResponse {
            detail: format!(
                "palette reply should contain 9 argument bytes, got {}",
                args.len()
            ),
        });
    }

    let index = usize::from(args[0]);
    if index >= PUSH2_PALETTE_SIZE {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("palette reply index out of range: {}", args[0]),
        });
    }

    let mut entry = [0_u8; 4];
    for channel in 0..4 {
        entry[channel] = decode_sysex_byte(args[1 + channel * 2], args[2 + channel * 2])?;
    }

    state.palette[index] = entry;
    state.factory_palette[index] = entry;
    state.factory_palette_valid[index] = true;
    Ok(())
}

pub(super) fn all_leds_off_commands() -> Vec<ProtocolCommand> {
    let mut commands =
        Vec::with_capacity(PUSH2_PAD_COUNT + PUSH2_RGB_BUTTON_COUNT + PUSH2_WHITE_BUTTON_COUNT + 1);
    for note in PAD_NOTE_MAP {
        commands.push(primary_command(vec![0x90, note, 0x00], false));
    }
    for cc in RGB_BUTTON_CC_MAP {
        commands.push(primary_command(vec![0xB0, cc, 0x00], false));
    }
    for cc in WHITE_BUTTON_CC_MAP {
        commands.push(primary_command(vec![0xB0, cc, 0x00], false));
    }
    let message = touch_strip_message(&encode_touch_strip(&[0; PUSH2_TOUCH_STRIP_LED_COUNT]));
    commands.push(primary_command_slice(&message, false));
    commands
}

fn derive_white_channel(rgb: [u8; 3]) -> u8 {
    let weighted =
        2_126_u32 * u32::from(rgb[0]) + 7_152_u32 * u32::from(rgb[1]) + 722_u32 * u32::from(rgb[2]);
    u8::try_from((weighted + 5_000) / 10_000).unwrap_or(u8::MAX)
}

fn palette_entry(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], derive_white_channel(rgb)]
}

fn find_existing_slot(
    state: &Push2State,
    entry: [u8; 4],
    assigned_slots: &[bool; 128],
) -> Option<u8> {
    state
        .palette
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, current)| {
            (*current == entry && !assigned_slots[index]).then(|| u8::try_from(index).ok())
        })
        .flatten()
}

fn next_free_slot(assigned_slots: &[bool; 128]) -> Option<u8> {
    assigned_slots
        .iter()
        .enumerate()
        .skip(1)
        .take(PUSH2_RGB_SLOT_LIMIT - 1)
        .find_map(|(index, assigned)| (!assigned).then(|| u8::try_from(index).ok()))
        .flatten()
}

fn collect_live_rgb_slots(prev_led_indices: &[u8]) -> [bool; 128] {
    let mut live_slots = [false; 128];
    for slot in prev_led_indices.iter().copied() {
        if is_rgb_palette_slot(slot) {
            live_slots[usize::from(slot)] = true;
        }
    }
    live_slots
}

fn preferred_rgb_slot(
    state: &Push2State,
    rgb_colors: &[[u8; 3]],
    led_index: usize,
    entry: [u8; 4],
    assigned_slots: &[bool; 128],
) -> Option<u8> {
    let slot = state.prev_led_indices[led_index];
    if !is_rgb_palette_slot(slot) || assigned_slots[usize::from(slot)] {
        return None;
    }

    rgb_slot_rewrite_is_safe(
        &state.prev_led_indices[..PUSH2_RGB_LED_COUNT],
        rgb_colors,
        slot,
        entry,
    )
    .then_some(slot)
}

fn rgb_slot_rewrite_is_safe(
    prev_led_indices: &[u8],
    rgb_colors: &[[u8; 3]],
    slot: u8,
    entry: [u8; 4],
) -> bool {
    prev_led_indices
        .iter()
        .zip(rgb_colors.iter())
        .filter(|(current_slot, _)| **current_slot == slot)
        .all(|(_, color)| palette_entry(*color) == entry)
}

fn next_free_inactive_slot(assigned_slots: &[bool; 128], live_slots: &[bool; 128]) -> Option<u8> {
    assigned_slots
        .iter()
        .enumerate()
        .skip(1)
        .take(PUSH2_RGB_SLOT_LIMIT - 1)
        .find_map(|(index, assigned)| {
            (!assigned && !live_slots[index]).then(|| u8::try_from(index).ok())
        })
        .flatten()
}

fn is_rgb_palette_slot(slot: u8) -> bool {
    usize::from(slot) < PUSH2_RGB_SLOT_LIMIT && slot != 0
}

fn white_button_palette_slot(rgb: [u8; 3]) -> (u8, [u8; 4]) {
    let white = derive_white_channel(rgb);
    if white == 0 {
        return (0, [0; 4]);
    }

    let level = 1_u8.saturating_add(
        u8::try_from(
            (u16::from(white.saturating_sub(1))
                * u16::from(PUSH2_WHITE_SLOT_COUNT.saturating_sub(1)))
                / 254,
        )
        .unwrap_or(PUSH2_WHITE_SLOT_COUNT.saturating_sub(1)),
    );

    let quantized_white =
        u8::try_from((u16::from(level) * 255 + 15) / u16::from(PUSH2_WHITE_SLOT_COUNT))
            .unwrap_or(u8::MAX);
    (
        PUSH2_WHITE_SLOT_START + level - 1,
        [0, 0, 0, quantized_white],
    )
}

fn encode_touch_strip(levels: &[u8; PUSH2_TOUCH_STRIP_LED_COUNT]) -> [u8; 16] {
    let mut packed = [0_u8; 16];
    for index in 0..15 {
        let low = levels[index * 2] & 0x07;
        let high = levels[index * 2 + 1] & 0x07;
        packed[index] = (high << 4) | low;
    }
    packed[15] = levels[30] & 0x07;
    packed
}

fn quantize_touch_strip(colors: &[[u8; 3]]) -> [u8; PUSH2_TOUCH_STRIP_LED_COUNT] {
    let mut levels = [0_u8; PUSH2_TOUCH_STRIP_LED_COUNT];
    for (index, color) in colors.iter().take(PUSH2_TOUCH_STRIP_LED_COUNT).enumerate() {
        let luma = derive_white_channel(*color);
        levels[index] = u8::try_from((u16::from(luma) * 7 + 127) / 255).unwrap_or(7);
    }
    levels
}

fn touch_strip_message(packed: &[u8; 16]) -> [u8; 24] {
    let mut message = [0_u8; 24];
    message[..7].copy_from_slice(&[
        0xF0,
        0x00,
        0x21,
        0x1D,
        0x01,
        0x01,
        PUSH2_CMD_SET_TOUCH_STRIP_LEDS,
    ]);
    message[7..23].copy_from_slice(packed);
    message[23] = 0xF7;
    message
}
