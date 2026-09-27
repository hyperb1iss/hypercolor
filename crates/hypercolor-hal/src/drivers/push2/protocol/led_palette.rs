//! Push 2 LED palette encoding and caching.
//!
//! The Push 2 uses a 128-entry RGBW palette stored on-device. Each LED is
//! addressed by palette index rather than raw color, so the host must manage
//! slot assignment, white-button quantization, and factory palette restoration.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::protocol::{CommandBuffer, ProtocolCommand, ProtocolError, TransferType};

use super::flow::{WireBudget, usb_midi_wire_bytes};
use super::{
    PAD_NOTE_MAP, PUSH2_CMD_SET_TOUCH_STRIP_LEDS, PUSH2_MIDI_LED_COUNT, PUSH2_PAD_COUNT,
    PUSH2_PALETTE_SIZE, PUSH2_PALETTE_WRITES_PER_BATCH, PUSH2_REAPPLY_PALETTE_MESSAGE,
    PUSH2_RGB_BUTTON_COUNT, PUSH2_RGB_LED_COUNT, PUSH2_RGB_SLOT_LIMIT, PUSH2_SET_PALETTE_ENTRY_LEN,
    PUSH2_TOUCH_STRIP_LED_COUNT, PUSH2_WHITE_BUTTON_COUNT, PUSH2_WHITE_SLOT_COUNT,
    PUSH2_WHITE_SLOT_START, Push2State, RGB_BUTTON_CC_MAP, WHITE_BUTTON_CC_MAP, decode_sysex_byte,
    primary_command, primary_command_slice, set_palette_entry_message,
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

/// Encode one LED batch from the newest frame, within `budget` wire bytes.
///
/// Every message that goes out is recorded in the believed state; anything
/// that does not fit stays pending and is recomputed from whatever frame is
/// newest when the next batch runs, so the lane coalesces to the latest
/// color per LED instead of replaying history. Returns `true` when work was
/// left for a later batch.
#[expect(
    clippy::too_many_lines,
    reason = "push2 batch encoding walks palette, key, and strip zones in one budgeted pass"
)]
pub(super) fn encode_led_batch(
    state: &mut Push2State,
    normalized: &[[u8; 3]],
    buffer: &mut CommandBuffer<'_>,
    budget: &mut WireBudget,
) -> bool {
    let rgb_colors = &normalized[..PUSH2_RGB_LED_COUNT];
    let white_button_colors = &normalized[PUSH2_RGB_LED_COUNT..PUSH2_MIDI_LED_COUNT];
    let touch_strip_colors = &normalized[PUSH2_MIDI_LED_COUNT..];
    let mut color_slots = HashMap::with_capacity(PUSH2_RGB_LED_COUNT);
    let mut assigned_slots = [false; PUSH2_PALETTE_SIZE];
    let live_rgb_slots = collect_live_rgb_slots(&state.prev_led_indices[..PUSH2_RGB_LED_COUNT]);
    let wanted_slots = collect_wanted_rgb_slots(state, rgb_colors);
    let mut white_button_slots = [0_u8; PUSH2_WHITE_BUTTON_COUNT];
    let mut white_button_ready = [true; PUSH2_WHITE_BUTTON_COUNT];
    assigned_slots[0] = true;
    assigned_slots[PUSH2_RGB_SLOT_LIMIT..].fill(true);
    color_slots.insert([0, 0, 0], 0_u8);

    let mut deferred = false;
    let mut palette_dirty = false;
    let mut rgb_palette_writes = 0_usize;
    let mut first_unwritten_color = None;
    let mut approximated = HashSet::new();

    let palette_start = state.link.palette_scan_offset % PUSH2_RGB_LED_COUNT;
    for step in 0..PUSH2_RGB_LED_COUNT {
        let index = (palette_start + step) % PUSH2_RGB_LED_COUNT;
        let color = rgb_colors[index];
        if color_slots.contains_key(&color) || approximated.contains(&color) {
            continue;
        }

        let entry = palette_entry(color);
        let can_write = rgb_palette_writes < PUSH2_PALETTE_WRITES_PER_BATCH
            && palette_write_fits(budget, palette_dirty);
        let occupancy = SlotOccupancy {
            assigned: &assigned_slots,
            live: &live_rgb_slots,
            wanted: &wanted_slots,
        };
        if let Some((slot, needs_write)) =
            choose_rgb_slot(state, rgb_colors, index, entry, &occupancy, can_write)
        {
            if needs_write {
                write_palette_entry(state, buffer, budget, &mut palette_dirty, slot, entry);
                rgb_palette_writes += 1;
            }
            assigned_slots[usize::from(slot)] = true;
            color_slots.insert(color, slot);
        } else {
            deferred = true;
            first_unwritten_color.get_or_insert(index);
            approximated.insert(color);
        }
    }
    // Over-budget colors borrow the nearest entry only once this batch's
    // writes are settled, so none of them borrows a slot that a later color
    // in the same batch rewrote in place.
    for color in approximated {
        let slot = nearest_existing_rgb_slot(&state.palette, palette_entry(color));
        color_slots.insert(color, slot);
    }
    // The next batch hands its palette writes to the colors this one could
    // not afford first, so a fast effect cannot starve the end of the grid.
    if let Some(index) = first_unwritten_color {
        state.link.palette_scan_offset = index;
    }

    for (index, color) in white_button_colors.iter().enumerate() {
        let (slot, entry) = white_button_palette_slot(*color);
        white_button_slots[index] = slot;
        let slot_index = usize::from(slot);
        if slot == 0 || (!state.force_palette[slot_index] && state.palette[slot_index] == entry) {
            continue;
        }
        // White slots are 31 fixed quantized levels, so they sit outside the
        // RGB write cap, but they still have to fit the wire budget.
        if palette_write_fits(budget, palette_dirty) {
            write_palette_entry(state, buffer, budget, &mut palette_dirty, slot, entry);
        } else {
            white_button_ready[index] = false;
            deferred = true;
        }
    }

    if palette_dirty {
        // Its wire cost was reserved with the first palette write.
        buffer.push_slice(
            &PUSH2_REAPPLY_PALETTE_MESSAGE,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
    }

    let led_start = state.link.led_scan_offset % PUSH2_MIDI_LED_COUNT;
    let mut first_unsent_led = None;
    for step in 0..PUSH2_MIDI_LED_COUNT {
        let led_index = (led_start + step) % PUSH2_MIDI_LED_COUNT;
        let (slot, ready) = if led_index < PUSH2_RGB_LED_COUNT {
            let slot = *color_slots
                .get(&rgb_colors[led_index])
                .expect("RGB colors should always resolve to a palette slot");
            (slot, true)
        } else {
            let white_index = led_index - PUSH2_RGB_LED_COUNT;
            (
                white_button_slots[white_index],
                white_button_ready[white_index],
            )
        };
        if state.prev_led_indices[led_index] == slot && !state.force_leds[led_index] {
            continue;
        }
        // A button whose white slot could not be written this batch keeps
        // its old index rather than pointing at an unwritten entry.
        if !ready || !budget.try_spend(3) {
            deferred = true;
            first_unsent_led.get_or_insert(led_index);
            continue;
        }

        let message = led_message(led_index, slot);
        buffer.push_slice(
            &message,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        state.prev_led_indices[led_index] = slot;
        state.force_leds[led_index] = false;
    }
    if let Some(led_index) = first_unsent_led {
        state.link.led_scan_offset = led_index;
    }

    let strip_levels = quantize_touch_strip(touch_strip_colors);
    if strip_levels != state.prev_touch_strip || state.force_touch_strip {
        let message = touch_strip_message(&encode_touch_strip(&strip_levels));
        if budget.try_spend(message.len()) {
            buffer.push_slice(
                &message,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            state.prev_touch_strip = strip_levels;
            state.force_touch_strip = false;
        } else {
            deferred = true;
        }
    }

    deferred
}

/// Whether one more palette write fits, counting the Reapply that the batch
/// owes once it writes any entry.
fn palette_write_fits(budget: &WireBudget, palette_dirty: bool) -> bool {
    let reapply = if palette_dirty {
        0
    } else {
        usb_midi_wire_bytes(PUSH2_REAPPLY_PALETTE_MESSAGE.len())
    };
    budget.remaining() >= usb_midi_wire_bytes(PUSH2_SET_PALETTE_ENTRY_LEN) + reapply
}

fn write_palette_entry(
    state: &mut Push2State,
    buffer: &mut CommandBuffer<'_>,
    budget: &mut WireBudget,
    palette_dirty: &mut bool,
    slot: u8,
    entry: [u8; 4],
) {
    if !*palette_dirty {
        let reserved = budget.try_spend(PUSH2_REAPPLY_PALETTE_MESSAGE.len());
        debug_assert!(reserved, "caller checked the budget holds the reapply");
        *palette_dirty = true;
    }
    let message = set_palette_entry_message(slot, entry);
    let spent = budget.try_spend(message.len());
    debug_assert!(spent, "caller checked the budget holds the palette write");
    buffer.push_slice(
        &message,
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Primary,
    );
    let slot = usize::from(slot);
    state.palette[slot] = entry;
    state.force_palette[slot] = false;
}

fn led_message(led_index: usize, slot: u8) -> [u8; 3] {
    if led_index < PUSH2_PAD_COUNT {
        [0x90, PAD_NOTE_MAP[led_index], slot]
    } else if led_index < PUSH2_RGB_LED_COUNT {
        [0xB0, RGB_BUTTON_CC_MAP[led_index - PUSH2_PAD_COUNT], slot]
    } else {
        [
            0xB0,
            WHITE_BUTTON_CC_MAP[led_index - PUSH2_RGB_LED_COUNT],
            slot,
        ]
    }
}

/// Decode a Get LED Color Palette Entry reply into its index and RGBW entry.
pub(super) fn parse_palette_entry_response(args: &[u8]) -> Result<(u8, [u8; 4]), ProtocolError> {
    if args.len() != 9 {
        return Err(ProtocolError::MalformedResponse {
            detail: format!(
                "palette reply should contain 9 argument bytes, got {}",
                args.len()
            ),
        });
    }

    let index = args[0];
    if usize::from(index) >= PUSH2_PALETTE_SIZE {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("palette reply index out of range: {index}"),
        });
    }

    let mut entry = [0_u8; 4];
    for channel in 0..4 {
        entry[channel] = decode_sysex_byte(args[1 + channel * 2], args[2 + channel * 2])?;
    }

    Ok((index, entry))
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

fn next_free_slot_where(
    assigned_slots: &[bool; 128],
    mut eligible: impl FnMut(usize) -> bool,
) -> Option<u8> {
    assigned_slots
        .iter()
        .enumerate()
        .skip(1)
        .take(PUSH2_RGB_SLOT_LIMIT - 1)
        .find_map(|(index, assigned)| {
            (!assigned && eligible(index)).then(|| u8::try_from(index).ok())
        })
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

/// Which palette slots are spoken for while one batch assigns colors.
struct SlotOccupancy<'a> {
    /// Claimed by a color earlier in this batch.
    assigned: &'a [bool; 128],
    /// Shown by an LED on the device right now.
    live: &'a [bool; 128],
    /// Already holding a color this frame wants, whose LED may not have been
    /// moved onto it yet because an earlier batch ran out of room.
    wanted: &'a [bool; 128],
}

/// Pick the palette slot for one RGB color and say whether it needs a write.
///
/// A slot marked for resync counts as untrusted: it is only used without a
/// rewrite when nothing better is affordable.
fn choose_rgb_slot(
    state: &Push2State,
    rgb_colors: &[[u8; 3]],
    led_index: usize,
    entry: [u8; 4],
    occupancy: &SlotOccupancy<'_>,
    can_write: bool,
) -> Option<(u8, bool)> {
    let assigned_slots = occupancy.assigned;
    let needs_write = |slot: u8| {
        let slot = usize::from(slot);
        state.force_palette[slot] || state.palette[slot] != entry
    };

    if let Some(preferred) = preferred_rgb_slot(state, rgb_colors, led_index, entry, assigned_slots)
    {
        let write = needs_write(preferred);
        if !write || can_write {
            return Some((preferred, write));
        }
    }

    if let Some(existing) = find_existing_slot(state, entry, assigned_slots) {
        let write = needs_write(existing);
        if !write || can_write {
            return Some((existing, write));
        }
    }

    if !can_write {
        return None;
    }

    // Overwrite a slot nobody shows and nobody wants first; a slot that
    // already holds a wanted color is cheaper to claim than to rewrite twice.
    let free_slot = next_free_slot_where(assigned_slots, |slot| {
        !occupancy.live[slot] && !occupancy.wanted[slot]
    })
    .or_else(|| next_free_slot_where(assigned_slots, |slot| !occupancy.live[slot]))
    .or_else(|| next_free_slot_where(assigned_slots, |_| true))
    .expect("Push 2 RGB zones use at most 92 unique colors");
    Some((free_slot, needs_write(free_slot)))
}

/// Trusted RGB slots whose entry matches a color some LED wants this frame.
fn collect_wanted_rgb_slots(state: &Push2State, rgb_colors: &[[u8; 3]]) -> [bool; 128] {
    // Black always rides slot 0 and never claims a palette slot.
    let wanted_entries: HashSet<[u8; 4]> = rgb_colors
        .iter()
        .filter(|color| **color != [0, 0, 0])
        .map(|color| palette_entry(*color))
        .collect();
    let mut wanted = [false; 128];
    for (slot, is_wanted) in wanted
        .iter_mut()
        .enumerate()
        .take(PUSH2_RGB_SLOT_LIMIT)
        .skip(1)
    {
        *is_wanted = !state.force_palette[slot] && wanted_entries.contains(&state.palette[slot]);
    }
    wanted
}

fn nearest_existing_rgb_slot(palette: &[[u8; 4]; PUSH2_PALETTE_SIZE], entry: [u8; 4]) -> u8 {
    let mut best_slot = 0_u8;
    let mut best_distance = u32::MAX;
    for (index, current) in palette.iter().enumerate().take(PUSH2_RGB_SLOT_LIMIT) {
        let distance = rgb_distance_sq(*current, entry);
        if distance < best_distance {
            best_distance = distance;
            best_slot = u8::try_from(index).unwrap_or(u8::MAX);
        }
    }
    best_slot
}

fn rgb_distance_sq(a: [u8; 4], b: [u8; 4]) -> u32 {
    // Host-written entries derive W from RGB; factory entries carry
    // device-chosen W, so a fallback match may briefly show a W mismatch
    // until the budget allows an exact rewrite.
    (0..3)
        .map(|channel| {
            let delta = i32::from(a[channel]) - i32::from(b[channel]);
            #[expect(
                clippy::cast_sign_loss,
                reason = "square of an i32 delta is non-negative"
            )]
            {
                (delta * delta) as u32
            }
        })
        .sum()
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
